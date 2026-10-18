use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};

use trioxide_analysis::{Analysis, AnalysisOptions};
use trioxide_common::{Diagnostic, ModuleId, log::ModuleLog};
use trioxide_js::{EmitOptions, PassOptions, transform_options};
use trioxide_partition::{
    FileOverride, Partition, PartitionMode, PartitionOptions, Partitioner, Placement, SourceModule,
};
use trioxide_project::{
    CompilerSettings, ContentCache, ContentHash, GraphOptions, ImportTarget, ModuleGraph,
    find_tsconfig,
};
use trioxide_types::{SourceText, TypeOracle, default_oracle};
use trioxide_wasm::{IslandBuilder, IslandSource};

use crate::{
    BuildOutput, Error, ExplainQuery, GlueMode, Invalidation, Mode, ModuleOutput, Options,
    OverrideTarget, Report, Result, World,
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
    /// Fact refreshes during compilation, and the time they took.
    refreshes: (usize, std::time::Duration),
    /// Modules whose last compile dropped exports no module used: an edit elsewhere may start
    /// using one, so [`update`](Self::update) invalidates them too.
    unexported: rustc_hash::FxHashSet<ModuleId>,
    partition: Partition,
    /// Islands from the partition that built: calls to their exports go to their glue.
    links: Vec<islands::IslandLink>,
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
            refreshes: (0, std::time::Duration::ZERO),
            unexported: rustc_hash::FxHashSet::default(),
            partition: Partition::default(),
            links: Vec::new(),
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

        // Islands first: modules only call the islands that built.
        let islands = self.build_islands();
        let mut modules = Vec::with_capacity(self.graph.modules().len());
        for index in 0..self.graph.modules().len() {
            let id = ModuleId::new(u32::try_from(index).expect("module count fits u32"));
            modules.push(self.compile(id));
        }
        Ok(BuildOutput {
            modules,
            islands,
            report: self.report(),
        })
    }

    /// Everything [`build`](Self::build) decides about the program reachable from `entries`,
    /// without building islands: analysis, partitioning and the JS passes, whose decision log
    /// [`explain`](Self::explain) then reports (`trioxide explain`). Fails only if an entry
    /// can't be read.
    pub fn plan(&mut self, entries: &[PathBuf]) -> Result<Report> {
        self.graph.add_entries(entries)?;
        self.built_from_entries = true;
        self.analyse();
        // Without a toolchain a build would run every island as JS; say so now.
        if !self.island_sources().is_empty() {
            let toolchain = trioxide_wasm::toolchain::Toolchain::detect();
            if !toolchain.can_build_islands() {
                let missing: Vec<&str> = toolchain.missing().iter().map(|m| m.what).collect();
                let ids: Vec<_> = self
                    .partition
                    .islands
                    .iter()
                    .map(|p| p.id.clone())
                    .collect();
                self.fall_back_to_js(
                    &ids,
                    &format!(
                        "a build would run it as JS: {} is missing (`trioxide check-toolchain`)",
                        missing.join(" and ")
                    ),
                );
            }
        }
        for index in 0..self.graph.modules().len() {
            let id = ModuleId::new(u32::try_from(index).expect("module count fits u32"));
            self.compile(id);
        }
        Ok(self.report())
    }

    /// Records in the decision log that the islands `ids` run as JS, and why.
    fn fall_back_to_js(&mut self, ids: &[trioxide_wasm::IslandId], why: &str) {
        for (_, decision) in &mut self.partition.decisions {
            if decision.target == trioxide_common::log::Target::Wasm
                && decision
                    .island
                    .as_ref()
                    .is_some_and(|island| ids.iter().any(|id| id.0 == *island))
            {
                decision.target = trioxide_common::log::Target::Js;
                decision.reason = format!("{}, but {why}", decision.reason);
            }
        }
    }

    /// Adds an island to every following [`build`](Self::build), as if the partitioner had
    /// chosen it: a debug and test hook for hand-written islands. Its glue is not wired into
    /// any module; callers import it themselves.
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

    /// The islands this build compiles: debug islands and the partition's.
    fn island_sources(&self) -> Vec<IslandSource> {
        if !self.options.wasm.enabled {
            return Vec::new();
        }
        let mut sources = self.debug_islands.clone();
        for plan in &self.partition.islands {
            let Some(mut source) = plan.source.clone() else {
                continue;
            };
            // Ready-mode glue runs the JS versions until the island is instantiated.
            if (plan.needs_fallback || self.options.wasm.glue == GlueMode::Ready)
                && let Some(module) = plan.module
            {
                let path = &self.graph.module(module).path;
                source.spec.fallback = Some(islands::fallback_specifier(path));
            }
            sources.push(source);
        }
        sources
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
        let failed: Vec<_> = self
            .partition
            .islands
            .iter()
            .filter(|plan| !artifacts.iter().any(|a| a.spec.id == plan.id))
            .map(|plan| plan.id.clone())
            .collect();
        self.fall_back_to_js(
            &failed,
            "its island wasn't built, so it runs as JS (see the warnings)",
        );
        self.links = self
            .partition
            .islands
            .iter()
            .filter(|plan| artifacts.iter().any(|a| a.spec.id == plan.id))
            .filter_map(|plan| {
                Some(islands::IslandLink {
                    id: plan.id.clone(),
                    module: plan.module?,
                    bindings: plan.bindings.clone(),
                    members: plan.functions.clone(),
                    // Ready-mode glue has no top-level await, so the module it falls back to
                    // may import it too (it only calls the JS versions until ready).
                    local_calls: self.options.wasm.glue == GlueMode::Ready || !plan.needs_fallback,
                    fallback: plan.needs_fallback || self.options.wasm.glue == GlueMode::Ready,
                })
            })
            .collect();
        // Outputs compiled before now don't call the islands.
        self.outputs.clear();
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
        let mut dependants = self.graph.dependants(id);
        // A module that dropped an export the edit may now import compiles differently.
        dependants.extend(self.unexported.iter().copied());
        dependants.sort_unstable();
        dependants.dedup();
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
        let mut diagnostics = self.run_analysis(&FxHashMap::default());
        self.analysed = true;
        self.stale = false;
        let partitioner = Partitioner::new(PartitionOptions {
            mode: match self.options.mode {
                Mode::JsOnly => PartitionMode::JsOnly,
                Mode::Auto => PartitionMode::Auto,
                Mode::WasmPreferred => PartitionMode::WasmPreferred,
            },
            wasm: self.options.wasm.enabled,
            cost: self.options.cost_model,
            overrides: self
                .options
                .overrides
                .iter()
                .map(|o| FileOverride {
                    files: o.files.clone(),
                    placement: match o.target {
                        OverrideTarget::Js => Placement::Js,
                        OverrideTarget::Wasm => Placement::Wasm,
                    },
                })
                .collect(),
            root: self.root.clone(),
        });
        let imports: Vec<Vec<ModuleId>> = self
            .graph
            .modules()
            .iter()
            .map(|module| {
                module
                    .imports
                    .iter()
                    .filter_map(|import| match import.target {
                        ImportTarget::Module(id) => Some(id),
                        _ => None,
                    })
                    .collect()
            })
            .collect();
        let sources: Vec<SourceModule<'_>> = self
            .graph
            .modules()
            .iter()
            .zip(&imports)
            .map(|(module, imports)| SourceModule {
                id: module.id,
                path: &module.path,
                source: &module.source,
                source_type: module.source_type,
                imports,
            })
            .collect();
        self.partition = partitioner.partition_sources(&self.analysis.facts, &sources);
        diagnostics.extend(self.partition.diagnostics.iter().cloned());
        self.links.clear();

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
                ..AnalysisOptions::default()
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

    /// How many times compiling modules has re-analysed the program to refresh facts after a
    /// pass group rewrote one (PLAN §3 "Refreshing facts"), and the time that took, since the
    /// project was created.
    pub fn refresh_stats(&self) -> (usize, std::time::Duration) {
        self.refreshes
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
        // Island functions stay, and so do the exports the glue's JS fallback calls.
        let mut keep = trioxide_js::Keep::default();
        for island in &self.partition.islands {
            // Calls anywhere into an island stay calls.
            keep.functions.extend(island.functions.iter().copied());
            keep.exports |= island.module == Some(id)
                || island
                    .functions
                    .iter()
                    .any(|function| function.module == id);
        }
        let input = ModuleSource {
            id,
            path: &module.path,
            source: &module.source,
            source_type: module.source_type,
            keep: &keep,
        };
        let rewrite = (!self.links.is_empty()).then(|| islands::Rewrite {
            module: id,
            links: &self.links,
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
            member_spans: self
                .links
                .iter()
                .flat_map(|link| &link.members)
                .filter(|member| member.module == id)
                .filter_map(|&member| self.analysis.facts.function(member).map(|f| f.span))
                .collect(),
        });
        // Passes see facts refreshed after each group that changes the module; other modules
        // compile against the program's own facts, so those come back afterwards.
        let mut facts =
            FactRefresher::new(&mut *self.oracle, &self.graph, setup, &mut self.analysis);
        let mut output = self
            .compiler
            .compile(&input, &mut facts, log, true, rewrite.as_ref());
        self.refreshes.0 += facts.refreshes();
        self.refreshes.1 += facts.refresh_time();
        facts.restore();
        let unexported = log
            .passes
            .iter()
            .any(|event| event.message.as_deref() == Some(trioxide_js::passes::UNEXPORTED));
        if unexported {
            self.unexported.insert(id);
        } else {
            self.unexported.remove(&id);
        }
        log.functions.extend(
            self.partition
                .decisions
                .iter()
                .filter(|(module, _)| *module == id)
                .map(|(_, decision)| decision.clone()),
        );
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
    let keep = trioxide_js::Keep::default();
    let input = ModuleSource {
        id: ModuleId::new(0),
        path,
        source,
        source_type,
        keep: &keep,
    };
    Ok(project
        .compiler
        .compile(&input, &mut facts, &mut log, false, None))
}
