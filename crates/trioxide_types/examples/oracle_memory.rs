//! Measures the tsrs oracle: program build time, query time, resident and virtual memory, and
//! memory growth over incremental updates under different rebuild policies.
//!
//! ```sh
//! cargo run --release -p trioxide_types --example oracle_memory
//! cargo run --release -p trioxide_types --example oracle_memory --features plain-ptrs
//! ```
//!
//! Each run uses the span-match corpus (`tests/corpus`), replicated `COPIES` times under
//! distinct in-memory paths (`COPIES=n` to change). Linux only (reads `/proc/self/status`).

#![expect(clippy::print_stdout, reason = "an example that reports numbers")]

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use trioxide_common::{ModuleId, Span};
use trioxide_types::{OracleInput, RebuildPolicy, SourceText, TsrsOracle, TypeOracle};

/// How many copies of the corpus to load; override with `COPIES=n`.
const COPIES: usize = 100;
const UPDATES: usize = 200;

fn status(field: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
        .unwrap_or(0)
        / 1024
}

fn identifiers(source: &str) -> Vec<Span> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            spans.push(Span::new(
                u32::try_from(start).unwrap(),
                u32::try_from(i).unwrap(),
            ));
        } else {
            i += 1;
        }
    }
    spans
}

fn main() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut originals: Vec<(String, String)> = std::fs::read_dir(&corpus)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(&path).unwrap())
        })
        .collect();
    originals.sort();
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    let copies = std::env::var("COPIES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(COPIES);
    for copy in 0..copies {
        for (name, source) in &originals {
            files.push((
                PathBuf::from(format!("/__bench__/c{copy}/{name}")),
                source.clone(),
            ));
        }
    }
    let lines: usize = files.iter().map(|(_, source)| source.lines().count()).sum();
    println!(
        "plain-ptrs: {}; {} modules, {lines} lines",
        cfg!(feature = "plain-ptrs"),
        files.len()
    );
    println!(
        "start: rss {} MiB, virtual {} MiB",
        status("VmRSS:"),
        status("VmSize:")
    );

    for (label, max_updates) in [("never rebuild", usize::MAX), ("rebuild every 50", 50)] {
        let modules: Vec<SourceText<'_>> = files
            .iter()
            .enumerate()
            .map(|(index, (path, source))| SourceText {
                module: ModuleId::new(u32::try_from(index).unwrap()),
                path,
                source,
            })
            .collect();
        let mut oracle = TsrsOracle::new(RebuildPolicy {
            max_updates,
            max_resident_bytes: None,
        });
        let started = Instant::now();
        oracle.load(&OracleInput {
            root: Path::new("/__bench__"),
            tsconfig: None,
            modules: &modules,
            check: false,
        });
        let built = started.elapsed();
        let started = Instant::now();
        let mut answered = 0usize;
        for module in &modules {
            for span in identifiers(module.source) {
                answered += usize::from(oracle.type_at(module.module, span).is_some());
            }
        }
        let queried = started.elapsed();
        println!(
            "[{label}] build {built:.2?}, {answered} type queries {queried:.2?}; rss {} MiB, virtual {} MiB",
            status("VmRSS:"),
            status("VmSize:")
        );

        let target = ModuleId::new(0);
        let (path, source) = &files[0];
        let spans = identifiers(source);
        let started = Instant::now();
        for round in 0..UPDATES {
            let edited = format!("{source}\nexport const round{round} = {round};\n");
            oracle.update(SourceText {
                module: target,
                path,
                source: &edited,
            });
            for span in &spans {
                let _ = oracle.type_at(target, *span);
            }
        }
        let stats = oracle.program_stats();
        println!(
            "[{label}] {UPDATES} updates {:.2?} ({} incremental, {} full builds); rss {} MiB, peak {} MiB",
            started.elapsed(),
            stats.updates,
            stats.full_builds,
            status("VmRSS:"),
            status("VmHWM:")
        );
        drop(oracle);
        println!("[{label}] dropped: rss {} MiB", status("VmRSS:"));
    }
}
