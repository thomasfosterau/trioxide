use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};

use trioxide_analysis::{Analysis, AnalysisOptions};
use trioxide_common::{Diagnostic, ModuleId, log::ModuleLog};
use trioxide_js::{EmitOptions, PassOptions, transform_options};
use trioxide_partition::{PartitionMode, PartitionOptions, Partitioner};
use trioxide_project::{
    CompilerSettings, ContentCache, ContentHash, GraphOptions, ModuleGraph, find_tsconfig,
};
use trioxide_types::{SourceText, TypeOracle, default_oracle};
use trioxide_wasm::{IslandBuilder, IslandSource};

use crate::{
    BuildOutput, Error, ExplainQuery, Invalidation, Mode, ModuleOutput, Options, Report, Result,
    World,
    analysis::{self, FactRefresher, analysis_inputs},
    islands,
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
    /// Whether the analysis has run (else every fact is unknown, which is safe).
    analysed: bool,
    /// Whether the facts may not match the graph: a source changed, an entry was added, or
    /// they were computed from other text. The next [`transform`](Self::transform)
    /// re-analyses first.
    stale: bool,
    diagnostics: Vec<Diagnostic>,
    debug_islands: Vec<IslandSource>,
    island_builder: Option<Box<dyn IslandBuilder>>,
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
            trust_types: options.trust_types,
            ..AnalysisOptions::default()
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
            analysed: false,
            stale: false,
            diagnostics: warnings,
            debug_islands: Vec::new(),
            island_builder: None,
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
        let islands = self.build_islands();
        Ok(BuildOutput {
            modules,
            islands,
            report: self.report(),
        })
    }

    /// Adds an island to every following [`build`](Self::build), as if the partitioner had
    /// chosen it: a debug and test hook until the Rust backend (S6) and partitioner (S8)
    /// produce islands. Its glue is not wired into any module; callers import it themselves.
    pub fn add_debug_island(&mut self, island: IslandSource) {
        self.debug_islands.push(island);
    }

    /// Replaces the island builder. By default islands are built with cargo, cached in
    /// [`Options::cache_dir`](crate::Options::cache_dir).
    pub fn set_island_builder(&mut self, builder: Box<dyn IslandBuilder>) {
        self.island_builder = Some(builder);
    }

    /// The Rust sources of the islands a [`build`](Self::build) of `entries` would compile,
    /// without compiling them (`--emit=rust`). Fails only if an entry can't be read.
    pub fn rust_islands(&mut self, entries: &[PathBuf]) -> Result<Vec<IslandSource>> {
        self.graph.add_entries(entries)?;
        self.built_from_entries = true;
        self.analyse();
        Ok(self.island_sources())
    }

    /// Writes the `trioxide_rt` crate islands depend on into the island cache and returns
    /// its directory, so emitted island crates can depend on it by path.
    pub fn island_runtime_dir(&self) -> Result<PathBuf> {
        islands::cargo_builder(&self.root, &self.options)
            .materialize_runtime()
            .map_err(|error| Error::Io {
                path: islands::cache_dir(&self.root, &self.options),
                source: std::io::Error::other(error.to_string()),
            })
    }

    /// The islands this build compiles: debug islands for now; S8's plan later.
    fn island_sources(&self) -> Vec<IslandSource> {
        if self.options.wasm.enabled {
            self.debug_islands.clone()
        } else {
            Vec::new()
        }
    }

    /// Builds the planned islands, recording fallbacks and failures as diagnostics.
    fn build_islands(&mut self) -> Vec<trioxide_wasm::IslandArtifact> {
        let sources = self.island_sources();
        if sources.is_empty() {
            return Vec::new();
        }
        let builder = self
            .island_builder
            .get_or_insert_with(|| Box::new(islands::cargo_builder(&self.root, &self.options)));
        let (artifacts, diagnostics) =
            islands::build(builder.as_ref(), &sources, self.options.wasm.require);
        self.diagnostics.extend(diagnostics);
        artifacts
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
        self.stale = true;
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
    /// the graph yet is added as an entry (with everything it imports). Once the program has
    /// been analysed, a transform after an [`update`](Self::update) or a new entry analyses
    /// it again first, so passes never see facts about other text.
    pub fn transform(&mut self, path: &Path) -> Result<ModuleOutput> {
        let id = if let Some(id) = self.graph.get(path) {
            id
        } else {
            self.stale = true;
            self.graph.add_entries(&[path.to_path_buf()])?[0]
        };
        if self.analysed && self.stale {
            self.analyse();
        }
        Ok(self.compile(id))
    }

    /// The analysis facts for the module at `path`, rendered for humans
    /// (`trioxide --emit=facts`). Analyses the program reachable from it (adding it as an
    /// entry if it isn't in the graph yet) under the configured world assumption.
    pub fn facts(&mut self, path: &Path) -> Result<String> {
        let id = match self.graph.get(path) {
            Some(id) => id,
            None => self.graph.add_entries(&[path.to_path_buf()])?[0],
        };
        self.analyse();
        let inputs = analysis_inputs(&self.graph);
        let module = inputs
            .iter()
            .find(|input| input.id == id)
            .expect("the module is in the graph");
        Ok(trioxide_analysis::render(
            &self.analysis.facts,
            module,
            &inputs,
        ))
    }

    /// The analysis facts from the last build or [`facts`](Self::facts) call.
    pub fn analysis(&self) -> &trioxide_analysis::Facts {
        &self.analysis.facts
    }

    /// The decision log, narrowed to `query`. Reflects the last
    /// [`build`](Self::build) or [`transform`](Self::transform) of each module.
    pub fn explain(&self, query: &ExplainQuery) -> Report {
        self.report().filter(query)
    }

    /// Re-analyses the program with some modules' sources replaced, e.g. by the regenerated
    /// (still TypeScript) source of a module the passes rewrote, and returns the new facts.
    ///
    /// The type oracle reloads the replaced sources too. Node ids in the facts are those a
    /// fresh parse of each source gets (`Parser::new(..)` with the module's source type, then
    /// `SemanticBuilder`), so a pass manager continues on a reparse of the text it passed.
    /// Modules not listed keep their graph sources; a path that isn't in the graph is ignored.
    ///
    /// The project keeps these facts (and the oracle the replaced text) until the next
    /// [`build`](Self::build), [`facts`](Self::facts) or [`transform`](Self::transform), which
    /// analyse the graph's own sources again. Cached outputs are dropped.
    pub fn reanalyse(&mut self, sources: &[(PathBuf, String)]) -> &trioxide_analysis::Facts {
        let overrides: FxHashMap<ModuleId, &str> = sources
            .iter()
            .filter_map(|(path, source)| Some((self.graph.get(path)?, source.as_str())))
            .collect();
        let diagnostics = self.run_analysis(&overrides);
        self.diagnostics.extend(diagnostics);
        self.outputs.clear();
        self.analysed = true;
        self.stale = true;
        &self.analysis.facts
    }

    /// Runs the whole-program stages: type oracle, analysis and partitioning.
    fn analyse(&mut self) {
        let diagnostics = self.run_analysis(&FxHashMap::default());
        self.analysed = true;
        self.stale = false;
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

    /// What an analysis run of this project needs besides the graph and the oracle.
    fn setup(&self) -> analysis::Setup {
        let world = self.options.world_or(self.built_from_entries);
        let (jsx_factory, jsx_fragment_factory) = analysis::jsx_factories(&self.settings);
        analysis::Setup {
            root: self.root.clone(),
            tsconfig: self.settings.path.clone(),
            check: self.options.check,
            options: AnalysisOptions {
                closed_world: world == World::App,
                trust_types: self.options.trust_types,
                jsx_factory,
                jsx_fragment_factory,
                entries: self.graph.entries().to_vec(),
            },
        }
    }

    /// Loads the oracle and analyses the graph, with `overrides` in place of some modules'
    /// sources. Returns the diagnostics.
    fn run_analysis(&mut self, overrides: &FxHashMap<ModuleId, &str>) -> Vec<Diagnostic> {
        let setup = self.setup();
        let (analysis, diagnostics) =
            analysis::analyse_graph(&mut *self.oracle, &self.graph, &setup, overrides);
        self.analysis = analysis;
        diagnostics
    }

    /// The id of the graph module at `path`, if it is in the graph.
    pub fn module_id(&self, path: &Path) -> Option<ModuleId> {
        self.graph.get(path)
    }

    /// The project's facts as a [`FactSource`](trioxide_js::FactSource) whose `refresh`
    /// re-analyses a rewritten module (see [`FactRefresher`]), for a pass manager to run with.
    ///
    /// The facts it refreshes replace the project's (and the oracle keeps the rewritten
    /// text) until the next [`build`](Self::build), [`facts`](Self::facts) or
    /// [`transform`](Self::transform), which analyse the graph's own sources again. Cached
    /// outputs are dropped.
    pub fn fact_refresher(&mut self) -> FactRefresher<'_> {
        self.stale = true;
        self.outputs.clear();
        let setup = self.setup();
        FactRefresher::new(&mut *self.oracle, &self.graph, setup, &mut self.analysis)
    }

    fn compile(&mut self, id: ModuleId) -> ModuleOutput {
        let module = self.graph.module(id);
        let key = module.hash.combine(self.options_hash);
        if let Some(cached) = self.outputs.get(id, key) {
            return cached.clone();
        }
        let setup = self.setup();
        let module = self.graph.module(id);
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
        // Passes see facts refreshed after each group that changes the module; other modules
        // compile against the program's own facts, so those come back afterwards.
        let mut facts =
            FactRefresher::new(&mut *self.oracle, &self.graph, setup, &mut self.analysis);
        let mut output = self.compiler.compile(&input, &mut facts, log, true);
        facts.restore();
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
