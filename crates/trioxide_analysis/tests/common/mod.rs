//! Loads an analysis fixture: its module graph, the tsrs oracle, and the analysis.

#![allow(dead_code, reason = "shared by several test binaries")]

use std::path::{Path, PathBuf};

use trioxide_analysis::{Analysis, AnalysisOptions, ModuleInput, analyze};
use trioxide_project::{GraphOptions, ImportTarget, ModuleGraph, find_tsconfig};
use trioxide_types::{OracleInput, SourceText, TsrsOracle, TypeOracle};

/// The analysis fixtures, sorted.
pub fn fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/analysis").canonicalize().unwrap();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("input.ts").is_file())
        .map(|path| path.join("input.ts"))
        .collect();
    entries.sort();
    entries
}

/// A loaded and analysed fixture.
pub struct Loaded {
    pub graph: ModuleGraph,
    pub analysis: Analysis,
    pub sources: Vec<(PathBuf, String)>,
    pub closed_world: bool,
}

impl Loaded {
    /// The analysis inputs, borrowing the graph.
    pub fn inputs(&self) -> Vec<ModuleInput<'_>> {
        inputs(&self.graph)
    }
}

fn inputs(graph: &ModuleGraph) -> Vec<ModuleInput<'_>> {
    graph
        .modules()
        .iter()
        .map(|module| ModuleInput {
            id: module.id,
            path: &module.path,
            source: &module.source,
            source_type: module.source_type,
            imports: module
                .imports
                .iter()
                .map(|import| {
                    let target = match import.target {
                        ImportTarget::Module(id) => Some(id),
                        _ => None,
                    };
                    (import.specifier.clone(), target)
                })
                .collect(),
        })
        .collect()
}

/// Loads and analyses the fixture at `entry`. A first line `// world: library` selects the
/// open world; the default is the closed (`app`) world.
pub fn load(entry: &Path) -> Loaded {
    let dir = entry.parent().unwrap();
    let source = std::fs::read_to_string(entry).unwrap();
    let closed_world = !source.starts_with("// world: library");
    let tsconfig = find_tsconfig(dir);
    let mut graph = ModuleGraph::new(GraphOptions {
        root: dir.to_path_buf(),
        tsconfig: tsconfig.clone(),
    });
    graph.add_entries(&[entry.to_path_buf()]).unwrap();
    let mut oracle = TsrsOracle::default();
    let sources: Vec<SourceText<'_>> = graph
        .modules()
        .iter()
        .map(|module| SourceText {
            module: module.id,
            path: &module.path,
            source: &module.source,
        })
        .collect();
    oracle.load(&OracleInput {
        root: dir,
        tsconfig: tsconfig.as_deref(),
        modules: &sources,
        check: false,
    });
    let analysis = analyze(
        &inputs(&graph),
        &mut oracle,
        AnalysisOptions {
            closed_world,
            trust_types: false,
        },
    );
    let sources = graph
        .modules()
        .iter()
        .map(|module| (module.path.clone(), module.source.to_string()))
        .collect();
    Loaded {
        graph,
        analysis,
        sources,
        closed_world,
    }
}
