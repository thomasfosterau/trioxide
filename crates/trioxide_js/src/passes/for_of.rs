//! PLAN §4 pass 7: `for…of` over proven arrays becomes an indexed loop, and spread of proven
//! arrays is lowered.
//!
//! **`for…of`.** `for (const x of items) body` becomes
//! `for (let i = 0; i < items.length; i++) { const x = items[i]; body }` when `items` is a
//! plain array with intact builtins ([`prove_array`]): the array iterator then runs no program
//! code, and the indexed loop does what it does.
//! - The iterator reads `length` before every step, so elements pushed during the loop are
//!   visited; the indexed loop re-reads it too, unless the loop resizes none of the arrays
//!   `items` may be ([`LoopFacts::resized`] with `resizes_untracked` false), when it is read
//!   once (`for (let i = 0, n = items.length; i < n; i++)`).
//! - A hole reads as `undefined` either way (nothing on the prototypes has indexed
//!   properties when the builtins are intact).
//! - The binding is declared inside the body, so it is still one per iteration (closures
//!   capture each), with the head's kind (`const`, `let`, `var`) and pattern (destructuring
//!   works the same). A plain identifier target becomes an assignment.
//! - `break`, `continue` and labels keep working: `continue` reaches the `i++`.
//! - An array expression other than a binding nothing reassigns (or one the body declares a
//!   name of its own for) is evaluated once, into a binding in the loop head, as the iterator
//!   held on to the array it was given.
//! - A destructuring head needs elements that are never nullish: destructuring `null` in a
//!   `for…of` head throws a different message than in a declaration.
//!
//! **Spread.** `[...a]` becomes `a.slice()` and `[...a, ...b]` becomes `a.concat(b)` when
//! every spread array is a plain array that never has holes (a spread turns holes into
//! `undefined`; `slice` and `concat` keep them). Spreads of tuples stay: TypeScript lets code
//! `push` onto a tuple, so its type doesn't fix its length.
//!
//! [`LoopFacts::resized`]: trioxide_analysis::LoopFacts::resized

use oxc_allocator::{ArenaVec, TakeIn};
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, AssignmentOperator, AssignmentTarget, BinaryOperator,
    Expression, ForStatementInit, ForStatementLeft, IdentifierName, NumberBase, Program, Statement,
    UpdateOperator, VariableDeclarationKind, VariableDeclarator,
};
use oxc_ast_visit::Visit;
use oxc_span::{GetSpan, SPAN, Span};
use oxc_syntax::{scope::ScopeFlags, symbol::SymbolFlags};
use oxc_traverse::{BoundIdentifier, Traverse, TraverseCtx};
use trioxide_types::Type;

use super::{
    Decisions, Position,
    arrays::{Kept, NamesIn, prove_array},
    track_position,
    util::{Query, never_nullish},
};
use crate::{Pass, PassContext};

/// PLAN §4 pass 7 (`"for-of"`).
pub struct ForOf;

impl Pass for ForOf {
    fn name(&self) -> &'static str {
        "for-of"
    }

    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
        let query = Query {
            facts: ctx.facts,
            module: ctx.module,
        };
        let mut visitor = Visitor {
            query,
            kept: Kept::new(program, ctx.keep, query),
            position: Position::default(),
            decisions: Decisions::new(self.name()),
            attempted: rustc_hash::FxHashSet::default(),
        };
        ctx.traverse(&mut visitor, program, ());
        let changed = visitor.decisions.flush(ctx.log);
        if changed {
            // The next speed pass gets a scoping that matches the rewritten AST.
            *ctx.scoping_mut() = super::speed::rescope(program);
        }
        changed
    }
}

struct Visitor<'f> {
    query: Query<'f>,
    kept: Kept,
    position: Position,
    decisions: Decisions,
    /// Loops already decided, by address (a loop in a list is tried before and after its body).
    attempted: rustc_hash::FxHashSet<usize>,
}

impl<'a> Traverse<'a, ()> for Visitor<'_> {
    track_position!(after_statement);

    fn enter_statements(
        &mut self,
        statements: &mut ArenaVec<'a, Statement<'a>>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.before_statements(statements, ctx);
    }

    fn exit_expression(&mut self, expression: &mut Expression<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        if !self.position.live() || self.kept.contains(expression.span()) {
            return;
        }
        if matches!(expression, Expression::ArrayExpression(_)) {
            self.array_spread(expression, ctx);
        }
    }
}

impl<'a> Visitor<'_> {
    /// Loops directly in a statement list are lowered before their bodies are visited, so an
    /// outer loop takes `i` and the loops in it `j`, `k`.
    fn before_statements(
        &mut self,
        statements: &mut ArenaVec<'a, Statement<'a>>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        for statement in statements.iter_mut() {
            let mut target = statement;
            while let Statement::LabeledStatement(labeled) = target {
                target = &mut labeled.body;
            }
            self.lower(target, ctx);
        }
    }

    fn after_statement(&mut self, statement: &mut Statement<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        self.lower(statement, ctx);
    }

    /// Lowers `statement` if it is a `for…of` loop over a proven array.
    fn lower(&mut self, statement: &mut Statement<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        if !self.position.live() {
            return;
        }
        let Statement::ForOfStatement(for_of) = statement else {
            return;
        };
        if self.query.dead(for_of.node_id.get())
            || self.kept.contains(for_of.span)
            || !self
                .attempted
                .insert(std::ptr::from_ref::<oxc_ast::ast::ForOfStatement<'a>>(for_of) as usize)
        {
            return;
        }
        if for_of.r#await {
            return;
        }
        let span = for_of.span;
        // Generated references carry the loop's span (DCE tells uses apart by span).
        let at = span;
        let proof = match prove_array(self.query, &for_of.right) {
            Ok(proof) => proof,
            Err(reason) => {
                self.decisions.skipped(span, reason);
                return;
            }
        };
        // Destructuring `null` or `undefined` in the head throws a different message than in a
        // declaration: patterns need elements that are never nullish.
        let destructures = matches!(
            &for_of.left,
            ForStatementLeft::VariableDeclaration(declaration)
                if declaration.declarations.iter().any(|d| !d.id.is_binding_identifier())
        );
        if destructures {
            let present = match self
                .query
                .ty(for_of.right.get_inner_expression().node_id())
                .value
            {
                Type::Array(element) => never_nullish(element),
                Type::Tuple(items) => items.iter().all(never_nullish),
                _ => false,
            };
            if !present {
                self.decisions.skipped(
                    span,
                    "the loop destructures elements that may be `null` or `undefined`",
                );
                return;
            }
        }
        let mut names = NamesIn::default();
        names.visit_for_of_statement(for_of);
        let head_names = match &for_of.left {
            ForStatementLeft::VariableDeclaration(declaration) => {
                let mut bound = NamesIn::default();
                for declarator in &declaration.declarations {
                    bound.visit_binding_pattern(&declarator.id);
                }
                bound.0
            }
            ForStatementLeft::AssignmentTargetIdentifier(_) => rustc_hash::FxHashSet::default(),
            _ => {
                self.decisions.skipped(
                    span,
                    "the loop assigns to a member or pattern, which it evaluates after reading \
                     the next element",
                );
                return;
            }
        };
        let mut right_names = NamesIn::default();
        right_names.visit_expression(&for_of.right);
        if head_names.iter().any(|name| right_names.0.contains(name)) {
            self.decisions.skipped(
                span,
                "the array expression reads a name the loop binds (in its temporal dead zone)",
            );
            return;
        }
        // Read the length once when nothing in the loop resizes the array.
        let fixed =
            self.query
                .node(for_of.node_id.get())
                .and_then(|node| self.query.facts.loop_facts(node))
                .is_some_and(|facts| {
                    !facts.resizes_untracked
                        && proof.sites.as_ref().is_some_and(|sites| {
                            !sites.iter().any(|site| facts.resized.contains(site))
                        })
                });
        let scope = for_of
            .scope_id
            .get()
            .unwrap_or_else(|| ctx.current_scope_id());
        let mut init: Vec<VariableDeclarator<'a>> = Vec::new();
        let right = for_of.right.take_in(&ctx.ast);
        // The body's own declarations would shadow the array's name where it reads `items[i]`.
        let mut declared = DeclaredIn::default();
        declared.visit_statement(&for_of.body);
        let reusable = stable_binding(&right, ctx)
            .filter(|binding| !declared.0.contains(binding.name.as_str()));
        let array = if let Some(binding) = reusable {
            binding
        } else {
            let base = match right.get_inner_expression() {
                Expression::StaticMemberExpression(member) => member.property.name.as_str(),
                _ => "items",
            };
            let binding = generate(&mut names, base, scope, ctx);
            init.push(VariableDeclarator::new(
                SPAN,
                binding.create_binding_pattern(ctx),
                None,
                Some(right),
                false,
                ctx,
            ));
            binding
        };
        let index =
            names.fresh_index(|name| ctx.scoping().find_binding(scope, name.into()).is_some());
        let index = generate_named(&index, scope, ctx);
        init.push(VariableDeclarator::new(
            SPAN,
            index.create_binding_pattern(ctx),
            None,
            Some(zero(ctx)),
            false,
            ctx,
        ));
        let bound = if fixed {
            let length = generate(&mut names, "length", scope, ctx);
            init.push(VariableDeclarator::new(
                SPAN,
                length.create_binding_pattern(ctx),
                None,
                Some(length_of(&array, at, ctx)),
                false,
                ctx,
            ));
            length.create_spanned_read_expression(at, ctx)
        } else {
            length_of(&array, at, ctx)
        };
        let test = Expression::new_binary_expression(
            SPAN,
            index.create_spanned_read_expression(at, ctx),
            BinaryOperator::LessThan,
            bound,
            ctx,
        );
        let update = Expression::new_update_expression(
            SPAN,
            UpdateOperator::Increment,
            false,
            index.create_spanned_read_write_simple_target(at, ctx),
            ctx,
        );
        let element = Expression::new_computed_member_expression(
            SPAN,
            array.create_spanned_read_expression(at, ctx),
            index.create_spanned_read_expression(at, ctx),
            false,
            ctx,
        );
        let first = match for_of.left.take_in(&ctx.ast) {
            ForStatementLeft::VariableDeclaration(mut declaration) => {
                let mut declarator = declaration
                    .declarations
                    .pop()
                    .expect("a for-of declaration declares one binding");
                declarator.init = Some(element);
                Statement::new_variable_declaration(
                    declaration.span,
                    declaration.kind,
                    ArenaVec::from_iter_in([declarator], &ctx.ast),
                    false,
                    ctx,
                )
            }
            ForStatementLeft::AssignmentTargetIdentifier(target) => {
                let target_span = target.span;
                let assignment = Expression::new_assignment_expression(
                    target_span,
                    AssignmentOperator::Assign,
                    AssignmentTarget::AssignmentTargetIdentifier(target),
                    element,
                    ctx,
                );
                Statement::new_expression_statement(target_span, assignment, ctx)
            }
            _ => unreachable!("other targets were declined"),
        };
        let body = for_of.body.take_in(&ctx.ast);
        let body = prepend(first, body, &head_names, scope, ctx);
        let kind = VariableDeclarationKind::Let;
        let init = ForStatementInit::new_variable_declaration(
            SPAN,
            kind,
            ArenaVec::from_iter_in(init, &ctx.ast),
            false,
            ctx,
        );
        *statement = Statement::new_for_statement_with_scope_id(
            span,
            Some(init),
            Some(test),
            Some(update),
            body,
            scope,
            ctx,
        );
        self.decisions.applied(span);
    }

    /// `[...a]` → `a.slice()`, `[...a, ...b]` → `a.concat(b)`; tuple spreads listed.
    fn array_spread(&mut self, expression: &mut Expression<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        let Expression::ArrayExpression(array) = expression else {
            return;
        };
        if array.elements.is_empty()
            || !array
                .elements
                .iter()
                .any(|element| matches!(element, ArrayExpressionElement::SpreadElement(_)))
        {
            return;
        }
        let span = array.span;
        let all_spread = array
            .elements
            .iter()
            .all(|element| matches!(element, ArrayExpressionElement::SpreadElement(_)));
        if all_spread {
            let mut proven = true;
            for element in &array.elements {
                let ArrayExpressionElement::SpreadElement(spread) = element else {
                    unreachable!("all elements are spreads")
                };
                match prove_array(self.query, &spread.argument) {
                    Ok(proof) if proof.packed => {}
                    Ok(_) => {
                        self.decisions.skipped(
                            spread.span,
                            "the array may have holes, which a spread reads as `undefined`",
                        );
                        proven = false;
                    }
                    Err(reason) => {
                        self.decisions.skipped(spread.span, reason);
                        proven = false;
                    }
                }
            }
            if proven {
                let mut arguments = array.elements.take_in(&ctx.ast).into_iter().map(|element| {
                    let ArrayExpressionElement::SpreadElement(spread) = element else {
                        unreachable!("all elements are spreads")
                    };
                    spread.unbox().argument
                });
                let first = arguments.next().expect("not empty");
                let rest: Vec<Argument<'a>> = arguments.map(Argument::from).collect();
                let method = if rest.is_empty() { "slice" } else { "concat" };
                let callee = Expression::new_static_member_expression(
                    span,
                    first,
                    IdentifierName::new(SPAN, method, ctx),
                    false,
                    ctx,
                );
                *expression = Expression::new_call_expression(
                    span,
                    callee,
                    None,
                    ArenaVec::from_iter_in(rest, &ctx.ast),
                    false,
                    ctx,
                );
                self.decisions.applied(span);
            }
        }
    }
}

/// The binding `expression` reads, if it is one nothing reassigns (so reading it again gives
/// the same value).
pub fn stable_binding<'a>(
    expression: &Expression<'a>,
    ctx: &TraverseCtx<'a, ()>,
) -> Option<BoundIdentifier<'a>> {
    let Expression::Identifier(ident) = expression.get_inner_expression() else {
        return None;
    };
    let symbol = ctx
        .scoping()
        .get_reference(ident.reference_id.get()?)
        .symbol_id()?;
    (!ctx.scoping().symbol_is_mutated(symbol)).then(|| BoundIdentifier::new(ident.name, symbol))
}

/// A new `let` binding in `scope`, named from `base` and fresh among `names`.
pub fn generate<'a>(
    names: &mut NamesIn,
    base: &str,
    scope: oxc_syntax::scope::ScopeId,
    ctx: &mut TraverseCtx<'a, ()>,
) -> BoundIdentifier<'a> {
    let name = names.fresh(base);
    generate_named(&name, scope, ctx)
}

/// A new `let` binding in `scope` called `name`.
pub fn generate_named<'a>(
    name: &str,
    scope: oxc_syntax::scope::ScopeId,
    ctx: &mut TraverseCtx<'a, ()>,
) -> BoundIdentifier<'a> {
    let name = oxc_ast::ast::Ident::from_str_in(name, ctx);
    ctx.generate_binding(name, scope, SymbolFlags::BlockScopedVariable)
}

/// `0`.
pub fn zero<'a>(ctx: &TraverseCtx<'a, ()>) -> Expression<'a> {
    Expression::new_numeric_literal(SPAN, 0.0, None, NumberBase::Decimal, ctx)
}

/// `array.length`, reading `array` at `at`.
pub fn length_of<'a>(
    array: &BoundIdentifier<'a>,
    at: Span,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    Expression::new_static_member_expression(
        SPAN,
        array.create_spanned_read_expression(at, ctx),
        IdentifierName::new(SPAN, "length", ctx),
        false,
        ctx,
    )
}

/// `body` with `first` run before it, in one block. A block body takes `first` at its top
/// unless it declares one of `bound` (which `first` binds) itself; then it nests.
fn prepend<'a>(
    first: Statement<'a>,
    body: Statement<'a>,
    bound: &rustc_hash::FxHashSet<String>,
    scope: oxc_syntax::scope::ScopeId,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Statement<'a> {
    if let Statement::BlockStatement(mut block) = body {
        let mut declared = NamesIn::default();
        for statement in &block.body {
            if let Statement::VariableDeclaration(declaration) = statement {
                for declarator in &declaration.declarations {
                    declared.visit_binding_pattern(&declarator.id);
                }
            } else if let Statement::FunctionDeclaration(function) = statement
                && let Some(id) = &function.id
            {
                declared.0.insert(id.name.to_string());
            } else if let Statement::ClassDeclaration(class) = statement
                && let Some(id) = &class.id
            {
                declared.0.insert(id.name.to_string());
            }
        }
        if !bound.iter().any(|name| declared.0.contains(name)) {
            block.body.insert(0, first);
            return Statement::BlockStatement(block);
        }
        let nested = Statement::BlockStatement(block);
        let scope = ctx.create_child_scope(scope, ScopeFlags::empty());
        return Statement::new_block_statement_with_scope_id(
            nested.span(),
            ArenaVec::from_iter_in([first, nested], &ctx.ast),
            scope,
            ctx,
        );
    }
    let scope = ctx.create_child_scope(scope, ScopeFlags::empty());
    Statement::new_block_statement_with_scope_id(
        body.span(),
        ArenaVec::from_iter_in([first, body], &ctx.ast),
        scope,
        ctx,
    )
}

/// The names a subtree declares (bindings only, not references).
#[derive(Default)]
struct DeclaredIn(rustc_hash::FxHashSet<String>);

impl<'a> Visit<'a> for DeclaredIn {
    fn visit_binding_identifier(&mut self, ident: &oxc_ast::ast::BindingIdentifier<'a>) {
        self.0.insert(ident.name.to_string());
    }
}
