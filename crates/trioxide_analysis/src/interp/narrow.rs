//! Flow narrowing: what a condition being true or false says about tracked locals
//! (`typeof`, `===`, truthiness, numeric comparisons, and `i < array.length` bounds).

use oxc_ast::ast::{BinaryExpression, Expression, UnaryOperator};
use oxc_syntax::{
    operator::{BinaryOperator, LogicalOperator},
    symbol::SymbolId,
};
use trioxide_types::{NumberType, Type};

use super::{
    Cx, Env,
    expr::{narrow_falsy, narrow_truthy},
};
use crate::{facts::Trust, numeric, value::Val};

impl<'a> Cx<'_, '_, 'a> {
    /// Evaluates a condition; returns the environments where it was truthy and falsy.
    pub fn condition(&mut self, test: &'a Expression<'a>) -> (Env, Env) {
        match test {
            Expression::ParenthesizedExpression(paren) => {
                let result = self.condition(&paren.expression);
                self.record_node(paren.node_id.get(), &self.recorded(&paren.expression));
                result
            }
            Expression::LogicalExpression(logical)
                if logical.operator != LogicalOperator::Coalesce =>
            {
                let (left_true, left_false) = self.condition(&logical.left);
                let left = self.recorded(&logical.left);
                let (env, short) = if logical.operator == LogicalOperator::And {
                    (left_true, left_false)
                } else {
                    (left_false, left_true)
                };
                self.env = env;
                let (right_true, right_false) = self.condition(&logical.right);
                let right = self.recorded(&logical.right);
                let value = if logical.operator == LogicalOperator::And {
                    narrow_falsy(&left).join(&right)
                } else {
                    narrow_truthy(&left).join(&right)
                };
                self.record_node(logical.node_id.get(), &value);
                self.env = right_true.join(&right_false).join(&short);
                if logical.operator == LogicalOperator::And {
                    (right_true, short.join(&right_false))
                } else {
                    (short.join(&right_true), right_false)
                }
            }
            Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::LogicalNot => {
                let (yes, no) = self.condition(&unary.argument);
                self.record_node(unary.node_id.get(), &Val::proven(Type::Boolean));
                (no, yes)
            }
            _ => {
                let value = self.expr(test);
                self.condition_of_value(test, &value)
            }
        }
    }

    /// The value recorded for an expression this round.
    fn recorded(&self, expression: &Expression<'_>) -> Val {
        self.an
            .output
            .nodes
            .get(&self.node(expression.node_id()))
            .cloned()
            .unwrap_or_else(Val::unknown)
    }

    /// Narrows the current environment by an already evaluated condition.
    pub fn condition_of_value(&mut self, test: &'a Expression<'a>, value: &Val) -> (Env, Env) {
        let mut yes = self.env.clone();
        let mut no = self.env.clone();
        if !self.env.reachable {
            return (yes, no);
        }
        // A condition that is a proven constant leaves one branch dead.
        if value.trust == Trust::Proven {
            if always_truthy(&value.ty) {
                no.reachable = false;
            } else if always_falsy(&value.ty) {
                yes.reachable = false;
            }
        }
        self.narrow(test, &mut yes, true);
        self.narrow(test, &mut no, false);
        (yes, no)
    }

    fn narrow(&mut self, test: &'a Expression<'a>, env: &mut Env, truthy: bool) {
        match test.without_parentheses() {
            Expression::Identifier(_) => {
                if let Some(symbol) = self.tracked_identifier(test) {
                    update(env, symbol, |value| {
                        if truthy {
                            narrow_truthy(value)
                        } else {
                            narrow_falsy(value)
                        }
                    });
                }
            }
            Expression::BinaryExpression(binary) => self.narrow_binary(binary, env, truthy),
            _ => {}
        }
    }

    fn narrow_binary(&mut self, binary: &'a BinaryExpression<'a>, env: &mut Env, truthy: bool) {
        use BinaryOperator as B;
        let operator = binary.operator;
        match operator {
            B::StrictEquality | B::StrictInequality | B::Equality | B::Inequality => {
                let equal = matches!(operator, B::StrictEquality | B::Equality) == truthy;
                let loose = matches!(operator, B::Equality | B::Inequality);
                // typeof x === "…"
                for (one, other) in [(&binary.left, &binary.right), (&binary.right, &binary.left)] {
                    if let Expression::UnaryExpression(unary) = one.without_parentheses()
                        && unary.operator == UnaryOperator::Typeof
                        && let Expression::StringLiteral(literal) = other.without_parentheses()
                        && let Some(symbol) = self.tracked_in(binary.span, &unary.argument)
                    {
                        let tag = literal.value.as_str().to_string();
                        update(env, symbol, |value| narrow_typeof(value, &tag, equal));
                        return;
                    }
                }
                // x === literal
                for (one, other) in [(&binary.left, &binary.right), (&binary.right, &binary.left)] {
                    let Some(symbol) = self.tracked_in(binary.span, one) else {
                        continue;
                    };
                    let constant = self.recorded(other);
                    if constant.trust != Trust::Proven {
                        continue;
                    }
                    update(env, symbol, |value| {
                        narrow_equal(value, &constant.ty, equal, loose)
                    });
                    if let Some(other_symbol) = self.tracked_in(binary.span, other) {
                        let _ = other_symbol;
                    }
                    return;
                }
            }
            B::LessThan | B::LessEqualThan | B::GreaterThan | B::GreaterEqualThan => {
                // Normalise to `left < right` / `left <= right` holding.
                let (less, strict) = match (operator, truthy) {
                    (B::LessThan, true) | (B::GreaterEqualThan, false) => (true, true),
                    (B::LessEqualThan, true) | (B::GreaterThan, false) => (true, false),
                    (B::GreaterThan, true) | (B::LessEqualThan, false) => (false, true),
                    (B::GreaterEqualThan, true) | (B::LessThan, false) => (false, false),
                    _ => return,
                };
                // A false comparison may be false because of NaN: only narrow on false when
                // both sides are proven non-NaN numbers.
                let left = self.recorded(&binary.left);
                let right = self.recorded(&binary.right);
                if !(truthy || (non_nan_number(&left) && non_nan_number(&right))) {
                    return;
                }
                let (Some(left_number), Some(right_number)) =
                    (left.ty.as_number(), right.ty.as_number())
                else {
                    return;
                };
                if let Some(symbol) = self.tracked_in(binary.span, &binary.left) {
                    update(env, symbol, |value| {
                        let Some(number) = value.ty.as_number() else {
                            return value.clone();
                        };
                        let narrowed = if less {
                            numeric::assume_less(number, right_number, strict)
                        } else {
                            numeric::assume_greater(number, right_number, strict)
                        };
                        // Narrowing by a declared bound gives a declared range.
                        let trust = value.trust.min(if right.trust == Trust::Proven {
                            Trust::Proven
                        } else {
                            Trust::Declared
                        });
                        if right.trust == Trust::Unknown {
                            return value.clone();
                        }
                        Val {
                            ty: number_or_never(narrowed, trust),
                            trust,
                            ..value.clone()
                        }
                    });
                }
                if let Some(symbol) = self.tracked_in(binary.span, &binary.right) {
                    update(env, symbol, |value| {
                        let Some(number) = value.ty.as_number() else {
                            return value.clone();
                        };
                        let narrowed = if less {
                            numeric::assume_greater(number, left_number, strict)
                        } else {
                            numeric::assume_less(number, left_number, strict)
                        };
                        let trust = value.trust.min(if left.trust == Trust::Proven {
                            Trust::Proven
                        } else {
                            Trust::Declared
                        });
                        if left.trust == Trust::Unknown {
                            return value.clone();
                        }
                        Val {
                            ty: number_or_never(narrowed, trust),
                            trust,
                            ..value.clone()
                        }
                    });
                }
                // `i + 3 < n` narrows `i` to `< n - 3` (and `n > i + 3` too).
                if right.trust != Trust::Unknown {
                    self.narrow_offset(env, binary, &binary.left, &right, less, strict);
                }
                if left.trust != Trust::Unknown {
                    self.narrow_offset(env, binary, &binary.right, &left, !less, strict);
                }
                // `i < a.length` / `a.length > i` and `i < n` with `n = a.length`.
                if truthy && strict {
                    let (index, bound) = if less {
                        (&binary.left, &binary.right)
                    } else {
                        (&binary.right, &binary.left)
                    };
                    if let Some(index) = self.tracked_in(binary.span, index)
                        && let Some(array) = self.length_of(bound, env)
                        && !env.bounds.contains(&(index, array))
                    {
                        env.bounds.push((index, array));
                    }
                }
            }
            _ => {}
        }
    }

    /// The array whose length `expression` is: `a.length` with `a` stable, or a local that
    /// holds it.
    fn length_of(&self, expression: &Expression<'_>, env: &Env) -> Option<SymbolId> {
        match expression.without_parentheses() {
            Expression::StaticMemberExpression(member) if member.property.name == "length" => {
                self.stable_symbol(&member.object)
            }
            Expression::Identifier(_) => {
                let n = self.tracked_identifier(expression)?;
                env.lengths
                    .iter()
                    .find(|(m, _)| *m == n)
                    .map(|(_, array)| *array)
            }
            _ => None,
        }
    }

    /// The tracked local `expression` names, unless code within `span` assigns it: a
    /// comparison's operands run before the narrowing applies, so a fact about the value
    /// compared says nothing about the one assigned.
    fn tracked_in(&self, span: oxc_span::Span, expression: &Expression<'_>) -> Option<SymbolId> {
        let symbol = self.tracked_identifier(expression)?;
        let semantic = &self.module.semantic;
        let written = semantic
            .scoping()
            .get_resolved_references(symbol)
            .any(|reference| {
                reference.is_write()
                    && span.contains_inclusive(oxc_span::GetSpan::span(
                        &semantic.nodes().kind(reference.node_id()),
                    ))
            });
        (!written).then_some(symbol)
    }

    /// Narrows `x` from `x + k < bound` (`less`, else `>`; `strict`, else `<=`/`>=`) with a
    /// constant `k`, as `x < bound - k`. Exact for an integer `x` of any size when `k` and
    /// `bound` are integers within ±2^52: rounding is monotone, the integers next to `bound`
    /// are exact, so the rounded sum compares with `bound` as the exact one does, and so
    /// is `bound - k`.
    fn narrow_offset(
        &self,
        env: &mut Env,
        binary: &BinaryExpression<'_>,
        side: &Expression<'_>,
        bound: &Val,
        less: bool,
        strict: bool,
    ) {
        const EXACT: f64 = 4_503_599_627_370_496.0;
        let exact = |n: NumberType| n.int && n.range.min >= -EXACT && n.range.max <= EXACT;
        let Expression::BinaryExpression(sum) = side.without_parentheses() else {
            return;
        };
        let constant = |expression: &Expression<'_>| {
            let value = self.recorded(expression);
            let number = value.ty.as_number()?;
            (value.trust == Trust::Proven && exact(number))
                .then(|| number.as_constant())
                .flatten()
        };
        let (symbol, offset) = match sum.operator {
            BinaryOperator::Addition => match (
                self.tracked_in(binary.span, &sum.left),
                self.tracked_in(binary.span, &sum.right),
            ) {
                (Some(symbol), _) if let Some(k) = constant(&sum.right) => (symbol, k),
                (_, Some(symbol)) if let Some(k) = constant(&sum.left) => (symbol, k),
                _ => return,
            },
            BinaryOperator::Subtraction => match (
                self.tracked_in(binary.span, &sum.left),
                constant(&sum.right),
            ) {
                (Some(symbol), Some(k)) => (symbol, -k),
                _ => return,
            },
            _ => return,
        };
        let Some(bound_number) = bound.ty.as_number().filter(|&n| exact(n)) else {
            return;
        };
        let shifted = numeric::sub(bound_number, NumberType::constant(offset));
        let bound_trust = if bound.trust == Trust::Proven {
            Trust::Proven
        } else {
            Trust::Declared
        };
        update(env, symbol, |value| match value.ty.as_number() {
            Some(number) if number.int => {
                let narrowed = if less {
                    numeric::assume_less(number, shifted, strict)
                } else {
                    numeric::assume_greater(number, shifted, strict)
                };
                let trust = value.trust.min(bound_trust);
                Val {
                    ty: number_or_never(narrowed, trust),
                    trust,
                    ..value.clone()
                }
            }
            _ => value.clone(),
        });
    }

    /// The tracked local `expression` names.
    pub fn tracked_identifier(&self, expression: &Expression<'_>) -> Option<SymbolId> {
        let Expression::Identifier(ident) = expression.without_parentheses() else {
            return None;
        };
        let scoping = self.module.semantic.scoping();
        let symbol = scoping
            .get_reference(ident.reference_id.get()?)
            .symbol_id()?;
        self.is_tracked_here(symbol).then_some(symbol)
    }
}

/// A narrowed number, or `never` when no number is left in its range (a proven
/// contradiction: the branch can't run, and nothing computed from the value there counts).
fn number_or_never(number: NumberType, trust: Trust) -> Type {
    if trust == Trust::Proven && number.range.min > number.range.max {
        Type::Never
    } else {
        Type::Number(number)
    }
}

fn update(env: &mut Env, symbol: SymbolId, narrow: impl FnOnce(&Val) -> Val) {
    if let Some(value) = env.vars.get(symbol) {
        let narrowed = narrow(value);
        // A proven contradiction: this branch can't run.
        if narrowed.ty == Type::Never && narrowed.trust == Trust::Proven && value.ty != Type::Never
        {
            env.reachable = false;
        }
        env.vars.insert(symbol, narrowed);
    }
}

fn non_nan_number(value: &Val) -> bool {
    value.trust >= Trust::Declared && value.ty.as_number().is_some_and(|number| number.non_nan)
}

/// `typeof value === tag` held (`equal`) or failed.
fn narrow_typeof(value: &Val, tag: &str, equal: bool) -> Val {
    let category: Option<Type> = match tag {
        "number" => Some(Type::NUMBER),
        "string" => Some(Type::String),
        "boolean" => Some(Type::Boolean),
        "bigint" => Some(Type::BigInt),
        "symbol" => Some(Type::Symbol),
        "undefined" => Some(Type::Undefined),
        _ => None,
    };
    let matches = |member: &Type| -> Option<bool> {
        if *member == Type::Unknown {
            // Anything: the test can go either way.
            return None;
        }
        Some(match tag {
            "number" => matches!(member, Type::Number(_)),
            "string" => matches!(member, Type::String),
            "boolean" => matches!(member, Type::Boolean),
            "bigint" => matches!(member, Type::BigInt),
            "symbol" => matches!(member, Type::Symbol),
            "undefined" => matches!(member, Type::Undefined),
            "function" => matches!(member, Type::Function(..)),
            "object" => matches!(
                member,
                Type::Null
                    | Type::Object(_)
                    | Type::Array(_)
                    | Type::Tuple(_)
                    | Type::TypedArray(_)
                    | Type::Instance(_)
            ),
            _ => return None,
        })
    };
    if equal {
        let candidates = Type::union(
            value
                .ty
                .members()
                .iter()
                .filter(|member| matches(member) != Some(false))
                .cloned(),
        );
        if value.trust == Trust::Proven {
            // Proven: keep what is known within the category (and `never` if none matched).
            return Val {
                ty: candidates,
                ..value.clone()
            };
        }
        // The check itself proves a primitive category, whatever TypeScript said.
        match category {
            Some(ty) => Val::proven(ty),
            None => Val {
                ty: candidates,
                ..value.clone()
            },
        }
    } else {
        if value.trust == Trust::Unknown {
            return value.clone();
        }
        Val {
            ty: Type::union(
                value
                    .ty
                    .members()
                    .iter()
                    .filter(|member| matches(member) != Some(true))
                    .cloned(),
            ),
            ..value.clone()
        }
    }
}

/// `value === constant` held (`equal`) or failed; `loose` for `==`.
fn narrow_equal(value: &Val, constant: &Type, equal: bool, loose: bool) -> Val {
    let nullish = matches!(constant, Type::Null | Type::Undefined);
    if loose && nullish {
        // `x == null` holds exactly for null and undefined.
        return if equal {
            if value.trust == Trust::Unknown {
                Val::proven(Type::union([Type::Null, Type::Undefined]))
            } else {
                Val {
                    ty: Type::union(
                        value
                            .ty
                            .members()
                            .iter()
                            .filter(|m| matches!(m, Type::Null | Type::Undefined | Type::Unknown))
                            .cloned(),
                    ),
                    ..value.clone()
                }
            }
        } else if value.trust == Trust::Unknown {
            value.clone()
        } else {
            Val {
                ty: value.ty.non_nullish(),
                ..value.clone()
            }
        };
    }
    if loose {
        return value.clone();
    }
    if equal {
        // Strict equality with a primitive constant: the value is that primitive. A number
        // constant `c` also equals `-c` only for zero, which `constant` already allows.
        match constant {
            Type::Number(number) if number.as_constant().is_some() => {
                let mut number: NumberType = *number;
                if number.as_constant() == Some(0.0) {
                    number.non_neg_zero = false;
                }
                return Val::number(number);
            }
            Type::Null | Type::Undefined | Type::Boolean | Type::String => {
                return Val::proven(constant.clone());
            }
            _ => {}
        }
        return value.clone();
    }
    if value.trust == Trust::Unknown {
        return value.clone();
    }
    match constant {
        Type::Null | Type::Undefined => Val {
            ty: Type::union(
                value
                    .ty
                    .members()
                    .iter()
                    .filter(|member| *member != constant)
                    .cloned(),
            ),
            ..value.clone()
        },
        _ => value.clone(),
    }
}

fn always_truthy(ty: &Type) -> bool {
    ty.members().iter().all(|member| match member {
        Type::Object(_)
        | Type::Array(_)
        | Type::Tuple(_)
        | Type::TypedArray(_)
        | Type::Instance(_)
        | Type::Function(..)
        | Type::Symbol => true,
        Type::Number(number) => {
            number.non_nan && (number.range.min > 0.0 || number.range.max < 0.0)
        }
        _ => false,
    }) && *ty != Type::Never
}

fn always_falsy(ty: &Type) -> bool {
    ty.members().iter().all(|member| match member {
        Type::Null | Type::Undefined => true,
        Type::Number(number) => {
            number.range.min == 0.0 && number.range.max == 0.0 && number.non_nan
        }
        _ => false,
    }) && *ty != Type::Never
}
