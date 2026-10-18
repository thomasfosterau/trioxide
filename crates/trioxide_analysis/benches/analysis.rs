//! Compiler throughput: analysis time against the time to parse with oxc and tsrs.
//!
//! The acceptance bar (S2) is analysis under twice the parse time. The corpus is the span
//! corpus of `trioxide_types` plus every analysis fixture, replicated under distinct paths.
//!
//! ```sh
//! cargo bench -p trioxide_analysis
//! ```

#![expect(
    missing_docs,
    reason = "`criterion_group!` generates an undocumented function"
)]

use std::path::{Path, PathBuf};

use criterion::{Criterion, criterion_group, criterion_main};
use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use trioxide_analysis::{AnalysisOptions, ModuleInput, analyze};
use trioxide_common::ModuleId;
use trioxide_types::{OracleInput, SourceText, TsrsOracle, TypeOracle};

const COPIES: usize = 20;

struct Corpus {
    files: Vec<(PathBuf, String)>,
}

impl Corpus {
    fn load() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut sources = Vec::new();
        for dir in [
            "crates/trioxide_types/tests/corpus",
            "tests/fixtures/analysis",
        ] {
            collect(&root.join(dir), &mut sources);
        }
        sources.sort();
        let mut files = Vec::new();
        let copies = std::env::var("COPIES")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(COPIES);
        for copy in 0..copies {
            for (name, source) in &sources {
                files.push((
                    PathBuf::from(format!("/__bench__/c{copy}/{name}")),
                    source.clone(),
                ));
            }
        }
        Self { files }
    }

    fn sources(&self) -> Vec<SourceText<'_>> {
        self.files
            .iter()
            .enumerate()
            .map(|(index, (path, source))| SourceText {
                module: ModuleId::new(u32::try_from(index).unwrap()),
                path,
                source,
            })
            .collect()
    }

    fn inputs(&self) -> Vec<ModuleInput<'_>> {
        self.files
            .iter()
            .enumerate()
            .map(|(index, (path, source))| ModuleInput {
                id: ModuleId::new(u32::try_from(index).unwrap()),
                path,
                source,
                source_type: SourceType::ts(),
                imports: Vec::new(),
            })
            .collect()
    }
}

/// `(flattened name, source)` for every `.ts` file below `dir`.
fn collect(dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "ts") {
            let name = path.to_string_lossy().replace(['/', '\\'], "_");
            out.push((name, std::fs::read_to_string(&path).unwrap()));
        }
    }
}

fn load_oracle(corpus: &Corpus) -> TsrsOracle {
    let mut oracle = TsrsOracle::default();
    oracle.load(&OracleInput {
        root: Path::new("/__bench__"),
        tsconfig: None,
        modules: &corpus.sources(),
        check: false,
    });
    oracle.flush();
    oracle
}

fn bench(c: &mut Criterion) {
    let corpus = Corpus::load();
    let runs = std::env::var("RUNS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(1);
    for _ in 0..runs {
        if std::env::var_os("TRIOXIDE_BENCH_STATS").is_none() {
            break;
        }
        let mut oracle = load_oracle(&corpus);
        let cpu = || -> f64 {
            let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
            let fields: Vec<&str> = stat
                .rsplit(')')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .collect();
            let ticks = |at: usize| {
                fields
                    .get(at)
                    .and_then(|field| field.parse::<f64>().ok())
                    .unwrap_or(0.0)
            };
            (ticks(11) + ticks(12)) * 10.0
        };
        let cpu_before = cpu();
        let started = std::time::Instant::now();
        let threads = std::env::var("THREADS")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        let chunk_units = std::env::var("CHUNK")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        let options = AnalysisOptions {
            threads,
            chunk_units,
            ..AnalysisOptions::default()
        };
        let analysis = analyze(&corpus.inputs(), &mut oracle, options);
        #[expect(clippy::print_stderr, reason = "opt-in statistics")]
        {
            eprintln!(
                "{} modules, {} checkers: {:?} in {:?} ({} ms of CPU)",
                corpus.files.len(),
                oracle.checker_count(),
                analysis.stats,
                started.elapsed(),
                cpu() - cpu_before
            );
        }
    }
    let mut group = c.benchmark_group("analysis");
    group.sample_size(10);
    group.bench_function("parse: oxc + tsrs", |b| {
        b.iter(|| {
            for (path, source) in &corpus.files {
                let allocator = Allocator::default();
                let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
                let program = allocator.alloc(parsed.program);
                let semantic = SemanticBuilder::new().build(program);
                std::hint::black_box((path, semantic.semantic.scoping().symbols_len()));
            }
            std::hint::black_box(load_oracle(&corpus).file_count());
        });
    });
    group.bench_function("analyze (with tsrs queries)", |b| {
        b.iter_batched(
            || load_oracle(&corpus),
            |mut oracle| {
                let inputs = corpus.inputs();
                let analysis = analyze(&inputs, &mut oracle, AnalysisOptions::default());
                // Returned, so criterion drops the facts (the product) outside the timing.
                (oracle, analysis)
            },
            criterion::BatchSize::PerIteration,
        );
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
