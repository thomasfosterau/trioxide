//! PLAN §4 pass 2: constant propagation and folding.
//!
//! - **Values**: an expression whose `ty` is a usable singleton (one number, `null` or
//!   `undefined`) and which is usable-pure becomes that literal. -0, NaN and int32 wraparound
//!   come out exactly as the range facts say; literals print through
//!   [`constant_expr`](super::util::constant_expr).
//! - **Ranges**: numeric comparisons whose operands' ranges decide them, and `Math.abs`,
//!   `floor`, `ceil`, `round`, `trunc`, `max` and `min` calls whose result is one of their
//!   arguments.
//! - **Branches**: `if`, `?:`, `&&` and `||` with a test whose truthiness is a literal's or
//!   the usable type's (and pure, when the test is dropped) become the branch taken. Branches
//!   that declare a `var` or function stay, since their bindings are visible outside them.
//!
//! Syntactic folding with `oxc_ecmascript` happens in [`Simplify`](super::Simplify).

use oxc_allocator::TakeIn;
use oxc_ast::ast::{Argument, BinaryOperator, Expression, LogicalOperator, Program, Statement};
use oxc_ecmascript::constant_evaluation::ConstantEvaluation;
use oxc_semantic::Scoping;
use oxc_span::GetSpan;
use oxc_syntax::symbol::SymbolId;
use oxc_traverse::{Traverse, TraverseCtx};
use rustc_hash::FxHashSet;
use trioxide_common::NodeId;
use trioxide_types::{NumberType, Type};

use super::{
    Decisions, Position, Unwrapped, track_position,
    util::{
        EvalCtx, Query, Slot, binds_or_suspends, caller_dependent, constant_expr, declares_hoisted,
        depends_on, is_canonical_literal, is_global_name, is_inert, truthiness, type_constant,
        weak,
    },
};
use crate::{Keep, Pass, PassContext};

/// PLAN §4 pass 2 (`"fold"`), run third.
pub struct Fold;

impl Pass for Fold {
    fn name(&self) -> &'static str {
        "fold"
    }

    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
        let dependent = (!ctx.specialize).then(|| caller_dependent(program, ctx.scoping()));
        let mut visitor = Visitor {
            dependent,
            query: Query {
                facts: ctx.facts,
                module: ctx.module,
            },
            keep: ctx.keep,
            position: Position::default(),
            decisions: Decisions::new(self.name()),
            unwrapped: Unwrapped::default(),
        };
        ctx.traverse(&mut visitor, program, ());
        visitor.decisions.flush(ctx.log)
    }
}

struct Visitor<'f> {
    query: Query<'f>,
    keep: &'f Keep,
    /// Bindings whose values may come from callers, unless `passes.specialize` is on.
    dependent: Option<FxHashSet<SymbolId>>,
    position: Position,
    decisions: Decisions,
    unwrapped: Unwrapped,
}

impl<'a> Traverse<'a, ()> for Visitor<'_> {
    track_position!(after_statement);

    fn exit_statements(
        &mut self,
        statements: &mut oxc_allocator::ArenaVec<'a, Statement<'a>>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.unwrapped.splice(statements, &ctx.ast);
    }

    fn enter_expression(&mut self, expression: &mut Expression<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        // A branch taken may itself fold.
        for _ in 0..4 {
            if !self.position.live() || !self.fold(expression, ctx) {
                break;
            }
        }
    }
}

impl Visitor<'_> {
    /// Folds `expression` once. Returns whether it changed.
    fn fold<'a>(&mut self, expression: &mut Expression<'a>, ctx: &mut TraverseCtx<'a, ()>) -> bool {
        if is_canonical_literal(expression)
            || matches!(
                expression,
                Expression::Super(_)
                    | Expression::NewTarget(_)
                    | Expression::ImportMeta(_)
                    | Expression::FunctionExpression(_)
                    | Expression::ArrowFunctionExpression(_)
                    | Expression::ClassExpression(_)
                    | Expression::ParenthesizedExpression(_)
            )
        {
            return false;
        }
        let slot = Slot::of(ctx);
        if matches!(slot, Slot::Delete | Slot::Statement) {
            return false;
        }
        let span = expression.span();
        let node = expression.node_id();
        // Literals: any value fold the slot admits.
        let literal_ok = slot.admits(&Expression::new_null_literal(span, ctx), true);
        if literal_ok && let Some(value) = self.value(expression, node, span, ctx.scoping()) {
            let literal = constant_expr(&value, span, ctx);
            *expression = slot.wrap(literal, ctx);
            self.decisions.applied(span);
            return true;
        }
        let take = |branch: &mut Expression<'a>, ctx: &TraverseCtx<'a, ()>| {
            slot.admits(branch, false).then(|| branch.take_in(&ctx.ast))
        };
        let replacement = match expression {
            Expression::BinaryExpression(_) if literal_ok => self
                .comparison(expression, ctx.scoping())
                .map(|value| Expression::new_boolean_literal(span, value, ctx)),
            Expression::CallExpression(_) => self.math(expression, slot, ctx),
            Expression::ConditionalExpression(conditional) => {
                match self.test(&conditional.test, ctx, true) {
                    Some(true) => take(&mut conditional.consequent, ctx),
                    Some(false) => take(&mut conditional.alternate, ctx),
                    None => None,
                }
            }
            Expression::LogicalExpression(logical) => {
                let keep_left = match logical.operator {
                    LogicalOperator::And => {
                        self.test(&logical.left, ctx, false).map(|truthy| !truthy)
                    }
                    LogicalOperator::Or => self.test(&logical.left, ctx, false),
                    LogicalOperator::Coalesce => None,
                };
                match keep_left {
                    Some(true) => take(&mut logical.left, ctx),
                    // The left operand is dropped: it must be pure.
                    Some(false) if self.test(&logical.left, ctx, true).is_some() => {
                        take(&mut logical.right, ctx)
                    }
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

    /// The single value of the expression `node`, if it is usable and the expression pure.
    fn value(
        &mut self,
        expression: &Expression<'_>,
        node: NodeId,
        span: oxc_span::Span,
        scoping: &Scoping,
    ) -> Option<trioxide_analysis::Constant> {
        let fact = self.query.ty(node);
        let value = type_constant(fact.value)?;
        if binds_or_suspends(expression) {
            return None;
        }
        if self.specialises(expression, scoping) {
            self.decisions
                .skipped(span, "the expression's single value depends on what its callers pass (`passes.specialize` is off)");
            return None;
        }
        if self.calls_island(expression) {
            self.decisions
                .skipped(span, "calls a WASM island function, which stays a call");
            return None;
        }
        if !self.query.usable(fact.trust) {
            self.decisions
                .skipped(span, weak(fact.trust, "the expression's single value"));
            return None;
        }
        let pure = self.query.is_pure(node);
        if !(pure.value && self.query.usable(pure.trust)) {
            if pure.value || pure.trust == trioxide_analysis::Trust::Unknown {
                // Evaluated as part of something else (a callee, a `?.` link), or unknown.
                return None;
            }
            self.decisions
                .skipped(span, "the expression has effects, so it stays");
            return None;
        }
        Some(value)
    }

    /// Whether a fact about `expression` may rest on what its function's callers pass (and
    /// `passes.specialize` is off).
    fn specialises(&self, expression: &Expression<'_>, scoping: &Scoping) -> bool {
        self.dependent
            .as_ref()
            .is_some_and(|dependent| depends_on(expression, dependent, scoping))
    }

    /// Whether evaluating `expression` calls a function compiled into a WASM island.
    fn calls_island(&self, expression: &Expression<'_>) -> bool {
        struct Find<'q, 'f> {
            query: Query<'f>,
            keep: &'q Keep,
            found: bool,
        }
        impl<'a> oxc_ast_visit::Visit<'a> for Find<'_, '_> {
            fn visit_call_expression(&mut self, call: &oxc_ast::ast::CallExpression<'a>) {
                if let Some(site) = self.query.node(call.node_id.get()) {
                    let resolution = self.query.facts.site_resolution(site);
                    self.found |= resolution
                        .callees
                        .iter()
                        .any(|callee| self.keep.functions.contains(callee));
                }
                oxc_ast_visit::walk::walk_call_expression(self, call);
            }
        }
        if self.keep.functions.is_empty() {
            return false;
        }
        let mut find = Find {
            query: self.query,
            keep: self.keep,
            found: false,
        };
        oxc_ast_visit::Visit::visit_expression(&mut find, expression);
        find.found
    }

    /// The number type of a usable numeric operand.
    fn number(&self, expression: &Expression<'_>) -> Option<NumberType> {
        match self.query.usable_ty(expression.node_id())? {
            Type::Number(number) => Some(*number),
            _ => None,
        }
    }

    /// A numeric comparison its operands' ranges decide.
    fn comparison(&mut self, expression: &Expression<'_>, scoping: &Scoping) -> Option<bool> {
        let Expression::BinaryExpression(binary) = expression else {
            return None;
        };
        if !matches!(
            binary.operator,
            BinaryOperator::LessThan
                | BinaryOperator::LessEqualThan
                | BinaryOperator::GreaterThan
                | BinaryOperator::GreaterEqualThan
                | BinaryOperator::StrictEquality
                | BinaryOperator::StrictInequality
                | BinaryOperator::Equality
                | BinaryOperator::Inequality
        ) {
            return None;
        }
        let (left, right) = (self.number(&binary.left)?, self.number(&binary.right)?);
        let outcome = compare(binary.operator, left, right)?;
        if self.specialises(expression, scoping) {
            self.decisions.skipped(
                binary.span,
                "the comparison depends on what its callers pass (`passes.specialize` is off)",
            );
            return None;
        }
        if !self.query.pure(binary.node_id.get()) || binds_or_suspends(expression) {
            self.decisions
                .skipped(binary.span, "the comparison's operands have effects");
            return None;
        }
        Some(outcome)
    }

    /// A `Math` call whose result is one of its arguments: that argument.
    fn math<'a>(
        &mut self,
        expression: &mut Expression<'a>,
        slot: Slot,
        ctx: &TraverseCtx<'a, ()>,
    ) -> Option<Expression<'a>> {
        let Expression::CallExpression(call) = expression else {
            return None;
        };
        let Expression::StaticMemberExpression(callee) = &call.callee else {
            return None;
        };
        let Expression::Identifier(math) = &callee.object else {
            return None;
        };
        if math.name != "Math" || !is_global_name(ctx.scoping(), ctx.current_scope_id(), "Math") {
            return None;
        }
        let arguments: Vec<&Expression<'_>> = call
            .arguments
            .iter()
            .map(Argument::as_expression)
            .collect::<Option<_>>()?;
        let numbers: Vec<NumberType> = arguments
            .iter()
            .map(|argument| self.number(argument))
            .collect::<Option<_>>()?;
        let index = match (callee.property.name.as_str(), numbers.as_slice()) {
            ("abs", [x]) if x.range.min >= 0.0 && x.non_neg_zero => 0,
            ("floor" | "ceil" | "round" | "trunc", [x]) if x.int => 0,
            ("max" | "min", [_]) => 0,
            ("max", [a, b]) => {
                if dominates(*a, *b) {
                    0
                } else if dominates(*b, *a) {
                    1
                } else {
                    return None;
                }
            }
            ("min", [a, b]) => {
                if dominates(*b, *a) {
                    0
                } else if dominates(*a, *b) {
                    1
                } else {
                    return None;
                }
            }
            _ => return None,
        };
        if call
            .arguments
            .iter()
            .any(|argument| argument.as_expression().is_some_and(binds_or_suspends))
        {
            return None;
        }
        if call
            .arguments
            .iter()
            .filter_map(Argument::as_expression)
            .any(|argument| self.specialises(argument, ctx.scoping()))
        {
            self.decisions.skipped(
                call.span,
                "the call's result depends on what its callers pass (`passes.specialize` is off)",
            );
            return None;
        }
        let result = self.query.ty(call.node_id.get());
        if !self.query.usable(result.trust) || !matches!(result.value, Type::Number(_)) {
            return None;
        }
        if !self.query.pure(call.node_id.get()) {
            self.decisions
                .skipped(call.span, "the call's arguments have effects");
            return None;
        }
        let argument = call.arguments[index].as_expression_mut()?;
        slot.admits(argument, false)
            .then(|| argument.take_in(&ctx.ast))
    }

    /// The truthiness of a test: a literal's, or the usable type's. With `dropped`, the test
    /// must also be pure (it won't be evaluated any more).
    fn test(
        &mut self,
        test: &Expression<'_>,
        ctx: &TraverseCtx<'_, ()>,
        dropped: bool,
    ) -> Option<bool> {
        if is_inert(test, ctx.scoping()) {
            let eval = EvalCtx {
                ast: &ctx.ast,
                scoping: ctx.scoping(),
            };
            return test.evaluate_value_to_boolean(&eval);
        }
        if self.specialises(test, ctx.scoping()) {
            if truthiness(self.query.ty(test.node_id()).value).is_some() {
                self.decisions.skipped(
                    test.span(),
                    "the test depends on what its callers pass (`passes.specialize` is off)",
                );
            }
            return None;
        }
        let ty = self.query.ty(test.node_id());
        let truthy = truthiness(ty.value)?;
        if !self.query.usable(ty.trust) {
            self.decisions
                .skipped(test.span(), weak(ty.trust, "the test's truthiness"));
            return None;
        }
        if dropped && (!self.query.pure(test.node_id()) || binds_or_suspends(test)) {
            return None;
        }
        Some(truthy)
    }

    fn after_statement<'a>(
        &mut self,
        statement: &mut Statement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if !self.position.live() || self.query.dead(statement.node_id()) {
            return;
        }
        let Statement::IfStatement(if_statement) = statement else {
            return;
        };
        let Some(truthy) = self.test(&if_statement.test, ctx, true) else {
            return;
        };
        let span = if_statement.span;
        let dropped = if truthy {
            if_statement.alternate.as_ref()
        } else {
            Some(&if_statement.consequent)
        };
        if dropped.is_some_and(declares_hoisted) {
            self.decisions
                .skipped(span, "the branch not taken declares a `var` or function");
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
}

/// Whether `a >= b` for every pair of values and `a` is never `-0` where they tie, so
/// `Math.max(a, b)` is `a` and `Math.min(b, a)` is `b` exactly (`max(-0, 0)` is `0`, and
/// `min(0, -0)` is `-0`).
fn dominates(a: NumberType, b: NumberType) -> bool {
    a.non_nan
        && b.non_nan
        && (a.range.min > b.range.max || (a.range.min >= b.range.max && a.non_neg_zero))
}

/// What `left <op> right` is for every pair of numbers in the operands' types, if one answer.
fn compare(operator: BinaryOperator, left: NumberType, right: NumberType) -> Option<bool> {
    let (l, r) = (left.range, right.range);
    let both = left.non_nan && right.non_nan;
    // `0 === -0` holds, so a zero point needs no sign check.
    let equal_points = both && l.as_point().is_some() && l.as_point() == r.as_point();
    let disjoint = l.max < r.min || r.max < l.min;
    match operator {
        BinaryOperator::LessThan => {
            if both && l.max < r.min {
                Some(true)
            } else if l.min >= r.max {
                // NaN compares false too.
                Some(false)
            } else {
                None
            }
        }
        BinaryOperator::LessEqualThan => {
            if both && l.max <= r.min {
                Some(true)
            } else if l.min > r.max {
                Some(false)
            } else {
                None
            }
        }
        BinaryOperator::GreaterThan => compare(BinaryOperator::LessThan, right, left),
        BinaryOperator::GreaterEqualThan => compare(BinaryOperator::LessEqualThan, right, left),
        BinaryOperator::StrictEquality | BinaryOperator::Equality => {
            if disjoint {
                Some(false)
            } else if equal_points {
                Some(true)
            } else {
                None
            }
        }
        BinaryOperator::StrictInequality | BinaryOperator::Inequality => {
            compare(BinaryOperator::StrictEquality, left, right).map(|equal| !equal)
        }
        _ => None,
    }
}
