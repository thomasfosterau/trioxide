//! The tsrs-backed [`TypeOracle`]. Nothing in this module may leak a tsrs type through the
//! crate's public API: callers get trioxide types, strings or spans only.
//!
//! # Program lifecycle
//!
//! [`TsrsOracle::load`] builds one tsrs program over an [`OverlayFs`] (the project's sources in
//! memory, everything else from disk, the bundled `lib.d.ts` files embedded). Updates are
//! applied lazily: the next query replaces the program with `update_program`, which reparses
//! only the changed file. tsrs never frees a parsed file on its own; [`RebuildPolicy`] decides
//! when to start from scratch instead, which lets the oracle give the old program's memory
//! back (PLAN §3, "Embedding constraints").
//!
//! # Checkers
//!
//! tsrs checkers take `&mut self`, and types from different checkers must never meet. Through
//! `&mut self` the oracle runs every query on the program's first checker; through its
//! [`SharedTypeQueries`] view, which several threads may use at once, a query runs on the
//! checker the program's pool assigns to the queried file, holding that checker's lock. Either
//! way each answer is converted before the checker is released, so no tsrs handle outlives a
//! query.

mod convert;
mod fs;
mod spanmap;

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use rustc_hash::FxHashMap;
use trioxide_common::{Diagnostic, ModuleId, NodeId, NodeRef, Severity, Span};
use tsrs_ast::{self as ast, Node, SourceFile};
use tsrs_checker::CheckMode;
use tsrs_compiler::{
    Checker, Context, Program, ProgramOptions, ProgramRegions, new_compiler_host, new_program,
};
use tsrs_core::{P, tspath};
use tsrs_tsoptions::{ParseConfigHost, ParsedCommandLine, get_parsed_command_line_of_config_file};
use tsrs_vfs::{FS, bundled};

use self::{
    convert::{Converter, Globals},
    fs::OverlayFs,
    spanmap::SpanMap,
};
use crate::{
    CheckerQueries, OracleInput, SharedTypeQueries, Signature, SourceText, TsTypeFlags, Type,
    TypeOracle,
};

/// When the oracle discards its incrementally updated program and builds a fresh one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebuildPolicy {
    /// Rebuild after this many incremental updates.
    pub max_updates: usize,
    /// Rebuild when the process's resident memory exceeds this many bytes (checked on each
    /// update; Linux only, ignored elsewhere).
    pub max_resident_bytes: Option<u64>,
}

impl Default for RebuildPolicy {
    fn default() -> Self {
        Self {
            max_updates: 100,
            max_resident_bytes: Some(4 << 30),
        }
    }
}

/// Counters for how many queries found a checker node for their span.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpanStats {
    /// Queries whose span matched a node.
    pub matched: u64,
    /// Queries whose span matched nothing (answered `None`).
    pub unmatched: u64,
}

/// Counters for the program's lifecycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProgramStats {
    /// Programs built from scratch.
    pub full_builds: u64,
    /// Incremental updates applied with `update_program`.
    pub updates: u64,
    /// Updates where tsrs could not reuse the old program and rebuilt it itself.
    pub update_fallbacks: u64,
    /// Queries abandoned because tsrs panicked (the program is rebuilt afterwards).
    pub panics: u64,
}

/// The parse-config host must be `'static`; it lives as long as the oracle's file system.
struct ConfigHost {
    fs: Arc<OverlayFs>,
    cwd: String,
}

impl ParseConfigHost for ConfigHost {
    fn fs(&self) -> &dyn FS {
        &*self.fs
    }

    fn get_current_directory(&self) -> &str {
        &self.cwd
    }
}

/// A live program and what must be released with it.
struct Live {
    program: &'static Program,
    /// The file data every version since the last full build shares, recorded when that
    /// build was made (the build itself is freed once an update replaces it).
    shared: tsrs_compiler::SharedProgramData,
    /// The file loader's regions of `full_build`.
    regions: Option<Arc<ProgramRegions>>,
}

struct ModuleEntry {
    /// The normalised path tsrs knows the module by.
    path: String,
    /// Registered declarations by span.
    declarations: FxHashMap<(u32, u32), NodeId>,
}

/// A [`TypeOracle`] backed by the tsrs TypeScript checker.
pub struct TsrsOracle {
    fs: Arc<OverlayFs>,
    root: String,
    tsconfig: Option<String>,
    check: bool,
    modules: FxHashMap<ModuleId, ModuleEntry>,
    by_path: FxHashMap<String, ModuleId>,
    config: Option<P<ParsedCommandLine>>,
    live: Option<Live>,
    dirty: Vec<String>,
    updates_since_build: usize,
    policy: RebuildPolicy,
    globals: Option<Globals>,
    /// The globals of each checker the shared view used, by checker address.
    shared_globals: Mutex<FxHashMap<usize, Arc<Globals>>>,
    matched: AtomicU64,
    unmatched: AtomicU64,
    /// A query through the shared view panicked: rebuild before the next use.
    shared_panicked: AtomicBool,
    program_stats: ProgramStats,
}

impl std::fmt::Debug for TsrsOracle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TsrsOracle")
            .field("root", &self.root)
            .field("modules", &self.modules.len())
            .finish_non_exhaustive()
    }
}

// SAFETY: tsrs declares its handles (`P<T>`) and programs usable from any thread ("by decree",
// tsrs PORTING.md "Threading"); every checker is reached through the program's pool, which
// locks it.
unsafe impl Send for TsrsOracle {}

// SAFETY: as for `Send`. The only `&self` methods that touch tsrs are the shared view's
// queries, which take the queried file's checker exclusively (`get_type_checker_for_file_
// exclusive`) and otherwise only read the oracle's maps; its mutable bookkeeping is atomics and
// a mutex. Everything else that changes the oracle takes `&mut self`.
unsafe impl Sync for TsrsOracle {}

impl Default for TsrsOracle {
    fn default() -> Self {
        Self::new(RebuildPolicy::default())
    }
}

impl TsrsOracle {
    /// An oracle over the real file system that rebuilds according to `policy`.
    pub fn new(policy: RebuildPolicy) -> Self {
        Self::with_fs(OverlayFs::over_os(), policy)
    }

    fn with_fs(fs: OverlayFs, policy: RebuildPolicy) -> Self {
        Self {
            fs: Arc::new(fs),
            root: "/".into(),
            tsconfig: None,
            check: false,
            modules: FxHashMap::default(),
            by_path: FxHashMap::default(),
            config: None,
            live: None,
            dirty: Vec::new(),
            updates_since_build: 0,
            policy,
            globals: None,
            shared_globals: Mutex::default(),
            matched: AtomicU64::new(0),
            unmatched: AtomicU64::new(0),
            shared_panicked: AtomicBool::new(false),
            program_stats: ProgramStats::default(),
        }
    }

    /// How many queries found a node for their span.
    pub fn span_stats(&self) -> SpanStats {
        SpanStats {
            matched: self.matched.load(Ordering::Relaxed),
            unmatched: self.unmatched.load(Ordering::Relaxed),
        }
    }

    /// How many checkers the current program has (each answers queries for its own files
    /// through the shared view).
    pub fn checker_count(&self) -> usize {
        self.live
            .as_ref()
            .map_or(0, |live| live.program.checker_count())
    }

    /// How often the program was built and updated.
    pub fn program_stats(&self) -> ProgramStats {
        self.program_stats
    }

    /// The number of files in the current program (sources and libraries), building it first
    /// if an update is pending.
    pub fn file_count(&mut self) -> usize {
        self.program()
            .map_or(0, |program| program.source_files().len())
    }

    /// Forces the pending updates (if any) to be applied now.
    pub fn flush(&mut self) {
        let _ = self.program();
    }

    fn config_host(&self) -> &'static ConfigHost {
        // Leaked like the programs it configures; one per load.
        Box::leak(Box::new(ConfigHost {
            fs: Arc::clone(&self.fs),
            cwd: self.root.clone(),
        }))
    }

    /// Parses the tsconfig, or makes a default one, and adds every module as a root file.
    fn parse_config(&mut self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let config_path = if let Some(path) = &self.tsconfig {
            path.clone()
        } else {
            // A config only the checker sees; strict, modern defaults.
            let path = tspath::combine_paths(&self.root, &["tsconfig.trioxide.json"]);
            self.fs.set(&path, Arc::from(DEFAULT_TSCONFIG));
            path
        };
        let (config, errors) = get_parsed_command_line_of_config_file(
            &config_path,
            None,
            None,
            self.config_host(),
            None,
        );
        let mut config = config;
        if let Some(config) = &mut config {
            // Trioxide supplies the root files itself (below), so a config listing none is fine.
            config
                .errors
                .retain(|error| !NO_INPUTS_CODES.contains(&error.code()));
        }
        for error in errors
            .iter()
            .chain(config.iter().flat_map(|c| c.errors.iter()))
        {
            diagnostics.push(
                Diagnostic::warning(format!("tsconfig: {}", error.localize()))
                    .with_code("trioxide(tsconfig)")
                    .with_file(&config_path),
            );
        }
        let Some(mut config) = config else {
            self.config = None;
            return diagnostics;
        };
        let mut paths: Vec<&String> = self.by_path.keys().collect();
        paths.sort();
        let case_sensitive = self.fs.use_case_sensitive_file_names();
        let known: rustc_hash::FxHashSet<String> = config
            .parsed_config
            .file_names
            .iter()
            .map(|name| path_key(name, case_sensitive))
            .collect();
        for path in paths {
            if !known.contains(&path_key(path, case_sensitive)) {
                config.parsed_config.file_names.push(path.clone());
            }
        }
        self.config = Some(P::new(config));
        diagnostics
    }

    /// Releases every program. Nothing may hold a tsrs handle when this runs.
    fn release(&mut self) {
        self.forget_globals();
        if let Some(live) = self.live.take() {
            // SAFETY: the oracle is the only owner of these programs. No checker of theirs is
            // held (handles never outlive a query), and the span maps and globals that pointed
            // into them were dropped above. `program` is the newest version; earlier versions
            // were freed as they were replaced (`apply_updates`). The shared data and regions go
            // last, after every program that used them (tsrs watcher.rs `install_program`).
            unsafe {
                tsrs_compiler::free_program(live.program);
                live.shared.free();
            }
            drop(live.regions);
        }
    }

    fn build_from_scratch(&mut self) {
        self.release();
        self.dirty.clear();
        self.updates_since_build = 0;
        let Some(config) = self.config else {
            return;
        };
        let host = self.host();
        let mut options = ProgramOptions::new(config, host);
        options.checker_regions = true;
        options.checker_diagnostic_regions = true;
        let regions = Arc::new(ProgramRegions::default());
        options.regions = Some(Arc::clone(&regions));
        let program = new_program(options);
        program.bind_source_files();
        self.program_stats.full_builds += 1;
        self.live = Some(Live {
            program,
            shared: tsrs_compiler::shared_program_data(program),
            regions: Some(regions),
        });
    }

    fn host(&self) -> Arc<dyn tsrs_compiler::CompilerHost> {
        let fs: Arc<dyn FS> = Arc::new(bundled::wrap_fs(Arc::clone(&self.fs)));
        new_compiler_host(&self.root, fs, &bundled::lib_path(), None, None, None)
    }

    fn over_budget(&self) -> bool {
        if self.updates_since_build >= self.policy.max_updates {
            return true;
        }
        match (self.policy.max_resident_bytes, resident_bytes()) {
            (Some(limit), Some(resident)) => resident > limit,
            _ => false,
        }
    }

    fn apply_updates(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        if self.live.is_none() || self.over_budget() {
            self.build_from_scratch();
            return;
        }
        self.forget_globals();
        let dirty = std::mem::take(&mut self.dirty);
        for path in dirty {
            let live = self.live.as_ref().expect("a live program");
            let Some(file) = live.program.get_source_file(&path) else {
                // Not in the program (e.g. a new module): only a full build picks it up.
                self.build_from_scratch();
                return;
            };
            let host = self.host();
            let (program, _, reused) = live.program.update_program(file.path(), host, None, None);
            program.bind_source_files();
            let old = live.program;
            self.program_stats.updates += 1;
            self.updates_since_build += 1;
            if reused {
                // SAFETY: `program` was made from `old`, which nothing else refers to: handles
                // never outlive a query and the span maps were dropped above.
                unsafe { tsrs_compiler::free_program(old) };
                self.live.as_mut().expect("a live program").program = program;
            } else {
                // tsrs rebuilt from scratch itself; the new program shares nothing with ours.
                self.program_stats.update_fallbacks += 1;
                let live = self.live.take().expect("a live program");
                // SAFETY: as above for `old`; the earlier versions were freed as they were
                // replaced, so the shared data of the last full build has no users left.
                unsafe {
                    tsrs_compiler::free_program(old);
                    live.shared.free();
                }
                drop(live.regions);
                let live = Live {
                    program,
                    shared: tsrs_compiler::shared_program_data(program),
                    regions: None,
                };
                self.live = Some(live);
            }
        }
    }

    /// Drops every handle into the current program's checkers.
    fn forget_globals(&mut self) {
        self.globals = None;
        self.shared_globals
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Forces a rebuild after a checker panicked: it may be inconsistent.
    fn schedule_rebuild(&mut self) {
        self.forget_globals();
        if self.live.is_some()
            && let Some(entry) = self.modules.values().next()
        {
            self.dirty.push(entry.path.clone());
            self.updates_since_build = self.policy.max_updates.max(1);
        }
    }

    fn program(&mut self) -> Option<&'static Program> {
        if self.shared_panicked.swap(false, Ordering::Relaxed) {
            self.program_stats.panics += 1;
            self.schedule_rebuild();
        }
        self.apply_updates();
        self.live.as_ref().map(|live| live.program)
    }

    fn source_file(&mut self, module: ModuleId) -> Option<P<SourceFile>> {
        let program = self.program()?;
        let path = &self.modules.get(&module)?.path;
        program.get_source_file(path)
    }

    /// The checker node matching `span`, counted in the span statistics.
    fn node_at(&mut self, module: ModuleId, span: Span) -> Option<P<Node>> {
        let file = self.source_file(module)?;
        self.node_in(file, span)
    }

    /// The checker node of `file` matching `span`, counted in the span statistics.
    fn node_in(&self, file: P<SourceFile>, span: Span) -> Option<P<Node>> {
        let node = SpanMap::build(file).get(span.start, span.end);
        let counter = if node.is_some() {
            &self.matched
        } else {
            &self.unmatched
        };
        counter.fetch_add(1, Ordering::Relaxed);
        node
    }

    /// The globals of `checker`, made on its first query through the shared view.
    fn shared_globals_of(&self, checker: &mut Checker) -> Arc<Globals> {
        let key = std::ptr::from_mut(checker).addr();
        let mut map = self
            .shared_globals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            map.entry(key)
                .or_insert_with(|| Arc::new(Globals::new(checker))),
        )
    }

    /// Runs `query` on the program's first checker. A tsrs panic is caught, answered `None`,
    /// and the program rebuilt before the next query.
    fn with_checker<R>(
        &mut self,
        module: ModuleId,
        span: Span,
        query: impl FnOnce(&mut Checker, P<Node>, &mut QueryCx<'_>) -> Option<R>,
    ) -> Option<R> {
        let node = self.node_at(module, span)?;
        let program = self.program()?;
        let mut handle = program.get_type_checker(&Context::default());
        let checker: &mut Checker = &mut handle;
        let globals = self.globals.get_or_insert_with(|| Globals::new(checker));
        let mut cx = QueryCx {
            modules: &self.modules,
            by_path: &self.by_path,
            globals,
            case_sensitive: self.fs.use_case_sensitive_file_names(),
        };
        let result = catch_unwind(AssertUnwindSafe(|| query(checker, node, &mut cx)));
        drop(handle);
        result.unwrap_or_else(|_| {
            self.program_stats.panics += 1;
            self.schedule_rebuild();
            None
        })
    }

    fn module_diagnostics(&mut self, module: ModuleId) -> Vec<Diagnostic> {
        let Some(file) = self.source_file(module) else {
            return Vec::new();
        };
        let Some(program) = self.program() else {
            return Vec::new();
        };
        let path = self.module_path(module);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let ctx = Context::default();
            let mut diagnostics = program.get_syntactic_diagnostics(&ctx, Some(file));
            diagnostics.extend(program.get_semantic_diagnostics(&ctx, Some(file)));
            diagnostics
                .iter()
                .map(|diagnostic| convert_diagnostic(diagnostic, &path))
                .collect::<Vec<_>>()
        }));
        result.unwrap_or_else(|_| {
            self.program_stats.panics += 1;
            vec![
                Diagnostic::warning("the TypeScript checker failed on this module")
                    .with_code("trioxide(checker)")
                    .with_file(path),
            ]
        })
    }

    fn module_path(&self, module: ModuleId) -> String {
        self.modules
            .get(&module)
            .map(|entry| entry.path.clone())
            .unwrap_or_default()
    }
}

/// What a query needs besides the checker.
struct QueryCx<'a> {
    modules: &'a FxHashMap<ModuleId, ModuleEntry>,
    by_path: &'a FxHashMap<String, ModuleId>,
    globals: &'a Globals,
    case_sensitive: bool,
}

impl QueryCx<'_> {
    fn module_of(&self, file: P<SourceFile>) -> Option<ModuleId> {
        self.by_path
            .get(&path_key(file.file_name(), self.case_sensitive))
            .copied()
    }

    /// The oxc node registered for a class or function declaration.
    fn resolve(&self, declaration: P<Node>) -> Option<NodeRef> {
        let file = ast::get_source_file_of_node(declaration)?;
        let module = self.module_of(file)?;
        let entry = self.modules.get(&module)?;
        let span = |node: P<Node>| {
            let start = tsrs_astnav::get_start_of_node(node, file, false);
            Some((start, node.end().source_offset()?))
        };
        let by_name = declaration.name().and_then(span);
        let by_node = span(declaration);
        [by_name, by_node]
            .into_iter()
            .flatten()
            .find_map(|key| entry.declarations.get(&key))
            .map(|&node| NodeRef { module, node })
    }

    fn converter<'c>(
        &'c self,
        checker: &'c mut Checker,
        resolve: &'c dyn Fn(P<Node>) -> Option<NodeRef>,
    ) -> Converter<'c> {
        Converter {
            checker,
            globals: self.globals,
            resolve,
        }
    }

    /// The span of a declaration's name (or of the declaration itself), and its module.
    fn declaration_span(&self, declaration: P<Node>) -> Option<(ModuleId, Span)> {
        let file = ast::get_source_file_of_node(declaration)?;
        let module = self.module_of(file)?;
        let node = ast::get_name_of_declaration(Some(declaration)).unwrap_or(declaration);
        let start = tsrs_astnav::get_start_of_node(node, file, false);
        Some((module, Span::new(start, node.end().source_offset()?)))
    }
}

/// The symbol at `node`, with import aliases resolved to what they import.
fn resolved_symbol(checker: &mut Checker, node: P<Node>) -> Option<P<tsrs_ast::Symbol>> {
    let symbol = checker.get_symbol_at_location_exported(node)?;
    if symbol.flags().intersects(tsrs_ast::SymbolFlags::Alias) {
        let target = checker.get_aliased_symbol(symbol);
        return (target != checker.get_unknown_symbol()).then_some(target);
    }
    Some(symbol)
}

fn convert_flags(ty: P<tsrs_checker::Type>) -> TsTypeFlags {
    use tsrs_checker::TypeFlags as F;
    let members: Vec<P<tsrs_checker::Type>> = if ty.flags().intersects(F::Union) {
        ty.types().to_vec()
    } else {
        vec![ty]
    };
    let any = |flags: F| {
        members
            .iter()
            .any(|member| member.flags().intersects(flags))
    };
    TsTypeFlags {
        any: any(F::Any),
        unknown: any(F::Unknown),
        literal: members.iter().all(|member| {
            member.flags().intersects(
                F::StringLiteral | F::NumberLiteral | F::BooleanLiteral | F::BigIntLiteral,
            )
        }),
        generic: any(F::TypeParameter | F::IndexedAccess | F::Conditional | F::Substitution),
        nullable: any(F::Null | F::Undefined | F::Void),
    }
}

impl SharedTypeQueries for TsrsOracle {
    fn type_at(&self, module: ModuleId, span: Span) -> Option<Type> {
        let program = self.live.as_ref()?.program;
        let file = program.get_source_file(&self.modules.get(&module)?.path)?;
        let node = self.node_in(file, span)?;
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut handle = program.get_type_checker_for_file_exclusive(&Context::default(), file);
            let checker: &mut Checker = &mut handle;
            let globals = self.shared_globals_of(checker);
            self.query_type(checker, &globals, node)
        }));
        result
            .inspect_err(|_| self.shared_panicked.store(true, Ordering::Relaxed))
            .ok()
    }

    fn workers(&self) -> usize {
        self.checker_count().max(1)
    }

    fn run_workers(&self, work: &(dyn Fn(usize, &mut dyn CheckerQueries) + Sync)) {
        let Some(live) = &self.live else {
            work(0, &mut NoProgram);
            return;
        };
        live.program.for_each_checker_parallel(|index, checker| {
            let globals = Globals::new(checker);
            let mut queries = BoundQueries {
                oracle: self,
                checker,
                globals,
            };
            work(index, &mut queries);
        });
    }
}

impl TsrsOracle {
    /// The converted type at `node`, asked of `checker`.
    fn query_type(&self, checker: &mut Checker, globals: &Globals, node: P<Node>) -> Type {
        let cx = QueryCx {
            modules: &self.modules,
            by_path: &self.by_path,
            globals,
            case_sensitive: self.fs.use_case_sensitive_file_names(),
        };
        let ty = checker.get_type_at_location(node);
        let resolve = |declaration| cx.resolve(declaration);
        cx.converter(checker, &resolve).convert(ty)
    }
}

/// A worker's queries, all on the checker it holds.
struct BoundQueries<'q> {
    oracle: &'q TsrsOracle,
    checker: &'q mut Checker,
    globals: Globals,
}

impl CheckerQueries for BoundQueries<'_> {
    fn type_at(&mut self, module: ModuleId, span: Span) -> Option<Type> {
        let oracle = self.oracle;
        let program = oracle.live.as_ref()?.program;
        let file = program.get_source_file(&oracle.modules.get(&module)?.path)?;
        let node = oracle.node_in(file, span)?;
        let result = catch_unwind(AssertUnwindSafe(|| {
            oracle.query_type(self.checker, &self.globals, node)
        }));
        result
            .inspect_err(|_| oracle.shared_panicked.store(true, Ordering::Relaxed))
            .ok()
    }
}

/// The queries of an oracle without a program: nothing is known.
struct NoProgram;

impl CheckerQueries for NoProgram {
    fn type_at(&mut self, _: ModuleId, _: Span) -> Option<Type> {
        None
    }
}

impl TypeOracle for TsrsOracle {
    fn name(&self) -> &'static str {
        "tsrs"
    }

    fn shared(&mut self) -> Option<&dyn SharedTypeQueries> {
        self.program()?;
        Some(self)
    }

    fn load(&mut self, input: &OracleInput<'_>) -> Vec<Diagnostic> {
        self.release();
        self.root = normalize(input.root);
        self.tsconfig = input.tsconfig.map(normalize);
        self.check = input.check;
        self.modules.clear();
        self.by_path.clear();
        let case_sensitive = self.fs.use_case_sensitive_file_names();
        for module in input.modules {
            let path = normalize(module.path);
            self.fs.set(&path, Arc::from(module.source));
            self.by_path
                .insert(path_key(&path, case_sensitive), module.module);
            self.modules.insert(
                module.module,
                ModuleEntry {
                    path,
                    declarations: FxHashMap::default(),
                },
            );
        }
        let mut diagnostics = self.parse_config();
        self.build_from_scratch();
        if let Some(live) = &self.live {
            let mut program_errors = live.program.get_config_file_parsing_diagnostics();
            program_errors.extend(live.program.get_program_diagnostics());
            if self.check {
                for error in program_errors {
                    diagnostics.push(convert_diagnostic(&error, ""));
                }
            }
        }
        diagnostics
    }

    fn update(&mut self, module: SourceText<'_>) {
        let path = normalize(module.path);
        self.fs.set(&path, Arc::from(module.source));
        let case_sensitive = self.fs.use_case_sensitive_file_names();
        if !self.modules.contains_key(&module.module) {
            self.by_path
                .insert(path_key(&path, case_sensitive), module.module);
            self.modules.insert(
                module.module,
                ModuleEntry {
                    path: path.clone(),
                    declarations: FxHashMap::default(),
                },
            );
            // A new root file needs a new root list.
            let _ = self.parse_config();
            self.updates_since_build = self.policy.max_updates.max(1);
        }
        if !self.dirty.contains(&path) {
            self.dirty.push(path);
        }
        if let Some(entry) = self.modules.get_mut(&module.module) {
            entry.declarations.clear();
        }
    }

    fn type_at(&mut self, module: ModuleId, span: Span) -> Option<Type> {
        self.with_checker(module, span, |checker, node, cx| {
            let ty = checker.get_type_at_location(node);
            let resolve = |declaration| cx.resolve(declaration);
            Some(cx.converter(checker, &resolve).convert(ty))
        })
    }

    fn symbol_type_at(&mut self, module: ModuleId, span: Span) -> Option<Type> {
        self.with_checker(module, span, |checker, node, cx| {
            let symbol = resolved_symbol(checker, node)?;
            let ty = checker.get_type_of_symbol_exported(symbol);
            let resolve = |declaration| cx.resolve(declaration);
            Some(cx.converter(checker, &resolve).convert(ty))
        })
    }

    fn type_flags_at(&mut self, module: ModuleId, span: Span) -> Option<TsTypeFlags> {
        self.with_checker(module, span, |checker, node, _| {
            Some(convert_flags(checker.get_type_at_location(node)))
        })
    }

    fn type_string_at(&mut self, module: ModuleId, span: Span) -> Option<String> {
        self.with_checker(module, span, |checker, node, _| {
            let ty = checker.get_type_at_location(node);
            Some(checker.type_to_string_exported(ty))
        })
    }

    fn declaration_of(&mut self, module: ModuleId, span: Span) -> Option<(ModuleId, Span)> {
        self.with_checker(module, span, |checker, node, cx| {
            let symbol = resolved_symbol(checker, node)?;
            let declaration = symbol
                .value_declaration()
                .or_else(|| symbol.declarations().first().copied())?;
            cx.declaration_span(declaration)
        })
    }

    fn resolved_signature(&mut self, module: ModuleId, span: Span) -> Option<Signature> {
        self.with_checker(module, span, |checker, node, cx| {
            if !ast::is_call_like_expression(node) {
                return None;
            }
            let signature = checker.get_resolved_signature(node, None, CheckMode::Normal);
            if signature == checker.get_unknown_signature() {
                return None;
            }
            let resolve = |declaration| cx.resolve(declaration);
            Some(cx.converter(checker, &resolve).convert_signature(signature))
        })
    }

    fn diagnostics(&mut self, module: ModuleId) -> Vec<Diagnostic> {
        if !self.check {
            return Vec::new();
        }
        self.module_diagnostics(module)
    }

    fn register_declarations(&mut self, module: ModuleId, declarations: &[(Span, NodeId)]) {
        if let Some(entry) = self.modules.get_mut(&module) {
            entry.declarations = declarations
                .iter()
                .map(|&(span, node)| ((span.start, span.end), node))
                .collect();
        }
    }

    fn matches_node(&mut self, module: ModuleId, span: Span) -> bool {
        self.node_at(module, span).is_some()
    }
}

impl Drop for TsrsOracle {
    fn drop(&mut self) {
        self.release();
    }
}

/// "The 'files' list in config file is empty" and "No inputs were found in config file".
const NO_INPUTS_CODES: [i32; 2] = [18002, 18003];

/// The checker's options when the project has no tsconfig: strict, modern, no emit.
const DEFAULT_TSCONFIG: &str = r#"{
  "compilerOptions": {
    "strict": true,
    "target": "esnext",
    "module": "esnext",
    "moduleResolution": "bundler",
    "allowJs": true,
    "jsx": "preserve",
    "noEmit": true,
    "skipLibCheck": true
  },
  "include": []
}"#;

fn normalize(path: &Path) -> String {
    tspath::normalize_path(&path.to_string_lossy())
}

fn path_key(path: &str, case_sensitive: bool) -> String {
    let path = tspath::normalize_path(path);
    if case_sensitive {
        path
    } else {
        path.to_lowercase()
    }
}

fn convert_diagnostic(diagnostic: &tsrs_ast::Diagnostic, fallback_file: &str) -> Diagnostic {
    let severity = match diagnostic.category().name() {
        "error" => Severity::Error,
        "warning" => Severity::Warning,
        _ => Severity::Advice,
    };
    let mut message = diagnostic.localize();
    append_chain(&mut message, diagnostic.message_chain(), 1);
    let mut converted =
        Diagnostic::new(severity, message).with_code(format!("TS{}", diagnostic.code()));
    let file = diagnostic.file().map_or_else(
        || fallback_file.to_string(),
        |file| file.file_name().to_string(),
    );
    if !file.is_empty() {
        converted = converted.with_file(file);
    }
    if let (Some(start), Some(end)) = (diagnostic.pos(), diagnostic.end()) {
        converted = converted.with_label(Span::new(start, end), None);
    }
    converted
}

fn append_chain(message: &mut String, chain: &[P<tsrs_ast::Diagnostic>], depth: usize) {
    for link in chain {
        message.push('\n');
        message.push_str(&"  ".repeat(depth));
        message.push_str(&link.localize());
        append_chain(message, link.message_chain(), depth + 1);
    }
}

/// The process's resident set size, where the platform makes it cheap to read.
fn resident_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        Some(pages * 4096)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}
