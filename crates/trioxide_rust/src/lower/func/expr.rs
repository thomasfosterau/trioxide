//! Expressions.

#[allow(
    clippy::wildcard_imports,
    reason = "one impl split over modules (`expect` is unfulfilled in test builds)"
)]
use super::{stmt::is_pure, *};

/// `Math` functions computed exactly as V8 does (`docs/ABI.md`, "Numerics"), by arity. The
/// transcendental ones are `trioxide_rt::math::EXACT_TRANSCENDENTALS` (a test checks).
const EXACT_MATH: &[(&str, MathFn, usize)] = &[
    ("abs", MathFn::Abs, 1),
    ("floor", MathFn::Floor, 1),
    ("ceil", MathFn::Ceil, 1),
    ("trunc", MathFn::Trunc, 1),
    ("sqrt", MathFn::Sqrt, 1),
    ("round", MathFn::Round, 1),
    ("sign", MathFn::Sign, 1),
    ("fround", MathFn::Fround, 1),
    ("tan", MathFn::Tan, 1),
    ("asin", MathFn::Asin, 1),
    ("acos", MathFn::Acos, 1),
    ("atan", MathFn::Atan, 1),
    ("expm1", MathFn::Expm1, 1),
    ("log2", MathFn::Log2, 1),
];

/// `Math` functions whose last bit may differ from V8's.
const INEXACT_MATH: &[&str] = &[
    "sin", "cos", "cosh", "tanh", "asinh", "acosh", "exp", "log", "log10", "cbrt", "sinh", "atanh",
    "log1p", "atan2",
];

impl<'a> FnCx<'_, '_, 'a> {
    #[expect(
        clippy::many_single_char_names,
        reason = "one short name per node kind"
    )]
    pub(super) fn expr(&mut self, expression: &'a Expression<'a>) -> Option<V> {
        let e = expression.get_inner_expression();
        match e {
            Expression::NumericLiteral(n) => Some(number_lit(n.value)),
            Expression::BooleanLiteral(b) => {
                Some(V::new(Expr::Lit(Lit::Bool(b.value)), Ty::Bool, Kind::Bool))
            }
            Expression::Identifier(id) => self.identifier(id),
            Expression::BinaryExpression(b) => self.binary(b),
            Expression::LogicalExpression(l) => self.logical(l),
            Expression::UnaryExpression(u) => self.unary(u),
            Expression::ConditionalExpression(c) => {
                let test = self.condition(&c.test);
                let a = self.expr(&c.consequent);
                let b = self.expr(&c.alternate);
                let (test, a, b) = (test?, a?, b?);
                let kind = a.kind.join(&b.kind);
                let ty = match (&kind, a.ty, b.ty) {
                    (Kind::Num { .. }, x, y) if x == y => x,
                    (Kind::Num { .. }, _, _) => Ty::F64,
                    (Kind::Bool | Kind::Value { .. }, x, _) => x,
                    _ => {
                        return self.fail(
                            c.span,
                            format!(
                                "chooses between {} and {}",
                                a.kind.describe(),
                                b.kind.describe()
                            ),
                        );
                    }
                };
                let a = self.coerce(a, ty, false, c.span)?;
                let b = self.coerce(b, ty, false, c.span)?;
                Some(V::new(
                    Expr::Cond(Box::new(test), Box::new(a), Box::new(b)),
                    ty,
                    kind,
                ))
            }
            Expression::CallExpression(c) => self.call(c),
            Expression::NewExpression(n) => self.new_expr(n),
            Expression::StaticMemberExpression(m) => self.static_member(m),
            Expression::ComputedMemberExpression(m) => self.element(m),
            Expression::ObjectExpression(o) => self.object(o),
            Expression::ThisExpression(t) => match self.class {
                Some(class) => {
                    let kind = self
                        .lw
                        .env
                        .params
                        .get(&(self.id, THIS))
                        .cloned()
                        .unwrap_or(Kind::Bottom);
                    let ty = Ty::Struct(self.struct_ref(StructSlot::Key(StructKey::Class(class))));
                    Some(V::new(Expr::SelfValue, ty, kind))
                }
                None if self.is_ctor => self.fail(t.span, "reads `this` in the constructor"),
                None => self.fail(t.span, "uses `this` outside a method"),
            },
            Expression::UpdateExpression(u) => self.update_value(u),
            Expression::AssignmentExpression(a) => {
                self.fail(a.span, "uses an assignment as a value")
            }
            Expression::StringLiteral(s) => {
                self.fail(s.span, "uses a string (strings aren't supported in WASM)")
            }
            Expression::TemplateLiteral(t) => {
                self.fail(t.span, "uses a string (strings aren't supported in WASM)")
            }
            Expression::ArrowFunctionExpression(f) => self.fail(f.span, "creates a closure"),
            Expression::FunctionExpression(f) => self.fail(f.span, "creates a closure"),
            Expression::ArrayExpression(a) => self.fail(a.span, "creates an array literal"),
            Expression::AwaitExpression(a) => self.fail(a.span, "uses `await`"),
            Expression::YieldExpression(y) => self.fail(y.span, "uses `yield`"),
            Expression::ChainExpression(c) => self.fail(c.span, "uses optional chaining"),
            Expression::NullLiteral(n) => self.fail(n.span, "uses `null`"),
            Expression::BigIntLiteral(b) => self.fail(b.span, "uses a bigint"),
            Expression::SequenceExpression(s) => self.fail(s.span, "uses the comma operator"),
            Expression::RegExpLiteral(r) => self.fail(r.span, "uses a regular expression"),
            Expression::ClassExpression(c) => self.fail(c.span, "creates a class"),
            other => self.fail(
                other.span(),
                "uses an expression the backend doesn't support",
            ),
        }
    }

    /// A number (possibly `undefined`, as `NaN`), or a reason.
    fn number(&mut self, expression: &'a Expression<'a>) -> Option<V> {
        let value = self.expr(expression)?;
        self.require_number(value, expression.span())
    }

    fn require_number(&mut self, value: V, span: Span) -> Option<V> {
        match value.kind {
            Kind::Num { .. } | Kind::Bottom | Kind::Error => Some(value),
            ref other => {
                let message = format!("uses {} where a number is expected", other.describe());
                self.fail(span, message)
            }
        }
    }

    /// A boolean condition from JS truthiness.
    pub(super) fn condition(&mut self, expression: &'a Expression<'a>) -> Option<Expr> {
        let value = self.expr(expression)?;
        self.truthy(value, expression.span())
    }

    fn truthy(&mut self, value: V, span: Span) -> Option<Expr> {
        match (&value.kind, value.ty) {
            (Kind::Bool, _) => Some(value.expr),
            (Kind::Num { .. } | Kind::Bottom, Ty::I32 | Ty::U32 | Ty::Index) => {
                let zero = zero_of(value.ty);
                Some(Expr::Binary(
                    BinOp::Ne,
                    Box::new(value.expr),
                    Box::new(zero),
                ))
            }
            (Kind::Num { .. } | Kind::Bottom, _) => {
                Some(Expr::Truthy(Ty::F64, Box::new(value.expr)))
            }
            (other, _) => {
                let message = format!("tests the truthiness of {}", other.describe());
                self.fail(span, message)
            }
        }
    }

    // ------------------------------------------------------------------------ identifiers

    fn identifier(&mut self, id: &'a IdentifierReference<'a>) -> Option<V> {
        let Some(symbol) = self.module.reference_symbol(id.reference_id.get()) else {
            return match id.name.as_str() {
                "NaN" => Some(V::new(Expr::Lit(Lit::F64(f64::NAN)), Ty::F64, Kind::NUM)),
                "Infinity" => Some(V::new(
                    Expr::Lit(Lit::F64(f64::INFINITY)),
                    Ty::F64,
                    Kind::NUM,
                )),
                "undefined" => self.fail(id.span, "uses `undefined`"),
                "arguments" => self.fail(id.span, "uses `arguments`"),
                name => self.fail(id.span, format!("uses the global `{name}` (a host API)")),
            };
        };
        if let Some(&local) = self.by_symbol.get(&symbol) {
            return Some(self.local_value(local));
        }
        let symbol = self.module.symbol(symbol);
        if let Some(value) = self.constant(symbol) {
            let declared_in = self.resolve(symbol).module;
            self.initialised(declared_in, id.node_id(), &id.name, id.span)?;
            return Some(value);
        }
        if self.resolve_function(symbol).is_some() {
            return self.fail(
                id.span,
                format!("uses the function `{}` as a value", id.name),
            );
        }
        self.fail(
            id.span,
            format!(
                "reads `{}`, which is defined outside the function (only module-level constants \
                 can be read)",
                id.name
            ),
        )
    }

    /// Checks that reading the module-level `let`/`const`/`class` binding `name` (declared in
    /// `module`) at `node` can't throw: the island has its value from the start, while JS
    /// throws if the read runs before the declaration (the temporal dead zone). Without an
    /// import cycle a module's body runs before its importers', so only the declaring
    /// module's own evaluation can read it early, which the analysis follows.
    fn initialised(
        &mut self,
        module: trioxide_common::ModuleId,
        node: NodeId,
        name: &str,
        span: Span,
    ) -> Option<()> {
        if self.lw.world.cyclic.contains(&module) {
            return self.fail(
                span,
                format!(
                    "reads `{name}`, declared in a module in an import cycle, where code may \
                     run before the declaration (and JS would throw)"
                ),
            );
        }
        let effects = self.facts().expr_effects(self.node(node));
        if !self.facts().is_usable(effects.trust) || effects.value.throws {
            return self.fail(
                span,
                format!(
                    "reads `{name}`, which the analysis can't prove is initialised whenever \
                     this runs (a read before its declaration throws)"
                ),
            );
        }
        Some(())
    }

    /// Follows an import to the binding it names.
    pub(super) fn resolve(&self, symbol: SymbolRef) -> SymbolRef {
        self.facts().resolve_import(symbol).unwrap_or(symbol)
    }

    /// A module-level `const` with a known number or boolean value.
    fn constant(&mut self, symbol: SymbolRef) -> Option<V> {
        let symbol = self.resolve(symbol);
        if let Some(&reference) = self.lw.const_refs.get(&symbol) {
            let value = self.lw.consts[reference.0].value;
            return Some(V::new(
                Expr::Const(reference),
                value.ty(),
                kind_of_ty(value.ty()),
            ));
        }
        let module = self.lw.world.module(symbol.module)?;
        let (node, kind) = module.declaration(symbol.symbol);
        let AstKind::VariableDeclarator(declarator) = kind else {
            return None;
        };
        let nodes = module.semantic.nodes();
        let AstKind::VariableDeclaration(declaration) = nodes.parent_kind(node) else {
            return None;
        };
        if declaration.kind != VariableDeclarationKind::Const {
            return None;
        }
        let top_level = matches!(
            nodes.parent_kind(nodes.parent_id(node)),
            AstKind::Program(_) | AstKind::ExportDeclaration(_)
        );
        if !top_level {
            return None;
        }
        let init = declarator
            .init
            .as_ref()
            .map(Expression::get_inner_expression);
        let value = if let Some(Expression::BooleanLiteral(b)) = init {
            Lit::Bool(b.value)
        } else {
            let fact = self.facts().symbol_ty(symbol);
            if !self.facts().is_usable(fact.trust) {
                return None;
            }
            let Type::Number(n) = fact.value else {
                return None;
            };
            Lit::F64(n.as_constant()?)
        };
        let name = names::rust_ident(declarator.id.get_identifier_name()?.as_str());
        let mut unique = name.clone();
        let mut n = 2;
        while self.lw.consts.iter().any(|c| c.name == unique)
            || self.lw.fn_names.values().any(|f| *f == unique)
        {
            unique = format!("{}_{n}", names::bare(&name));
            n += 1;
        }
        let reference = crate::rir::ConstRef(self.lw.consts.len());
        self.lw.consts.push(crate::rir::ConstDef {
            name: unique,
            value,
        });
        self.lw.const_refs.insert(symbol, reference);
        self.lw.env.changed = true;
        Some(V::new(
            Expr::Const(reference),
            value.ty(),
            kind_of_ty(value.ty()),
        ))
    }

    /// The program function a binding names, if it is a plain, never reassigned function
    /// declaration.
    fn resolve_function(&self, symbol: SymbolRef) -> Option<FunctionId> {
        let symbol = self.resolve(symbol);
        let module = self.lw.world.module(symbol.module)?;
        if module.semantic.scoping().symbol_is_mutated(symbol.symbol) {
            return None;
        }
        let (node, kind) = module.declaration(symbol.symbol);
        let AstKind::Function(_) = kind else {
            return None;
        };
        let id = module.node(node);
        let info = self.lw.world.functions.get(&id)?;
        (info.role == Role::Free).then_some(id)
    }

    fn resolve_class(&self, symbol: SymbolRef) -> Option<ClassId> {
        let symbol = self.resolve(symbol);
        let module = self.lw.world.module(symbol.module)?;
        let (node, kind) = module.declaration(symbol.symbol);
        let AstKind::Class(_) = kind else { return None };
        Some(module.node(node))
    }

    // -------------------------------------------------------------------------- operators

    fn binary(&mut self, b: &'a BinaryExpression<'a>) -> Option<V> {
        let op = b.operator;
        match op {
            BinaryOperator::In | BinaryOperator::Instanceof => {
                self.fail(b.span, format!("uses `{}`", op.as_str()))
            }
            BinaryOperator::Exponential => {
                let base = self.number(&b.left)?;
                self.power(base, &b.right, b.span)
            }
            BinaryOperator::BitwiseOR
                if matches!(b.right.get_inner_expression(), Expression::NumericLiteral(n) if n.value == 0.0)
                    && matches!(b.left.get_inner_expression(), Expression::BinaryExpression(d) if d.operator == BinaryOperator::Division) =>
            {
                // `(a / b) | 0`
                let Expression::BinaryExpression(division) = b.left.get_inner_expression() else {
                    unreachable!("matched above")
                };
                let x = self.number(&division.left)?;
                let y = self.number(&division.right)?;
                if x.ty == Ty::I32 && y.ty == Ty::I32 {
                    return Some(V::new(
                        Expr::Binary(BinOp::DivTruncI32, Box::new(x.expr), Box::new(y.expr)),
                        Ty::I32,
                        Kind::NUM,
                    ));
                }
                let quotient = self.arith(BinaryOperator::Division, x, y, None, division.span)?;
                let zero = number_lit(0.0);
                self.bits(BinaryOperator::BitwiseOR, quotient, zero, b.span)
            }
            BinaryOperator::BitwiseOR
            | BinaryOperator::BitwiseAnd
            | BinaryOperator::BitwiseXOR
            | BinaryOperator::ShiftLeft
            | BinaryOperator::ShiftRight
            | BinaryOperator::ShiftRightZeroFill => {
                let left = self.int32_operand(&b.left)?;
                let right = self.int32_operand(&b.right)?;
                self.bits(op, left, right, b.span)
            }
            _ => {
                let left = self.expr(&b.left)?;
                let fact = self.num_fact(b.node_id());
                self.binary_values(op, left, &b.right, fact, b.span)
            }
        }
    }

    /// An operand of a bitwise operator, which applies `ToInt32` to it: `a + b` and `a - b` on
    /// two int32s wrap exactly (the exact sum is below 2^33, so the number JS computes is
    /// exact too, and `ToInt32` keeps it modulo 2^32).
    fn int32_operand(&mut self, expression: &'a Expression<'a>) -> Option<V> {
        if let Expression::BinaryExpression(inner) = expression.get_inner_expression()
            && matches!(
                inner.operator,
                BinaryOperator::Addition | BinaryOperator::Subtraction
            )
        {
            let left = self.expr(&inner.left)?;
            let right = self.expr(&inner.right)?;
            if matches!(left.ty, Ty::I32 | Ty::U32) && matches!(right.ty, Ty::I32 | Ty::U32) {
                let op = if inner.operator == BinaryOperator::Addition {
                    BinOp::WrapAdd
                } else {
                    BinOp::WrapSub
                };
                let l = reinterpret(left, Ty::I32);
                let r = reinterpret(right, Ty::I32);
                return Some(V::new(
                    Expr::Binary(op, Box::new(l), Box::new(r)),
                    Ty::I32,
                    Kind::NUM,
                ));
            }
            let fact = self.num_fact(inner.node_id());
            return self.arith(inner.operator, left, right, fact, inner.span);
        }
        self.expr(expression)
    }

    /// `left op right` with `left` already lowered (also used for compound assignments).
    pub(super) fn binary_values(
        &mut self,
        op: BinaryOperator,
        left: V,
        right: &'a Expression<'a>,
        result: Option<NumberType>,
        span: Span,
    ) -> Option<V> {
        if op == BinaryOperator::Exponential {
            let left = self.require_number(left, span)?;
            return self.power(left, right, span);
        }
        let right = self.expr(right)?;
        match op {
            BinaryOperator::Addition
            | BinaryOperator::Subtraction
            | BinaryOperator::Multiplication
            | BinaryOperator::Division
            | BinaryOperator::Remainder => self.arith(op, left, right, result, span),
            BinaryOperator::BitwiseOR
            | BinaryOperator::BitwiseAnd
            | BinaryOperator::BitwiseXOR
            | BinaryOperator::ShiftLeft
            | BinaryOperator::ShiftRight
            | BinaryOperator::ShiftRightZeroFill => self.bits(op, left, right, span),
            BinaryOperator::LessThan
            | BinaryOperator::LessEqualThan
            | BinaryOperator::GreaterThan
            | BinaryOperator::GreaterEqualThan => {
                let left = self.require_number(left, span)?;
                let right = self.require_number(right, span)?;
                let bin = match op {
                    BinaryOperator::LessThan => BinOp::Lt,
                    BinaryOperator::LessEqualThan => BinOp::Le,
                    BinaryOperator::GreaterThan => BinOp::Gt,
                    _ => BinOp::Ge,
                };
                self.compare(bin, left, right, span)
            }
            BinaryOperator::Equality
            | BinaryOperator::Inequality
            | BinaryOperator::StrictEquality
            | BinaryOperator::StrictInequality => {
                let bin = if matches!(
                    op,
                    BinaryOperator::Equality | BinaryOperator::StrictEquality
                ) {
                    BinOp::Eq
                } else {
                    BinOp::Ne
                };
                match (&left.kind, &right.kind) {
                    (Kind::Bool, Kind::Bool) => Some(V::new(
                        Expr::Binary(bin, Box::new(left.expr), Box::new(right.expr)),
                        Ty::Bool,
                        Kind::Bool,
                    )),
                    // `undefined === x` and `NaN === x` are both false for any number `x`,
                    // so one side may be undefined; both may not (`undefined === undefined`).
                    (Kind::Num { undef: a }, Kind::Num { undef: b }) if !(*a && *b) => {
                        self.compare(bin, left, right, span)
                    }
                    (Kind::Num { .. }, Kind::Num { .. }) => self.fail(
                        span,
                        "compares two values that may both be undefined (unchecked array reads)",
                    ),
                    (a, b) => self.fail(
                        span,
                        format!("compares {} with {}", a.describe(), b.describe()),
                    ),
                }
            }
            _ => self.fail(span, format!("uses `{}`", op.as_str())),
        }
    }

    fn compare(&mut self, op: BinOp, left: V, right: V, span: Span) -> Option<V> {
        // Comparisons with a literal NaN are constant (`!=` is true, the rest false); both
        // sides are still evaluated.
        let is_nan = |v: &V| matches!(v.expr, Expr::Lit(Lit::F64(x)) if x.is_nan());
        if is_nan(&left) || is_nan(&right) {
            let result = Expr::Lit(Lit::Bool(op == BinOp::Ne));
            let mut stmts = Vec::new();
            for side in [left, right] {
                if !is_pure(&side.expr) {
                    stmts.push(Stmt::Expr(side.expr));
                }
            }
            let expr = if stmts.is_empty() {
                result
            } else {
                Expr::Block(stmts, Box::new(result))
            };
            return Some(V::new(expr, Ty::Bool, Kind::Bool));
        }
        let ty = match (left.ty, right.ty) {
            (Ty::I32, Ty::I32) => Ty::I32,
            (Ty::U32, Ty::U32) => Ty::U32,
            (Ty::Index, Ty::Index) => Ty::Index,
            _ => Ty::F64,
        };
        let l = self.coerce(left, ty, false, span)?;
        let r = self.coerce(right, ty, false, span)?;
        Some(V::new(
            Expr::Binary(op, Box::new(l), Box::new(r)),
            Ty::Bool,
            Kind::Bool,
        ))
    }

    /// `+ - * / %` on numbers. `result` is the range fact of the result, which decides
    /// whether integer arithmetic is exact.
    pub(super) fn arith(
        &mut self,
        op: BinaryOperator,
        left: V,
        right: V,
        result: Option<NumberType>,
        span: Span,
    ) -> Option<V> {
        let left = self.require_number(left, span)?;
        let right = self.require_number(right, span)?;
        let ints = matches!(left.ty, Ty::I32 | Ty::U32) && matches!(right.ty, Ty::I32 | Ty::U32);
        let wrap = match op {
            BinaryOperator::Addition => Some(BinOp::IntAdd),
            BinaryOperator::Subtraction => Some(BinOp::IntSub),
            BinaryOperator::Multiplication => Some(BinOp::IntMul),
            _ => None,
        };
        if let (true, Some(wrap), Some(result)) = (ints, wrap, result) {
            let target = if is_int32(result) {
                Some(Ty::I32)
            } else if is_uint32(result) {
                Some(Ty::U32)
            } else {
                None
            };
            if let Some(target) = target {
                // Arithmetic modulo 2^32 is exact here: the result is proven in range. An
                // operand reinterpreted between i32 and u32 may wrap on the way there.
                let wrap = if left.ty == target && right.ty == target {
                    wrap
                } else {
                    match wrap {
                        BinOp::IntAdd => BinOp::WrapAdd,
                        BinOp::IntSub => BinOp::WrapSub,
                        _ => BinOp::WrapMul,
                    }
                };
                let l = reinterpret(left, target);
                let r = reinterpret(right, target);
                return Some(V::new(
                    Expr::Binary(wrap, Box::new(l), Box::new(r)),
                    target,
                    Kind::NUM,
                ));
            }
        }
        if op == BinaryOperator::Remainder && left.ty == Ty::I32 && right.ty == Ty::I32 {
            return Some(V::new(
                Expr::Binary(BinOp::RemI32, Box::new(left.expr), Box::new(right.expr)),
                Ty::F64,
                Kind::NUM,
            ));
        }
        let bin = match op {
            BinaryOperator::Addition => BinOp::Add,
            BinaryOperator::Subtraction => BinOp::Sub,
            BinaryOperator::Multiplication => BinOp::Mul,
            BinaryOperator::Division => BinOp::Div,
            _ => BinOp::Rem,
        };
        let l = self.coerce(left, Ty::F64, false, span)?;
        let r = self.coerce(right, Ty::F64, false, span)?;
        Some(V::new(
            Expr::Binary(bin, Box::new(l), Box::new(r)),
            Ty::F64,
            Kind::NUM,
        ))
    }

    /// `x ** n` for the exponents where the result is exact in both V8 and the runtime.
    fn power(&mut self, base: V, exponent: &'a Expression<'a>, span: Span) -> Option<V> {
        let exponent_value = match exponent.get_inner_expression() {
            Expression::NumericLiteral(n) => Some(n.value),
            _ => None,
        };
        let base_expr = self.coerce(base, Ty::F64, false, span)?;
        match exponent_value {
            Some(2.0) => {
                if is_pure(&base_expr) {
                    let mul =
                        Expr::Binary(BinOp::Mul, Box::new(base_expr.clone()), Box::new(base_expr));
                    return Some(V::new(mul, Ty::F64, Kind::NUM));
                }
                let local = self.temp("base", Ty::F64);
                let mul = Expr::Binary(
                    BinOp::Mul,
                    Box::new(Expr::Local(local)),
                    Box::new(Expr::Local(local)),
                );
                Some(V::new(
                    Expr::Block(vec![Stmt::Let(local, base_expr)], Box::new(mul)),
                    Ty::F64,
                    Kind::NUM,
                ))
            }
            Some(1.0) => Some(V::new(base_expr, Ty::F64, Kind::NUM)),
            _ => self.fail(
                span,
                "uses `**` or `Math.pow` with an exponent other than 1 or 2 (the result may differ \
                 from V8's in the last bit)",
            ),
        }
    }

    /// The bitwise and shift operators: `ToInt32` on both sides.
    fn bits(&mut self, op: BinaryOperator, left: V, right: V, span: Span) -> Option<V> {
        let left = self.require_number(left, span)?;
        let right = self.require_number(right, span)?;
        let right_is_zero = matches!(right.expr, Expr::Lit(Lit::I32(0)));
        if right_is_zero && op == BinaryOperator::BitwiseOR {
            return Some(V::new(Self::int32_of(left), Ty::I32, Kind::NUM));
        }
        if right_is_zero && op == BinaryOperator::ShiftRightZeroFill {
            let expr = match left.ty {
                Ty::U32 => left.expr,
                Ty::I32 => convert(Conv::Exact, Ty::I32, Ty::U32, left.expr),
                _ => {
                    let f = self.coerce(left, Ty::F64, false, span)?;
                    convert(Conv::ToUint32, Ty::F64, Ty::U32, f)
                }
            };
            return Some(V::new(expr, Ty::U32, Kind::NUM));
        }
        let l = Self::int32_of(left);
        let r = Self::int32_of(right);
        let (bin, ty) = match op {
            BinaryOperator::BitwiseAnd => (BinOp::BitAnd, Ty::I32),
            BinaryOperator::BitwiseOR => (BinOp::BitOr, Ty::I32),
            BinaryOperator::BitwiseXOR => (BinOp::BitXor, Ty::I32),
            BinaryOperator::ShiftLeft => (BinOp::Shl, Ty::I32),
            BinaryOperator::ShiftRight => (BinOp::Shr, Ty::I32),
            _ => (BinOp::Ushr, Ty::U32),
        };
        Some(V::new(
            Expr::Binary(bin, Box::new(l), Box::new(r)),
            ty,
            Kind::NUM,
        ))
    }

    /// `ToInt32` of a number.
    fn int32_of(value: V) -> Expr {
        match value.ty {
            Ty::I32 => value.expr,
            Ty::U32 => convert(Conv::Exact, Ty::U32, Ty::I32, value.expr),
            Ty::Index => {
                let f = convert(Conv::Exact, Ty::Index, Ty::F64, value.expr);
                convert(Conv::ToInt32, Ty::F64, Ty::I32, f)
            }
            _ => convert(Conv::ToInt32, Ty::F64, Ty::I32, value.expr),
        }
    }

    fn logical(&mut self, l: &'a LogicalExpression<'a>) -> Option<V> {
        let or = match l.operator {
            LogicalOperator::Or => true,
            LogicalOperator::And => false,
            LogicalOperator::Coalesce => return self.fail(l.span, "uses `??`"),
        };
        let left = self.expr(&l.left)?;
        let right = self.expr(&l.right)?;
        match (&left.kind, &right.kind) {
            (Kind::Bool, Kind::Bool) => Some(V::new(
                Expr::Binary(
                    if or { BinOp::Or } else { BinOp::And },
                    Box::new(left.expr),
                    Box::new(right.expr),
                ),
                Ty::Bool,
                Kind::Bool,
            )),
            (Kind::Num { .. }, Kind::Num { .. }) => {
                let kind = left.kind.join(&right.kind);
                let ty = if left.ty == right.ty && left.ty != Ty::Index {
                    left.ty
                } else {
                    Ty::F64
                };
                let lhs = self.coerce(left, ty, false, l.span)?;
                let rhs = self.coerce(right, ty, false, l.span)?;
                Some(V::new(
                    Expr::NumberLogic {
                        or,
                        ty,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    ty,
                    kind,
                ))
            }
            (a, b) => self.fail(
                l.span,
                format!(
                    "combines {} and {} with `{}`",
                    a.describe(),
                    b.describe(),
                    l.operator.as_str()
                ),
            ),
        }
    }

    fn unary(&mut self, u: &'a UnaryExpression<'a>) -> Option<V> {
        match u.operator {
            UnaryOperator::UnaryNegation => {
                let value = self.number(&u.argument)?;
                match value.expr {
                    Expr::Lit(Lit::I32(v)) if v != 0 && v != i32::MIN => {
                        return Some(V::new(Expr::Lit(Lit::I32(-v)), Ty::I32, Kind::NUM));
                    }
                    Expr::Lit(Lit::F64(v)) => {
                        return Some(V::new(Expr::Lit(Lit::F64(-v)), Ty::F64, Kind::NUM));
                    }
                    _ => {}
                }
                if value.ty == Ty::I32 && self.num_fact(u.node_id()).is_some_and(is_int32) {
                    return Some(V::new(
                        Expr::Unary(UnOp::NegI32, Box::new(value.expr)),
                        Ty::I32,
                        Kind::NUM,
                    ));
                }
                let f = self.coerce(value, Ty::F64, false, u.span)?;
                Some(V::new(
                    Expr::Unary(UnOp::NegF64, Box::new(f)),
                    Ty::F64,
                    Kind::NUM,
                ))
            }
            UnaryOperator::UnaryPlus => {
                let value = self.number(&u.argument)?;
                Some(V {
                    kind: Kind::NUM,
                    ..value
                })
            }
            UnaryOperator::LogicalNot => {
                let test = self.condition(&u.argument)?;
                Some(V::new(
                    Expr::Unary(UnOp::Not, Box::new(test)),
                    Ty::Bool,
                    Kind::Bool,
                ))
            }
            UnaryOperator::BitwiseNot => {
                let value = self.number(&u.argument)?;
                let i = Self::int32_of(value);
                Some(V::new(
                    Expr::Unary(UnOp::BitNot, Box::new(i)),
                    Ty::I32,
                    Kind::NUM,
                ))
            }
            op => self.fail(u.span, format!("uses `{}`", op.as_str())),
        }
    }

    /// `x++` / `++x` used as a value, on a local.
    fn update_value(&mut self, u: &'a UpdateExpression<'a>) -> Option<V> {
        let SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = &u.argument else {
            return self.fail(u.span, "uses `++`/`--` on an element or field as a value");
        };
        let local = self.assignable_local(id)?;
        let current = self.local_value(local);
        let ty = current.ty;
        let mut stmts = Vec::new();
        let old = if u.prefix {
            None
        } else {
            let old = self.temp("old", ty);
            stmts.push(Stmt::Let(old, current.expr.clone()));
            Some(old)
        };
        let op = match u.operator {
            UpdateOperator::Increment => BinaryOperator::Addition,
            UpdateOperator::Decrement => BinaryOperator::Subtraction,
        };
        let fact = match ty {
            Ty::I32 | Ty::U32 => self.info[local.0].symbol.and_then(|s| self.symbol_fact(s)),
            _ => None,
        };
        let one = V::new(Expr::Lit(Lit::I32(1)), Ty::I32, Kind::NUM);
        let value = self.arith(op, current, one, fact, u.span)?;
        self.assign_local(local, value, &id.name, u.span, &mut stmts);
        let result = Expr::Local(old.unwrap_or(local));
        Some(V::new(Expr::Block(stmts, Box::new(result)), ty, Kind::NUM))
    }

    // ------------------------------------------------------------------------------ calls

    fn args(
        &mut self,
        arguments: &'a [Argument<'a>],
        span: Span,
    ) -> Option<Vec<(V, Span, &'a Expression<'a>)>> {
        let mut values = Vec::new();
        let mut ok = true;
        for argument in arguments {
            let Some(expression) = argument.as_expression() else {
                self.fail::<()>(span, "spreads arguments");
                ok = false;
                continue;
            };
            match self.expr(expression) {
                Some(value) => values.push((value, expression.span(), expression)),
                None => ok = false,
            }
        }
        ok.then_some(values)
    }

    fn call(&mut self, c: &'a CallExpression<'a>) -> Option<V> {
        if c.optional {
            return self.fail(c.span, "uses an optional call");
        }
        match c.callee.get_inner_expression() {
            Expression::StaticMemberExpression(member) => {
                if self.is_global(&member.object, "Math") {
                    return self.math_call(c, member.property.name.as_str());
                }
                if member.property.name == "fill"
                    && let Expression::NewExpression(new) = member.object.get_inner_expression()
                    && self.is_global(&new.callee, "Array")
                {
                    return self.new_number_array(c, new);
                }
                let receiver = self.expr(&member.object)?;
                let name = member.property.name.as_str();
                match &receiver.kind {
                    Kind::Value {
                        key: StructKey::Class(class),
                        ..
                    } => self.method_call(*class, receiver, name, c),
                    Kind::Array { .. } => {
                        self.fail(c.span, format!("calls the array method `.{name}()`"))
                    }
                    other => {
                        self.fail(c.span, format!("calls `.{name}()` on {}", other.describe()))
                    }
                }
            }
            Expression::Identifier(id) => {
                let Some(symbol) = self.module.reference_symbol(id.reference_id.get()) else {
                    return self.fail(
                        c.span,
                        format!("calls the global `{}` (a host API)", id.name),
                    );
                };
                let symbol = self.module.symbol(symbol);
                let Some(callee) = self.resolve_function(symbol) else {
                    return self.fail(
                        c.span,
                        format!(
                            "calls `{}`, which isn't a function declaration the backend can follow",
                            id.name
                        ),
                    );
                };
                self.function_call(callee, None, c.arguments.as_slice(), c.span)
            }
            other => self.fail(other.span(), "calls a computed function"),
        }
    }

    fn is_global(&self, expression: &Expression<'_>, name: &str) -> bool {
        matches!(expression.get_inner_expression(), Expression::Identifier(id)
            if id.name == name && self.module.reference_symbol(id.reference_id.get()).is_none())
    }

    /// Calls an island function (or method, with `receiver`).
    fn function_call(
        &mut self,
        callee: FunctionId,
        receiver: Option<V>,
        arguments: &'a [Argument<'a>],
        span: Span,
    ) -> Option<V> {
        let world = self.lw.world;
        let info = world.functions.get(&callee);
        if let Some(directive) = self.lw.world.directive(callee)
            && directive.placement == crate::world::Placement::Js
        {
            let name = self.lw.display_name(callee);
            return self.fail(span, format!("calls `{name}`, which is marked \"use js\""));
        }
        let declared = match info {
            Some(info) => info.func.params.items.len(),
            None => 0,
        };
        if arguments.len() != declared {
            let name = self.lw.display_name(callee);
            return self.fail(
                span,
                format!(
                    "calls `{name}` with {} argument(s); it declares {declared}",
                    arguments.len()
                ),
            );
        }
        let func = self.lw.member(callee);
        self.lw.env.calls.insert((self.id, callee, span));
        let args = self.args(arguments, span)?;
        if let Some(receiver) = &receiver {
            self.lw.env.join_param(callee, THIS, &receiver.kind);
        }
        let mut lowered = Vec::new();
        let mut passed: Vec<(LocalId, bool)> = Vec::new();
        for (index, (value, arg_span, _)) in args.into_iter().enumerate() {
            self.lw.env.join_param(callee, index, &value.kind);
            let callee_mutates = self.lw.env.mutated.contains(&(callee, index));
            match (&value.kind, &value.expr) {
                (Kind::Array { .. } | Kind::ParamStruct { .. }, Expr::Local(local)) => {
                    let local = *local;
                    if let Some(param) = self.info[local.0].param {
                        self.lw.env.passes.insert((self.id, param, callee, index));
                    } else if let Kind::Array { params, .. } = value.kind
                        && params != 0
                        && callee_mutates
                    {
                        let name = self.lw.display_name(callee);
                        return self.fail(
                            arg_span,
                            format!("passes an array that may be one of its parameters to `{name}`, which writes to it"),
                        );
                    }
                    if passed
                        .iter()
                        .any(|&(l, m)| l == local && (m || callee_mutates))
                    {
                        let name = self.lw.display_name(callee);
                        return self.fail(arg_span, format!("passes the same array or object twice to `{name}`, which writes to it"));
                    }
                    passed.push((local, callee_mutates));
                }
                (Kind::ParamStruct { .. }, _) => {
                    return self.fail(arg_span, "passes an object parameter indirectly");
                }
                _ => {}
            }
            let ty = self.param_ty(callee, index);
            lowered.push(self.coerce(value, ty, true, arg_span)?);
        }
        let ret = self.ret_ty(callee);
        let kind = match self
            .lw
            .env
            .rets
            .get(&callee)
            .cloned()
            .unwrap_or(Kind::Bottom)
        {
            Kind::Bottom => Kind::Bottom,
            Kind::Array { elem, .. } => Kind::Array {
                elem,
                fresh: true,
                params: 0,
            },
            other => other,
        };
        let ret = match (ret, &kind) {
            (Ty::Array(array), _) => Ty::Array(ArrayTy {
                access: Access::Fresh,
                ..array
            }),
            (ret, _) => ret,
        };
        let expr = match receiver {
            Some(receiver) => Expr::MethodCall(func, Box::new(receiver.expr), lowered),
            None => Expr::Call(func, lowered),
        };
        Some(V::new(expr, ret, kind))
    }

    /// The Rust type of parameter `index` of `callee`, as the callee itself computes it.
    fn param_ty(&self, callee: FunctionId, index: usize) -> Ty {
        let kind = normalize_param(
            self.lw
                .env
                .params
                .get(&(callee, index))
                .cloned()
                .unwrap_or(Kind::Bottom),
            index,
        );
        let symbol = self.lw.world.functions.get(&callee).and_then(|info| {
            match &info.func.params.items.get(index)?.pattern {
                BindingPattern::BindingIdentifier(id) => {
                    let module = self.lw.world.module(callee.module)?;
                    Some(module.symbol(id.symbol_id.get()?))
                }
                _ => None,
            }
        });
        self.kind_ty(&kind, symbol, Some((callee, index)))
    }

    fn method_call(
        &mut self,
        class: ClassId,
        receiver: V,
        name: &str,
        c: &'a CallExpression<'a>,
    ) -> Option<V> {
        let Ok(model) = self.lw.class_model(class) else {
            return self.fail(
                c.span,
                "calls a method of a class the backend can't compile",
            );
        };
        let Some(&method) = model.methods.get(name) else {
            let class_name = self.lw.class_name(class);
            return self.fail(
                c.span,
                format!("calls `.{name}()`, which isn't a method of `{class_name}`"),
            );
        };
        self.function_call(method, Some(receiver), c.arguments.as_slice(), c.span)
    }

    fn math_call(&mut self, c: &'a CallExpression<'a>, name: &str) -> Option<V> {
        let fact = self.facts().ty(self.node(c.node_id()));
        if !self.facts().is_usable(fact.trust) || !matches!(fact.value, Type::Number(_)) {
            return self.fail(c.span, format!("can't prove `Math.{name}` is the builtin"));
        }
        let args = self.args(c.arguments.as_slice(), c.span)?;
        let mut values = Vec::new();
        for (value, span, _) in args {
            values.push(self.require_number(value, span)?);
        }
        let arity_error = |cx: &mut Self| {
            cx.fail(
                c.span,
                format!("calls `Math.{name}` with {} arguments", c.arguments.len()),
            )
        };
        if let Some(&(_, function, arity)) = EXACT_MATH.iter().find(|(n, _, _)| *n == name) {
            if values.len() != arity {
                return arity_error(self);
            }
            let args = values
                .into_iter()
                .map(|v| self.coerce(v, Ty::F64, false, c.span))
                .collect::<Option<Vec<_>>>()?;
            return Some(V::new(Expr::Math(function, args), Ty::F64, Kind::NUM));
        }
        match name {
            "clz32" => {
                let [value] = <[V; 1]>::try_from(values)
                    .ok()
                    .or_else(|| arity_error(self).map(|_: V| unreachable!()))?;
                let x = self.coerce(value, Ty::F64, false, c.span)?;
                let clz = Expr::Math(MathFn::Clz32, vec![x]);
                Some(V::new(
                    convert(Conv::Exact, Ty::U32, Ty::I32, clz),
                    Ty::I32,
                    Kind::NUM,
                ))
            }
            "imul" => {
                let [a, b] = <[V; 2]>::try_from(values)
                    .ok()
                    .or_else(|| arity_error(self).map(|_: V| unreachable!()))?;
                if a.ty == Ty::I32 && b.ty == Ty::I32 {
                    return Some(V::new(
                        Expr::Math(MathFn::ImulI32, vec![a.expr, b.expr]),
                        Ty::I32,
                        Kind::NUM,
                    ));
                }
                let a = self.coerce(a, Ty::F64, false, c.span)?;
                let b = self.coerce(b, Ty::F64, false, c.span)?;
                Some(V::new(
                    Expr::Math(MathFn::Imul, vec![a, b]),
                    Ty::I32,
                    Kind::NUM,
                ))
            }
            "max" | "min" => {
                let function = if name == "max" {
                    MathFn::Max
                } else {
                    MathFn::Min
                };
                let mut values = values.into_iter();
                let Some(first) = values.next() else {
                    let value = if name == "max" {
                        f64::NEG_INFINITY
                    } else {
                        f64::INFINITY
                    };
                    return Some(V::new(Expr::Lit(Lit::F64(value)), Ty::F64, Kind::NUM));
                };
                let mut acc = self.coerce(first, Ty::F64, false, c.span)?;
                for value in values {
                    let value = self.coerce(value, Ty::F64, false, c.span)?;
                    acc = Expr::Math(function, vec![acc, value]);
                }
                Some(V::new(acc, Ty::F64, Kind::NUM))
            }
            "hypot" => {
                let function = match values.len() {
                    2 => MathFn::Hypot2,
                    3 => MathFn::Hypot3,
                    _ => return arity_error(self),
                };
                let args = values
                    .into_iter()
                    .map(|v| self.coerce(v, Ty::F64, false, c.span))
                    .collect::<Option<Vec<_>>>()?;
                Some(V::new(Expr::Math(function, args), Ty::F64, Kind::NUM))
            }
            "pow" => {
                let [base, _] = <[V; 2]>::try_from(values)
                    .ok()
                    .or_else(|| arity_error(self).map(|_: V| unreachable!()))?;
                let exponent = c.arguments[1].as_expression().expect("checked by args");
                self.power(base, exponent, c.span)
            }
            "random" => self.fail(c.span, "uses `Math.random` (host state)"),
            name if INEXACT_MATH.contains(&name) => self.fail(
                c.span,
                format!(
                    "uses `Math.{name}`, whose result may differ from V8's in the last bit \
                     (docs/ABI.md, \"Numerics\")"
                ),
            ),
            name => self.fail(
                c.span,
                format!("uses `Math.{name}`, which the backend doesn't support"),
            ),
        }
    }

    // ----------------------------------------------------------------- objects and arrays

    fn new_expr(&mut self, n: &'a NewExpression<'a>) -> Option<V> {
        let Expression::Identifier(id) = n.callee.get_inner_expression() else {
            return self.fail(n.span, "constructs a computed class");
        };
        let Some(symbol) = self.module.reference_symbol(id.reference_id.get()) else {
            if let Some(kind) = TypedArrayKind::from_constructor_name(id.name.as_str()) {
                return self.new_typed_array(n, kind);
            }
            return self.fail(
                n.span,
                format!("constructs the global `{}` (a host API)", id.name),
            );
        };
        let Some(class) = self.resolve_class(self.module.symbol(symbol)) else {
            return self.fail(
                n.span,
                format!(
                    "constructs `{}`, which isn't a class the backend can follow",
                    id.name
                ),
            );
        };
        if let Err((span, message)) = self.lw.class_model(class) {
            let name = self.lw.class_name(class);
            self.reasons.push(Reason {
                function: self.id,
                span: n.span,
                message: format!("constructs `{name}`, which can't be compiled"),
            });
            return self.fail(span, format!("`{name}` {message}"));
        }
        if self.lw.world.cyclic.contains(&class.module) || !self.lw.world.declared_first(class) {
            return self.fail(
                n.span,
                format!(
                    "constructs `{}`, which may run before the class declaration (JS would \
                     throw): code runs before it in its module, or the module is in an import \
                     cycle",
                    id.name
                ),
            );
        }
        let ctor = self.lw.classes[&class].ctor;
        let value = self.function_call(ctor, None, n.arguments.as_slice(), n.span)?;
        let fields = self.ctor_field_kinds(class, n);
        let ty = Ty::Struct(self.struct_ref(StructSlot::Key(StructKey::Class(class))));
        Some(V::new(
            value.expr,
            ty,
            Kind::Value {
                key: StructKey::Class(class),
                fields: fields.into(),
            },
        ))
    }

    /// The kinds of the fields `new C(…)` produces at `n`: the constructor evaluated with this
    /// site's argument kinds.
    fn ctor_field_kinds(&mut self, class: ClassId, n: &'a NewExpression<'a>) -> Vec<Kind> {
        let fallback = || -> Vec<Kind> { Vec::new() };
        if self.lw.ctor_depth > 4 {
            return self
                .lw
                .env
                .class_fields
                .get(&class)
                .cloned()
                .unwrap_or_else(fallback);
        }
        // Argument kinds, lowered again without keeping anything.
        let mut kinds = Vec::new();
        let saved = self.reasons.len();
        for argument in &n.arguments {
            let kind = argument
                .as_expression()
                .and_then(|e| self.expr(e))
                .map_or(Kind::Error, |v| v.kind);
            kinds.push(kind);
        }
        self.reasons.truncate(saved);
        let ctor = self.lw.classes[&class].ctor;
        self.lw.ctor_depth += 1;
        let changed = self.lw.env.changed;
        let mut cx = FnCx::new(self.lw, ctor, Some(kinds));
        let _ = cx.run();
        let produced = cx.ctor_kinds.take();
        self.lw.env.changed = changed;
        self.lw.ctor_depth -= 1;
        produced.unwrap_or_default()
    }

    fn new_typed_array(&mut self, n: &'a NewExpression<'a>, kind: TypedArrayKind) -> Option<V> {
        let fact = self.facts().ty(self.node(n.node_id()));
        if !self.facts().is_usable(fact.trust) || *fact.value != Type::TypedArray(kind) {
            return self.fail(
                n.span,
                format!("can't prove `{}` is the builtin", kind.constructor_name()),
            );
        }
        let Some(element) = typed_element(kind) else {
            return self.fail(n.span, "creates a bigint array");
        };
        let [argument] = n.arguments.as_slice() else {
            return self.fail(
                n.span,
                "calls a typed array constructor with other than one argument",
            );
        };
        let Some(length) = argument.as_expression() else {
            return self.fail(n.span, "spreads arguments");
        };
        let (len, exact) = self.array_length(length)?;
        let elem = Elem::Typed(element);
        Some(V::new(
            Expr::NewArray {
                elem,
                len: Box::new(len),
                exact,
            },
            Ty::Array(ArrayTy {
                elem,
                access: Access::Fresh,
            }),
            Kind::Array {
                elem,
                fresh: true,
                params: 0,
            },
        ))
    }

    /// A new array's length: exact (an [`Ty::Index`]) when it is another array's length, a
    /// constant or proven a valid index; otherwise an `f64` for `ToIndex`.
    fn array_length(&mut self, length: &'a Expression<'a>) -> Option<(Expr, bool)> {
        if let Expression::StaticMemberExpression(member) = length.get_inner_expression()
            && member.property.name == "length"
        {
            let array = self.expr(&member.object)?;
            if matches!(array.kind, Kind::Array { .. }) {
                return Some((Expr::Len(Box::new(array.expr), Ty::Index), true));
            }
        }
        let value = self.number(length)?;
        let proven = matches!(value.kind, Kind::Num { undef: false })
            && self.expr_fact(length).is_some_and(is_index);
        if proven {
            let expr = match value.ty {
                Ty::Index => value.expr,
                from => convert(Conv::Exact, from, Ty::Index, value.expr),
            };
            return Some((expr, true));
        }
        Some((self.coerce(value, Ty::F64, false, length.span())?, false))
    }

    /// `new Array(n).fill(0)`.
    fn new_number_array(
        &mut self,
        c: &'a CallExpression<'a>,
        new: &'a NewExpression<'a>,
    ) -> Option<V> {
        let fact = self.facts().ty(self.node(c.node_id()));
        if !self.facts().is_usable(fact.trust) || !matches!(fact.value, Type::Array(_)) {
            return self.fail(c.span, "can't prove `Array` and `.fill` are the builtins");
        }
        let zero = matches!(c.arguments.as_slice(), [arg] if matches!(
            arg.as_expression().map(Expression::get_inner_expression),
            Some(Expression::NumericLiteral(n)) if n.value == 0.0 && n.value.is_sign_positive()
        ));
        if !zero {
            return self.fail(c.span, "fills a new array with something other than `0`");
        }
        let [length] = new.arguments.as_slice() else {
            return self.fail(new.span, "calls `new Array` with other than one argument");
        };
        let length = length.as_expression()?;
        let (len, exact) = self.array_length(length)?;
        if !exact {
            return self.fail(
                length.span(),
                "creates an array whose length the analysis can't prove valid (`new Array` throws \
                 on fractional and negative lengths)",
            );
        }
        Some(V::new(
            Expr::NewArray {
                elem: Elem::Number,
                len: Box::new(len),
                exact: true,
            },
            Ty::Array(ArrayTy {
                elem: Elem::Number,
                access: Access::Fresh,
            }),
            Kind::Array {
                elem: Elem::Number,
                fresh: true,
                params: 0,
            },
        ))
    }

    pub(super) fn static_member(&mut self, m: &'a StaticMemberExpression<'a>) -> Option<V> {
        self.member_read(m, false)
    }

    /// Reads `m`; `target` when it is also assigned (`p.x += 1`), which has no type fact of
    /// its own: the field's other reads must prove it.
    pub(super) fn member_read(
        &mut self,
        m: &'a StaticMemberExpression<'a>,
        target: bool,
    ) -> Option<V> {
        if m.optional {
            return self.fail(m.span, "uses optional chaining");
        }
        let name = m.property.name.as_str();
        let namespace = if self.is_global(&m.object, "Math") {
            Some("Math")
        } else if self.is_global(&m.object, "Number") {
            Some("Number")
        } else {
            None
        };
        if let Some(namespace) = namespace {
            // The fact proves the namespace is the builtin; the value is the spec's.
            if let (Some(_), Some(value)) = (
                self.num_fact(m.node_id()),
                builtin_constant(namespace, name),
            ) {
                return Some(number_lit(value));
            }
            return self.fail(
                m.span,
                format!("reads `{namespace}.{name}`, which the backend doesn't know"),
            );
        }
        let object = self.expr(&m.object)?;
        match object.kind.clone() {
            Kind::Array { .. } if name == "length" => {
                let ty = if self.num_fact(m.node_id()).is_some_and(is_int32) {
                    Ty::I32
                } else {
                    Ty::F64
                };
                Some(V::new(Expr::Len(Box::new(object.expr), ty), ty, Kind::NUM))
            }
            Kind::Value { key, fields } => {
                let names = match key {
                    StructKey::Class(class) => {
                        self.lw.classes.get(&class).map(|m| m.fields.clone())
                    }
                    StructKey::Literal(site) => {
                        self.lw.env.literals.get(&site).map(|m| m.fields.clone())
                    }
                }
                .unwrap_or_default();
                let Some(index) = names.iter().position(|n| n == name) else {
                    return self.fail(
                        m.span,
                        format!("reads `.{name}`, which the object doesn't have"),
                    );
                };
                let kind = fields.get(index).cloned().unwrap_or(Kind::Bottom);
                let joined = match key {
                    StructKey::Class(class) => self
                        .lw
                        .env
                        .class_fields
                        .get(&class)
                        .and_then(|f| f.get(index).cloned()),
                    StructKey::Literal(site) => self
                        .lw
                        .env
                        .literals
                        .get(&site)
                        .and_then(|m| m.kinds.get(index).cloned()),
                }
                .unwrap_or(Kind::Bottom);
                let ty = self.lw.value_field_ty(&joined);
                Some(V::new(
                    Expr::Field(Box::new(object.expr), name.to_string()),
                    ty,
                    kind,
                ))
            }
            Kind::ParamStruct { group } if target => {
                let read = self.lw.env.groups[group].field_mut(name).read;
                let kind = if read == Some(FieldRead::Bool) {
                    Kind::Bool
                } else {
                    Kind::NUM
                };
                if self.final_round() && read.is_none() {
                    return self.fail(
                        m.span,
                        format!("can't prove `.{name}` is a number or boolean (no read of it has a type fact)"),
                    );
                }
                let ty = self.group_field_ty(group, name);
                Some(V::new(
                    Expr::Field(Box::new(object.expr), name.to_string()),
                    ty,
                    kind,
                ))
            }
            Kind::ParamStruct { group } => {
                let node = self.node(m.node_id());
                if self.facts().call_graph().callees(node).next().is_some() {
                    return self.fail(
                        m.span,
                        format!("reads `.{name}`, which may run an accessor"),
                    );
                }
                // The analysis may know the field at function entry (`ExtraFacts`); otherwise
                // this read's own fact must prove it.
                let param_symbol = match &object.expr {
                    Expr::Local(local) => self.info[local.0].symbol,
                    _ => None,
                };
                let entry_fact =
                    param_symbol.and_then(|symbol| self.lw.extra.param_field(symbol, name));
                let fact = match &entry_fact {
                    Some(fact) if self.facts().is_usable(fact.trust) => fact.as_ref(),
                    _ => self.facts().ty(node),
                };
                let read = match fact.value {
                    Type::Number(n) if self.facts().is_usable(fact.trust) => {
                        Some(FieldRead::Number(*n))
                    }
                    Type::Boolean if self.facts().is_usable(fact.trust) => Some(FieldRead::Bool),
                    _ => None,
                };
                let Some(read) = read else {
                    return self.fail(
                        m.span,
                        format!(
                            "can't prove `.{name}` is always a number or a boolean{}",
                            trust_hint(fact.trust)
                        ),
                    );
                };
                let entry = self.lw.env.groups[group].field_mut(name);
                let joined = match (entry.read, read) {
                    (None, read) => Some(read),
                    (Some(FieldRead::Number(a)), FieldRead::Number(b)) => {
                        Some(FieldRead::Number(a.join(b)))
                    }
                    (Some(FieldRead::Bool), FieldRead::Bool) => Some(FieldRead::Bool),
                    _ => None,
                };
                let Some(joined) = joined else {
                    return self.fail(
                        m.span,
                        format!("`.{name}` is read as both a number and a boolean"),
                    );
                };
                if entry.read != Some(joined) {
                    entry.read = Some(joined);
                    self.lw.env.changed = true;
                }
                entry.reads = entry.reads.max(1);
                let ty = self.group_field_ty(group, name);
                let kind = if read == FieldRead::Bool {
                    Kind::Bool
                } else {
                    Kind::NUM
                };
                Some(V::new(
                    Expr::Field(Box::new(object.expr), name.to_string()),
                    ty,
                    kind,
                ))
            }
            other => self.fail(m.span, format!("reads `.{name}` of {}", other.describe())),
        }
    }

    fn element(&mut self, m: &'a ComputedMemberExpression<'a>) -> Option<V> {
        if m.optional {
            return self.fail(m.span, "uses optional chaining");
        }
        let array = self.expr(&m.object)?;
        let Kind::Array { elem, .. } = array.kind else {
            let message = format!(
                "reads a computed property of {} (dynamic keys)",
                array.kind.describe()
            );
            return self.fail(m.span, message);
        };
        let index = self.index(&m.expression, m.node_id())?;
        let local = self.proven_in_bounds(&m.object, &m.expression);
        let proven_number = match elem {
            Elem::Typed(_) => true,
            Elem::Number => {
                let fact = self.facts().ty(self.node(m.node_id()));
                // A `number[]` created in the island has no holes.
                (self.facts().is_usable(fact.trust) && matches!(fact.value, Type::Number(_)))
                    || matches!(
                        array.kind,
                        Kind::Array {
                            fresh: true,
                            params: 0,
                            ..
                        }
                    )
            }
        };
        let defined = (index.in_bounds || local) && proven_number;
        let int = index.in_bounds
            && defined
            && matches!(
                elem,
                Elem::Typed(
                    TypedArrayElement::I8
                        | TypedArrayElement::U8
                        | TypedArrayElement::U8Clamped
                        | TypedArrayElement::I16
                        | TypedArrayElement::U16
                        | TypedArrayElement::I32
                )
            );
        let get = Expr::Get {
            array: Box::new(array.expr),
            index: Box::new(index),
        };
        if int {
            return Some(V::new(
                convert(Conv::Exact, Ty::F64, Ty::I32, get),
                Ty::I32,
                Kind::NUM,
            ));
        }
        Some(V::new(get, Ty::F64, Kind::Num { undef: !defined }))
    }

    /// An array index; in bounds if the analysis proves it for the access at `node`.
    pub(super) fn index(&mut self, expression: &'a Expression<'a>, node: NodeId) -> Option<Index> {
        let value = self.number(expression)?;
        let fact = self.facts().in_bounds(self.node(node));
        let in_bounds = fact.value && self.facts().is_usable(fact.trust);
        Some(Index {
            value: value.expr,
            ty: value.ty,
            in_bounds,
        })
    }

    fn object(&mut self, o: &'a ObjectExpression<'a>) -> Option<V> {
        let site = self.node(o.node_id());
        let mut fields = Vec::new();
        let mut values = Vec::new();
        for property in &o.properties {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                return self.fail(o.span, "spreads an object");
            };
            let name = match &property.key {
                PropertyKey::StaticIdentifier(id) if !property.computed => id.name.to_string(),
                _ => {
                    return self.fail(
                        property.span,
                        "uses a computed or non-identifier property key",
                    );
                }
            };
            if property.kind != PropertyKind::Init || property.method {
                return self.fail(property.span, "defines a getter, setter or method");
            }
            if fields.contains(&name) {
                return self.fail(property.span, format!("defines `{name}` twice"));
            }
            let value = self.expr(&property.value)?;
            match &value.kind {
                Kind::Num { .. } | Kind::Bool | Kind::Value { .. } => {}
                other => {
                    let message = format!("stores {} in the property `{name}`", other.describe());
                    return self.fail(property.span, message);
                }
            }
            fields.push(name);
            values.push(value);
        }
        if fields.is_empty() {
            return self.fail(o.span, "creates an empty object");
        }
        let kinds: Vec<Kind> = values.iter().map(|v| v.kind.clone()).collect();
        {
            let model = self.lw.env.literals.entry(site).or_default();
            if model.fields.is_empty() {
                model.fields.clone_from(&fields);
                model.kinds = vec![Kind::Bottom; fields.len()];
                self.lw.env.changed = true;
            }
            let mut changed = false;
            for (slot, kind) in model.kinds.iter_mut().zip(&kinds) {
                super::super::Env::join_into(&mut changed, slot, kind);
            }
            self.lw.env.changed |= changed;
        }
        let struct_ref = self.struct_ref(StructSlot::Key(StructKey::Literal(site)));
        let mut lit = Vec::new();
        for (name, value) in fields.iter().zip(values) {
            let ty = if self.final_round() {
                self.lw.structs[struct_ref.0]
                    .fields
                    .iter()
                    .find(|(n, _)| n == name)
                    .map_or(Ty::F64, |(_, ty)| *ty)
            } else {
                Ty::F64
            };
            lit.push((name.clone(), self.coerce(value, ty, false, o.span)?));
        }
        Some(V::new(
            Expr::StructLit(struct_ref, lit),
            Ty::Struct(struct_ref),
            Kind::Value {
                key: StructKey::Literal(site),
                fields: kinds.into(),
            },
        ))
    }

    // ------------------------------------------------------------------------ conversions

    /// Converts `value` to `ty`. Narrowing a number to `i32`/`u32` needs `proven`: the
    /// analysis proved every value that reaches here fits.
    pub(super) fn coerce(&mut self, value: V, ty: Ty, proven: bool, span: Span) -> Option<Expr> {
        if value.ty == ty {
            return Some(value.expr);
        }
        if !self.final_round() {
            return Some(value.expr);
        }
        match (value.ty, ty) {
            (Ty::I32 | Ty::Index, Ty::F64) if matches!(value.expr, Expr::Len(..)) => {
                let Expr::Len(array, _) = value.expr else {
                    unreachable!("matched")
                };
                Some(Expr::Len(array, Ty::F64))
            }
            (Ty::I32 | Ty::U32 | Ty::Index, Ty::F64) => {
                Some(convert(Conv::Exact, value.ty, Ty::F64, value.expr))
            }
            (Ty::F64 | Ty::I32 | Ty::U32 | Ty::Index, Ty::I32 | Ty::U32 | Ty::Index) if proven => {
                Some(convert(Conv::Exact, value.ty, ty, value.expr))
            }
            (Ty::Array(from), Ty::Array(to)) if from.elem == to.elem => {
                match (from.access, to.access) {
                    (_, Access::Shared) | (Access::Fresh | Access::Mut, Access::Mut) => {
                        Some(value.expr)
                    }
                    _ => self.fail(span, "writes to an array it only borrows"),
                }
            }
            (Ty::StructRef(a, Access::Mut), Ty::StructRef(b, Access::Shared)) if a == b => {
                Some(value.expr)
            }
            (Ty::Struct(a), Ty::Struct(b)) if a == b => Some(value.expr),
            _ => self.fail(
                span,
                format!(
                    "{} ({:?} as {:?})",
                    super::super::TYPE_MISMATCH,
                    value.ty,
                    ty
                ),
            ),
        }
    }
}

/// A number literal, as `i32` or `u32` when it is one (never for `-0`).
pub(super) fn number_lit(value: f64) -> V {
    let int = value.fract() == 0.0 && !(value == 0.0 && value.is_sign_negative());
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "range checked"
    )]
    let lit = if int && (-2_147_483_648.0..=2_147_483_647.0).contains(&value) {
        Lit::I32(value as i32)
    } else if int && (0.0..=4_294_967_295.0).contains(&value) {
        Lit::U32(value as u32)
    } else {
        Lit::F64(value)
    };
    V::new(Expr::Lit(lit), lit.ty(), Kind::NUM)
}

pub(super) fn convert(conv: Conv, from: Ty, to: Ty, arg: Expr) -> Expr {
    if conv == Conv::Exact
        && let Expr::Lit(lit) = &arg
    {
        let value = match *lit {
            Lit::I32(v) => f64::from(v),
            Lit::U32(v) => f64::from(v),
            Lit::F64(v) => v,
            Lit::Bool(_) => {
                return Expr::Convert {
                    conv,
                    from,
                    to,
                    arg: Box::new(arg),
                };
            }
        };
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "exact by construction"
        )]
        match to {
            Ty::F64 => return Expr::Lit(Lit::F64(value)),
            Ty::I32 if from != Ty::U32 => return Expr::Lit(Lit::I32(value as i32)),
            Ty::U32 if from != Ty::I32 => return Expr::Lit(Lit::U32(value as u32)),
            _ => {}
        }
    }
    Expr::Convert {
        conv,
        from,
        to,
        arg: Box::new(arg),
    }
}

/// The bits of an `i32`/`u32` as the other type (exact modulo 2^32).
fn reinterpret(value: V, to: Ty) -> Expr {
    if value.ty == to {
        value.expr
    } else {
        convert(Conv::Exact, value.ty, to, value.expr)
    }
}

fn zero_of(ty: Ty) -> Expr {
    match ty {
        Ty::U32 => Expr::Lit(Lit::U32(0)),
        Ty::Index => convert(Conv::Exact, Ty::I32, Ty::Index, Expr::Lit(Lit::I32(0))),
        _ => Expr::Lit(Lit::I32(0)),
    }
}

/// The value of a constant property of `Math` or `Number` (non-writable in every engine).
fn builtin_constant(namespace: &str, name: &str) -> Option<f64> {
    use std::f64::consts;
    Some(match (namespace, name) {
        ("Math", "PI") => consts::PI,
        ("Math", "E") => consts::E,
        ("Math", "LN2") => consts::LN_2,
        ("Math", "LN10") => consts::LN_10,
        ("Math", "LOG2E") => consts::LOG2_E,
        ("Math", "LOG10E") => consts::LOG10_E,
        ("Math", "SQRT2") => consts::SQRT_2,
        ("Math", "SQRT1_2") => consts::FRAC_1_SQRT_2,
        ("Number", "MAX_SAFE_INTEGER") => 9_007_199_254_740_991.0,
        ("Number", "MIN_SAFE_INTEGER") => -9_007_199_254_740_991.0,
        ("Number", "EPSILON") => f64::EPSILON,
        ("Number", "MAX_VALUE") => f64::MAX,
        ("Number", "MIN_VALUE") => 5e-324,
        ("Number", "POSITIVE_INFINITY") => f64::INFINITY,
        ("Number", "NEGATIVE_INFINITY") => f64::NEG_INFINITY,
        ("Number", "NaN") => f64::NAN,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_math_matches_the_runtime() {
        const ALGEBRAIC: &[&str] = &[
            "abs", "floor", "ceil", "trunc", "sqrt", "round", "sign", "fround",
        ];
        for (name, _, _) in EXACT_MATH {
            assert!(
                ALGEBRAIC.contains(name) || trioxide_rt::math::EXACT_TRANSCENDENTALS.contains(name),
                "{name} isn't exact in trioxide_rt"
            );
        }
        for name in trioxide_rt::math::EXACT_TRANSCENDENTALS {
            assert!(
                EXACT_MATH.iter().any(|(n, _, _)| n == name),
                "{name} is exact in trioxide_rt but not lowered"
            );
        }
    }
}
