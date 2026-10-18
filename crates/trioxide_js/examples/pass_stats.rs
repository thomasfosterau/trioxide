//! Counts what the passes did over the fixture corpus (`tests/fixtures`, `tests/islands`):
//! rewrites and skips per pass, fact refreshes and their time, and how much smaller the
//! output got. Run with `cargo run -p trioxide_js --example pass_stats --release`.

#![expect(clippy::print_stdout, reason = "an example that reports numbers")]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use trioxide::{Options, Project};
use trioxide_common::log::PassOutcome;

fn discover(dir: &Path, entries: &mut Vec<PathBuf>) {
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    children.sort();
    if let Some(entry) = ["input.ts", "input.tsx"]
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
    {
        entries.push(entry);
    }
    for child in children {
        if child.is_dir() && child.file_name().is_some_and(|name| name != "node_modules") {
            discover(&child, entries);
        }
    }
}

fn main() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut entries = Vec::new();
    for root in ["tests/fixtures", "tests/islands"] {
        discover(&repo.join(root).canonicalize().unwrap(), &mut entries);
    }
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let (mut refreshes, mut refresh_time) = (0, Duration::ZERO);
    let (mut modules, mut changed) = (0, 0);
    let (mut before, mut after) = (0, 0);
    let mut build_time = Duration::ZERO;
    for entry in &entries {
        let dir = entry.parent().unwrap();
        let options = Options::find_config(dir)
            .map_or_else(Options::default, |path| Options::from_file(&path).unwrap());
        let mut baseline_options = options.clone();
        for pass in [
            "imports",
            "inline",
            "checks",
            "fold",
            "dce",
            "licm",
            "hidden-class",
            "for-of",
            "pipelines",
            "simplify",
        ] {
            baseline_options.passes.insert(pass.into(), false);
        }
        let mut baseline = Project::new(dir.to_path_buf(), baseline_options).unwrap();
        let unoptimised = baseline.build(std::slice::from_ref(entry)).unwrap();

        let mut project = Project::new(dir.to_path_buf(), options).unwrap();
        let started = Instant::now();
        let output = project.build(std::slice::from_ref(entry)).unwrap();
        build_time += started.elapsed();
        let (count, time) = project.refresh_stats();
        refreshes += count;
        refresh_time += time;
        for (module, plain) in output.modules.iter().zip(&unoptimised.modules) {
            modules += 1;
            before += plain.code.len();
            after += module.code.len();
            changed += usize::from(module.code != plain.code);
        }
        for module in &output.report.modules {
            for event in &module.log.passes {
                let slot = counts.entry(event.pass.clone()).or_default();
                match event.outcome {
                    PassOutcome::Applied => slot.0 += 1,
                    PassOutcome::Skipped { .. } => slot.1 += 1,
                }
            }
        }
    }
    println!(
        "{} fixtures, {modules} modules ({changed} changed)\n",
        entries.len()
    );
    println!("| pass | applied | skipped |\n|---|---:|---:|");
    for (pass, (applied, skipped)) in &counts {
        println!("| {pass} | {applied} | {skipped} |");
    }
    println!(
        "\noutput: {before} → {after} bytes ({:.1}%)",
        100.0
            * (f64::from(u32::try_from(after).unwrap())
                - f64::from(u32::try_from(before).unwrap()))
            / f64::from(u32::try_from(before).unwrap())
    );
    println!(
        "fact refreshes: {refreshes} ({:.1} ms); builds with passes: {:.0} ms",
        refresh_time.as_secs_f64() * 1e3,
        build_time.as_secs_f64() * 1e3
    );
}
