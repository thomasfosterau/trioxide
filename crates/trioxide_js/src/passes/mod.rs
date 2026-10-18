//! The optimisation passes (PLAN §4) and the default pipeline.
//!
//! Foundation passes, in one fact-driven group that runs once (its facts are refreshed at most
//! once per module, before it, and only if an earlier group changed the module). In run
//! order, with their PLAN §4 numbers:
//!
//! - [`Inline`] (`"inline"`, pass 1): references to constant bindings, imports of them and
//!   enum members become literals.
//! - [`Checks`] (`"checks"`, pass 4): `?.`, `??`, `typeof` and nullish comparisons the types
//!   decide. It runs before fold, which then takes the guards it made constant.
//! - [`Fold`] (`"fold"`, pass 2): expressions the facts prove constant, comparisons and
//!   `Math.*` calls the ranges decide, and branches with constant tests.
//! - [`Dce`] (`"dce"`, pass 3): unreachable statements, unused pure bindings and functions,
//!   unused exports (app world) and pure expression statements.
//!
//! The speed passes follow in the same group, so they read the same facts and cost no refresh
//! (a group of their own would re-analyse the whole program for every module the foundation
//! passes changed). Each rebuilds the scoping when it rewrites (`speed::rescope` keeps node
//! ids, so the next one still gets facts about the nodes it kept). The loop and shape passes
//! come first, so a pipeline's callbacks are still written in its calls when it is lowered
//! (`closure-hoist` would lift them out):
//!
//! - [`Licm`] (`"licm"`, pass 9): loop invariants and unchanging array lengths move out of
//!   loops, under readable names.
//! - [`HiddenClass`] (`"hidden-class"`, pass 10): late or conditional properties of classes
//!   whose instances' keys nothing observes become fields; `delete` of them an assignment.
//! - [`ForOf`] (`"for-of"`, pass 7): `for…of` over plain arrays becomes an indexed loop;
//!   spreads of arrays without holes become `slice`/`concat`.
//! - [`Pipelines`] (`"pipelines"`, pass 6): `map`/`filter`/… chains become one loop.
//! - [`PartialEval`] (`"partial-eval"`, pass 11): top-level calls with constant arguments
//!   evaluated by a fuel-bounded interpreter and replaced with the literal they return.
//! - [`Scalar`] (`"scalar"`, pass 8, off by default): object and array literals that never
//!   leave their function become one local per key.
//! - [`ClosureHoist`] (`"closure-hoist"`, pass 12): closures that capture nothing and whose
//!   identity can't be seen are created once.
//! - [`Params`] (`"params"`): with `passes.specialize`, parameters every caller passes the
//!   same literal leave the signature and the calls.
//! - [`InlineFn`] (`"inline-fn"`, pass 5, off by default): calls of small or single-call-site
//!   functions and methods become their bodies.
//!
//! `inline-fn` and `scalar` are off unless `passes` turns them on: V8's optimising tier does
//! both itself in hot code, so they only measured faster in code that isn't hot yet.
//!
//! Then a syntactic cleanup group, run to a fixed point without reading facts:
//! [`Simplify`] (`"simplify"`) folds what the first group left constant with
//! `oxc_ecmascript`, takes branches with literal tests and drops empty and inert code.
//!
//! The import guard (`"imports"`) brackets both: removing the last use of an import would
//! let the TypeScript strip elide the whole import, so a module would no longer run; the
//! guard keeps such imports as side-effect imports.
//!
//! Every pass checks its facts with [`Facts::is_usable`](trioxide_analysis::Facts::is_usable)
//! and logs each rewrite (`applied`) and each declined candidate (`skipped`, with the reason).
//! Where a pass may rewrite is decided by one helper, [`util::Slot`], which looks through
//! parentheses and TypeScript-only wrappers.
//!
//! **Specialisation.** In a closed world a parameter's facts are what every caller passes, so
//! they can make a function's body constant (`y < height` becomes `y < 4`). That is sound but
//! hard to read, so it is off unless `passes.specialize` is `true`: with it off, no fact is
//! folded about an expression that reads a parameter, `this`, or a local assigned from them,
//! and DCE only removes unreachable code that follows a `return`, `throw`, `break` or
//! `continue` (the rest may be unreachable only because of what callers pass). Calls are
//! still folded where they are made (`f(3)` becomes `6`).
//!
//! **Termination.** Purity facts don't prove that a call returns: a pure call that never
//! returns (an infinite loop, a stack overflow) can be removed or folded, so the output may
//! finish where the original didn't. This is accepted, as it is in most optimising compilers.
//!
//! **Islands.** The facade passes a [`Keep`](crate::Keep) per module: exports of modules that
//! define WASM island functions stay (the glue's JS fallback calls them by name), calls to
//! island functions are not folded, and island functions are not removed. Functions forced to
//! an island (`"use wasm"`, `@trioxide wasm`) are never removed either.

mod arrays;
mod checks;
mod closure_hoist;
mod dce;
mod fold;
mod for_of;
mod hidden_class;
mod imports;
mod inline;
mod inline_fn;
mod licm;
mod partial_eval;
mod pipelines;
mod scalar;
mod simplify;
mod speed;
mod util;

use std::{cell::RefCell, rc::Rc};

use oxc_ast::ast::{ArrowFunctionExpression, Function, Statement};
use oxc_span::Span;
use trioxide_common::log::ModuleLog;

pub use checks::Checks;
pub use closure_hoist::ClosureHoist;
pub use dce::Dce;
pub use fold::Fold;
pub use for_of::ForOf;
pub use hidden_class::HiddenClass;
pub use imports::{KeepImports, RecordImports};
pub use inline::Inline;
pub use inline_fn::{InlineFn, Params};
pub use licm::Licm;
pub use partial_eval::PartialEval;
pub use pipelines::Pipelines;
pub use scalar::Scalar;
pub use simplify::Simplify;

/// Whether a function is a forced WASM island: a `"use wasm"` directive in its body, or a
/// `@trioxide wasm` tag in a comment attached at `starts` (the function's, or its `export`'s).
fn forced_island(
    function: &Function<'_>,
    tagged: &rustc_hash::FxHashSet<u32>,
    starts: &[u32],
) -> bool {
    function.body.as_ref().is_some_and(|body| {
        body.directives
            .iter()
            .any(|directive| directive.directive == "use wasm")
    }) || starts.iter().any(|start| tagged.contains(start))
}

/// The positions comments with a `@trioxide wasm` tag are attached to.
fn wasm_tags(program: &oxc_ast::ast::Program<'_>) -> rustc_hash::FxHashSet<u32> {
    program
        .comments
        .iter()
        .filter(|comment| {
            comment
                .span
                .source_text(program.source_text)
                .contains("@trioxide wasm")
        })
        .map(|comment| comment.attached_to)
        .collect()
}

use crate::PassGroup;
use util::Query;

/// The default groups: the fact-driven foundation and speed passes, then the syntactic
/// cleanup.
pub fn default_groups() -> Vec<PassGroup> {
    let imports = Rc::new(RefCell::new(imports::Guard::default()));
    vec![
        PassGroup {
            name: "foundation",
            passes: vec![
                Box::new(RecordImports::new(Rc::clone(&imports))),
                Box::new(Inline),
                Box::new(Checks),
                Box::new(Fold),
                Box::new(Dce),
                // Speed passes (S5b: PLAN §4 passes 9, 10, 7, 6; S5a: 11, 8, 12, 5) read the same
                // facts, so they cost no refresh: a group of their own would re-analyse the whole
                // program for every module the foundation passes changed. S5b's come first, so
                // pipelines still find their callbacks in the calls closure-hoist would lift.
                Box::new(Licm),
                Box::new(HiddenClass),
                Box::new(ForOf),
                Box::new(Pipelines),
                Box::new(PartialEval),
                Box::new(Scalar),
                Box::new(ClosureHoist),
                Box::new(Params),
                Box::new(InlineFn),
            ],
            fixpoint: false,
        },
        PassGroup {
            name: "cleanup",
            passes: vec![Box::new(Simplify), Box::new(KeepImports::new(imports))],
            fixpoint: true,
        },
    ]
}

/// Decisions a pass made during one traversal, flushed into the [`ModuleLog`] afterwards.
struct Decisions {
    pass: &'static str,
    events: Vec<(Option<Span>, Option<String>, Option<&'static str>)>,
    applied: usize,
}

/// The message on a DCE event that dropped an export no module used (the facade invalidates
/// such modules when another changes).
pub const UNEXPORTED: &str = "export removed: no module uses it";

impl Decisions {
    fn new(pass: &'static str) -> Self {
        Self {
            pass,
            events: Vec::new(),
            applied: 0,
        }
    }

    fn applied(&mut self, span: Span) {
        self.applied += 1;
        self.events.push((Some(span), None, None));
    }

    fn applied_with(&mut self, span: Span, message: &'static str) {
        self.applied += 1;
        self.events.push((Some(span), None, Some(message)));
    }

    /// A rewrite not worth logging (dropping redundant parentheses).
    fn quiet(&mut self) {
        self.applied += 1;
    }

    fn skipped(&mut self, span: Span, reason: impl Into<String>) {
        self.events.push((Some(span), Some(reason.into()), None));
    }

    /// Writes the decisions into `log` and returns whether anything was rewritten.
    fn flush(self, log: &mut ModuleLog) -> bool {
        for (span, reason, message) in self.events {
            match reason {
                None => log.applied(self.pass, span),
                Some(reason) => log.skipped(self.pass, span, reason),
            }
            if let (Some(message), Some(event)) = (message, log.passes.last_mut()) {
                event.message = Some(message.to_string());
            }
        }
        self.applied > 0
    }
}

/// Where a traversal is: inside code proven never to run (whose facts are vacuous), or inside
/// TypeScript enum and namespace bodies (which the TypeScript strip rewrites, and which passes
/// leave alone).
#[derive(Default)]
struct Position {
    dead: Vec<bool>,
    dead_depth: usize,
    opaque_depth: usize,
}

impl Position {
    /// Whether facts may be used here.
    fn live(&self) -> bool {
        self.dead_depth == 0 && self.opaque_depth == 0
    }

    /// Whether the code here is left alone.
    fn opaque(&self) -> bool {
        self.opaque_depth > 0
    }

    fn enter(&mut self, dead: bool) {
        self.dead.push(dead);
        self.dead_depth += usize::from(dead);
    }

    fn exit(&mut self) {
        if self.dead.pop() == Some(true) {
            self.dead_depth -= 1;
        }
    }

    fn enter_statement(&mut self, statement: &Statement<'_>, query: Query<'_>) {
        self.enter(query.dead(statement.node_id()));
    }

    fn enter_function(&mut self, function: &Function<'_>, query: Query<'_>) {
        self.enter(query.dead(function.node_id()));
    }

    fn enter_arrow(&mut self, arrow: &ArrowFunctionExpression<'_>, query: Query<'_>) {
        self.enter(query.dead(arrow.node_id()));
    }

    fn enter_opaque(&mut self) {
        self.opaque_depth += 1;
    }

    fn exit_opaque(&mut self) {
        self.opaque_depth -= 1;
    }
}

/// Where the leading comments of removed statements go: a comment separated from the statement
/// by a blank line is about the code around it (a file header, a section title), not the
/// statement, so it moves to the next statement that stays.
///
/// The comments of a removed `;` (what folding an `if` with no branch taken leaves) and of an
/// `export` dropped from a declaration always move: they are about what is still there.
#[derive(Default)]
struct Comments(Vec<(u32, u32, bool)>);

impl Comments {
    /// Removes the statements `keep` says go, noting where their comments move.
    fn remove<'a>(
        &mut self,
        statements: &mut oxc_allocator::ArenaVec<'a, Statement<'a>>,
        keep: &[bool],
    ) {
        use oxc_span::GetSpan;
        let mut next = None;
        for (statement, &kept) in statements.iter().zip(keep).rev() {
            let start = statement.span().start;
            if kept {
                next = Some(start);
            } else if let Some(next) = next {
                let always = matches!(statement, Statement::EmptyStatement(_));
                self.0.push((start, next, always));
            }
        }
        let mut keep = keep.iter();
        statements.retain(|_| *keep.next().expect("a flag per statement"));
    }

    /// Notes that the code at `from` now starts at `to` (all its comments move).
    fn rename(&mut self, from: u32, to: u32) {
        if from != to {
            self.0.push((from, to, true));
        }
    }

    /// Re-attaches the comments of `program` as noted.
    fn apply(self, program: &mut oxc_ast::ast::Program<'_>) {
        if self.0.is_empty() {
            return;
        }
        let moved: rustc_hash::FxHashMap<u32, (u32, bool)> = self
            .0
            .into_iter()
            .map(|(from, to, always)| (from, (to, always)))
            .collect();
        let source = program.source_text;
        for comment in &mut program.comments {
            let Some(&(to, always)) = moved.get(&comment.attached_to) else {
                continue;
            };
            let between = source
                .get(comment.span.end as usize..comment.attached_to as usize)
                .unwrap_or("");
            if comment.is_leading() && (always || between.matches('\n').count() >= 2) {
                comment.attached_to = to;
            }
        }
    }
}

/// Blocks a pass made out of an `if` it folded (`if (true) { … }` → `{ … }`), to splice into
/// the statement list they are in once the list is visited. Blocks the source wrote stay.
#[derive(Default)]
struct Unwrapped(rustc_hash::FxHashSet<Span>);

impl Unwrapped {
    /// Records `taken`, the branch replacing an `if` whose parent is `parent`, if it can be
    /// spliced: a block in a statement list that declares nothing block-scoped.
    fn record(&mut self, taken: &Statement<'_>, parent: &oxc_traverse::Ancestor<'_, '_>) {
        use oxc_traverse::Ancestor;
        let in_list = matches!(
            parent,
            Ancestor::ProgramBody(_)
                | Ancestor::BlockStatementBody(_)
                | Ancestor::FunctionBodyStatements(_)
                | Ancestor::SwitchCaseConsequent(_)
                | Ancestor::StaticBlockBody(_)
        );
        if let Statement::BlockStatement(block) = taken
            && in_list
            && !block.body.iter().any(declares_block_scoped)
        {
            self.0.insert(block.span);
        }
    }

    /// Splices the recorded blocks of `statements` into it.
    fn splice<'a>(
        &mut self,
        statements: &mut oxc_allocator::ArenaVec<'a, Statement<'a>>,
        ast: &oxc_ast::builder::AstBuilder<'a>,
    ) {
        if self.0.is_empty() {
            return;
        }
        let mut index = 0;
        while index < statements.len() {
            if let Statement::BlockStatement(block) = &mut statements[index]
                && self.0.remove(&block.span)
            {
                let body = oxc_allocator::TakeIn::take_in(&mut block.body, ast);
                statements.remove(index);
                let count = body.len();
                for (offset, statement) in body.into_iter().enumerate() {
                    statements.insert(index + offset, statement);
                }
                index += count;
            } else {
                index += 1;
            }
        }
    }
}

/// Whether a statement declares a binding scoped to the block it is in.
fn declares_block_scoped(statement: &Statement<'_>) -> bool {
    match statement {
        Statement::VariableDeclaration(declaration) => !declaration.kind.is_var(),
        Statement::ClassDeclaration(_)
        | Statement::FunctionDeclaration(_)
        | Statement::TSEnumDeclaration(_)
        | Statement::TSNamespaceDeclaration(_)
        | Statement::TSGlobalDeclaration(_)
        | Statement::TSImportEqualsDeclaration(_)
        | Statement::TSTypeAliasDeclaration(_)
        | Statement::TSInterfaceDeclaration(_) => true,
        _ => false,
    }
}

/// Implements the [`Position`] bookkeeping hooks of an `oxc_traverse` visitor that has
/// `position: Position` and `query: Query` fields. With `after_statement`, `exit_statement`
/// also calls the visitor's `after_statement(statement, ctx)`; with `before_statement`,
/// `enter_statement` also calls its `before_statement(statement, ctx)` (and `exit_statement`
/// its `after_statement`).
macro_rules! track_position {
    (after_statement) => {
        $crate::passes::track_position!(@hooks self, statement, ctx, {
            let _ = (statement, ctx);
        }, {
            self.after_statement(statement, ctx);
        });
    };
    (before_statement) => {
        $crate::passes::track_position!(@hooks self, statement, ctx, {
            self.before_statement(statement, ctx);
        }, {
            self.after_statement(statement, ctx);
        });
    };
    () => {
        $crate::passes::track_position!(@hooks self, statement, ctx, {
            let _ = (statement, ctx);
        }, {
            let _ = (statement, ctx);
        });
    };
    (@hooks $self:ident, $statement:ident, $ctx:ident, $before:block, $after:block) => {
        fn enter_statement(
            &mut $self,
            $statement: &mut oxc_ast::ast::Statement<'a>,
            $ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
        ) {
            $self.position.enter_statement($statement, $self.query);
            $before
        }

        fn exit_statement(
            &mut $self,
            $statement: &mut oxc_ast::ast::Statement<'a>,
            $ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
        ) {
            $self.position.exit();
            $after
        }

        fn enter_function(
            &mut self,
            function: &mut oxc_ast::ast::Function<'a>,
            _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
        ) {
            self.position.enter_function(function, self.query);
        }

        fn exit_function(
            &mut self,
            _function: &mut oxc_ast::ast::Function<'a>,
            _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
        ) {
            self.position.exit();
        }

        fn enter_arrow_function_expression(
            &mut self,
            arrow: &mut oxc_ast::ast::ArrowFunctionExpression<'a>,
            _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
        ) {
            self.position.enter_arrow(arrow, self.query);
        }

        fn exit_arrow_function_expression(
            &mut self,
            _arrow: &mut oxc_ast::ast::ArrowFunctionExpression<'a>,
            _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
        ) {
            self.position.exit();
        }

        fn exit_object_property(
            &mut self,
            property: &mut oxc_ast::ast::ObjectProperty<'a>,
            _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
        ) {
            // `{ x }` whose value was rewritten prints as `{ x: <value> }`.
            if property.shorthand
                && !matches!(property.value, oxc_ast::ast::Expression::Identifier(_))
            {
                property.shorthand = false;
            }
        }

        fn enter_ts_enum_declaration(
            &mut self,
            _declaration: &mut oxc_ast::ast::TSEnumDeclaration<'a>,
            _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
        ) {
            self.position.enter_opaque();
        }

        fn exit_ts_enum_declaration(
            &mut self,
            _declaration: &mut oxc_ast::ast::TSEnumDeclaration<'a>,
            _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
        ) {
            self.position.exit_opaque();
        }

        fn enter_ts_namespace_declaration(
            &mut self,
            _declaration: &mut oxc_ast::ast::TSNamespaceDeclaration<'a>,
            _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
        ) {
            self.position.enter_opaque();
        }

        fn exit_ts_namespace_declaration(
            &mut self,
            _declaration: &mut oxc_ast::ast::TSNamespaceDeclaration<'a>,
            _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
        ) {
            self.position.exit_opaque();
        }
    };
}

pub(crate) use track_position;
