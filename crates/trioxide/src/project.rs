use std::path::{Path, PathBuf};

use trioxide_analysis::{Analysis, AnalysisOptions, ModuleInput, analyze};
use trioxide_common::{Diagnostic, ModuleId, log::ModuleLog};
use trioxide_js::{EmitOptions, PassOptions, transform_options};
use trioxide_partition::{PartitionMode, PartitionOptions, Partitioner};
use trioxide_project::{
    CompilerSettings, ContentCache, ContentHash, GraphOptions, ImportTarget, ModuleGraph,
    find_tsconfig,
};
use trioxide_types::{OracleInput, SourceText, TypeOracle, default_oracle};

use crate::{
    BuildOutput, Error, ExplainQuery, Invalidation, Mode, ModuleOutput, Options, Report, Result,
    World,
    pipeline::{ModuleCompiler, ModuleSource},
    report::ModuleReport,
};

/// A TypeScript project being compiled: the module graph plus every cache (PLAN D8).
///
/// The core is whole-program: [`build`](Self::build) loads the graph from entry points, asks
/// the type oracle about every module, analyses, partitions and emits. Per-file consumers
/// (the Vite plugin's `transform` hook) call [`transform`](Self::transform), which serves
/// cached results and compiles modules it hasn't seen on demand. [`update`](Self::update)
/// feeds edits in.
///
/// Outputs are cached by content hash (module source combined with the options), so repeated
/// transforms of unchanged modules are free.
pub struct Project {
    root: PathBuf,
    options: Options,
    settings: CompilerSettings,
    graph: ModuleGraph,
    oracle: Box<dyn TypeOracle>,
    analysis: Analysis,
    compiler: ModuleCompiler,
    options_hash: ContentHash,
    outputs: ContentCache<ModuleOutput>,
    logs: Vec<ModuleLog>,
    built_from_entries: bool,
    diagnostics: Vec<Diagnostic>,
}

impl std::fmt::Debug for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Project")
            .field("root", &self.root)
            .field("graph", &self.graph)
            .finish_non_exhaustive()
    }
}

impl Project {
    /// Creates a project rooted at `root` (relative paths are resolved against it; a relative
    /// `root` against the working directory). The root need not exist, e.g. when only
    /// in-memory sources are transformed.
    ///
    /// Reads the tsconfig (`options.tsconfig`, else the nearest `tsconfig.json` at or above
    /// `root`). Nothing else is loaded until [`build`](Self::build) or
    /// [`transform`](Self::transform).
    pub fn new(root: PathBuf, options: Options) -> Result<Self> {
        let root = if root.is_absolute() {
            root
        } else {
            let cwd = std::env::current_dir().map_err(|source| Error::Io {
                path: root.clone(),
                source,
            })?;
            cwd.join(root)
        };
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        let tsconfig = match &options.tsconfig {
            Some(path) => Some(root.join(path)),
            None => find_tsconfig(&root),
        };
        let mut settings = match &tsconfig {
            Some(path) => CompilerSettings::load(path)?,
            None => CompilerSettings::without_tsconfig(),
        };
        if let Some(target) = &options.target {
            settings.target = Some(target.to_ascii_lowercase());
        }

        let (transform, warnings) = transform_options(&settings);
        let compiler = ModuleCompiler {
            emit: EmitOptions {
                transform,
                source_map: options.sourcemap,
            },
            passes: PassOptions {
                enabled: options.passes.clone(),
                trust_types: options.trust_types,
            },
        };
        let options_hash = ContentHash::of(
            serde_json::to_string(&(&options, &settings))
                .expect("options serialise")
                .as_bytes(),
        );
        let analysis = Analysis::empty(AnalysisOptions {
            closed_world: false,
            trust_types: options.trust_types,
        });
        let graph = ModuleGraph::new(GraphOptions {
            root: root.clone(),
            tsconfig,
        });
        Ok(Self {
            root,
            options,
            settings,
            graph,
            oracle: default_oracle(),
            analysis,
            compiler,
            options_hash,
            outputs: ContentCache::default(),
            logs: Vec::new(),
            built_from_entries: false,
            diagnostics: warnings,
        })
    }

    /// The canonical project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The options the project was created with.
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// The tsconfig settings in effect.
    pub fn compiler_settings(&self) -> &CompilerSettings {
        &self.settings
    }

    /// The module graph.
    pub fn graph(&self) -> &ModuleGraph {
        &self.graph
    }

    /// Compiles the whole program reachable from `entries` (PLAN §2 steps 1–8).
    ///
    /// Fails only if an entry can't be read; per-module problems are diagnostics on the
    /// outputs.
    pub fn build(&mut self, entries: &[PathBuf]) -> Result<BuildOutput> {
        self.graph.add_entries(entries)?;
        self.built_from_entries = true;
        self.analyse();

        let mut modules = Vec::with_capacity(self.graph.modules().len());
        for index in 0..self.graph.modules().len() {
            let id = ModuleId::new(u32::try_from(index).expect("module count fits u32"));
            modules.push(self.compile(id));
        }
        Ok(BuildOutput {
            modules,
            islands: Vec::new(),
            report: self.report(),
        })
    }

    /// Records a change to `path`: new in-memory contents (`Some`), or "re-read from disk"
    /// (`None`). Returns what needs invalidating. Paths not in the graph are remembered and
    /// used if they are loaded later.
    pub fn update(&mut self, path: &Path, source: Option<String>) -> Result<Invalidation> {
        let Some(id) = self.graph.set_source(path, source) else {
            return Ok(Invalidation::default());
        };
        let module = self.graph.module(id);
        self.oracle.update(SourceText {
            module: id,
            path: &module.path,
            source: &module.source,
        });
        let dependants = self.graph.dependants(id);
        Ok(Invalidation {
            changed: Some(module.path.clone()),
            dependants: dependants
                .into_iter()
                .filter(|&dependant| dependant != id)
                .map(|dependant| self.graph.module(dependant).path.clone())
                .collect(),
        })
    }

    /// The compiled form of one module, for per-file consumers such as the Vite plugin.
    ///
    /// Served from cache when the module and options are unchanged. A module that isn't in
    /// the graph yet is added as an entry (with everything it imports).
    pub fn transform(&mut self, path: &Path) -> Result<ModuleOutput> {
        let id = match self.graph.get(path) {
            Some(id) => id,
            None => self.graph.add_entries(&[path.to_path_buf()])?[0],
        };
        Ok(self.compile(id))
    }

    /// The decision log, narrowed to `query`. Reflects the last
    /// [`build`](Self::build) or [`transform`](Self::transform) of each module.
    pub fn explain(&self, query: &ExplainQuery) -> Report {
        self.report().filter(query)
    }

    /// Runs the whole-program stages: type oracle, analysis and partitioning.
    fn analyse(&mut self) {
        let modules = self.graph.modules();
        let sources: Vec<SourceText<'_>> = modules
            .iter()
            .map(|module| SourceText {
                module: module.id,
                path: &module.path,
                source: &module.source,
            })
            .collect();
        let input = OracleInput {
            root: &self.root,
            tsconfig: self.settings.path.as_deref(),
            modules: &sources,
            check: self.options.check,
        };
        let mut diagnostics = self.oracle.load(&input);
        if self.options.check {
            for module in modules {
                diagnostics.extend(self.oracle.diagnostics(module.id));
            }
        }

        let world = self.options.world_or(self.built_from_entries);
        let inputs = analysis_inputs(&self.graph);
        self.analysis = analyze(
            &inputs,
            &mut *self.oracle,
            AnalysisOptions {
                closed_world: world == World::App,
                trust_types: self.options.trust_types,
            },
        );
        diagnostics.append(&mut self.analysis.diagnostics);

        let partitioner = Partitioner::new(PartitionOptions {
            mode: match self.options.mode {
                Mode::JsOnly => PartitionMode::JsOnly,
                Mode::Auto => PartitionMode::Auto,
                Mode::WasmPreferred => PartitionMode::WasmPreferred,
            },
            wasm: self.options.wasm.enabled,
        });
        let partition = partitioner.partition(&self.analysis.facts);
        debug_assert!(partition.is_all_js(), "island emission is not wired up yet");

        // The facts changed, so outputs computed against older facts are stale.
        self.outputs.clear();
        self.diagnostics
            .retain(|diagnostic| diagnostic.code.as_deref() == Some("trioxide(target)"));
        self.diagnostics.extend(diagnostics);
    }

    fn compile(&mut self, id: ModuleId) -> ModuleOutput {
        let module = self.graph.module(id);
        let key = module.hash.combine(self.options_hash);
        if let Some(cached) = self.outputs.get(id, key) {
            return cached.clone();
        }
        if self.logs.len() <= id.index() {
            self.logs.resize_with(id.index() + 1, ModuleLog::default);
        }
        let log = &mut self.logs[id.index()];
        *log = ModuleLog::default();
        let input = ModuleSource {
            id,
            path: &module.path,
            source: &module.source,
            source_type: module.source_type,
        };
        let mut output = self
            .compiler
            .compile(&input, &mut self.analysis.facts, log, true);
        let mut diagnostics = module.diagnostics.clone();
        diagnostics.append(&mut output.diagnostics);
        output.diagnostics = diagnostics;
        self.outputs.insert(id, key, output.clone());
        output
    }

    fn report(&self) -> Report {
        Report {
            type_oracle: self.oracle.name().to_string(),
            modules: self
                .graph
                .modules()
                .iter()
                .map(|module| ModuleReport {
                    path: module.path.clone(),
                    log: self
                        .logs
                        .get(module.id.index())
                        .cloned()
                        .unwrap_or_default(),
                })
                .collect(),
            diagnostics: self.diagnostics.clone(),
        }
    }
}

/// The analysis inputs for every module of `graph`.
fn analysis_inputs(graph: &ModuleGraph) -> Vec<ModuleInput<'_>> {
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

/// Strips TypeScript from one module without analysis or optimisation: plain
/// `oxc_transformer` output with the same tsconfig handling as a [`Project`].
///
/// This is the baseline the differential suite compares trioxide's output against.
pub fn strip_types(
    root: &Path,
    path: &Path,
    source: &str,
    options: &Options,
) -> Result<ModuleOutput> {
    let project = Project::new(root.to_path_buf(), options.clone())?;
    let source_type =
        oxc_span::SourceType::from_path(path).unwrap_or_else(|_| oxc_span::SourceType::ts());
    let mut facts = trioxide_analysis::Facts::new(false);
    let mut log = ModuleLog::default();
    let input = ModuleSource {
        id: ModuleId::new(0),
        path,
        source,
        source_type,
    };
    Ok(project
        .compiler
        .compile(&input, &mut facts, &mut log, false))
}
