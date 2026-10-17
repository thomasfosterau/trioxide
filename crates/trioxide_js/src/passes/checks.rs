//! Pass 4: non-null and `typeof` check elimination (PLAN §4).
//!
//! - `a?.b` becomes `a.b`, and `a?.()` becomes `a()`, when `ty(a)` is usable and excludes
//!   `null` and `undefined`.
//! - `x ?? y` becomes `x` under the same condition, and `y` when `x` is proven nullish and
//!   pure.
//! - `typeof x === "…"`, `x === null`, `x == null`, `x !== undefined` (and their negations and
//!   mirror images) become `true` or `false` when the type decides them and the comparison is
//!   pure.
//!
//! Guards that become constant are folded by [`Fold`](super::Fold), which runs next.

use oxc_allocator::TakeIn;
use oxc_ast::ast::{
    BinaryOperator, CallExpression, ChainElement, Expression, LogicalOperator, MemberExpression,
    Program, UnaryOperator,
};
use oxc_span::{GetSpan, Span};
use oxc_traverse::{Traverse, TraverseCtx};
use trioxide_common::NodeId;
use trioxide_types::Type;

use super::{
    Decisions, Position, track_position,
    util::{
        Query, always_nullish, binds_or_suspends, is_global_constant, is_protected_position,
        never_nullish, typeof_names, weak,
    },
};
use crate::{Pass, PassContext};

/// Pass 4 (`"checks"`).
pub struct Checks;

impl Pass for Checks {
    fn name(&self) -> &'static str {
        "checks"
    }

    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
        let mut visitor = Visitor {
            query: Query {
                facts: ctx.facts,
                module: ctx.module,
            },
            position: Position::default(),
            decisions: Decisions::new(self.name()),
        };
        ctx.traverse(&mut visitor, program, ());
        visitor.decisions.flush(ctx.log)
    }
}

struct Visitor<'f> {
    query: Query<'f>,
    position: Position,
    decisions: Decisions,
}

impl<'a> Traverse<'a, ()> for Visitor<'_> {
    track_position!();

    fn exit_expression(&mut self, expression: &mut Expression<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        if !self.position.live() {
            return;
        }
        match expression {
            Expression::ChainExpression(chain) => {
                if !self.chain(&mut chain.expression) {
                    *expression = Expression::from(chain.expression.take_in(&ctx.ast));
                }
            }
            Expression::LogicalExpression(logical)
                if logical.operator == LogicalOperator::Coalesce =>
            {
                let span = logical.span;
                let left = logical.left.node_id();
                let fact = self.query.ty(left);
                if self.query.usable(fact.trust) && never_nullish(fact.value) {
                    self.decisions.applied(span);
                    let value = logical.left.take_in(&ctx.ast);
                    *expression = value;
                } else if self.query.usable(fact.trust)
                    && always_nullish(fact.value)
                    && self.query.pure(left)
                    && !binds_or_suspends(&logical.left)
                {
                    self.decisions.applied(span);
                    let value = logical.right.take_in(&ctx.ast);
                    *expression = value;
                } else if fact.trust == trioxide_analysis::Trust::Declared
                    && (never_nullish(fact.value) || always_nullish(fact.value))
                {
                    self.decisions
                        .skipped(span, weak(fact.trust, "the left operand's type"));
                }
            }
            Expression::BinaryExpression(_) if !is_protected_position(ctx) => {
                if let Some(value) = self.comparison(expression, ctx) {
                    let span = expression.span();
                    self.decisions.applied(span);
                    *expression = Expression::new_boolean_literal(span, value, ctx);
                }
            }
            _ => {}
        }
    }
}

impl Visitor<'_> {
    /// Whether the value at `node` is usable-proven never nullish; logs a skip when only
    /// declared types say so.
    fn non_nullish(&mut self, node: NodeId, span: Span) -> bool {
        let fact = self.query.ty(node);
        if !never_nullish(fact.value) {
            return false;
        }
        if self.query.usable(fact.trust) {
            self.decisions.applied(span);
            true
        } else {
            self.decisions
                .skipped(span, weak(fact.trust, "the receiver's type"));
            false
        }
    }

    /// Removes the `?.` links whose receivers can't be nullish. Returns whether any is left.
    fn chain(&mut self, element: &mut ChainElement<'_>) -> bool {
        match element {
            ChainElement::CallExpression(call) => self.call(call),
            ChainElement::TSNonNullExpression(non_null) => self.part(&mut non_null.expression),
            member => self.member(member.as_member_expression_mut().expect("a member link")),
        }
    }

    fn part(&mut self, expression: &mut Expression<'_>) -> bool {
        match expression {
            Expression::CallExpression(call) => self.call(call),
            Expression::TSNonNullExpression(non_null) => self.part(&mut non_null.expression),
            other => match other.as_member_expression_mut() {
                Some(member) => self.member(member),
                None => false,
            },
        }
    }

    fn call(&mut self, call: &mut CallExpression<'_>) -> bool {
        let mut left = self.part(&mut call.callee);
        if call.optional {
            if self.non_nullish(call.callee.node_id(), call.span) {
                call.optional = false;
            } else {
                left = true;
            }
        }
        left
    }

    fn member(&mut self, member: &mut MemberExpression<'_>) -> bool {
        let span = member.span();
        let (object, optional) = match member {
            MemberExpression::StaticMemberExpression(m) => {
                let m = &mut **m;
                (&mut m.object, &mut m.optional)
            }
            MemberExpression::ComputedMemberExpression(m) => {
                let m = &mut **m;
                (&mut m.object, &mut m.optional)
            }
            MemberExpression::PrivateFieldExpression(m) => {
                let m = &mut **m;
                (&mut m.object, &mut m.optional)
            }
        };
        let object_node = object.node_id();
        let mut left = self.part(object);
        if *optional {
            if self.non_nullish(object_node, span) {
                *optional = false;
            } else {
                left = true;
            }
        }
        left
    }

    /// The value of a `typeof` or nullish comparison the types decide.
    fn comparison(
        &mut self,
        expression: &Expression<'_>,
        ctx: &TraverseCtx<'_, ()>,
    ) -> Option<bool> {
        let Expression::BinaryExpression(binary) = expression else {
            return None;
        };
        let (strict, negated) = match binary.operator {
            BinaryOperator::StrictEquality => (true, false),
            BinaryOperator::StrictInequality => (true, true),
            BinaryOperator::Equality => (false, false),
            BinaryOperator::Inequality => (false, true),
            _ => return None,
        };
        let scoping = ctx.scoping();
        let (subject, decided) = if let Some((operand, name)) =
            typeof_test(&binary.left, &binary.right)
                .or_else(|| typeof_test(&binary.right, &binary.left))
        {
            (operand, Test::Typeof(name))
        } else {
            let operand = nullish_operand(&binary.left, &binary.right, scoping)
                .or_else(|| nullish_operand(&binary.right, &binary.left, scoping))?;
            let null = is_null(&binary.left) || is_null(&binary.right);
            (operand, Test::Nullish { null, strict })
        };
        let fact = self.query.ty(subject.node_id());
        let outcome = decided.decide(fact.value)?;
        if !self.query.usable(fact.trust) {
            self.decisions
                .skipped(binary.span, weak(fact.trust, "the operand's type"));
            return None;
        }
        let pure = self.query.is_pure(binary.node_id.get());
        if !(pure.value && self.query.usable(pure.trust)) || binds_or_suspends(expression) {
            self.decisions.skipped(
                binary.span,
                if self.query.usable(pure.trust) {
                    "evaluating the operand may have effects".to_string()
                } else {
                    weak(pure.trust, "whether evaluating the operand has effects")
                },
            );
            return None;
        }
        Some(outcome != negated)
    }
}

enum Test<'s> {
    Typeof(&'s str),
    Nullish { null: bool, strict: bool },
}

impl Test<'_> {
    /// Whether `x <op> …` holds for every value of `ty` (`Some(true)`), for none
    /// (`Some(false)`), or depends on the value.
    fn decide(&self, ty: &Type) -> Option<bool> {
        match self {
            Self::Typeof(name) => {
                let names = typeof_names(ty)?;
                if !names.contains(name) {
                    Some(false)
                } else if names == [*name] {
                    Some(true)
                } else {
                    None
                }
            }
            &Self::Nullish { null, strict } => {
                if matches!(ty, Type::Unknown | Type::Never) {
                    return None;
                }
                let hits = |member: &Type| match member {
                    Type::Null => null || !strict,
                    Type::Undefined => !null || !strict,
                    _ => false,
                };
                let members = ty.members();
                if members.iter().all(hits) {
                    Some(true)
                } else if !members.iter().any(hits)
                    && members
                        .iter()
                        .all(|member| !matches!(member, Type::Unknown))
                {
                    Some(false)
                } else {
                    None
                }
            }
        }
    }
}

/// `typeof operand` compared with the string `other`.
fn typeof_test<'e, 'a>(
    typeof_side: &'e Expression<'a>,
    other: &'e Expression<'a>,
) -> Option<(&'e Expression<'a>, &'e str)> {
    let Expression::UnaryExpression(unary) = typeof_side.without_parentheses() else {
        return None;
    };
    let Expression::StringLiteral(name) = other.without_parentheses() else {
        return None;
    };
    (unary.operator == UnaryOperator::Typeof).then_some((&unary.argument, name.value.as_str()))
}

/// The operand compared with `null` or `undefined` (`other`).
fn nullish_operand<'e, 'a>(
    operand: &'e Expression<'a>,
    other: &'e Expression<'a>,
    scoping: &oxc_semantic::Scoping,
) -> Option<&'e Expression<'a>> {
    let nullish = match other.without_parentheses() {
        Expression::NullLiteral(_) => true,
        Expression::Identifier(ident) => {
            ident.name == "undefined" && is_global_constant(ident, scoping)
        }
        Expression::UnaryExpression(unary) => {
            unary.operator == UnaryOperator::Void
                && matches!(
                    unary.argument.without_parentheses(),
                    Expression::NumericLiteral(_) | Expression::StringLiteral(_)
                )
        }
        _ => false,
    };
    (nullish && !is_nullish_literal(operand, scoping)).then_some(operand)
}

fn is_null(expression: &Expression<'_>) -> bool {
    matches!(expression.without_parentheses(), Expression::NullLiteral(_))
}

fn is_nullish_literal(expression: &Expression<'_>, scoping: &oxc_semantic::Scoping) -> bool {
    match expression.without_parentheses() {
        Expression::NullLiteral(_) => true,
        Expression::Identifier(ident) => {
            ident.name == "undefined" && is_global_constant(ident, scoping)
        }
        _ => false,
    }
}
