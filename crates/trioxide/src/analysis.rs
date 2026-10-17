//! Running the whole-program analysis for a module graph, and refreshing its facts after
//! passes rewrite a module (PLAN §2 step 5, §3 "Refreshing facts").

use std::path::PathBuf;

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_parser::Parser;
use oxc_semantic::{AstNodes, Scoping, SemanticBuilder};
use rustc_hash::FxHashMap;
use trioxide_analysis::{Analysis, AnalysisOptions, Facts, ModuleInput, analyze};
use trioxide_common::{Diagnostic, ModuleId};
use trioxide_js::FactSource;
use trioxide_project::{CompilerSettings, ImportTarget, JsxMode, ModuleGraph};
use trioxide_types::{OracleInput, SourceText, TypeOracle};

/// The analysis inputs for every module of `graph`.
pub fn analysis_inputs(graph: &ModuleGraph) -> Vec<ModuleInput<'_>> {
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

/// The classic JSX factory and fragment factory a tsconfig implies (`None` for the
/// automatic runtime), as [`AnalysisOptions`] wants them.
pub fn jsx_factories(settings: &CompilerSettings) -> (Option<String>, Option<String>) {
    if settings.jsx != JsxMode::Classic {
        return (None, None);
    }
    (
        Some(
            settings
                .jsx_factory
                .clone()
                .unwrap_or_else(|| "React.createElement".into()),
        ),
        Some(
            settings
                .jsx_fragment_factory
                .clone()
                .unwrap_or_else(|| "React.Fragment".into()),
        ),
    )
}

/// Everything an analysis run needs besides the graph and the oracle.
#[derive(Debug, Clone)]
pub(crate) struct Setup {
    pub root: PathBuf,
    pub tsconfig: Option<PathBuf>,
    pub check: bool,
    pub options: AnalysisOptions,
}

/// Loads `graph` into `oracle` (with `overrides` in place of some modules' sources) and
/// analyses it. Returns the analysis and every diagnostic.
pub(crate) fn analyse_graph(
    oracle: &mut dyn TypeOracle,
    graph: &ModuleGraph,
    setup: &Setup,
    overrides: &FxHashMap<ModuleId, &str>,
) -> (Analysis, Vec<Diagnostic>) {
    let modules = graph.modules();
    let sources: Vec<SourceText<'_>> = modules
        .iter()
        .map(|module| SourceText {
            module: module.id,
            path: &module.path,
            source: overrides.get(&module.id).copied().unwrap_or(&module.source),
        })
        .collect();
    let mut diagnostics = oracle.load(&OracleInput {
        root: &setup.root,
        tsconfig: setup.tsconfig.as_deref(),
        modules: &sources,
        check: setup.check,
    });
    if setup.check {
        for module in modules {
            diagnostics.extend(oracle.diagnostics(module.id));
        }
    }
    let mut inputs = analysis_inputs(graph);
    for input in &mut inputs {
        if let Some(source) = overrides.get(&input.id) {
            input.source = source;
        }
    }
    let mut analysis = analyze(&inputs, oracle, setup.options.clone());
    diagnostics.append(&mut analysis.diagnostics);
    (analysis, diagnostics)
}

/// The facts of a [`Project`](crate::Project), refreshed whenever a pass group rewrites a
/// module: the module is printed back out (passes run before the TypeScript strip, so it is
/// still TypeScript), and the oracle and analysis run again over the program with that text.
///
/// Cost: every refresh is a whole-program analysis, oracle load included, as costly as the
/// build's own. That is acceptable for v1 with few pass groups; NodeId-stable incremental
/// analysis (PLAN §3) is where this gets cheap.
///
/// Facts are keyed by node ids, which a semantic build assigns in traversal order; the
/// printed text, parsed again, numbers its nodes the same way exactly when it has the same
/// shape as the rewritten AST. When printing changes the shape (parentheses a pass didn't
/// create, for example), the module's facts are dropped instead: unknown is always safe.
pub struct FactRefresher<'p> {
    oracle: &'p mut dyn TypeOracle,
    graph: &'p ModuleGraph,
    setup: Setup,
    analysis: &'p mut Analysis,
    overrides: FxHashMap<ModuleId, String>,
    refreshes: usize,
    /// The facts before the first refresh, for [`restore`](Self::restore).
    original: Option<Facts>,
}

impl<'p> FactRefresher<'p> {
    pub(crate) fn new(
        oracle: &'p mut dyn TypeOracle,
        graph: &'p ModuleGraph,
        setup: Setup,
        analysis: &'p mut Analysis,
    ) -> Self {
        Self {
            oracle,
            graph,
            setup,
            analysis,
            overrides: FxHashMap::default(),
            refreshes: 0,
            original: None,
        }
    }

    /// Puts back the facts from before any refresh (for compiling one module without
    /// changing what the others compile against).
    pub(crate) fn restore(self) {
        let Some(original) = self.original else {
            return;
        };
        self.analysis.facts = original;
        // The oracle saw the rewritten text: give it back the graph's, which incremental
        // updates build on.
        let sources: Vec<SourceText<'_>> = self
            .graph
            .modules()
            .iter()
            .map(|module| SourceText {
                module: module.id,
                path: &module.path,
                source: &module.source,
            })
            .collect();
        let _ = self.oracle.load(&OracleInput {
            root: &self.setup.root,
            tsconfig: self.setup.tsconfig.as_deref(),
            modules: &sources,
            check: self.setup.check,
        });
    }

    /// How many times the facts were recomputed.
    pub fn refreshes(&self) -> usize {
        self.refreshes
    }
}

impl FactSource for FactRefresher<'_> {
    fn facts(&self) -> &Facts {
        &self.analysis.facts
    }

    fn refresh(&mut self, module: ModuleId, program: &Program<'_>, _scoping: &Scoping) {
        let text = oxc_codegen::Codegen::new().build(program).code;
        let source_type = self.graph.module(module).source_type;
        // The rewritten AST's node ids, in traversal order (as the pass manager's semantic
        // rebuild assigned them), against the printed text's.
        let aligned = {
            let allocator = Allocator::default();
            let reparsed = Parser::new(&allocator, &text, source_type).parse();
            let reparsed = allocator.alloc(reparsed.program);
            let ours = SemanticBuilder::new().with_build_nodes(true).build(program);
            let theirs = SemanticBuilder::new()
                .with_build_nodes(true)
                .build(reparsed);
            same_shape(ours.semantic.nodes(), theirs.semantic.nodes())
        };
        if self.original.is_none() {
            self.original = Some(self.analysis.facts.clone());
        }
        self.overrides.insert(module, text);
        let overrides: FxHashMap<ModuleId, &str> = self
            .overrides
            .iter()
            .map(|(module, text)| (*module, text.as_str()))
            .collect();
        let (mut analysis, _) =
            analyse_graph(&mut *self.oracle, self.graph, &self.setup, &overrides);
        if !aligned {
            analysis.facts.forget_module(module);
        }
        *self.analysis = analysis;
        self.refreshes += 1;
    }
}

/// Whether two semantic builds visited the same kinds of nodes in the same order.
fn same_shape(ours: &AstNodes<'_>, theirs: &AstNodes<'_>) -> bool {
    ours.len() == theirs.len()
        && ours
            .iter()
            .zip(theirs.iter())
            .all(|(a, b)| std::mem::discriminant(&a.kind()) == std::mem::discriminant(&b.kind()))
}
