//! PLAN §4 pass 8: scalar replacement.
//!
//! `const p = { x: a, y: b };` whose object never leaves the function becomes one local per
//! key (`let p_x = a, p_y = b;`), and every `p.x` the local `p_x`. Tuples (`const t = [a, b]`)
//! work the same way (`t[0]` → `t_0`).
//!
//! **Facts.** The literal's allocation site must be `escapes = None` and have a stable shape
//! (no key added or deleted later), and every read and write of a key must go through a value
//! whose `points_to` is exactly that site.
//!
//! **Syntax.** The declaration is a `const` or `let` in a statement list, outside WASM island
//! functions and code proven never to run. The literal has plain `key: value` properties with
//! literal keys (no accessors, methods, spread, computed keys or `__proto__`), or array
//! elements without holes or spread. Every reference to the binding, in the same function and
//! after its declaration, is a read, assignment, compound assignment or update of one of those
//! keys: `p.k` or `p["k"]`, never a callee (`p.m()` passes `p` as `this`), a `delete` operand,
//! a destructuring target or optional chain. So the object is never compared by identity, used
//! with `in`, spread, `JSON`, `Object.*` or `for…in`, or passed anywhere: all of those use the
//! binding itself. A key the literal doesn't have would be read through the prototype, so a
//! use of one is rejected too (and with all keys set at creation, no key is read before it is
//! written).
//!
//! **Instances.** `const p = new C(a, b)` works the same way when `C` is a class declared in
//! the module ([`ClassPlan`]): no heritage, decorators or instance accessors, fields with
//! literal initialisers (or none), and a constructor that only assigns parameters or literals
//! to `this.key` (each key and parameter once; no defaults, rest or parameter properties). The
//! locals are the assigned arguments in argument order, then the other keys' values; arguments
//! the constructor ignores must be pure (they are dropped). The site must be a non-escaping
//! `Instance` of exactly that class with a stable shape, and the callee must point to exactly
//! the class and be pure (so it isn't read in its temporal dead zone). Method calls are
//! rejected as for literals, and a read of a method (no field of that name) as an unknown key.
//!
//! Off by default (`passes: { "scalar": true }` turns it on): V8's optimising tier already
//! replaces such objects in hot code, so the measured wins are in code that isn't hot yet.
//!
//! The binding is never reassigned (`let` too). The locals are `const` unless a key is
//! written, and their names are the binding's and the key's (`p_x`), fresh where taken.

use oxc_allocator::{ArenaVec, TakeIn};
use oxc_ast::ast::{
    ArrayExpressionElement, AssignmentTarget, BindingPattern, ComputedMemberExpression, Expression,
    Ident, IdentifierReference, ObjectPropertyKind, Program, PropertyKey, PropertyKind,
    SimpleAssignmentTarget, Statement, StaticMemberExpression, VariableDeclaration,
    VariableDeclarationKind, VariableDeclarator,
};
use oxc_ast::builder::AstBuilder;
use oxc_ast_visit::{Visit, VisitMut, walk, walk_mut};
use oxc_semantic::Scoping;
use oxc_span::Span;
use oxc_syntax::{scope::ScopeFlags, symbol::SymbolId};
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_analysis::{AllocKind, Escape};
use trioxide_common::NodeId;

use super::{
    Decisions,
    speed::{Namer, is_anonymous, is_island, rescope},
    util::{Query, has_dynamic_scope},
};
use crate::{Pass, PassContext};

/// PLAN §4 pass 8 (`"scalar"`), a speed pass of the foundation group.
pub struct Scalar;

impl Pass for Scalar {
    fn name(&self) -> &'static str {
        "scalar"
    }

    /// Off unless `passes` turns it on: V8's optimising tier already inlines and replaces
    /// non-escaping objects in hot code, where this pass measured no faster (PLAN §4).
    fn enabled_by_default(&self) -> bool {
        false
    }

    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
        let query = Query {
            facts: ctx.facts,
            module: ctx.module,
        };
        if has_dynamic_scope(program, ctx.scoping()) {
            return false;
        }
        let mut decisions = Decisions::new(self.name());
        let tagged = super::wasm_tags(program);
        let mut classes = Classes {
            scoping: ctx.scoping(),
            plans: FxHashMap::default(),
        };
        classes.visit_program(program);
        let classes = classes.plans;
        let mut find = Find {
            query,
            scoping: ctx.scoping(),
            keep: ctx.keep,
            tagged: &tagged,
            classes: &classes,
            candidates: FxHashMap::default(),
            functions: Vec::new(),
            dead: 0,
            opaque: 0,
            target: 0,
        };
        find.visit_program(program);
        let mut candidates = find.candidates;
        let mut accepted: FxHashMap<SymbolId, Replacement> = FxHashMap::default();
        let mut namer = Namer::new(ctx.scoping());
        let mut sorted: Vec<_> = candidates.drain().collect();
        sorted.sort_by_key(|(_, candidate)| candidate.span.start);
        for (symbol, candidate) in sorted {
            if let Some(reason) = candidate.rejected {
                decisions.skipped(candidate.span, reason);
                continue;
            }
            if let Some(reason) =
                Self::facts_problem(&candidate, query, ctx.scoping(), symbol, &classes)
            {
                decisions.skipped(candidate.span, reason);
                continue;
            }
            let base = ctx.scoping().symbol_name(symbol).to_string();
            let scope = ctx.scoping().symbol_scope_id(symbol);
            let names = candidate
                .keys
                .iter()
                .map(|key| {
                    (
                        key.clone(),
                        namer.fresh(&format!("{base}_{key}"), scope, ctx.scoping()),
                    )
                })
                .collect();
            decisions.applied(candidate.span);
            accepted.insert(
                symbol,
                Replacement {
                    names,
                    written: candidate.written,
                    class: candidate.instance.as_ref().map(|(class, _, _)| *class),
                },
            );
        }
        if accepted.is_empty() {
            return decisions.flush(ctx.log);
        }
        let mut rewrite = Rewrite {
            scoping: ctx.scoping(),
            accepted: &accepted,
            classes: &classes,
            ast: AstBuilder::new(ctx.allocator),
        };
        rewrite.visit_program(program);
        *ctx.scoping_mut() = rescope(program);
        decisions.flush(ctx.log)
    }
}

impl Scalar {
    /// Why the facts don't allow replacing the candidate, if they don't.
    fn facts_problem(
        candidate: &Candidate,
        query: Query<'_>,
        scoping: &Scoping,
        symbol: SymbolId,
        classes: &FxHashMap<SymbolId, Result<ClassPlan, &'static str>>,
    ) -> Option<String> {
        if scoping.symbol_is_mutated(symbol) {
            return Some("the binding is reassigned".into());
        }
        let Some(site) = query.node(candidate.site) else {
            return Some("the literal has no facts (an earlier pass made it)".into());
        };
        let Some(allocation) = query.facts.allocation(site) else {
            return Some("the analysis has no facts about the literal".into());
        };
        if let Some((class, callee, ignored)) = &candidate.instance {
            if let Some(reason) = Self::instance_problem(query, classes, *class, *callee, ignored) {
                return Some(reason);
            }
            let class = classes[class]
                .as_ref()
                .ok()
                .and_then(|plan| query.node(plan.node));
            if !class.is_some_and(|class| allocation.kind == AllocKind::Instance(class)) {
                return Some("the analysis doesn't see an instance of the class here".into());
            }
        } else if !matches!(allocation.kind, AllocKind::Object | AllocKind::Array) {
            return Some("the analysis doesn't see a plain object or array here".into());
        }
        if !query.usable(allocation.escape.trust) || allocation.escape.value != Escape::None {
            return Some(format!(
                "the object escapes the function ({})",
                match allocation.escape.value {
                    Escape::None => "unknown",
                    Escape::Known => "other code holds it",
                    Escape::Unknown => "code trioxide doesn't see may hold it",
                }
            ));
        }
        if !(allocation.stable_shape.value && query.usable(allocation.stable_shape.trust)) {
            return Some("the object's shape changes after creation".into());
        }
        for &object in &candidate.objects {
            let points = query.node(object).map(|node| query.facts.points_to(node));
            let exact = points.is_some_and(|fact| {
                query.usable(fact.trust) && fact.value.is_some_and(|sites| sites == [site])
            });
            if !exact {
                return Some(
                    "a use may see another object (points-to isn't exactly the literal)".into(),
                );
            }
        }
        None
    }

    /// Why the facts don't allow replacing `new C(…)` by its fields, if they don't: the callee
    /// must be exactly the class (and its read can't throw: not in its temporal dead zone),
    /// and the arguments the constructor ignores must be pure.
    fn instance_problem(
        query: Query<'_>,
        classes: &FxHashMap<SymbolId, Result<ClassPlan, &'static str>>,
        class: SymbolId,
        callee: NodeId,
        ignored: &[NodeId],
    ) -> Option<String> {
        let plan = classes[&class].as_ref().ok()?;
        let class = query.node(plan.node)?;
        let callee = query.node(callee)?;
        let points = query.facts.points_to(callee);
        if !(query.usable(points.trust) && points.value.is_some_and(|sites| sites == [class])) {
            return Some("the callee may be something other than the class".into());
        }
        let pure = query.facts.is_pure(callee);
        if !(pure.value && query.usable(pure.trust)) {
            return Some("reading the class may throw (its declaration may not have run)".into());
        }
        for &argument in ignored {
            let pure = query.node(argument).map(|node| query.facts.is_pure(node));
            if !pure.is_some_and(|pure| pure.value && query.usable(pure.trust)) {
                return Some("an argument the constructor ignores may have effects".into());
            }
        }
        None
    }
}

/// A declaration that may become locals.
struct Candidate {
    /// The literal (its allocation site).
    site: NodeId,
    span: Span,
    keys: Vec<String>,
    /// The identifier nodes that read or write a key.
    objects: Vec<NodeId>,
    written: bool,
    /// The function the declaration is in (`None`: the module).
    function: Option<NodeId>,
    rejected: Option<String>,
    /// For `new C(…)`: the class's binding, the callee, and the arguments the constructor
    /// ignores (they are dropped, so they must be pure).
    instance: Option<(SymbolId, NodeId, Vec<NodeId>)>,
}

/// The names a replaced binding's keys get.
struct Replacement {
    names: FxHashMap<String, String>,
    written: bool,
    /// For `new C(…)`: the class's binding.
    class: Option<SymbolId>,
}

/// The keys of a literal, if it has the plain form.
fn literal_keys(init: &Expression<'_>) -> Result<Vec<String>, &'static str> {
    let mut keys = Vec::new();
    match init {
        Expression::ObjectExpression(object) => {
            for property in &object.properties {
                let ObjectPropertyKind::ObjectProperty(property) = property else {
                    return Err("the literal spreads another object");
                };
                if property.kind != PropertyKind::Init || property.method {
                    return Err("the literal has accessors or methods");
                }
                if property.computed {
                    return Err("the literal has a computed key");
                }
                let key = static_key(&property.key).ok_or("the literal has an unusual key")?;
                if key == "__proto__" {
                    return Err("the literal sets `__proto__`");
                }
                if keys.contains(&key) {
                    return Err("the literal repeats a key");
                }
                if is_anonymous(&property.value) {
                    // It is named after its key; a local would give it the local's name.
                    return Err("a key holds an anonymous function or class (named after the key)");
                }
                keys.push(key);
            }
        }
        Expression::ArrayExpression(array) => {
            for (index, element) in array.elements.iter().enumerate() {
                match element {
                    ArrayExpressionElement::SpreadElement(_) => {
                        return Err("the array literal spreads");
                    }
                    ArrayExpressionElement::Elision(_) => {
                        return Err("the array literal has holes");
                    }
                    element => {
                        if element.as_expression().is_some_and(is_anonymous) {
                            // `[() => 1][0].name` is empty; a local would give it its name.
                            return Err("an element is an anonymous function or class");
                        }
                        keys.push(index.to_string());
                    }
                }
            }
        }
        _ => return Err("not a literal"),
    }
    Ok(keys)
}

/// A property key as the string it is, for identifiers, strings and small integers.
fn static_key(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(ident) => Some(ident.name.to_string()),
        PropertyKey::StringLiteral(literal) if !literal.lone_surrogates => {
            Some(literal.value.to_string())
        }
        PropertyKey::NumericLiteral(literal) => integer_key(literal.value),
        _ => None,
    }
}

/// The canonical key of an array index or integer-named property.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked range"
)]
fn integer_key(value: f64) -> Option<String> {
    (value.fract() == 0.0 && (0.0..1e9).contains(&value) && !value.is_sign_negative())
        .then(|| (value as u64).to_string())
}

/// The key a computed member's expression names, if it is a literal.
fn computed_key(expression: &Expression<'_>) -> Option<String> {
    match expression.without_parentheses() {
        Expression::StringLiteral(literal) if !literal.lone_surrogates => {
            Some(literal.value.to_string())
        }
        Expression::NumericLiteral(literal) => integer_key(literal.value),
        _ => None,
    }
}

/// A constant a field starts with.
#[derive(Clone)]
enum Constant {
    Number { value: f64, negative: bool },
    String(String),
    Boolean(bool),
    Null,
    Undefined,
}

impl Constant {
    fn of(expression: &Expression<'_>) -> Option<Self> {
        Some(match expression.get_inner_expression() {
            Expression::NumericLiteral(literal) => Self::Number {
                value: literal.value,
                negative: false,
            },
            Expression::UnaryExpression(unary)
                if unary.operator == oxc_syntax::operator::UnaryOperator::UnaryNegation =>
            {
                let Expression::NumericLiteral(literal) = unary.argument.get_inner_expression()
                else {
                    return None;
                };
                Self::Number {
                    value: literal.value,
                    negative: true,
                }
            }
            Expression::StringLiteral(literal) if !literal.lone_surrogates => {
                Self::String(literal.value.to_string())
            }
            Expression::BooleanLiteral(literal) => Self::Boolean(literal.value),
            Expression::NullLiteral(_) => Self::Null,
            _ => return None,
        })
    }

    fn expression<'a>(&self, span: Span, ast: &AstBuilder<'a>) -> Expression<'a> {
        use oxc_syntax::{number::NumberBase, operator::UnaryOperator};
        let number =
            |value| Expression::new_numeric_literal(span, value, None, NumberBase::Decimal, ast);
        match self {
            Self::Number { value, negative } => {
                if *negative {
                    Expression::new_unary_expression(
                        span,
                        UnaryOperator::UnaryNegation,
                        number(*value),
                        ast,
                    )
                } else {
                    number(*value)
                }
            }
            Self::String(value) => Expression::new_string_literal(
                span,
                Ident::from_str_in(value, ast).as_str(),
                None,
                ast,
            ),
            Self::Boolean(value) => Expression::new_boolean_literal(span, *value, ast),
            Self::Null => Expression::new_null_literal(span, ast),
            Self::Undefined => {
                Expression::new_unary_expression(span, UnaryOperator::Void, number(0.0), ast)
            }
        }
    }
}

/// Where an instance's key gets its value.
#[derive(Clone)]
enum Source {
    /// The constructor's parameter at this index.
    Parameter(usize),
    Constant(Constant),
}

/// A class whose instances `scalar` can replace: no heritage, no instance accessors, fields
/// with constant initialisers, and a constructor that only assigns parameters or constants to
/// `this.key`, each key and parameter at most once.
struct ClassPlan {
    /// The class (its allocation site).
    node: NodeId,
    /// The constructor's parameter count.
    parameters: usize,
    /// The instance keys in the order instances get them, and their values.
    keys: Vec<(String, Source)>,
}

impl ClassPlan {
    fn of(class: &oxc_ast::ast::Class<'_>, scoping: &Scoping) -> Result<Self, &'static str> {
        if class.heritage.is_some() {
            return Err("the class extends another");
        }
        if class.declare || class.r#abstract || !class.decorators.is_empty() {
            return Err("the class is declared, abstract or decorated");
        }
        let mut keys: Vec<(String, Source)> = Vec::new();
        let mut constructor = None;
        for element in &class.body.body {
            match element {
                oxc_ast::ast::ClassElement::MethodDefinition(method) => {
                    if !method.decorators.is_empty() {
                        return Err("a method is decorated");
                    }
                    match method.kind {
                        oxc_ast::ast::MethodDefinitionKind::Constructor => {
                            constructor = Some(&method.value);
                        }
                        oxc_ast::ast::MethodDefinitionKind::Method => {}
                        oxc_ast::ast::MethodDefinitionKind::Get
                        | oxc_ast::ast::MethodDefinitionKind::Set => {
                            if !method.r#static {
                                return Err("the class has instance accessors");
                            }
                        }
                    }
                }
                oxc_ast::ast::ClassElement::PropertyDefinition(property) => {
                    if property.r#static || property.declare {
                        continue;
                    }
                    if !property.decorators.is_empty() || property.computed {
                        return Err("a field is decorated or has a computed key");
                    }
                    let key = static_key(&property.key).ok_or("a field has an unusual key")?;
                    let value = match &property.value {
                        None => Constant::Undefined,
                        Some(value) => {
                            Constant::of(value).ok_or("a field's initialiser isn't a literal")?
                        }
                    };
                    if keys.iter().any(|(known, _)| *known == key) {
                        return Err("the class declares a field twice");
                    }
                    keys.push((key, Source::Constant(value)));
                }
                oxc_ast::ast::ClassElement::AccessorProperty(property) => {
                    if !property.r#static {
                        return Err("the class has `accessor` fields");
                    }
                }
                oxc_ast::ast::ClassElement::StaticBlock(_)
                | oxc_ast::ast::ClassElement::TSIndexSignature(_) => {}
            }
        }
        let Some(constructor) = constructor else {
            return Ok(Self {
                node: class.node_id(),
                parameters: 0,
                keys,
            });
        };
        let params = &constructor.params;
        if params.rest.is_some() {
            return Err("the constructor has a rest parameter");
        }
        let mut parameters = Vec::new();
        for param in &params.items {
            let BindingPattern::BindingIdentifier(id) = &param.pattern else {
                return Err("a constructor parameter is a pattern");
            };
            if param.initializer.is_some()
                || param.accessibility.is_some()
                || param.readonly
                || param.r#override
                || !param.decorators.is_empty()
            {
                return Err(
                    "a constructor parameter has a default, is a parameter property or is decorated",
                );
            }
            parameters.push(id.symbol_id());
        }
        let body = constructor
            .body
            .as_ref()
            .ok_or("the constructor has no body")?;
        if !body.directives.is_empty() {
            return Err("the constructor has directives");
        }
        let mut assigned: Vec<String> = Vec::new();
        let mut used = vec![false; parameters.len()];
        for statement in &body.statements {
            let assignment = match statement {
                Statement::ExpressionStatement(statement) => match &statement.expression {
                    Expression::AssignmentExpression(assignment)
                        if assignment.operator
                            == oxc_syntax::operator::AssignmentOperator::Assign =>
                    {
                        assignment
                    }
                    _ => return Err("the constructor does more than assign fields"),
                },
                _ => return Err("the constructor does more than assign fields"),
            };
            let AssignmentTarget::StaticMemberExpression(member) = &assignment.left else {
                return Err("the constructor does more than assign fields");
            };
            if !matches!(member.object, Expression::ThisExpression(_)) {
                return Err("the constructor does more than assign fields");
            }
            let key = member.property.name.to_string();
            if key == "__proto__" {
                return Err("the constructor sets `__proto__`");
            }
            if assigned.contains(&key) {
                return Err("the constructor assigns a key twice");
            }
            let source = match assignment.right.get_inner_expression() {
                Expression::Identifier(ident) => {
                    let symbol = ident
                        .reference_id
                        .get()
                        .and_then(|reference| scoping.get_reference(reference).symbol_id());
                    let index = symbol
                        .and_then(|symbol| parameters.iter().position(|&param| param == symbol))
                        .ok_or(
                            "the constructor assigns something other than a parameter or literal",
                        )?;
                    if std::mem::replace(&mut used[index], true) {
                        return Err("the constructor assigns a parameter twice");
                    }
                    Source::Parameter(index)
                }
                value => Source::Constant(Constant::of(value).ok_or(
                    "the constructor assigns something other than a parameter or literal",
                )?),
            };
            assigned.push(key.clone());
            match keys.iter_mut().find(|(known, _)| *known == key) {
                Some(entry) => entry.1 = source,
                None => keys.push((key, source)),
            }
        }
        Ok(Self {
            node: class.node_id(),
            parameters: parameters.len(),
            keys,
        })
    }

    /// The key a parameter is assigned to.
    fn parameter_key(&self, index: usize) -> Option<&str> {
        self.keys.iter().find_map(|(key, source)| {
            matches!(source, Source::Parameter(at) if *at == index).then_some(key.as_str())
        })
    }
}

/// The classes declared in a module, and whether each can be replaced.
struct Classes<'s> {
    scoping: &'s Scoping,
    plans: FxHashMap<SymbolId, Result<ClassPlan, &'static str>>,
}

impl<'a> Visit<'a> for Classes<'_> {
    fn visit_class(&mut self, class: &oxc_ast::ast::Class<'a>) {
        if class.r#type == oxc_ast::ast::ClassType::ClassDeclaration
            && let Some(id) = &class.id
        {
            self.plans
                .insert(id.symbol_id(), ClassPlan::of(class, self.scoping));
        }
        walk::walk_class(self, class);
    }
}

/// Finds candidates and checks every use of them.
struct Find<'s, 'q> {
    query: Query<'q>,
    scoping: &'s Scoping,
    keep: &'s crate::Keep,
    tagged: &'s FxHashSet<u32>,
    classes: &'s FxHashMap<SymbolId, Result<ClassPlan, &'static str>>,
    candidates: FxHashMap<SymbolId, Candidate>,
    /// Enclosing functions, and whether each is an island.
    functions: Vec<(NodeId, bool)>,
    dead: usize,
    opaque: usize,
    /// Inside a destructuring or `for…in/of` target.
    target: usize,
}

impl Find<'_, '_> {
    fn symbol_of(&self, ident: &IdentifierReference<'_>) -> Option<SymbolId> {
        ident
            .reference_id
            .get()
            .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
    }

    /// The candidate an expression (`p`) names, if it does.
    fn candidate_of(&self, object: &Expression<'_>) -> Option<(SymbolId, NodeId)> {
        let Expression::Identifier(ident) = object else {
            return None;
        };
        let symbol = self.symbol_of(ident)?;
        self.candidates
            .contains_key(&symbol)
            .then(|| (symbol, ident.node_id()))
    }

    fn reject(&mut self, symbol: SymbolId, reason: &str) {
        if let Some(candidate) = self.candidates.get_mut(&symbol)
            && candidate.rejected.is_none()
        {
            candidate.rejected = Some(reason.to_string());
        }
    }

    /// Records a use of `key` through the identifier `object` (a reference to `symbol`).
    fn key_use(
        &mut self,
        symbol: SymbolId,
        object: NodeId,
        span: Span,
        key: Option<&str>,
        write: bool,
    ) {
        let function = self.functions.last().map(|(node, _)| *node);
        let in_target = self.target > 0;
        let Some(candidate) = self.candidates.get_mut(&symbol) else {
            return;
        };
        let reason = if in_target {
            Some("a key is a destructuring or `for…of` target")
        } else if candidate.function != function {
            Some("a closure uses the object")
        } else if span.start < candidate.span.end {
            Some("the object is used before its declaration ends")
        } else {
            match key {
                Some(key) if candidate.keys.iter().any(|known| known == key) => None,
                Some(_) => Some(
                    "a key the literal doesn't have is used (it would be read through the prototype)",
                ),
                None => Some("a key is computed"),
            }
        };
        if let Some(reason) = reason {
            if candidate.rejected.is_none() {
                candidate.rejected = Some(reason.to_string());
            }
            return;
        }
        candidate.objects.push(object);
        candidate.written |= write;
    }

    /// Handles a member expression in a write position. Returns the candidate it writes, if any.
    fn member_target(
        &mut self,
        target: &SimpleAssignmentTarget<'_>,
        span: Span,
    ) -> Option<SymbolId> {
        match target {
            SimpleAssignmentTarget::StaticMemberExpression(member) => {
                if let Some((symbol, object)) = self.candidate_of(&member.object) {
                    let key = Some(member.property.name.as_str());
                    self.key_use(symbol, object, span, key, true);
                    return Some(symbol);
                }
            }
            SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                if let Some((symbol, object)) = self.candidate_of(&member.object) {
                    let key = computed_key(&member.expression);
                    self.key_use(symbol, object, span, key.as_deref(), true);
                    self.visit_expression(&member.expression);
                    return Some(symbol);
                }
            }
            _ => {}
        }
        None
    }
}

impl<'a> Visit<'a> for Find<'_, '_> {
    fn visit_statement(&mut self, statement: &Statement<'a>) {
        let dead = self.query.dead(statement.node_id());
        self.dead += usize::from(dead);
        walk::walk_statement(self, statement);
        self.dead -= usize::from(dead);
    }

    fn visit_ts_namespace_declaration(
        &mut self,
        declaration: &oxc_ast::ast::TSNamespaceDeclaration<'a>,
    ) {
        self.opaque += 1;
        walk::walk_ts_namespace_declaration(self, declaration);
        self.opaque -= 1;
    }

    fn visit_function(&mut self, function: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
        let island = is_island(
            self.keep,
            self.query.node(function.node_id()),
            super::forced_island(function, self.tagged, &[function.span.start]),
        ) || self.functions.last().is_some_and(|(_, island)| *island);
        let dead = self.query.dead(function.node_id());
        self.dead += usize::from(dead);
        self.functions.push((function.node_id(), island));
        walk::walk_function(self, function, flags);
        self.functions.pop();
        self.dead -= usize::from(dead);
    }

    fn visit_arrow_function_expression(
        &mut self,
        arrow: &oxc_ast::ast::ArrowFunctionExpression<'a>,
    ) {
        let island = is_island(self.keep, self.query.node(arrow.node_id()), false)
            || self.functions.last().is_some_and(|(_, island)| *island);
        let dead = self.query.dead(arrow.node_id());
        self.dead += usize::from(dead);
        self.functions.push((arrow.node_id(), island));
        walk::walk_arrow_function_expression(self, arrow);
        self.functions.pop();
        self.dead -= usize::from(dead);
    }

    fn visit_statements(&mut self, statements: &ArenaVec<'a, Statement<'a>>) {
        for statement in statements {
            if let Statement::VariableDeclaration(declaration) = statement {
                self.declaration(declaration);
            }
        }
        walk::walk_statements(self, statements);
    }

    fn visit_assignment_expression(&mut self, assignment: &oxc_ast::ast::AssignmentExpression<'a>) {
        let written = assignment
            .left
            .as_simple_assignment_target()
            .and_then(|target| self.member_target(target, assignment.span));
        if let Some(symbol) = written {
            if is_anonymous(&assignment.right) {
                // `p.f = () => 1` leaves the function unnamed; `p_f = () => 1` would name it.
                self.reject(symbol, "a key is assigned an anonymous function or class");
            }
        } else {
            self.target += usize::from(!matches!(
                assignment.left,
                AssignmentTarget::AssignmentTargetIdentifier(_)
                    | AssignmentTarget::StaticMemberExpression(_)
                    | AssignmentTarget::ComputedMemberExpression(_)
                    | AssignmentTarget::PrivateFieldExpression(_)
            ));
            self.visit_assignment_target(&assignment.left);
            self.target = self.target.saturating_sub(usize::from(!matches!(
                assignment.left,
                AssignmentTarget::AssignmentTargetIdentifier(_)
                    | AssignmentTarget::StaticMemberExpression(_)
                    | AssignmentTarget::ComputedMemberExpression(_)
                    | AssignmentTarget::PrivateFieldExpression(_)
            )));
        }
        self.visit_expression(&assignment.right);
    }

    fn visit_update_expression(&mut self, update: &oxc_ast::ast::UpdateExpression<'a>) {
        if self.member_target(&update.argument, update.span).is_none() {
            walk::walk_update_expression(self, update);
        }
    }

    fn visit_for_in_statement(&mut self, statement: &oxc_ast::ast::ForInStatement<'a>) {
        self.target += 1;
        self.visit_for_statement_left(&statement.left);
        self.target -= 1;
        self.visit_expression(&statement.right);
        self.visit_statement(&statement.body);
    }

    fn visit_for_of_statement(&mut self, statement: &oxc_ast::ast::ForOfStatement<'a>) {
        self.target += 1;
        self.visit_for_statement_left(&statement.left);
        self.target -= 1;
        self.visit_expression(&statement.right);
        self.visit_statement(&statement.body);
    }

    fn visit_call_expression(&mut self, call: &oxc_ast::ast::CallExpression<'a>) {
        if let Some(member) = call.callee.get_member_expr()
            && let Some((symbol, _)) = self.candidate_of(member.object())
        {
            self.reject(
                symbol,
                "a key is called as a method (which passes the object as `this`)",
            );
        }
        walk::walk_call_expression(self, call);
    }

    fn visit_tagged_template_expression(
        &mut self,
        tagged: &oxc_ast::ast::TaggedTemplateExpression<'a>,
    ) {
        if let Some(member) = tagged.tag.without_parentheses().get_member_expr()
            && let Some((symbol, _)) = self.candidate_of(member.object())
        {
            self.reject(
                symbol,
                "a key is a template tag (which passes the object as `this`)",
            );
        }
        walk::walk_tagged_template_expression(self, tagged);
    }

    fn visit_unary_expression(&mut self, unary: &oxc_ast::ast::UnaryExpression<'a>) {
        if unary.operator == oxc_syntax::operator::UnaryOperator::Delete
            && let Some(member) = unary.argument.without_parentheses().get_member_expr()
            && let Some((symbol, _)) = self.candidate_of(member.object())
        {
            self.reject(symbol, "a key is deleted");
        }
        walk::walk_unary_expression(self, unary);
    }

    fn visit_chain_expression(&mut self, chain: &oxc_ast::ast::ChainExpression<'a>) {
        if let Some(member) = chain.expression.as_member_expression()
            && let Some((symbol, _)) = self.candidate_of(member.object())
        {
            self.reject(symbol, "the object is used in an optional chain");
        }
        walk::walk_chain_expression(self, chain);
    }

    fn visit_static_member_expression(&mut self, member: &StaticMemberExpression<'a>) {
        if let Some((symbol, object)) = self.candidate_of(&member.object) {
            if member.optional {
                self.reject(symbol, "the object is used in an optional chain");
                return;
            }
            self.key_use(
                symbol,
                object,
                member.span,
                Some(member.property.name.as_str()),
                false,
            );
            return;
        }
        walk::walk_static_member_expression(self, member);
    }

    fn visit_computed_member_expression(&mut self, member: &ComputedMemberExpression<'a>) {
        if let Some((symbol, object)) = self.candidate_of(&member.object) {
            if member.optional {
                self.reject(symbol, "the object is used in an optional chain");
                return;
            }
            let key = computed_key(&member.expression);
            self.key_use(symbol, object, member.span, key.as_deref(), false);
            self.visit_expression(&member.expression);
            return;
        }
        walk::walk_computed_member_expression(self, member);
    }

    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        // Every use that reaches here is the object itself, not one of its keys.
        if let Some(symbol) = self.symbol_of(ident) {
            self.reject(
                symbol,
                "the object itself is used (compared, passed, spread or inspected)",
            );
        }
    }
}

impl Find<'_, '_> {
    /// Notes the plain-literal declarators of `declaration` as candidates.
    fn declaration(&mut self, declaration: &VariableDeclaration<'_>) {
        if !matches!(
            declaration.kind,
            VariableDeclarationKind::Const | VariableDeclarationKind::Let
        ) || declaration.declare
            || self.opaque > 0
        {
            return;
        }
        let function = self.functions.last().copied();
        if function.is_some_and(|(_, island)| island) {
            return;
        }
        for declarator in &declaration.declarations {
            let (BindingPattern::BindingIdentifier(id), Some(init)) =
                (&declarator.id, &declarator.init)
            else {
                continue;
            };
            if self.dead > 0 {
                // Facts in code that never runs are vacuous.
                continue;
            }
            let (keys, rejected, instance) = match init {
                Expression::ObjectExpression(object) if object.properties.is_empty() => continue,
                Expression::ArrayExpression(array) if array.elements.is_empty() => continue,
                Expression::ObjectExpression(_) | Expression::ArrayExpression(_) => {
                    match literal_keys(init) {
                        Ok(keys) => (keys, None, None),
                        Err(reason) => (Vec::new(), Some(reason.to_string()), None),
                    }
                }
                Expression::NewExpression(new) => {
                    let Some((class, plan)) = self.class_of(&new.callee) else {
                        continue;
                    };
                    match Self::instance(new, plan) {
                        Ok((keys, ignored)) => {
                            (keys, None, Some((class, new.callee.node_id(), ignored)))
                        }
                        Err(reason) => (Vec::new(), Some(reason.to_string()), None),
                    }
                }
                _ => continue,
            };
            if keys.is_empty() && rejected.is_none() {
                // Nothing to replace.
                continue;
            }
            self.candidates.insert(
                id.symbol_id(),
                Candidate {
                    site: init.node_id(),
                    span: declarator.span,
                    keys,
                    objects: Vec::new(),
                    written: false,
                    function: function.map(|(node, _)| node),
                    rejected,
                    instance,
                },
            );
        }
    }
}

impl<'s> Find<'s, '_> {
    /// The class a `new` callee names, if it is one declared in this module, and its plan.
    fn class_of(
        &self,
        callee: &Expression<'_>,
    ) -> Option<(SymbolId, &'s Result<ClassPlan, &'static str>)> {
        let Expression::Identifier(ident) = callee.without_parentheses() else {
            return None;
        };
        let symbol = self.symbol_of(ident)?;
        self.classes.get(&symbol).map(|plan| (symbol, plan))
    }

    /// The keys of `new C(…)`'s instance and the arguments the constructor ignores, if the
    /// class and the call have the plain form.
    fn instance(
        new: &oxc_ast::ast::NewExpression<'_>,
        plan: &Result<ClassPlan, &'static str>,
    ) -> Result<(Vec<String>, Vec<NodeId>), &'static str> {
        let plan = plan.as_ref().map_err(|reason| *reason)?;
        let mut ignored = Vec::new();
        for (index, argument) in new.arguments.iter().enumerate() {
            let argument = argument.as_expression().ok_or("an argument is spread")?;
            if index >= plan.parameters || plan.parameter_key(index).is_none() {
                ignored.push(argument.node_id());
            } else if is_anonymous(argument) {
                // `this.f = f` keeps it unnamed; a local would name it.
                return Err("an argument is an anonymous function or class");
            }
        }
        Ok((
            plan.keys.iter().map(|(key, _)| key.clone()).collect(),
            ignored,
        ))
    }
}

/// Replaces the accepted bindings' declarations and key uses.
struct Rewrite<'s, 'a> {
    scoping: &'s Scoping,
    accepted: &'s FxHashMap<SymbolId, Replacement>,
    classes: &'s FxHashMap<SymbolId, Result<ClassPlan, &'static str>>,
    ast: AstBuilder<'a>,
}

impl<'a> Rewrite<'_, 'a> {
    /// The local for `object.key`, if `object` is a replaced binding.
    fn local(&self, object: &Expression<'_>, key: Option<String>) -> Option<&str> {
        let Expression::Identifier(ident) = object else {
            return None;
        };
        let symbol = ident
            .reference_id
            .get()
            .and_then(|reference| self.scoping.get_reference(reference).symbol_id())?;
        let replacement = self.accepted.get(&symbol)?;
        replacement.names.get(&key?).map(String::as_str)
    }

    /// The values `new C(…)`'s instance keys get: the arguments the constructor assigns, in
    /// argument order (the ones it ignores are pure and dropped), then the rest.
    fn instance_values(
        &self,
        plan: &ClassPlan,
        new: &mut oxc_ast::ast::NewExpression<'a>,
        span: Span,
    ) -> Vec<(String, Expression<'a>)> {
        let mut values = Vec::new();
        let given = new.arguments.len();
        for (index, argument) in new.arguments.iter_mut().enumerate() {
            if index < plan.parameters
                && let Some(key) = plan.parameter_key(index)
                && let Some(argument) = argument.as_expression_mut()
            {
                values.push((key.to_string(), argument.take_in(&self.ast)));
            }
        }
        for (key, source) in &plan.keys {
            match source {
                Source::Parameter(index) if *index < given => {}
                Source::Parameter(_) => {
                    values.push((key.clone(), Constant::Undefined.expression(span, &self.ast)));
                }
                Source::Constant(constant) => {
                    values.push((key.clone(), constant.expression(span, &self.ast)));
                }
            }
        }
        values
    }

    fn identifier(&self, span: Span, name: &str) -> IdentifierReference<'a> {
        IdentifierReference::new(span, Ident::from_str_in(name, &self.ast), &self.ast)
    }

    /// The local a member target writes, if it is a replaced key.
    fn target_local(&self, target: &SimpleAssignmentTarget<'_>) -> Option<(Span, String)> {
        match target {
            SimpleAssignmentTarget::StaticMemberExpression(member) => self
                .local(&member.object, Some(member.property.name.to_string()))
                .map(|name| (member.span, name.to_string())),
            SimpleAssignmentTarget::ComputedMemberExpression(member) => self
                .local(&member.object, computed_key(&member.expression))
                .map(|name| (member.span, name.to_string())),
            _ => None,
        }
    }
}

impl<'a> VisitMut<'a> for Rewrite<'_, 'a> {
    fn visit_variable_declaration(&mut self, declaration: &mut VariableDeclaration<'a>) {
        walk_mut::walk_variable_declaration(self, declaration);
        let mut written = false;
        let mut changed = false;
        let mut declarators = ArenaVec::new_in(&self.ast);
        for mut declarator in declaration.declarations.take_in(&self.ast) {
            let replacement = declarator
                .id
                .get_binding_identifier()
                .and_then(|id| self.accepted.get(&id.symbol_id()));
            let (Some(replacement), Some(init)) = (replacement, declarator.init.as_mut()) else {
                declarators.push(declarator);
                continue;
            };
            changed = true;
            written |= replacement.written;
            let values: Vec<(String, Expression<'a>)> = match init {
                Expression::ObjectExpression(object) => object
                    .properties
                    .iter_mut()
                    .filter_map(|property| match property {
                        ObjectPropertyKind::ObjectProperty(property) => Some((
                            static_key(&property.key).unwrap_or_default(),
                            property.value.take_in(&self.ast),
                        )),
                        ObjectPropertyKind::SpreadProperty(_) => None,
                    })
                    .collect(),
                Expression::ArrayExpression(array) => array
                    .elements
                    .iter_mut()
                    .enumerate()
                    .filter_map(|(index, element)| {
                        element
                            .as_expression_mut()
                            .map(|value| (index.to_string(), value.take_in(&self.ast)))
                    })
                    .collect(),
                Expression::NewExpression(new) => {
                    let plan = replacement
                        .class
                        .and_then(|class| self.classes[&class].as_ref().ok());
                    plan.map(|plan| self.instance_values(plan, new, declarator.span))
                        .unwrap_or_default()
                }
                _ => Vec::new(),
            };
            for (key, value) in values {
                let name = &replacement.names[&key];
                declarators.push(VariableDeclarator::new(
                    declarator.span,
                    BindingPattern::new_binding_identifier(
                        declarator.span,
                        Ident::from_str_in(name, &self.ast),
                        &self.ast,
                    ),
                    None,
                    Some(value),
                    false,
                    &self.ast,
                ));
            }
        }
        declaration.declarations = declarators;
        if changed && written && declaration.kind == VariableDeclarationKind::Const {
            declaration.kind = VariableDeclarationKind::Let;
        }
    }

    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        let local = match expression {
            Expression::StaticMemberExpression(member) => self
                .local(&member.object, Some(member.property.name.to_string()))
                .map(|name| (member.span, name.to_string())),
            Expression::ComputedMemberExpression(member) => self
                .local(&member.object, computed_key(&member.expression))
                .map(|name| (member.span, name.to_string())),
            _ => None,
        };
        if let Some((span, name)) = local {
            *expression = Expression::Identifier(oxc_allocator::ArenaBox::new_in(
                self.identifier(span, &name),
                &self.ast,
            ));
            return;
        }
        walk_mut::walk_expression(self, expression);
    }

    fn visit_assignment_target(&mut self, target: &mut AssignmentTarget<'a>) {
        if let Some(simple) = target.as_simple_assignment_target()
            && let Some((span, name)) = self.target_local(simple)
        {
            *target = AssignmentTarget::AssignmentTargetIdentifier(
                oxc_allocator::ArenaBox::new_in(self.identifier(span, &name), &self.ast),
            );
            return;
        }
        walk_mut::walk_assignment_target(self, target);
    }

    fn visit_simple_assignment_target(&mut self, target: &mut SimpleAssignmentTarget<'a>) {
        if let Some((span, name)) = self.target_local(target) {
            *target = SimpleAssignmentTarget::AssignmentTargetIdentifier(
                oxc_allocator::ArenaBox::new_in(self.identifier(span, &name), &self.ast),
            );
            return;
        }
        walk_mut::walk_simple_assignment_target(self, target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_keys() {
        assert_eq!(integer_key(0.0), Some("0".into()));
        assert_eq!(integer_key(-0.0), None);
        assert_eq!(integer_key(1.5), None);
    }
}
