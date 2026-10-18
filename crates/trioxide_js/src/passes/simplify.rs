//! The syntactic cleanup after the fact-driven passes: it reads no facts, so it may run to a
//! fixed point without refreshing them.
//!
//! - Inert expressions (literals and operators over them, see
//!   [`is_inert`](super::util::is_inert)) fold to literals with `oxc_ecmascript`'s constant
//!   evaluation; strings longer than
//!   [`MAX_STRING_BYTES`](super::util::MAX_STRING_BYTES) stay as written.
//! - `if`, `?:`, `&&`, `||` and `while` with inert tests take the branch they always take
//!   (keeping branches that declare a `var` or function).
//! - Empty statements, inert expression statements and unused bindings with inert
//!   initialisers go; blocks in statement lists that declare nothing block-scoped are
//!   flattened.

use std::sync::LazyLock;

use oxc_allocator::{ArenaVec, TakeIn};
use oxc_ast::ast::{Expression, LogicalOperator, Program, Statement};
use oxc_ecmascript::constant_evaluation::ConstantEvaluation;
use oxc_span::GetSpan;
use oxc_traverse::{Traverse, TraverseCtx};
use trioxide_analysis::Facts;

use super::{
    Comments, Decisions, Position, Unwrapped, track_position,
    util::{
        EvalCtx, Query, Slot, constant_expr, declares_hoisted, from_ecmascript,
        is_canonical_literal, is_inert, small_enough,
    },
};
use crate::{Pass, PassContext};

/// No facts: what syntactic passes query, so stale facts are never read.
pub(super) static NO_FACTS: LazyLock<Facts> = LazyLock::new(Facts::default);

/// The cleanup pass (`"simplify"`).
pub struct Simplify;

impl Pass for Simplify {
    fn name(&self) -> &'static str {
        "simplify"
    }

    fn reads_facts(&self) -> bool {
        false
    }

    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
        let query = Query {
            facts: &NO_FACTS,
            module: ctx.module,
        };
        let mut visitor = Visitor {
            query,
            position: Position::default(),
            decisions: Decisions::new(self.name()),
            unwrapped: Unwrapped::default(),
            comments: Comments::default(),
        };
        ctx.traverse(&mut visitor, program, ());
        let comments = std::mem::take(&mut visitor.comments);
        comments.apply(program);
        let folded = visitor.decisions.flush(ctx.log);
        // Unused bindings with inert initialisers, inert statements.
        let removed = super::dce::run(self.name(), program, ctx, None);
        folded || removed
    }
}

struct Visitor<'f> {
    query: Query<'f>,
    position: Position,
    decisions: Decisions,
    unwrapped: Unwrapped,
    comments: Comments,
}

impl<'a> Traverse<'a, ()> for Visitor<'_> {
    track_position!(after_statement);

    fn enter_expression(&mut self, expression: &mut Expression<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        for _ in 0..4 {
            if self.position.opaque() || !self.fold(expression, ctx) {
                break;
            }
        }
    }

    fn exit_statements(
        &mut self,
        statements: &mut ArenaVec<'a, Statement<'a>>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if self.position.opaque() {
            return;
        }
        self.unwrapped.splice(statements, &ctx.ast);
        // Statements after a `return`, `throw`, `break` or `continue` in the same list never run.
        if let Some(end) = statements.iter().position(|statement| {
            matches!(
                statement,
                Statement::ReturnStatement(_)
                    | Statement::ThrowStatement(_)
                    | Statement::BreakStatement(_)
                    | Statement::ContinueStatement(_)
            )
        }) {
            let keep: Vec<bool> = statements
                .iter()
                .enumerate()
                .map(|(index, statement)| {
                    // Bindings declared there are still visible (or in their dead zone)
                    // elsewhere.
                    let keep = index <= end
                        || declares_hoisted(statement)
                        || super::declares_block_scoped(statement);
                    if !keep {
                        self.decisions.applied(statement.span());
                    }
                    keep
                })
                .collect();
            self.comments.remove(statements, &keep);
        }
        let keep: Vec<bool> = statements
            .iter()
            .map(|statement| {
                let empty = match statement {
                    Statement::EmptyStatement(_) => true,
                    Statement::BlockStatement(block) => block.body.is_empty(),
                    _ => false,
                };
                if empty {
                    self.decisions.applied(statement.span());
                }
                !empty
            })
            .collect();
        self.comments.remove(statements, &keep);
    }
}

/// The value of an inert bitwise operation (`|`, `&`, `^`, `<<`, `>>`, `>>>`, `~`), computed
/// with ToInt32/ToUint32 exactly, or of unary `-`/`+` over a folded number
/// (`oxc_ecmascript` leaves these alone).
fn int32(expression: &Expression<'_>, eval: &EvalCtx<'_, '_>) -> Option<f64> {
    use oxc_ast::ast::{BinaryOperator, UnaryOperator};
    use oxc_ecmascript::{ToInt32, ToUint32};
    match expression {
        Expression::BinaryExpression(binary) => {
            let left = binary
                .left
                .without_parentheses()
                .evaluate_value_to_number(eval)?;
            let right = binary
                .right
                .without_parentheses()
                .evaluate_value_to_number(eval)?;
            let shift = right.to_uint_32() & 31;
            Some(match binary.operator {
                BinaryOperator::BitwiseOR => f64::from(left.to_int_32() | right.to_int_32()),
                BinaryOperator::BitwiseAnd => f64::from(left.to_int_32() & right.to_int_32()),
                BinaryOperator::BitwiseXOR => f64::from(left.to_int_32() ^ right.to_int_32()),
                BinaryOperator::ShiftLeft => f64::from(left.to_int_32().wrapping_shl(shift)),
                BinaryOperator::ShiftRight => f64::from(left.to_int_32() >> shift),
                BinaryOperator::ShiftRightZeroFill => f64::from(left.to_uint_32() >> shift),
                _ => return None,
            })
        }
        // `oxc_ecmascript` leaves `-(-0)` and `+(-1)` alone.
        Expression::UnaryExpression(unary)
            if matches!(
                unary.operator,
                UnaryOperator::UnaryNegation | UnaryOperator::UnaryPlus
            ) && !matches!(unary.argument, Expression::NumericLiteral(_)) =>
        {
            let value = number(&unary.argument, eval)?;
            Some(if unary.operator == UnaryOperator::UnaryNegation {
                -value
            } else {
                value
            })
        }
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::BitwiseNot => {
            let argument = unary.argument.without_parentheses();
            Some(f64::from(
                !argument.evaluate_value_to_number(eval)?.to_int_32(),
            ))
        }
        _ => None,
    }
}

/// Whether an optional chain always short-circuits: its base is `null` or `undefined` and the
/// link on the base is `?.`, so the chain is `undefined` and nothing else in it runs.
fn short_circuits(element: &oxc_ast::ast::ChainElement<'_>, ctx: &TraverseCtx<'_, ()>) -> bool {
    use oxc_ast::ast::ChainElement;
    fn link(expression: &Expression<'_>, ctx: &TraverseCtx<'_, ()>) -> bool {
        match expression {
            Expression::CallExpression(call) => step(&call.callee, call.optional, ctx),
            Expression::TSNonNullExpression(non_null) => link(&non_null.expression, ctx),
            other => other
                .as_member_expression()
                .is_some_and(|member| step(member.object(), member.optional(), ctx)),
        }
    }
    fn step(object: &Expression<'_>, optional: bool, ctx: &TraverseCtx<'_, ()>) -> bool {
        let nullish = match object {
            Expression::NullLiteral(_) => true,
            Expression::Identifier(ident) => {
                ident.name == "undefined" && super::util::is_global_constant(ident, ctx.scoping())
            }
            Expression::UnaryExpression(unary) => {
                unary.operator == oxc_ast::ast::UnaryOperator::Void
                    && is_inert(&unary.argument, ctx.scoping())
            }
            _ => false,
        };
        if nullish { optional } else { link(object, ctx) }
    }
    match element {
        ChainElement::CallExpression(call) => step(&call.callee, call.optional, ctx),
        ChainElement::TSNonNullExpression(non_null) => link(&non_null.expression, ctx),
        member => member
            .as_member_expression()
            .is_some_and(|member| step(member.object(), member.optional(), ctx)),
    }
}

/// `ToNumber` of a constant operand that isn't a bigint (bigint operators work differently).
fn number(expression: &Expression<'_>, eval: &EvalCtx<'_, '_>) -> Option<f64> {
    use oxc_ecmascript::ToNumber;
    let value = expression.without_parentheses().evaluate_value(eval)?;
    if value.is_big_int() {
        return None;
    }
    value.to_number(eval)
}

impl Visitor<'_> {
    fn eval_ctx<'a, 's>(ctx: &'s TraverseCtx<'a, ()>) -> EvalCtx<'a, 's> {
        EvalCtx {
            ast: &ctx.ast,
            scoping: ctx.scoping(),
        }
    }

    fn truthiness(test: &Expression<'_>, ctx: &TraverseCtx<'_, ()>) -> Option<bool> {
        if !is_inert(test, ctx.scoping()) {
            return None;
        }
        test.evaluate_value_to_boolean(&Self::eval_ctx(ctx))
    }

    fn fold<'a>(&mut self, expression: &mut Expression<'a>, ctx: &mut TraverseCtx<'a, ()>) -> bool {
        let slot = Slot::of(ctx);
        if matches!(slot, Slot::Delete | Slot::Statement) {
            return false;
        }
        // Parentheses left around what a fold made a primary expression (but `(eval)(…)` is
        // an indirect eval).
        if let Expression::ParenthesizedExpression(paren) = expression
            && !(slot == Slot::Callee && matches!(paren.expression, Expression::Identifier(_)))
            && matches!(
                &paren.expression,
                Expression::NumericLiteral(_)
                    | Expression::StringLiteral(_)
                    | Expression::BooleanLiteral(_)
                    | Expression::NullLiteral(_)
                    | Expression::BigIntLiteral(_)
                    | Expression::Identifier(_)
            )
            && !matches!(&paren.expression, Expression::NumericLiteral(n) if n.value.is_sign_negative())
        {
            *expression = paren.expression.take_in(&ctx.ast);
            self.decisions.quiet();
            return true;
        }
        if is_canonical_literal(expression)
            || matches!(
                expression,
                Expression::ParenthesizedExpression(_)
                    | Expression::FunctionExpression(_)
                    | Expression::ArrowFunctionExpression(_)
            )
        {
            return false;
        }
        let span = expression.span();
        let literal_ok = slot.admits(&Expression::new_null_literal(span, ctx), true);
        let take = |branch: &mut Expression<'a>, ctx: &TraverseCtx<'a, ()>| {
            slot.admits(branch, false).then(|| branch.take_in(&ctx.ast))
        };
        let replacement = match expression {
            Expression::ChainExpression(chain)
                if literal_ok && short_circuits(&chain.expression, ctx) =>
            {
                Some(constant_expr(
                    &trioxide_analysis::Constant::Undefined,
                    span,
                    ctx,
                ))
            }
            Expression::ConditionalExpression(conditional) => {
                match Self::truthiness(&conditional.test, ctx) {
                    Some(true) => take(&mut conditional.consequent, ctx),
                    Some(false) => take(&mut conditional.alternate, ctx),
                    None => None,
                }
            }
            Expression::LogicalExpression(logical) => {
                let left = match logical.operator {
                    LogicalOperator::Coalesce => {
                        if is_inert(&logical.left, ctx.scoping()) {
                            logical
                                .left
                                .evaluate_value(&Self::eval_ctx(ctx))
                                .map(|value| value.is_null() || value.is_undefined())
                                .map(|nullish| !nullish)
                        } else {
                            None
                        }
                    }
                    LogicalOperator::And => {
                        Self::truthiness(&logical.left, ctx).map(|truthy| !truthy)
                    }
                    LogicalOperator::Or => Self::truthiness(&logical.left, ctx),
                };
                match left {
                    Some(true) => take(&mut logical.left, ctx),
                    Some(false) => take(&mut logical.right, ctx),
                    None => None,
                }
            }
            _ if literal_ok && is_inert(expression, ctx.scoping()) => {
                let value = int32(expression, &Self::eval_ctx(ctx))
                    .map(oxc_ecmascript::ConstantValue::Number)
                    .or_else(|| expression.evaluate_value(&Self::eval_ctx(ctx)));
                match value.map(|value| from_ecmascript(&value)) {
                    Some(value) if small_enough(&value) => Some(constant_expr(&value, span, ctx)),
                    _ => None,
                }
            }
            _ => None,
        };
        match replacement {
            Some(replacement) => {
                *expression = slot.wrap(replacement, ctx);
                self.decisions.applied(span);
                true
            }
            None => false,
        }
    }

    fn after_statement<'a>(
        &mut self,
        statement: &mut Statement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if self.position.opaque() {
            return;
        }
        let span = statement.span();
        match statement {
            Statement::IfStatement(if_statement) => {
                let Some(truthy) = Self::truthiness(&if_statement.test, ctx) else {
                    return;
                };
                let dropped = if truthy {
                    if_statement.alternate.as_ref()
                } else {
                    Some(&if_statement.consequent)
                };
                if dropped.is_some_and(declares_hoisted) {
                    return;
                }
                let taken = if truthy {
                    Some(if_statement.consequent.take_in(&ctx.ast))
                } else {
                    if_statement
                        .alternate
                        .as_mut()
                        .map(|alternate| alternate.take_in(&ctx.ast))
                };
                if let Some(taken) = &taken {
                    self.unwrapped.record(taken, &ctx.parent());
                }
                *statement = taken.unwrap_or_else(|| Statement::new_empty_statement(span, ctx));
                self.decisions.applied(span);
            }
            Statement::WhileStatement(while_statement)
                if Self::truthiness(&while_statement.test, ctx) == Some(false)
                    && !declares_hoisted(&while_statement.body) =>
            {
                *statement = Statement::new_empty_statement(span, ctx);
                self.decisions.applied(span);
            }
            _ => {}
        }
    }
}
