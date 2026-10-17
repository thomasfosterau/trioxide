//! Pass 1: constant and enum inlining, across modules (PLAN §4).
//!
//! A read of a binding becomes a literal when the binding (or the export an import resolves
//! to) is a `const` with a literal value ([`Facts::symbol_constant`]), or is never reassigned
//! and its type is a usable singleton (`symbol_ty`: one number, `null` or `undefined`); the
//! read must be usable-pure, so a read in a temporal dead zone (which throws) stays. An enum
//! member read `E.A` (`enum` and `const enum`, `E` local or imported) becomes its value when
//! [`Facts::enum_member`] is usable: `Proven` when nothing can change the enum object,
//! `Declared` when only TypeScript's read-only members say so. Strings longer than
//! [`MAX_STRING_BYTES`](super::util::MAX_STRING_BYTES) stay, for readability. Declarations
//! are left for DCE.
//!
//! [`Facts::symbol_constant`]: trioxide_analysis::Facts::symbol_constant
//! [`Facts::enum_member`]: trioxide_analysis::Facts::enum_member

use oxc_ast::ast::{Expression, Program};
use oxc_span::GetSpan;
use oxc_traverse::{Traverse, TraverseCtx};
use trioxide_analysis::Constant;
use trioxide_common::SymbolRef;

use super::{
    Decisions, Position, track_position,
    util::{Query, constant_expr, parenthesize_for, small_enough, type_constant, weak},
};
use crate::{Pass, PassContext};

/// Pass 1 (`"inline"`).
pub struct Inline;

impl Pass for Inline {
    fn name(&self) -> &'static str {
        "inline"
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

    fn enter_expression(&mut self, expression: &mut Expression<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        if !self.position.live() || super::util::is_protected_position(ctx) {
            return;
        }
        let value = match expression {
            Expression::Identifier(_) => self.binding(expression, ctx),
            Expression::StaticMemberExpression(_) | Expression::ComputedMemberExpression(_) => {
                self.enum_member(expression, ctx)
            }
            _ => None,
        };
        if let Some(value) = value {
            let span = expression.span();
            if let Some(object) = reference_of(expression) {
                ctx.scoping_mut().delete_reference(object);
            }
            let literal = constant_expr(&value, span, ctx);
            *expression = parenthesize_for(literal, ctx);
            self.decisions.applied(span);
        }
    }
}

/// The reference a read (`x`, or the `E` of `E.A`) makes.
fn reference_of(expression: &Expression<'_>) -> Option<oxc_syntax::reference::ReferenceId> {
    let ident = match expression {
        Expression::Identifier(ident) => ident,
        Expression::StaticMemberExpression(member) => match &member.object {
            Expression::Identifier(ident) => ident,
            _ => return None,
        },
        Expression::ComputedMemberExpression(member) => match &member.object {
            Expression::Identifier(ident) => ident,
            _ => return None,
        },
        _ => return None,
    };
    ident.reference_id.get()
}

impl Visitor<'_> {
    /// The symbol a reference resolves to, if it is a program binding.
    fn symbol(&self, expression: &Expression<'_>, ctx: &TraverseCtx<'_, ()>) -> Option<SymbolRef> {
        let reference = reference_of(expression)?;
        let symbol = ctx.scoping().get_reference(reference).symbol_id()?;
        Some(SymbolRef {
            module: self.query.module,
            symbol,
        })
    }

    /// The value a read of a binding always has.
    fn binding(
        &mut self,
        expression: &Expression<'_>,
        ctx: &TraverseCtx<'_, ()>,
    ) -> Option<Constant> {
        let Expression::Identifier(ident) = expression else {
            return None;
        };
        let symbol = self.symbol(expression, ctx)?;
        let facts = self.query.facts;
        let value = if let Some(constant) = facts.symbol_constant(symbol).value {
            constant.clone()
        } else {
            let target = facts.resolve_import(symbol).unwrap_or(symbol);
            let fact = facts.symbol_ty(target);
            let value = type_constant(fact.value)?;
            if !self.query.usable(fact.trust) {
                self.decisions
                    .skipped(ident.span, weak(fact.trust, "the binding's single value"));
                return None;
            }
            if target == symbol && ctx.scoping().symbol_is_mutated(symbol.symbol) {
                self.decisions
                    .skipped(ident.span, "the binding is reassigned");
                return None;
            }
            value
        };
        self.checked(value, ident.node_id.get(), ident.span)
    }

    /// The value an enum member read always has.
    fn enum_member(
        &mut self,
        expression: &Expression<'_>,
        ctx: &TraverseCtx<'_, ()>,
    ) -> Option<Constant> {
        let (name, span) = match expression {
            Expression::StaticMemberExpression(member) => {
                (member.property.name.as_str(), member.span)
            }
            Expression::ComputedMemberExpression(member) => match &member.expression {
                Expression::StringLiteral(key) => (key.value.as_str(), member.span),
                _ => return None,
            },
            _ => return None,
        };
        let symbol = self.symbol(expression, ctx)?;
        let flags = ctx.scoping().symbol_flags(symbol.symbol);
        if !flags.is_enum() && !flags.is_import() {
            return None;
        }
        let fact = self.query.facts.enum_member(symbol, name);
        let constant = fact.value?;
        if !self.query.usable(fact.trust) {
            self.decisions.skipped(
                span,
                weak(
                    fact.trust,
                    "the member's value (the enum object is used as a value, so only TypeScript \
                     says its members are read-only)",
                ),
            );
            return None;
        }
        if !small_enough(constant) {
            self.decisions
                .skipped(span, "the string is too long to copy into each use");
            return None;
        }
        Some(constant.clone())
    }

    /// `value`, if it may replace the read `node`: short enough, and the read is pure.
    fn checked(
        &mut self,
        value: Constant,
        node: trioxide_common::NodeId,
        span: oxc_span::Span,
    ) -> Option<Constant> {
        if !small_enough(&value) {
            self.decisions
                .skipped(span, "the string is too long to copy into each use");
            return None;
        }
        let pure = self.query.is_pure(node);
        if !self.query.usable(pure.trust) {
            self.decisions
                .skipped(span, weak(pure.trust, "whether the read can throw"));
            return None;
        }
        if !pure.value {
            self.decisions
                .skipped(span, "the read may throw (e.g. in its temporal dead zone)");
            return None;
        }
        Some(value)
    }
}
