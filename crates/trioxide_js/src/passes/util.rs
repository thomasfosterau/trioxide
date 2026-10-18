//! What the passes share: fact queries that never answer for synthesised nodes, literals,
//! syntactic purity and reference counting.

use oxc_allocator::{Allocator, GetAllocator};
use oxc_ast::{
    ast::{
        BinaryOperator, Expression, IdentifierReference, NumberBase, Program, Statement,
        UnaryOperator,
    },
    builder::{AstBuilder, GetAstBuilder},
};
use oxc_ast_visit::{Visit, walk};
use oxc_ecmascript::{
    ConstantValue, GlobalContext,
    constant_evaluation::ConstantEvaluationCtx,
    side_effects::{MayHaveSideEffectsContext, PropertyReadSideEffects},
};
use oxc_semantic::Scoping;
use oxc_span::{GetSpan, Span};
use oxc_syntax::{
    reference::ReferenceFlags,
    scope::{ScopeFlags, ScopeId},
    symbol::SymbolId,
};
use oxc_traverse::{Ancestor, TraverseCtx};
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_analysis::{Constant, Fact, Facts, Trust};
use trioxide_common::{ModuleId, NodeId, NodeRef};
use trioxide_types::Type;

/// Strings longer than this (in bytes) are not copied into each use, for readability.
pub const MAX_STRING_BYTES: usize = 32;

/// Fact queries for one module. Nodes a pass created have no facts ([`NodeId::DUMMY`], which
/// is also the program's id), so they answer `Unknown`.
#[derive(Clone, Copy)]
pub struct Query<'f> {
    pub facts: &'f Facts,
    pub module: ModuleId,
}

impl<'f> Query<'f> {
    pub fn node(&self, id: NodeId) -> Option<NodeRef> {
        (id != NodeId::DUMMY).then_some(NodeRef {
            module: self.module,
            node: id,
        })
    }

    pub fn usable(&self, trust: Trust) -> bool {
        self.facts.is_usable(trust)
    }

    pub fn ty(&self, id: NodeId) -> Fact<&'f Type> {
        match self.node(id) {
            Some(node) => self.facts.ty(node),
            None => Fact {
                value: &Type::Unknown,
                trust: Trust::Unknown,
            },
        }
    }

    /// The type, if it is usable as proof.
    pub fn usable_ty(&self, id: NodeId) -> Option<&'f Type> {
        let fact = self.ty(id);
        (self.usable(fact.trust) && !matches!(fact.value, Type::Unknown | Type::Never))
            .then_some(fact.value)
    }

    pub fn is_pure(&self, id: NodeId) -> Fact<bool> {
        match self.node(id) {
            Some(node) => self.facts.is_pure(node),
            None => Fact {
                value: false,
                trust: Trust::Unknown,
            },
        }
    }

    /// Whether `id` is usable-pure.
    pub fn pure(&self, id: NodeId) -> bool {
        let fact = self.is_pure(id);
        fact.value && self.usable(fact.trust)
    }

    /// Whether `id` is proven never to run.
    pub fn dead(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|node| {
            let fact = self.facts.reachable(node);
            !fact.value && fact.trust == Trust::Proven
        })
    }
}

/// Why a fact didn't justify a rewrite, for the decision log.
pub fn weak(trust: Trust, what: &str) -> String {
    match trust {
        Trust::Declared => format!("{what} is only Declared (trustTypes is off)"),
        _ => format!("{what} is unknown"),
    }
}

/// The single value every member of `ty` allows, if there is one.
pub fn type_constant(ty: &Type) -> Option<Constant> {
    match ty {
        // A zero is `-0` only when `non_neg_zero` is off, and then it isn't a single value;
        // so a zero constant is `+0` whatever sign its interval bound has.
        Type::Number(number) => number
            .as_constant()
            .map(|value| Constant::Number(if value == 0.0 { 0.0 } else { value })),
        Type::Null => Some(Constant::Null),
        Type::Undefined => Some(Constant::Undefined),
        _ => None,
    }
}

/// Whether every value of `ty` is truthy (`Some(true)`) or falsy (`Some(false)`).
pub fn truthiness(ty: &Type) -> Option<bool> {
    let mut answer = None;
    for member in ty.members() {
        let this = match member {
            Type::Null | Type::Undefined => false,
            Type::Number(number) => {
                if number.non_nan && number.range.as_point() == Some(0.0) {
                    false
                } else if number.non_nan && (number.range.min > 0.0 || number.range.max < 0.0) {
                    true
                } else {
                    return None;
                }
            }
            Type::Array(_)
            | Type::TypedArray(_)
            | Type::Tuple(_)
            | Type::Object(_)
            | Type::Instance(_)
            | Type::Function(..)
            | Type::Symbol => true,
            _ => return None,
        };
        match answer {
            None => answer = Some(this),
            Some(previous) if previous == this => {}
            Some(_) => return None,
        }
    }
    answer
}

/// What `typeof` may answer for a value of `ty`, if every member is known.
pub fn typeof_names(ty: &Type) -> Option<Vec<&'static str>> {
    let mut names = Vec::new();
    for member in ty.members() {
        let name = match member {
            Type::Number(_) => "number",
            Type::Boolean => "boolean",
            Type::String => "string",
            Type::BigInt => "bigint",
            Type::Symbol => "symbol",
            Type::Undefined => "undefined",
            Type::Function(..) => "function",
            Type::Null
            | Type::Array(_)
            | Type::TypedArray(_)
            | Type::Tuple(_)
            | Type::Object(_)
            | Type::Instance(_) => "object",
            _ => return None,
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    (!names.is_empty()).then_some(names)
}

/// Whether `ty` is usable proof that a value is never `null` or `undefined`.
pub fn never_nullish(ty: &Type) -> bool {
    !matches!(ty, Type::Unknown | Type::Never) && !ty.may_be_nullish()
}

/// Whether every value of `ty` is `null` or `undefined`.
pub fn always_nullish(ty: &Type) -> bool {
    !matches!(ty, Type::Never)
        && ty
            .members()
            .iter()
            .all(|member| matches!(member, Type::Null | Type::Undefined))
}

/// Whether `name` resolves to the global (no binding shadows it) from `scope`.
pub fn is_global_name(scoping: &Scoping, scope: ScopeId, name: &str) -> bool {
    scoping.find_binding(scope, name.into()).is_none()
}

/// The literal for `constant`: negative numbers as unary minus over a literal, and `NaN`,
/// `Infinity` and `undefined` by name unless shadowed (then `0 / 0`, `1 / 0`, `void 0`).
pub fn constant_expr<'a, S>(
    constant: &Constant,
    span: Span,
    ctx: &mut TraverseCtx<'a, S>,
) -> Expression<'a> {
    let scope = ctx.current_scope_id();
    let global = |ctx: &TraverseCtx<'a, S>, name| is_global_name(ctx.scoping(), scope, name);
    match constant {
        Constant::Number(value) => {
            let value = *value;
            let number = |value: f64, ctx: &TraverseCtx<'a, S>| {
                Expression::new_numeric_literal(span, value, None, NumberBase::Decimal, ctx)
            };
            if value.is_nan() {
                return if global(ctx, "NaN") {
                    ctx.create_unbound_ident_expr(span, "NaN".into(), ReferenceFlags::Read)
                } else {
                    let (zero, other) = (number(0.0, ctx), number(0.0, ctx));
                    Expression::new_binary_expression(
                        span,
                        zero,
                        BinaryOperator::Division,
                        other,
                        ctx,
                    )
                };
            }
            let magnitude = if !value.is_infinite() {
                number(value.abs(), ctx)
            } else if global(ctx, "Infinity") {
                ctx.create_unbound_ident_expr(span, "Infinity".into(), ReferenceFlags::Read)
            } else {
                let (one, zero) = (number(1.0, ctx), number(0.0, ctx));
                Expression::new_binary_expression(span, one, BinaryOperator::Division, zero, ctx)
            };
            if !value.is_sign_negative() {
                return magnitude;
            }
            let operand = if matches!(magnitude, Expression::BinaryExpression(_)) {
                Expression::new_parenthesized_expression(span, magnitude, ctx)
            } else {
                magnitude
            };
            Expression::new_unary_expression(span, UnaryOperator::UnaryNegation, operand, ctx)
        }
        Constant::String(value) => Expression::new_string_literal(
            span,
            oxc_ast::ast::Str::from_str_in(value, ctx),
            None,
            ctx,
        ),
        Constant::Boolean(value) => Expression::new_boolean_literal(span, *value, ctx),
        Constant::BigInt(digits) => Expression::new_big_int_literal(
            span,
            oxc_ast::ast::Str::from_str_in(digits, ctx),
            None,
            oxc_syntax::number::BigintBase::Decimal,
            ctx,
        ),
        Constant::Null => Expression::new_null_literal(span, ctx),
        Constant::Undefined => {
            if global(ctx, "undefined") {
                ctx.create_unbound_ident_expr(span, "undefined".into(), ReferenceFlags::Read)
            } else {
                Expression::new_unary_expression(
                    span,
                    UnaryOperator::Void,
                    Expression::new_numeric_literal(span, 0.0, None, NumberBase::Decimal, ctx),
                    ctx,
                )
            }
        }
    }
}

/// Whether `constant` may be copied into a use: short enough, and (for strings) printable as
/// written. Strings with invisible formatting characters (a BOM, zero-width or bidi marks)
/// would read wrong, and U+FFFD is how oxc encodes lone surrogates, which would be corrupted.
pub fn small_enough(constant: &Constant) -> bool {
    let invisible = |c: char| {
        matches!(
            c,
            '\u{00AD}'
                | '\u{061C}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2069}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFD}'
        )
    };
    match constant {
        Constant::String(value) => value.len() <= MAX_STRING_BYTES && !value.chars().any(invisible),
        Constant::BigInt(digits) => digits.len() <= MAX_STRING_BYTES,
        _ => true,
    }
}

/// `expression` in parentheses if it isn't a primary, member or call expression and its parent
/// binds tighter than a conditional (an operator, member object, callee or tag).
pub fn parenthesize_for<'a, S>(
    expression: Expression<'a>,
    ctx: &TraverseCtx<'a, S>,
) -> Expression<'a> {
    let compound = !matches!(
        expression,
        Expression::NumericLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::BigIntLiteral(_)
            | Expression::RegExpLiteral(_)
            | Expression::TemplateLiteral(_)
            | Expression::Identifier(_)
            | Expression::ThisExpression(_)
            | Expression::ParenthesizedExpression(_)
            | Expression::ArrayExpression(_)
            | Expression::ObjectExpression(_)
            | Expression::CallExpression(_)
            | Expression::StaticMemberExpression(_)
            | Expression::ComputedMemberExpression(_)
            | Expression::PrivateFieldExpression(_)
    );
    let tight = matches!(
        ctx.parent(),
        Ancestor::BinaryExpressionLeft(_)
            | Ancestor::BinaryExpressionRight(_)
            | Ancestor::UnaryExpressionArgument(_)
            | Ancestor::StaticMemberExpressionObject(_)
            | Ancestor::ComputedMemberExpressionObject(_)
            | Ancestor::PrivateFieldExpressionObject(_)
            | Ancestor::CallExpressionCallee(_)
            | Ancestor::NewExpressionCallee(_)
            | Ancestor::TaggedTemplateExpressionTag(_)
            | Ancestor::AwaitExpressionArgument(_)
            | Ancestor::LogicalExpressionLeft(_)
            | Ancestor::LogicalExpressionRight(_)
    );
    // `new (f())()` is not `new f()()`.
    let call_under_new = matches!(expression, Expression::CallExpression(_))
        && matches!(ctx.parent(), Ancestor::NewExpressionCallee(_));
    if (compound && tight) || call_under_new {
        let span = expression.span();
        Expression::new_parenthesized_expression(span, expression, ctx)
    } else {
        expression
    }
}

/// What the expression being visited is to its context, looking through parentheses and
/// TypeScript-only wrappers (`as`, `!`, `satisfies`, `<T>x`), which change nothing at run time.
/// Every pass decides where it may rewrite through this (see [`Slot::admits`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// An ordinary value.
    Value,
    /// A callee or template tag: what it is decides `this`, and an `eval` callee makes a
    /// direct eval.
    Callee,
    /// The operand of `typeof`: an identifier there doesn't throw when undeclared.
    Typeof,
    /// The operand of `delete`, which acts on the reference, not the value.
    Delete,
    /// A whole expression statement: DCE removes those, and a literal there could print as a
    /// directive.
    Statement,
}

impl Slot {
    /// The slot of the expression `ctx` is visiting.
    pub fn of<S>(ctx: &TraverseCtx<'_, S>) -> Self {
        for ancestor in ctx.ancestors() {
            return match ancestor {
                Ancestor::ParenthesizedExpressionExpression(_)
                | Ancestor::TSAsExpressionExpression(_)
                | Ancestor::TSSatisfiesExpressionExpression(_)
                | Ancestor::TSNonNullExpressionExpression(_)
                | Ancestor::TSTypeAssertionExpression(_)
                | Ancestor::TSInstantiationExpressionExpression(_) => continue,
                Ancestor::CallExpressionCallee(_)
                | Ancestor::NewExpressionCallee(_)
                | Ancestor::TaggedTemplateExpressionTag(_) => Self::Callee,
                Ancestor::UnaryExpressionArgument(unary) => match *unary.operator() {
                    UnaryOperator::Delete => Self::Delete,
                    UnaryOperator::Typeof => Self::Typeof,
                    _ => Self::Value,
                },
                Ancestor::ExpressionStatementExpression(_) => Self::Statement,
                _ => Self::Value,
            };
        }
        Self::Value
    }

    /// Whether `replacement` may take the place of the visited expression. A `value` fold
    /// (the expression becomes the literal it evaluates to) is never done in a callee, where
    /// it would change the error a call of a non-function throws. A replacement that is an
    /// identifier never goes under `typeof`, where an undeclared one would stop throwing a
    /// temporal-dead-zone or reference error the original threw.
    pub fn admits(self, replacement: &Expression<'_>, value: bool) -> bool {
        match self {
            Self::Value => true,
            Self::Callee => !value,
            Self::Typeof => !matches!(replacement.without_parentheses(), Expression::Identifier(_)),
            Self::Delete | Self::Statement => false,
        }
    }

    /// `replacement`, ready to take the visited expression's place: in a callee, a member
    /// expression or `eval` becomes `(0, x)` so it keeps the original's `this` (undefined)
    /// and stays an indirect eval; elsewhere it is parenthesised where precedence needs it.
    pub fn wrap<'a, S>(
        self,
        replacement: Expression<'a>,
        ctx: &TraverseCtx<'a, S>,
    ) -> Expression<'a> {
        let rebinds = match replacement.without_parentheses() {
            Expression::StaticMemberExpression(_)
            | Expression::ComputedMemberExpression(_)
            | Expression::PrivateFieldExpression(_)
            | Expression::ChainExpression(_) => true,
            Expression::Identifier(ident) => ident.name == "eval",
            _ => false,
        };
        if self == Self::Callee && rebinds {
            let span = replacement.span();
            let zero = Expression::new_numeric_literal(span, 0.0, None, NumberBase::Decimal, ctx);
            let sequence = Expression::new_sequence_expression(
                span,
                oxc_allocator::ArenaVec::from_iter_in([zero, replacement], &ctx.ast),
                ctx,
            );
            return Expression::new_parenthesized_expression(span, sequence, ctx);
        }
        parenthesize_for(replacement, ctx)
    }
}

/// Whether the expression is a literal in the form [`constant_expr`] prints, so folding it
/// again would change nothing.
#[expect(clippy::float_cmp, reason = "exact literal values")]
pub fn is_canonical_literal(expression: &Expression<'_>) -> bool {
    match expression.without_parentheses() {
        Expression::NumericLiteral(literal) => !literal.value.is_sign_negative(),
        Expression::StringLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_) => true,
        Expression::TemplateLiteral(template) => template.expressions.is_empty(),
        Expression::Identifier(ident) => {
            matches!(ident.name.as_str(), "undefined" | "NaN" | "Infinity")
        }
        Expression::UnaryExpression(unary) => match unary.operator {
            UnaryOperator::UnaryNegation => match unary.argument.without_parentheses() {
                Expression::NumericLiteral(_) => true,
                Expression::Identifier(ident) => ident.name == "Infinity",
                other @ Expression::BinaryExpression(_) => is_canonical_literal(other),
                _ => false,
            },
            UnaryOperator::Void => {
                matches!(&unary.argument, Expression::NumericLiteral(literal) if literal.value == 0.0)
            }
            _ => false,
        },
        Expression::BinaryExpression(binary) => {
            binary.operator == BinaryOperator::Division
                && matches!(
                    (&binary.left, &binary.right),
                    (Expression::NumericLiteral(left), Expression::NumericLiteral(right))
                        if (left.value == 0.0 || left.value == 1.0) && right.value == 0.0
                )
        }
        _ => false,
    }
}

/// Converts an `oxc_ecmascript` constant.
pub fn from_ecmascript(value: &ConstantValue<'_>) -> Constant {
    match value {
        ConstantValue::Number(value) => Constant::Number(*value),
        ConstantValue::BigInt(value) => Constant::BigInt(value.to_string()),
        ConstantValue::String(value) => Constant::String(value.to_string()),
        ConstantValue::Boolean(value) => Constant::Boolean(*value),
        ConstantValue::Undefined => Constant::Undefined,
        ConstantValue::Null => Constant::Null,
    }
}

/// Whether evaluating `expression` provably does nothing and can't throw, from syntax alone:
/// literals, global `undefined`/`NaN`/`Infinity`, function creation, and operators over those
/// that can't throw or run user code (no bigints, `in` or `instanceof`; operands are primitives).
pub fn is_inert(expression: &Expression<'_>, scoping: &Scoping) -> bool {
    match expression {
        Expression::NumericLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::FunctionExpression(_)
        | Expression::ArrowFunctionExpression(_) => true,
        Expression::TemplateLiteral(template) => template.expressions.is_empty(),
        Expression::Identifier(ident) => is_global_constant(ident, scoping),
        Expression::ParenthesizedExpression(paren) => is_inert(&paren.expression, scoping),
        Expression::UnaryExpression(unary) => {
            let primitive = is_primitive_inert(&unary.argument, scoping);
            match unary.operator {
                UnaryOperator::LogicalNot | UnaryOperator::Void | UnaryOperator::Typeof => {
                    is_inert(&unary.argument, scoping)
                }
                UnaryOperator::UnaryNegation | UnaryOperator::BitwiseNot => primitive,
                // `+1n` throws.
                UnaryOperator::UnaryPlus => primitive && !may_be_bigint(&unary.argument),
                UnaryOperator::Delete => false,
            }
        }
        Expression::BinaryExpression(binary) => {
            !matches!(
                binary.operator,
                BinaryOperator::In | BinaryOperator::Instanceof
            ) && is_number_like_inert(&binary.left, scoping)
                && is_number_like_inert(&binary.right, scoping)
        }
        Expression::LogicalExpression(logical) => {
            is_inert(&logical.left, scoping) && is_inert(&logical.right, scoping)
        }
        Expression::ConditionalExpression(conditional) => {
            is_inert(&conditional.test, scoping)
                && is_inert(&conditional.consequent, scoping)
                && is_inert(&conditional.alternate, scoping)
        }
        Expression::SequenceExpression(sequence) => sequence
            .expressions
            .iter()
            .all(|expression| is_inert(expression, scoping)),
        _ => false,
    }
}

/// Inert and a primitive.
fn is_primitive_inert(expression: &Expression<'_>, scoping: &Scoping) -> bool {
    !matches!(
        expression.without_parentheses(),
        Expression::FunctionExpression(_) | Expression::ArrowFunctionExpression(_)
    ) && is_inert(expression, scoping)
}

/// Inert, a primitive and not a bigint (so arithmetic on it can't throw).
fn is_number_like_inert(expression: &Expression<'_>, scoping: &Scoping) -> bool {
    !may_be_bigint(expression) && is_primitive_inert(expression, scoping)
}

/// Whether the value of an expression over literals may be a bigint.
fn may_be_bigint(expression: &Expression<'_>) -> bool {
    match expression {
        Expression::BigIntLiteral(_) => true,
        Expression::ParenthesizedExpression(paren) => may_be_bigint(&paren.expression),
        Expression::UnaryExpression(unary) => match unary.operator {
            UnaryOperator::UnaryNegation | UnaryOperator::BitwiseNot => {
                may_be_bigint(&unary.argument)
            }
            _ => false,
        },
        Expression::BinaryExpression(binary) => {
            !binary.operator.is_equality()
                && !binary.operator.is_compare()
                && !matches!(
                    binary.operator,
                    BinaryOperator::In | BinaryOperator::Instanceof
                )
                && (may_be_bigint(&binary.left) || may_be_bigint(&binary.right))
        }
        Expression::LogicalExpression(logical) => {
            may_be_bigint(&logical.left) || may_be_bigint(&logical.right)
        }
        Expression::ConditionalExpression(conditional) => {
            may_be_bigint(&conditional.consequent) || may_be_bigint(&conditional.alternate)
        }
        Expression::SequenceExpression(sequence) => {
            sequence.expressions.last().is_some_and(may_be_bigint)
        }
        _ => false,
    }
}

/// `undefined`, `NaN` or `Infinity`, unshadowed.
pub fn is_global_constant(ident: &IdentifierReference<'_>, scoping: &Scoping) -> bool {
    matches!(ident.name.as_str(), "undefined" | "NaN" | "Infinity")
        && ident
            .reference_id
            .get()
            .is_some_and(|reference| scoping.get_reference(reference).symbol_id().is_none())
}

/// Where each symbol is still referenced in `program`: the spans of its live identifier
/// references. A scoping kept up to date only by `oxc_traverse` still lists references in
/// subtrees a pass removed; this walk doesn't.
pub fn live_references(program: &Program<'_>, scoping: &Scoping) -> FxHashMap<SymbolId, Vec<Span>> {
    struct Collect<'s> {
        scoping: &'s Scoping,
        found: FxHashMap<SymbolId, Vec<Span>>,
    }
    impl<'a> Visit<'a> for Collect<'_> {
        fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
            if let Some(reference) = ident.reference_id.get()
                && let Some(symbol) = self.scoping.get_reference(reference).symbol_id()
            {
                self.found.entry(symbol).or_default().push(ident.span);
            }
        }
    }
    let mut collect = Collect {
        scoping,
        found: FxHashMap::default(),
    };
    collect.visit_program(program);
    collect.found
}

/// Whether `expression` calls `super(…)` (which binds `this`), `await`s or `yield`s (which
/// suspend, so other code runs first), or `delete`s (which always acts on its object, optional
/// chain or not): effects purity facts don't record, or must not be trusted to record, so such
/// an expression must not be dropped or replaced however pure it reads.
pub fn binds_or_suspends(expression: &Expression<'_>) -> bool {
    struct Find {
        found: bool,
    }
    impl<'a> Visit<'a> for Find {
        fn visit_call_expression(&mut self, call: &oxc_ast::ast::CallExpression<'a>) {
            if matches!(call.callee, Expression::Super(_)) {
                self.found = true;
            }
            walk::walk_call_expression(self, call);
        }

        fn visit_await_expression(&mut self, _: &oxc_ast::ast::AwaitExpression<'a>) {
            self.found = true;
        }

        fn visit_unary_expression(&mut self, unary: &oxc_ast::ast::UnaryExpression<'a>) {
            if unary.operator == UnaryOperator::Delete {
                self.found = true;
            }
            walk::walk_unary_expression(self, unary);
        }

        fn visit_yield_expression(&mut self, _: &oxc_ast::ast::YieldExpression<'a>) {
            self.found = true;
        }

        fn visit_function(&mut self, _: &oxc_ast::ast::Function<'a>, _: ScopeFlags) {}

        fn visit_arrow_function_expression(
            &mut self,
            _: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
        }

        fn visit_class(&mut self, _: &oxc_ast::ast::Class<'a>) {}
    }
    let mut find = Find { found: false };
    find.visit_expression(expression);
    find.found
}

/// Whether the program has a direct `eval` call or a `with` statement, through which code may
/// read any binding by name.
pub fn has_dynamic_scope(program: &Program<'_>, scoping: &Scoping) -> bool {
    struct Find<'s> {
        scoping: &'s Scoping,
        found: bool,
    }
    impl<'a> Visit<'a> for Find<'_> {
        fn visit_call_expression(&mut self, call: &oxc_ast::ast::CallExpression<'a>) {
            if let Expression::Identifier(ident) = &call.callee
                && ident.name == "eval"
                && !ident.reference_id.get().is_some_and(|reference| {
                    self.scoping.get_reference(reference).symbol_id().is_some()
                })
            {
                self.found = true;
            }
            walk::walk_call_expression(self, call);
        }

        fn visit_with_statement(&mut self, statement: &oxc_ast::ast::WithStatement<'a>) {
            self.found = true;
            walk::walk_with_statement(self, statement);
        }
    }
    let mut find = Find {
        scoping,
        found: false,
    };
    find.visit_program(program);
    find.found
}

/// Whether removing `statement` would remove a binding visible outside it: a `var`
/// (hoisted to the function) anywhere in it outside nested functions, a function declaration
/// directly in it, or an import or export.
pub fn declares_hoisted(statement: &Statement<'_>) -> bool {
    struct Find {
        found: bool,
    }
    impl<'a> Visit<'a> for Find {
        fn visit_variable_declaration(
            &mut self,
            declaration: &oxc_ast::ast::VariableDeclaration<'a>,
        ) {
            if declaration.kind == oxc_ast::ast::VariableDeclarationKind::Var {
                self.found = true;
            }
            walk::walk_variable_declaration(self, declaration);
        }

        fn visit_function(&mut self, _: &oxc_ast::ast::Function<'a>, _: ScopeFlags) {}

        fn visit_arrow_function_expression(
            &mut self,
            _: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
        }

        fn visit_class(&mut self, class: &oxc_ast::ast::Class<'a>) {
            // Class bodies are strict code; a `var` in a static block is its own.
            let _ = class;
        }
    }
    // Module declarations take effect wherever they are written: imports are linked and run
    // first, and exports (an exported function is hoisted too) are the module's interface.
    if matches!(
        statement,
        Statement::FunctionDeclaration(_)
            | Statement::ImportDeclaration(_)
            | Statement::ExportDeclaration(_)
            | Statement::ExportNamedDeclaration(_)
            | Statement::ExportFromDeclaration(_)
            | Statement::ExportDefaultDeclaration(_)
            | Statement::ExportAllDeclaration(_)
            | Statement::TSExportAssignment(_)
    ) {
        return true;
    }
    let mut find = Find { found: false };
    find.visit_statement(statement);
    find.found
}

/// The context `oxc_ecmascript` evaluates and checks side effects in.
pub struct EvalCtx<'a, 's> {
    pub ast: &'s AstBuilder<'a>,
    pub scoping: &'s Scoping,
}

impl<'a> GlobalContext<'a> for EvalCtx<'a, '_> {
    fn is_global_reference(&self, reference: &IdentifierReference<'a>) -> bool {
        reference
            .reference_id
            .get()
            .is_some_and(|id| self.scoping.get_reference(id).symbol_id().is_none())
    }
}

impl<'a> MayHaveSideEffectsContext<'a> for EvalCtx<'a, '_> {
    fn annotations(&self) -> bool {
        false
    }

    fn manual_pure_functions(&self, _callee: &Expression) -> bool {
        false
    }

    fn property_read_side_effects(&self) -> PropertyReadSideEffects {
        PropertyReadSideEffects::All
    }

    fn unknown_global_side_effects(&self) -> bool {
        true
    }
}

impl<'a> GetAstBuilder<'a> for EvalCtx<'a, '_> {
    type Builder = AstBuilder<'a>;

    fn builder(&self) -> &AstBuilder<'a> {
        self.ast
    }
}

impl<'a> GetAllocator<'a> for EvalCtx<'a, '_> {
    fn allocator(&self) -> &'a Allocator {
        self.ast.allocator()
    }
}

impl<'a> ConstantEvaluationCtx<'a> for EvalCtx<'a, '_> {}

/// The bindings whose values may come from a function's callers: parameters, and locals
/// assigned (transitively) from expressions that read them or `this`. A fact about an
/// expression reading one of these may hold only because every caller passes the same values,
/// so folding it specialises the function to its callers (`passes.specialize`).
pub fn caller_dependent(program: &Program<'_>, scoping: &Scoping) -> FxHashSet<SymbolId> {
    let mut tainted: FxHashSet<SymbolId> = FxHashSet::default();
    let mut flows: Vec<(Vec<SymbolId>, Vec<SymbolId>, bool)> = Vec::new();
    {
        struct Collect<'s> {
            scoping: &'s Scoping,
            parameters: Vec<SymbolId>,
            flows: Vec<(Vec<SymbolId>, Vec<SymbolId>, bool)>,
        }
        impl<'a> Visit<'a> for Collect<'_> {
            fn visit_formal_parameter(&mut self, parameter: &oxc_ast::ast::FormalParameter<'a>) {
                parameter.pattern.all_binding_identifiers(&mut |ident| {
                    self.parameters.push(ident.symbol_id());
                    true
                });
                walk::walk_formal_parameter(self, parameter);
            }

            fn visit_variable_declarator(
                &mut self,
                declarator: &oxc_ast::ast::VariableDeclarator<'a>,
            ) {
                if let Some(init) = &declarator.init {
                    let mut targets = Vec::new();
                    declarator.id.all_binding_identifiers(&mut |ident| {
                        targets.push(ident.symbol_id());
                        true
                    });
                    let (reads, this) = reads_of(init, self.scoping);
                    self.flows.push((targets, reads, this));
                }
                walk::walk_variable_declarator(self, declarator);
            }

            fn visit_assignment_expression(
                &mut self,
                assignment: &oxc_ast::ast::AssignmentExpression<'a>,
            ) {
                let mut targets = Vec::new();
                collect_targets(&assignment.left, self.scoping, &mut targets);
                if !targets.is_empty() {
                    let (mut reads, this) = reads_of(&assignment.right, self.scoping);
                    // `x += y` keeps x's old value in the mix.
                    if assignment.operator != oxc_ast::ast::AssignmentOperator::Assign {
                        reads.extend(targets.iter().copied());
                    }
                    self.flows.push((targets, reads, this));
                }
                walk::walk_assignment_expression(self, assignment);
            }
        }
        let mut walk = Collect {
            scoping,
            parameters: Vec::new(),
            flows: Vec::new(),
        };
        walk.visit_program(program);
        tainted.extend(walk.parameters);
        flows.append(&mut walk.flows);
    }
    loop {
        let before = tainted.len();
        for (targets, reads, this) in &flows {
            if *this || reads.iter().any(|read| tainted.contains(read)) {
                tainted.extend(targets.iter().copied());
            }
        }
        if tainted.len() == before {
            return tainted;
        }
    }
}

/// The program bindings `expression` reads, and whether it reads `this` (outside nested
/// non-arrow functions, which have their own).
fn reads_of(expression: &Expression<'_>, scoping: &Scoping) -> (Vec<SymbolId>, bool) {
    struct Reads<'s> {
        scoping: &'s Scoping,
        symbols: Vec<SymbolId>,
        this: bool,
    }
    impl<'a> Visit<'a> for Reads<'_> {
        fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
            if let Some(symbol) = ident
                .reference_id
                .get()
                .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
            {
                self.symbols.push(symbol);
            }
        }

        fn visit_this_expression(&mut self, _: &oxc_ast::ast::ThisExpression) {
            self.this = true;
        }

        fn visit_function(&mut self, _: &oxc_ast::ast::Function<'a>, _: ScopeFlags) {}
    }
    let mut reads = Reads {
        scoping,
        symbols: Vec::new(),
        this: false,
    };
    reads.visit_expression(expression);
    (reads.symbols, reads.this)
}

/// The identifiers an assignment target writes.
fn collect_targets(
    target: &oxc_ast::ast::AssignmentTarget<'_>,
    scoping: &Scoping,
    out: &mut Vec<SymbolId>,
) {
    struct Targets<'s, 'o> {
        scoping: &'s Scoping,
        out: &'o mut Vec<SymbolId>,
    }
    impl<'a> Visit<'a> for Targets<'_, '_> {
        fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
            if let Some(symbol) = ident
                .reference_id
                .get()
                .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
            {
                self.out.push(symbol);
            }
        }
    }
    Targets { scoping, out }.visit_assignment_target(target);
}

/// Whether evaluating `expression` reads `this` or one of `dependent`'s bindings.
pub fn depends_on(
    expression: &Expression<'_>,
    dependent: &FxHashSet<SymbolId>,
    scoping: &Scoping,
) -> bool {
    let (reads, this) = reads_of(expression, scoping);
    this || reads.iter().any(|symbol| dependent.contains(symbol))
}
