//! Pass 3: dead code elimination (PLAN §4).
//!
//! On every statement list:
//! - **Unreachable code**: statements with `reachable` `Proven` false go, unless they declare
//!   something visible outside them (a hoisted `var` or function, or a `let`/`const`/`class`
//!   that is referenced, whose temporal dead zone must stay).
//! - **Unused bindings**: a `var`/`let`/`const` declarator with no references left whose
//!   initialiser is absent, inert or usable-pure goes (not in a module with a direct `eval` or
//!   `with`, which can read any binding by name).
//! - **Unused functions**: a function declaration with no references outside itself goes when
//!   `callers_complete` is usable-true.
//! - **Unused exports**: at the top level, `export` is dropped from declarations and
//!   specifiers whose exports no module can use (`export_used` `Proven` false: app world,
//!   not an entry); the bindings then go if unused.
//! - **Expression statements**: an expression statement that is inert or usable-pure goes.
//!
//! Purity facts count implicit calls, throws (TDZ reads included) and unknown code, so nothing
//! whose evaluation may throw or run user code is removed. The pass repeats until nothing
//! more goes, since each removal can leave other bindings unused.

use oxc_allocator::{ArenaVec, TakeIn};
use oxc_ast::ast::{
    Declaration, ExportNamedDeclaration, ModuleExportName, Program, Statement,
    VariableDeclarationKind,
};
use oxc_span::{GetSpan, Span};
use oxc_syntax::symbol::SymbolId;
use oxc_traverse::{Ancestor, Traverse, TraverseCtx};
use rustc_hash::FxHashMap;
use trioxide_common::SymbolRef;

use super::{
    Comments, Decisions, Position, track_position,
    util::{
        Query, binds_or_suspends, declares_hoisted, has_dynamic_scope, is_inert, live_references,
        weak,
    },
};
use crate::{Pass, PassContext};

/// Pass 3 (`"dce"`).
pub struct Dce;

impl Pass for Dce {
    fn name(&self) -> &'static str {
        "dce"
    }

    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
        let query = Query {
            facts: ctx.facts,
            module: ctx.module,
        };
        run(self.name(), program, ctx, Some(query))
    }
}

/// Runs DCE with facts, or (`query` `None`) with syntax alone: then only inert code goes.
pub(super) fn run<'a>(
    name: &'static str,
    program: &mut Program<'a>,
    ctx: &mut PassContext<'a, '_>,
    query: Option<Query<'_>>,
) -> bool {
    let dynamic_scope = has_dynamic_scope(program, ctx.scoping());
    let mut changed = false;
    for _ in 0..8 {
        let references = live_references(program, ctx.scoping());
        let mut visitor = Visitor {
            query: query.unwrap_or(Query {
                facts: ctx.facts,
                module: ctx.module,
            }),
            facts: query.is_some(),
            position: Position::default(),
            decisions: Decisions::new(name),
            references,
            dynamic_scope,
            comments: Comments::default(),
        };
        ctx.traverse(&mut visitor, program, ());
        visitor.comments.apply(program);
        if !visitor.decisions.flush(ctx.log) {
            break;
        }
        changed = true;
    }
    changed
}

struct Visitor<'f> {
    query: Query<'f>,
    /// Whether facts may be used (else only syntax).
    facts: bool,
    position: Position,
    decisions: Decisions,
    references: FxHashMap<SymbolId, Vec<Span>>,
    dynamic_scope: bool,
    comments: Comments,
}

impl<'a> Traverse<'a, ()> for Visitor<'_> {
    track_position!();

    fn exit_statements(
        &mut self,
        statements: &mut ArenaVec<'a, Statement<'a>>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if self.position.opaque() {
            return;
        }
        let top_level = matches!(ctx.parent(), Ancestor::ProgramBody(_));
        let live = self.position.live();
        let keep: Vec<bool> = statements
            .iter_mut()
            .map(|statement| self.statement(statement, top_level, live, ctx))
            .collect();
        self.comments.remove(statements, &keep);
    }
}

impl Visitor<'_> {
    /// References to `symbol` outside `within`.
    fn referenced(&self, symbol: SymbolId, within: Span) -> bool {
        self.references
            .get(&symbol)
            .is_some_and(|spans| spans.iter().any(|span| !within.contains_inclusive(*span)))
    }

    /// Decides one statement of a list; may rewrite it. Returns whether it stays.
    fn statement<'a>(
        &mut self,
        statement: &mut Statement<'a>,
        top_level: bool,
        live: bool,
        ctx: &mut TraverseCtx<'a, ()>,
    ) -> bool {
        let span = statement.span();
        if self.facts && live && self.query.dead(statement.node_id()) {
            if declares_hoisted(statement) {
                self.decisions.skipped(
                    span,
                    "unreachable, but declares a `var` or function, which are visible before it",
                );
            } else if self.declares_referenced(statement) {
                self.decisions.skipped(
                    span,
                    "unreachable, but declares a binding used elsewhere (its temporal dead zone)",
                );
            } else {
                self.decisions.applied(span);
                return false;
            }
        }
        match statement {
            Statement::EmptyStatement(_) => {
                return false;
            }
            Statement::ExpressionStatement(expression) => {
                let scoping = ctx.scoping();
                if is_inert(&expression.expression, scoping) {
                    self.decisions.applied(span);
                    return false;
                }
                if self.facts && live && !binds_or_suspends(&expression.expression) {
                    let fact = self.query.is_pure(expression.expression.node_id());
                    if fact.value && self.query.usable(fact.trust) {
                        self.decisions.applied(span);
                        return false;
                    }
                    if fact.value && fact.trust == trioxide_analysis::Trust::Declared {
                        self.decisions
                            .skipped(span, weak(fact.trust, "the expression's purity"));
                    }
                }
            }
            Statement::IfStatement(if_statement)
                if is_empty(&if_statement.consequent)
                    && if_statement.alternate.as_ref().is_none_or(is_empty) =>
            {
                // `if (test) {}`: only the test is left.
                let test = &if_statement.test;
                if is_inert(test, ctx.scoping())
                    || (self.facts
                        && live
                        && self.query.pure(test.node_id())
                        && !binds_or_suspends(test))
                {
                    self.decisions.applied(span);
                    return false;
                }
            }
            Statement::VariableDeclaration(_) => return self.variables(statement, live, ctx),
            Statement::FunctionDeclaration(function) if self.facts && live => {
                let (Some(id), Some(_)) = (&function.id, &function.body) else {
                    return true;
                };
                if function.declare || self.dynamic_scope {
                    return true;
                }
                let symbol = id.symbol_id();
                if self.referenced(symbol, function.span) {
                    return true;
                }
                let unit = self.query.node(function.node_id());
                let complete = unit.map_or(trioxide_analysis::Fact::unknown(), |unit| {
                    self.query.facts.callers_complete(unit)
                });
                if complete.value && self.query.usable(complete.trust) {
                    self.decisions.applied(span);
                    return false;
                }
                self.decisions.skipped(
                    span,
                    if self.query.usable(complete.trust) {
                        "unused here, but code trioxide doesn't see may call it".to_string()
                    } else {
                        weak(complete.trust, "whether every caller is known")
                    },
                );
            }
            Statement::ExportDeclaration(_) if top_level && self.facts => {
                self.unexport_declaration(statement, ctx);
            }
            Statement::ExportNamedDeclaration(export) if top_level && self.facts => {
                return self.unexport_specifiers(export, ctx);
            }
            _ => {}
        }
        true
    }

    /// Whether a lexical declaration directly in `statement` is referenced.
    fn declares_referenced(&self, statement: &Statement<'_>) -> bool {
        match statement {
            Statement::VariableDeclaration(declaration) => {
                declaration.declarations.iter().any(|declarator| {
                    declarator
                        .id
                        .get_binding_identifier()
                        .is_none_or(|id| self.referenced(id.symbol_id(), declarator.span))
                })
            }
            Statement::ClassDeclaration(class) => class
                .id
                .as_ref()
                .is_none_or(|id| self.referenced(id.symbol_id(), class.span)),
            _ => false,
        }
    }

    /// Drops unused declarators. Returns whether the statement stays.
    fn variables<'a>(
        &mut self,
        statement: &mut Statement<'a>,
        live: bool,
        ctx: &mut TraverseCtx<'a, ()>,
    ) -> bool {
        let Statement::VariableDeclaration(declaration) = statement else {
            return true;
        };
        if declaration.declare
            || self.dynamic_scope
            || matches!(
                declaration.kind,
                VariableDeclarationKind::Using | VariableDeclarationKind::AwaitUsing
            )
        {
            return true;
        }
        let scoping = ctx.scoping();
        let mut removed = Vec::new();
        for (index, declarator) in declaration.declarations.iter().enumerate() {
            let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &declarator.id else {
                continue;
            };
            let symbol = id.symbol_id();
            if self.referenced(symbol, declarator.span) {
                continue;
            }
            match &declarator.init {
                None => removed.push(index),
                Some(init) if is_inert(init, scoping) => removed.push(index),
                Some(init) if self.facts && live && !binds_or_suspends(init) => {
                    let fact = self.query.is_pure(init.node_id());
                    if fact.value && self.query.usable(fact.trust) {
                        removed.push(index);
                    } else {
                        self.decisions.skipped(
                            declarator.span,
                            if self.query.usable(fact.trust) {
                                "unused, but its initialiser has effects".to_string()
                            } else {
                                weak(fact.trust, "the initialiser's purity")
                            },
                        );
                    }
                }
                Some(_) => {}
            }
        }
        if removed.is_empty() {
            return true;
        }
        for &index in removed.iter().rev() {
            let declarator = declaration.declarations.remove(index);
            self.decisions.applied(declarator.span);
        }
        !declaration.declarations.is_empty()
    }

    /// `export <declaration>` → `<declaration>` when no module can use the exports.
    fn unexport_declaration<'a>(
        &mut self,
        statement: &mut Statement<'a>,
        ctx: &TraverseCtx<'a, ()>,
    ) {
        let Statement::ExportDeclaration(export) = statement else {
            return;
        };
        let scoping = ctx.scoping();
        let symbols: Vec<SymbolId> = match &export.declaration {
            Declaration::VariableDeclaration(declaration) if !declaration.declare => declaration
                .declarations
                .iter()
                .filter_map(|declarator| declarator.id.get_binding_identifier())
                .map(oxc_ast::ast::BindingIdentifier::symbol_id)
                .collect(),
            Declaration::FunctionDeclaration(function) if !function.declare => function
                .id
                .iter()
                .map(oxc_ast::ast::BindingIdentifier::symbol_id)
                .collect(),
            Declaration::ClassDeclaration(class) if !class.declare => class
                .id
                .iter()
                .map(oxc_ast::ast::BindingIdentifier::symbol_id)
                .collect(),
            _ => return,
        };
        if symbols.is_empty() {
            return;
        }
        let span = export.span;
        if !symbols.iter().all(|&symbol| self.export_unused(symbol)) {
            return;
        }
        let _ = scoping;
        let declaration = export.declaration.take_in(&ctx.ast);
        *statement = Statement::from(declaration);
        self.decisions.applied(span);
    }

    /// Drops `export { … }` specifiers no module can use. Returns whether the statement stays.
    fn unexport_specifiers<'a>(
        &mut self,
        export: &mut ExportNamedDeclaration<'a>,
        ctx: &TraverseCtx<'a, ()>,
    ) -> bool {
        if export.export_kind.is_type() || export.specifiers.is_empty() {
            return true;
        }
        let scoping = ctx.scoping();
        let before = export.specifiers.len();
        let mut spans = Vec::new();
        export.specifiers.retain(|specifier| {
            let ModuleExportName::IdentifierReference(local) = &specifier.local else {
                return true;
            };
            let Some(symbol) = local
                .reference_id
                .get()
                .and_then(|reference| scoping.get_reference(reference).symbol_id())
            else {
                return true;
            };
            if specifier.export_kind.is_type() || scoping.symbol_flags(symbol).is_import() {
                return true;
            }
            let unused = self.export_unused(symbol);
            if unused {
                spans.push(specifier.span);
            }
            !unused
        });
        for span in spans {
            self.decisions.applied(span);
        }
        export.specifiers.len() == before || !export.specifiers.is_empty()
    }

    fn export_unused(&self, symbol: SymbolId) -> bool {
        let fact = self.query.facts.export_used(SymbolRef {
            module: self.query.module,
            symbol,
        });
        !fact.value && fact.trust == trioxide_analysis::Trust::Proven
    }
}

/// An empty block or `;`.
fn is_empty(statement: &Statement<'_>) -> bool {
    match statement {
        Statement::EmptyStatement(_) => true,
        Statement::BlockStatement(block) => block.body.is_empty(),
        _ => false,
    }
}
