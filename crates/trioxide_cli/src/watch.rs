use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use trioxide::Project;

use crate::{Failure, GlobalArgs, Renderer, add_debug_islands, build_once, resolve_config};

const POLL_INTERVAL: Duration = Duration::from_millis(100);

type Stamp = Option<(SystemTime, u64)>;

/// `trioxide build --watch`: builds, then rebuilds whenever a module in the graph, an entry,
/// the trioxide config or the tsconfig changes. Runs until the process is interrupted.
///
/// Changes are found by polling modification times, which works the same on every platform
/// and filesystem (including network mounts and containers) at the cost of a short delay.
pub fn watch(
    global: &GlobalArgs,
    entries: &[PathBuf],
    out_dir: &Path,
    renderer: &Renderer,
) -> Result<(), Failure> {
    loop {
        let (root, options, config) = resolve_config(global, entries.first(), renderer)?;
        let mut project = Project::new(root, options)?;
        add_debug_islands(global, &mut project)?;
        let settings_files: Vec<PathBuf> = config
            .into_iter()
            .chain(project.compiler_settings().path.clone())
            .collect();
        // Stamps taken before each build, so edits made while it runs still trigger a rebuild.
        let mut before = stamps(entries.iter().chain(&settings_files));
        loop {
            let started = Instant::now();
            let result = build_once(&mut project, entries, out_dir, global.emit, renderer);

            let mut watched: Vec<PathBuf> = project
                .graph()
                .modules()
                .iter()
                .map(|module| module.path.clone())
                .chain(entries.iter().cloned())
                .chain(settings_files.iter().cloned())
                .collect();
            watched.sort();
            watched.dedup();
            // Modules first seen in this build had no stamp before it. Stamp them now, and if a
            // file no longer holds what the build read, it changed in between: rebuild.
            for module in project.graph().modules() {
                if !before.contains_key(&module.path) {
                    let stamp = stamp(&module.path);
                    let current = std::fs::read_to_string(&module.path).ok();
                    let unchanged = current.as_deref() == Some(&*module.source);
                    before.insert(module.path.clone(), if unchanged { stamp } else { None });
                }
            }
            for (path, stamp) in stamps(&watched) {
                before.entry(path).or_insert(stamp);
            }
            before.retain(|path, _| watched.contains(path));

            match result {
                Ok(()) => renderer.info(&format!(
                    "built {} modules in {} ms; watching for changes",
                    project.graph().modules().len(),
                    started.elapsed().as_millis()
                )),
                Err(Failure::Error(message)) => {
                    if !message.is_empty() {
                        renderer.error(&message);
                    }
                    renderer.info("watching for changes");
                }
                Err(usage @ Failure::Usage(_)) => return Err(usage),
            }
            let changed = wait_for_changes(&before);
            if changed.iter().any(|path| settings_files.contains(path)) {
                renderer.info("configuration changed; reloading");
                break;
            }
            before = stamps(&watched);
            for path in changed {
                if let Err(error) = project.update(&path, None) {
                    renderer.error(&error.to_string());
                }
            }
        }
    }
}

/// Blocks until a path's stamp differs from `before`; returns every path that changed.
fn wait_for_changes(before: &BTreeMap<PathBuf, Stamp>) -> Vec<PathBuf> {
    let changed = |now: &BTreeMap<PathBuf, Stamp>| -> Vec<PathBuf> {
        now.iter()
            .filter(|(path, stamp)| before.get(*path) != Some(*stamp))
            .map(|(path, _)| path.clone())
            .collect()
    };
    loop {
        std::thread::sleep(POLL_INTERVAL);
        if !changed(&stamps(before.keys())).is_empty() {
            // Let a burst of writes (an editor saving several files) settle first.
            std::thread::sleep(POLL_INTERVAL);
            return changed(&stamps(before.keys()));
        }
    }
}

fn stamps<'a>(paths: impl IntoIterator<Item = &'a PathBuf>) -> BTreeMap<PathBuf, Stamp> {
    paths
        .into_iter()
        .map(|path| (path.clone(), stamp(path)))
        .collect()
}

fn stamp(path: &Path) -> Stamp {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| Some((meta.modified().ok()?, meta.len())))
}
