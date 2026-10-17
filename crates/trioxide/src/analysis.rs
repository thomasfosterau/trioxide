//! Running the whole-program analysis for a module graph, and refreshing its facts after
//! passes rewrite a module (PLAN §2 step 5, §3 "Refreshing facts").

use std::path::PathBuf;

use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_ast::ast::{Program, UnaryOperator};
use oxc_parser::Parser;
use oxc_semantic::{AstNodes, Scoping, SemanticBuilder};
use oxc_span::{GetSpan, Span};

use rustc_hash::FxHashMap;
use trioxide_analysis::{Analysis, AnalysisOptions, Facts, ModuleInput, analyze};
use trioxide_common::{Diagnostic, ModuleId, NodeId, NodeRef};
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
/// still TypeScript), and the oracle and analysis run again over the program with that text
/// (without TypeScript's diagnostics).
///
/// Cost: every refresh is a whole-program analysis, oracle load included, about as costly as
/// the build's own. The pass manager refreshes only when facts are next read, so a module
/// costs one re-analysis per pass-group iteration that changed it and is followed by another
/// iteration (PLAN §3 "Refreshing facts" has the formula). NodeId-stable incremental analysis
/// is where this gets cheap.
///
/// Facts are keyed by node ids, which a semantic build assigns in traversal order. The
/// printed text, parsed again, is numbered the same way except where printing changed the
/// tree: parentheses it added or dropped, and folded numbers (`-1`, `NaN`, `Infinity`) it
/// prints as an operator or a name. The refreshed facts are mapped back to the rewritten
/// AST's ids across those differences; a parenthesized expression gets its contents' facts.
/// When the trees differ in any other way the module's facts are dropped instead: unknown is
/// always safe. Function spans are the rewritten AST's (the original source's, for nodes the
/// passes kept).
pub struct FactRefresher<'p> {
    oracle: &'p mut dyn TypeOracle,
    graph: &'p ModuleGraph,
    setup: Setup,
    analysis: &'p mut Analysis,
    /// Each rewritten module's printed text, and how its ids map back.
    rewritten: FxHashMap<ModuleId, (String, Option<Alignment>)>,
    refreshes: usize,
    refresh_time: std::time::Duration,
    /// The analysis before the first refresh, for [`restore`](Self::restore).
    original: Option<(Facts, trioxide_analysis::Stats)>,
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
            setup: Setup {
                check: false,
                ..setup
            },
            analysis,
            rewritten: FxHashMap::default(),
            refreshes: 0,
            refresh_time: std::time::Duration::ZERO,
            original: None,
        }
    }

    /// Puts back the facts and stats from before any refresh (for compiling one module
    /// without changing what the others compile against). The oracle keeps the rewritten
    /// text; every analysis loads it afresh.
    pub(crate) fn restore(self) {
        if let Some((facts, stats)) = self.original {
            self.analysis.facts = facts;
            self.analysis.stats = stats;
        }
    }

    /// How many times the facts were recomputed.
    pub fn refreshes(&self) -> usize {
        self.refreshes
    }

    /// The time spent recomputing them.
    pub fn refresh_time(&self) -> std::time::Duration {
        self.refresh_time
    }
}

impl FactSource for FactRefresher<'_> {
    fn facts(&self) -> &Facts {
        &self.analysis.facts
    }

    fn refresh(&mut self, module: ModuleId, program: &Program<'_>, _scoping: &Scoping) {
        let started = std::time::Instant::now();
        let text = oxc_codegen::Codegen::new().build(program).code;
        let source_type = self.graph.module(module).source_type;
        let alignment = {
            let allocator = Allocator::default();
            let reparsed = Parser::new(&allocator, &text, source_type).parse();
            let reparsed = allocator.alloc(reparsed.program);
            let ours = SemanticBuilder::new().with_build_nodes(true).build(program);
            let theirs = SemanticBuilder::new()
                .with_build_nodes(true)
                .build(reparsed);
            align(ours.semantic.nodes(), theirs.semantic.nodes())
        };
        if self.original.is_none() {
            self.original = Some((self.analysis.facts.clone(), self.analysis.stats));
        }
        self.rewritten.insert(module, (text, alignment));
        let overrides: FxHashMap<ModuleId, &str> = self
            .rewritten
            .iter()
            .map(|(module, (text, _))| (*module, text.as_str()))
            .collect();
        let (mut analysis, _) =
            analyse_graph(&mut *self.oracle, self.graph, &self.setup, &overrides);
        // Every rewritten module was analysed from its printed text again.
        for (&module, (_, alignment)) in &self.rewritten {
            match alignment {
                Some(alignment) => alignment.apply(&mut analysis.facts, module),
                None => analysis.facts.forget_module(module),
            }
        }
        *self.analysis = analysis;
        self.refreshes += 1;
        self.refresh_time += started.elapsed();
    }
}

/// How the node ids of a module's printed and reparsed text map to the rewritten AST's.
struct Alignment {
    /// Reparsed id to rewritten id.
    ids: FxHashMap<NodeId, NodeId>,
    /// Rewritten parenthesized expressions, with the rewritten node whose value they have.
    aliases: Vec<(NodeId, NodeId)>,
    /// The rewritten AST's units and their spans.
    spans: Vec<(NodeId, Span)>,
}

impl Alignment {
    fn apply(&self, facts: &mut Facts, module: ModuleId) {
        facts.remap_module(module, &self.ids);
        let node = |node| NodeRef { module, node };
        for &(alias, of) in &self.aliases {
            facts.alias(node(alias), node(of));
        }
        for &(unit, span) in &self.spans {
            facts.set_function_span(node(unit), span);
        }
    }
}

/// Walks two semantic builds in step, skipping parentheses and matching a folded number to
/// how it prints. `None` when the trees differ otherwise.
fn align(ours: &AstNodes<'_>, theirs: &AstNodes<'_>) -> Option<Alignment> {
    let ours: Vec<_> = ours.iter().collect();
    let theirs: Vec<_> = theirs.iter().collect();
    let paren = |kind: AstKind<'_>| matches!(kind, AstKind::ParenthesizedExpression(_));
    let mut alignment = Alignment {
        ids: FxHashMap::default(),
        aliases: Vec::new(),
        spans: Vec::new(),
    };
    let mut parens = Vec::new();
    let (mut i, mut j) = (0, 0);
    loop {
        while i < ours.len() && paren(ours[i].kind()) {
            parens.push(ours[i].id());
            i += 1;
        }
        while j < theirs.len() && paren(theirs[j].kind()) {
            j += 1;
        }
        if i == ours.len() || j == theirs.len() {
            return (i == ours.len() && j == theirs.len()).then_some(alignment);
        }
        let (our, their) = (ours[i], theirs[j]);
        alignment
            .aliases
            .extend(parens.drain(..).map(|alias| (alias, our.id())));
        if matches!(
            our.kind(),
            AstKind::Function(_)
                | AstKind::ArrowFunctionExpression(_)
                | AstKind::StaticBlock(_)
                | AstKind::ClassBody(_)
                | AstKind::Program(_)
        ) {
            alignment.spans.push((our.id(), our.kind().span()));
        }
        alignment.ids.insert(their.id(), our.id());
        i += 1;
        j += 1;
        if std::mem::discriminant(&our.kind()) == std::mem::discriminant(&their.kind()) {
            continue;
        }
        let AstKind::NumericLiteral(literal) = our.kind() else {
            return None;
        };
        match their.kind() {
            AstKind::IdentifierReference(name)
                if (literal.value.is_nan() && name.name == "NaN")
                    || (literal.value == f64::INFINITY && name.name == "Infinity") => {}
            AstKind::UnaryExpression(unary)
                if unary.operator == UnaryOperator::UnaryNegation
                    && literal.value.is_sign_negative() =>
            {
                // The operand: the magnitude, or `Infinity`.
                while j < theirs.len() && paren(theirs[j].kind()) {
                    j += 1;
                }
                let operand = theirs.get(j)?;
                let magnitude = -literal.value;
                let fits = match operand.kind() {
                    AstKind::NumericLiteral(operand) => {
                        operand.value.to_bits() == magnitude.to_bits()
                    }
                    AstKind::IdentifierReference(name) => {
                        magnitude == f64::INFINITY && name.name == "Infinity"
                    }
                    _ => false,
                };
                if !fits {
                    return None;
                }
                j += 1;
            }
            _ => return None,
        }
    }
}
