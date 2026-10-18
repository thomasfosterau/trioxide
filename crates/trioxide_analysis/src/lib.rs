//! Trioxide's whole-program analysis, stored as side tables.
//!
//! Analysis never mutates the AST. It produces [`Facts`]: tables keyed by
//! [`NodeRef`] / [`SymbolRef`] (a module id plus oxc's per-module `NodeId` / `SymbolId`), where
//! every entry carries a [`Trust`] level (PLAN D1). Optimisation passes and the partitioner
//! read facts and check trust before acting.
//!
//! S2 fills these tables: the linker, call graph, effect summaries, soundness refinement of
//! TS types, ranges, escape analysis and builtin models (PLAN §2 step 4). Until then
//! [`analyze`] returns empty tables, so every query answers `Unknown`.

mod builtins;
mod callgraph;
mod constants;
mod facts;
mod interp;
mod link;
mod numeric;
mod observe;
mod render;
mod shapes;
mod summary;
mod value;
mod world;

pub use constants::Constant;
pub use facts::{
    AllocKind, Allocation, CallEdge, CallGraph, ClassShape, Effects, Escape, Fact, Facts,
    FunctionInfo, Induction, LoopFacts, SiteResolution, Trust,
};
pub use interp::Stats;
pub use render::render;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_span::SourceType;
use trioxide_common::{Diagnostic, ModuleId, Severity};
use trioxide_types::TypeOracle;

use crate::{link::Links, world::World};

/// One module to analyse.
#[derive(Debug, Clone)]
pub struct ModuleInput<'s> {
    /// The module's id in the graph.
    pub id: ModuleId,
    /// Its path.
    pub path: &'s Path,
    /// Its source.
    pub source: &'s str,
    /// How to parse it.
    pub source_type: SourceType,
    /// Each import specifier and the graph module it resolved to (`None`: outside the graph).
    pub imports: Vec<(String, Option<ModuleId>)>,
}

/// Analysis settings derived from the user's options.
#[derive(Debug, Clone, Default)]
pub struct AnalysisOptions {
    /// Closed world (`app`): every caller of every function is visible (PLAN D2).
    pub closed_world: bool,
    /// Let `Declared` facts count as proof (`trustTypes`, PLAN D1). Passes consult
    /// [`Facts::is_usable`] rather than reading this directly.
    pub trust_types: bool,
    /// The classic JSX factory (`jsxFactory`, e.g. `"h"` or `"React.createElement"`), which
    /// compiled JSX calls. `None` for the automatic runtime.
    pub jsx_factory: Option<String>,
    /// The classic JSX fragment factory (`jsxFragmentFactory`).
    pub jsx_fragment_factory: Option<String>,
    /// The entry modules. Their exports (and what they re-export) are open even in a closed
    /// world: the host calls them. Empty: every module no other module imports.
    pub entries: Vec<ModuleId>,
}

/// The result of analysing a project.
#[derive(Debug, Default)]
pub struct Analysis {
    /// The fact tables.
    pub facts: Facts,
    /// Problems found while analysing.
    pub diagnostics: Vec<Diagnostic>,
    /// How the run went.
    pub stats: Stats,
}

impl Analysis {
    /// No facts at all: every query answers `Unknown`.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "public API used by the facade"
    )]
    pub fn empty(options: AnalysisOptions) -> Self {
        Self {
            facts: Facts::new(options.trust_types),
            diagnostics: Vec::new(),
            stats: Stats::default(),
        }
    }
}

/// Analyses the whole program: parses every module with oxc, registers its classes and
/// functions with the type oracle, links imports, interprets every unit to a fixed point, and
/// summarises the result as [`Facts`].
///
/// The oracle must already have `load`ed the same sources; spans in queries are byte offsets
/// into `modules[i].source`. Node ids match the ones the pipeline's own parse and semantic
/// build produce for the same source.
#[expect(
    clippy::needless_pass_by_value,
    reason = "public API used by the facade"
)]
pub fn analyze(
    modules: &[ModuleInput<'_>],
    oracle: &mut dyn TypeOracle,
    options: AnalysisOptions,
) -> Analysis {
    let allocators: Vec<Allocator> = modules.iter().map(|_| Allocator::default()).collect();
    let world = World::build(&allocators, modules);
    register_declarations(&world, oracle);
    let links = Links::build(&world);
    let results = interp::run(&world, &links, oracle, options.clone());
    let mut diagnostics = Vec::new();
    if !results.converged {
        diagnostics.push(
            Diagnostic::new(
                Severity::Warning,
                "analysis did not reach a fixed point; no facts are reported",
            )
            .with_code("trioxide(analysis)"),
        );
    } else if results.stats.saturated > 0 {
        diagnostics.push(
            Diagnostic::new(
                Severity::Warning,
                format!(
                    "analysis gave up on {} values that kept changing; what depends on them is \
                     unknown",
                    results.stats.saturated
                ),
            )
            .with_code("trioxide(analysis)"),
        );
    }
    let mut facts = summary::facts(&world, &links, &results, &options);
    constants::record(&world, &links, &options, &mut facts);
    Analysis {
        facts,
        diagnostics,
        stats: results.stats,
    }
}

/// Tells the oracle which oxc nodes declare classes and functions, keyed by name span (or
/// node span when anonymous), so its `Instance` and `Function` answers can name them.
fn register_declarations(world: &World<'_>, oracle: &mut dyn TypeOracle) {
    use oxc_ast::AstKind;
    for module in &world.modules {
        let mut declarations = Vec::new();
        for node in module.semantic.nodes().iter() {
            let span = match node.kind() {
                AstKind::Function(function) => {
                    function.id.as_ref().map_or(function.span, |id| id.span)
                }
                AstKind::ArrowFunctionExpression(arrow) => arrow.span,
                AstKind::Class(class) => class.id.as_ref().map_or(class.span, |id| id.span),
                _ => continue,
            };
            declarations.push((span, node.id()));
        }
        oracle.register_declarations(module.id, &declarations);
    }
}
