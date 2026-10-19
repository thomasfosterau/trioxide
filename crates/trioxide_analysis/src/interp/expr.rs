//! Expressions.

use oxc_ast::ast::{
    ArrayExpressionElement, AssignmentTarget, AssignmentTargetMaybeDefault,
    AssignmentTargetProperty, BinaryExpression, Class, ClassElement, Expression, MemberExpression,
    ObjectPropertyKind, PropertyKind, SimpleAssignmentTarget, TemplateLiteral, UnaryExpression,
    UpdateExpression,
};
use oxc_span::GetSpan;
use oxc_syntax::operator::{
    AssignmentOperator, BinaryOperator, LogicalOperator, UnaryOperator, UpdateOperator,
};
use trioxide_types::{NumberType, Type};

use super::{Cx, EffectKind, Escape, ObjKind, call::Accessor};
use crate::{
    builtins,
    facts::Trust,
    numeric,
    value::{AllocId, Pts, Val},
    world::static_key,
};

/// A property key as far as the analysis can tell.
#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    /// A known name (numbers in canonical string form).
    Name(String),
    /// A number (an index, for arrays).
    Index(NumberType, Trust),
    /// Unknown.
    Unknown,
    /// None: the key has no value (yet, in a run on stale inputs), so the access can't happen.
    Never,
}

impl Key {
    /// The key a computed property value names.
    pub fn of_value(value: &Val) -> Self {
        if value.trust == Trust::Unknown {
            return Self::Unknown;
        }
        if value.ty == Type::Never && value.trust == Trust::Proven {
            return Self::Never;
        }
        match &value.ty {
            Type::Number(number) => {
                if value.trust == Trust::Proven
                    && let Some(constant) = number.as_constant()
                    && constant >= 0.0
                    && constant.fract() == 0.0
                {
                    return Self::Name(crate::world::number_key(constant));
                }
                Self::Index(*number, value.trust)
            }
            _ => Self::Unknown,
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Self::Name(name) => Some(name),
            _ => None,
        }
    }

    fn is_index(&self) -> bool {
        match self {
            Self::Index(..) => true,
            Self::Name(name) => is_array_index(name),
            Self::Unknown | Self::Never => false,
        }
    }

    /// The smallest and largest index the key may be, when it is proven an array index.
    fn index_range(&self) -> Option<(f64, f64)> {
        match self {
            Self::Index(number, Trust::Proven)
                if number.int && number.non_nan && number.range.min >= 0.0 =>
            {
                Some((number.range.min, number.range.max))
            }
            Self::Name(name) if is_array_index(name) => {
                let index = name.parse::<f64>().ok()?;
                Some((index, index))
            }
            _ => None,
        }
    }
}

impl<'a> Cx<'_, '_, 'a> {
    /// Evaluates an expression and records its value.
    pub fn expr(&mut self, expression: &'a Expression<'a>) -> Val {
        self.position = expression.span().start;
        self.frames.push(super::Frame::new(self.record.edges.len()));
        let value = self.expr_inner(expression);
        let frame = self.pop_frame();
        // Every evaluation is recorded, pure ones too: an expression with no record (a
        // callee member, a part of a `?.` chain) has unknown effects, not none.
        #[expect(clippy::cast_possible_truncation, reason = "edges of one unit")]
        let visit = super::ExprVisit {
            node: expression.node_id(),
            proven: frame.proven,
            declared: frame.declared,
            local_writes: frame.local_writes,
            volatile: frame.volatile,
            shared_reads: frame.shared_reads,
            edges: (frame.edges as u32, self.record.edges.len() as u32),
        };
        self.record.expr_effects.push(visit);
        self.record_node(expression.node_id(), &value);
        value
    }

    fn expr_inner(&mut self, expression: &'a Expression<'a>) -> Val {
        match expression {
            Expression::BooleanLiteral(_) => Val::proven(Type::Boolean),
            Expression::NullLiteral(_) => Val::proven(Type::Null),
            Expression::NumericLiteral(literal) => Val::number(NumberType::constant(literal.value)),
            Expression::BigIntLiteral(_) => Val::proven(Type::BigInt),
            Expression::StringLiteral(_) => Val::proven(Type::String),
            Expression::TemplateLiteral(template) => self.template(template),
            Expression::RegExpLiteral(literal) => {
                let site = self.alloc(literal.node_id.get(), ObjKind::Opaque);
                Val {
                    ty: Type::Unknown,
                    trust: Trust::Unknown,
                    pts: Pts::site(site),
                    poisoned: false,
                }
            }
            Expression::Identifier(ident) => {
                let value = self.identifier(ident);
                self.or_declared(value, ident.node_id.get(), ident.span)
            }
            Expression::ParenthesizedExpression(paren) => self.expr(&paren.expression),
            Expression::SequenceExpression(sequence) => {
                let mut last = Val::proven(Type::Undefined);
                for expression in &sequence.expressions {
                    last = self.expr(expression);
                }
                last
            }
            Expression::ArrayExpression(array) => self.array_literal(array),
            Expression::ObjectExpression(object) => self.object_literal(object),
            Expression::FunctionExpression(function) => self.function_value(function.node_id.get()),
            Expression::ArrowFunctionExpression(arrow) => self.function_value(arrow.node_id.get()),
            Expression::ClassExpression(class) => self.class_definition(class),
            Expression::UnaryExpression(unary) => self.unary(unary),
            Expression::BinaryExpression(binary) => self.binary(binary),
            Expression::LogicalExpression(logical) => self.logical(logical),
            Expression::ConditionalExpression(conditional) => {
                let (yes, no) = self.condition(&conditional.test);
                let saved = std::mem::replace(&mut self.env, yes);
                let consequent = self.expr(&conditional.consequent);
                let after_yes = std::mem::replace(&mut self.env, no);
                let alternate = self.expr(&conditional.alternate);
                self.env = after_yes.join(&self.env);
                let _ = saved;
                consequent.join(&alternate)
            }
            Expression::AssignmentExpression(assign) => self.assignment(assign),
            Expression::UpdateExpression(update) => self.update(update),
            Expression::CallExpression(call) => {
                let value = self.call(call);
                self.or_declared(value, call.node_id.get(), call.span)
            }
            Expression::NewExpression(new) => {
                let value = self.construct(new);
                self.or_declared(value, new.node_id.get(), new.span)
            }
            Expression::TaggedTemplateExpression(tagged) => {
                // A call like any other (`p.f`x`` has `this` = `p`): the strings array, then
                // each substitution.
                let value =
                    self.call_with(tagged.node_id.get(), &tagged.tag, false, |cx: &mut Self| {
                        let mut values = vec![cx.template_strings(&tagged.quasi)];
                        for expression in &tagged.quasi.expressions {
                            values.push(cx.expr(expression));
                        }
                        super::call::Args {
                            values,
                            spread: false,
                        }
                    });
                self.or_declared(value, tagged.node_id.get(), tagged.span)
            }
            Expression::ChainExpression(chain) => {
                let value = match &chain.expression {
                    oxc_ast::ast::ChainElement::CallExpression(call) => {
                        let value = self.call(call);
                        let value = self.or_declared(value, call.node_id.get(), call.span);
                        self.record_node(call.node_id.get(), &value);
                        value
                    }
                    oxc_ast::ast::ChainElement::TSNonNullExpression(non_null) => {
                        self.expr(&non_null.expression)
                    }
                    element => match element.as_member_expression() {
                        Some(member) => {
                            let value = self.member(member);
                            self.record_node(member.node_id(), &value);
                            value
                        }
                        None => Val::unknown(),
                    },
                };
                // Some link may have short-circuited.
                value.join(&Val::proven(Type::Undefined))
            }
            Expression::ThisExpression(this) => {
                let value = self.this.clone();
                self.or_declared(value, this.node_id.get(), this.span)
            }
            Expression::AwaitExpression(await_expression) => {
                let value = self.expr(&await_expression.argument);
                // Awaiting a primitive gives it back; anything else may be a thenable whose
                // `then` the engine calls with its own resolve and reject: its methods are open.
                if value.is_primitive(false) {
                    value
                } else {
                    self.escape_value(&value, Escape::Unknown);
                    self.effect(EffectKind::CallsUnknown, false, value.is_primitive(true));
                    self.always(EffectKind::Throws);
                    let value = Val::unknown();
                    self.or_declared(value, await_expression.node_id.get(), await_expression.span)
                }
            }
            Expression::YieldExpression(yield_expression) => {
                if let Some(argument) = &yield_expression.argument {
                    let value = self.expr(argument);
                    self.escape_value(&value, Escape::Unknown);
                }
                self.always(EffectKind::CallsUnknown);
                self.always(EffectKind::Throws);
                Val::unknown()
            }
            Expression::PrivateInExpression(private_in) => {
                let value = self.expr(&private_in.right);
                self.effect(
                    EffectKind::Throws,
                    value.trust == Trust::Proven
                        && !value.ty.may_be_nullish()
                        && !value.is_primitive(false),
                    !value.ty.may_be_nullish() && !value.is_primitive(true),
                );
                Val::proven(Type::Boolean)
            }
            Expression::TSAsExpression(cast) => {
                let value = self.expr(&cast.expression);
                self.or_declared(value, cast.node_id.get(), cast.span)
            }
            Expression::TSSatisfiesExpression(satisfies) => self.expr(&satisfies.expression),
            Expression::TSTypeAssertion(cast) => {
                let value = self.expr(&cast.expression);
                self.or_declared(value, cast.node_id.get(), cast.span)
            }
            Expression::TSNonNullExpression(non_null) => {
                // `!` changes nothing at run time: the value may still be nullish.
                let value = self.expr(&non_null.expression);
                self.or_declared(value, non_null.node_id.get(), non_null.span)
            }
            Expression::TSInstantiationExpression(instantiation) => {
                self.expr(&instantiation.expression)
            }
            Expression::ComputedMemberExpression(_)
            | Expression::StaticMemberExpression(_)
            | Expression::PrivateFieldExpression(_) => {
                let member = expression
                    .as_member_expression()
                    .expect("a member expression");
                self.member(member)
            }
            Expression::Super(_) | Expression::ImportMeta(_) | Expression::NewTarget(_) => {
                Val::unknown()
            }
            Expression::ImportExpression(import) => {
                // The promise resolves to the module's namespace, which unknown code handles.
                if let Expression::StringLiteral(specifier) = &import.source
                    && let Some(Some(module)) =
                        self.module.imports.get(specifier.value.as_str()).copied()
                {
                    let unit = self.unit.id;
                    let namespace = self.an.namespace(module, unit);
                    self.escape_value(&namespace, Escape::Unknown);
                }
                let value = self.expr(&import.source);
                self.escape_value(&value, Escape::Unknown);
                if let Some(options) = &import.options {
                    let value = self.expr(options);
                    self.escape_value(&value, Escape::Unknown);
                }
                self.always(EffectKind::CallsUnknown);
                self.always(EffectKind::Allocates);
                Val::unknown()
            }
            Expression::JSXElement(_)
            | Expression::JSXFragment(_)
            | Expression::V8IntrinsicExpression(_) => {
                self.opaque_subtree(expression);
                Val::unknown()
            }
        }
    }

    /// Evaluates the expressions inside JSX (or other syntax the interpreter does not model)
    /// as flowing to unknown code.
    pub fn opaque_subtree(&mut self, expression: &'a Expression<'a>) {
        match expression {
            Expression::JSXElement(element) => self.jsx_element(element),
            Expression::JSXFragment(fragment) => {
                self.jsx_factories(fragment.node_id.get(), false);
                self.jsx_factories(fragment.node_id.get(), true);
                self.jsx_children(&fragment.children);
            }
            Expression::V8IntrinsicExpression(intrinsic) => {
                for argument in &intrinsic.arguments {
                    if let Some(expression) = argument.as_expression() {
                        let value = self.expr(expression);
                        self.escape_value(&value, Escape::Unknown);
                    }
                }
            }
            other => {
                let value = self.expr(other);
                self.escape_value(&value, Escape::Unknown);
            }
        }
        self.always(EffectKind::CallsUnknown);
        self.always(EffectKind::Throws);
        self.always(EffectKind::Allocates);
    }

    /// Compiled JSX calls the JSX factory (and passes the fragment factory): their values
    /// escape, as calls from generated code trioxide doesn't follow.
    fn jsx_factories(&mut self, node: oxc_syntax::node::NodeId, fragment: bool) {
        let factory = if fragment {
            self.an.options.jsx_fragment_factory.clone()
        } else {
            self.an.options.jsx_factory.clone()
        };
        let Some(factory) = factory else { return };
        let root = factory.split('.').next().unwrap_or_default().to_string();
        let scoping = self.module.semantic.scoping();
        let scope = self.module.semantic.nodes().get_node(node).scope_id();
        if let Some(symbol) = scoping.find_binding(scope, root.as_str().into()) {
            let value = self.read_symbol(symbol);
            self.escape_value(&value, Escape::Unknown);
        }
    }

    fn jsx_element(&mut self, element: &'a oxc_ast::ast::JSXElement<'a>) {
        use oxc_ast::ast::{JSXAttributeItem, JSXAttributeValue, JSXElementName};
        self.jsx_factories(element.node_id.get(), false);
        if let JSXElementName::IdentifierReference(ident) = &element.opening_element.name {
            let value = self.identifier(ident);
            self.record_node(ident.node_id.get(), &value);
            self.escape_value(&value, Escape::Unknown);
        }
        for attribute in &element.opening_element.attributes {
            match attribute {
                JSXAttributeItem::Attribute(attribute) => match &attribute.value {
                    Some(JSXAttributeValue::ExpressionContainer(container)) => {
                        self.jsx_container(container);
                    }
                    Some(JSXAttributeValue::Element(element)) => self.jsx_element(element),
                    Some(JSXAttributeValue::Fragment(fragment)) => {
                        self.jsx_children(&fragment.children);
                    }
                    Some(JSXAttributeValue::StringLiteral(_)) | None => {}
                },
                JSXAttributeItem::SpreadAttribute(spread) => {
                    let value = self.expr(&spread.argument);
                    self.escape_value(&value, Escape::Unknown);
                }
            }
        }
        self.jsx_children(&element.children);
    }

    fn jsx_children(&mut self, children: &'a [oxc_ast::ast::JSXChild<'a>]) {
        use oxc_ast::ast::JSXChild;
        for child in children {
            match child {
                JSXChild::Element(element) => self.jsx_element(element),
                JSXChild::Fragment(fragment) => {
                    self.jsx_factories(fragment.node_id.get(), false);
                    self.jsx_factories(fragment.node_id.get(), true);
                    self.jsx_children(&fragment.children);
                }
                JSXChild::ExpressionContainer(container) => self.jsx_container(container),
                JSXChild::Spread(spread) => {
                    let value = self.expr(&spread.expression);
                    self.escape_value(&value, Escape::Unknown);
                }
                JSXChild::Text(_) => {}
            }
        }
    }

    fn jsx_container(&mut self, container: &'a oxc_ast::ast::JSXExpressionContainer<'a>) {
        if let Some(expression) = container.expression.as_expression() {
            let value = self.expr(expression);
            self.escape_value(&value, Escape::Unknown);
        }
    }

    fn identifier(&mut self, ident: &'a oxc_ast::ast::IdentifierReference<'a>) -> Val {
        let scoping = self.module.semantic.scoping();
        let symbol = ident
            .reference_id
            .get()
            .and_then(|reference| scoping.get_reference(reference).symbol_id());
        match symbol {
            Some(symbol) => self.read_symbol(symbol),
            None => self.global(ident.name.as_str()),
        }
    }

    /// The value of an unbound (global) name.
    fn global(&mut self, name: &str) -> Val {
        match name {
            // Non-writable, non-configurable properties of the global object.
            "undefined" => Val::proven(Type::Undefined),
            "NaN" => Val::number(NumberType::ANY),
            "Infinity" => Val::number(NumberType::constant(f64::INFINITY)),
            _ => {
                self.always(EffectKind::Reads);
                // A global that doesn't exist throws a ReferenceError.
                self.always(EffectKind::Throws);
                Val::unknown()
            }
        }
    }

    /// The frozen strings array a tag receives, with its frozen `raw` array.
    fn template_strings(&mut self, quasi: &'a TemplateLiteral<'a>) -> Val {
        // Cooked strings are `undefined` where an escape is invalid.
        let cooked = if quasi
            .quasis
            .iter()
            .all(|element| element.value.cooked.is_some())
        {
            Type::String
        } else {
            Type::union([Type::String, Type::Undefined])
        };
        let length = u32::try_from(quasi.quasis.len()).ok().map(f64::from);
        let array = |cx: &mut Self, node: oxc_syntax::node::NodeId, element: Type| {
            let site = cx.alloc(node, ObjKind::Array);
            let first = cx
                .an
                .heap(site)
                .is_some_and(|obj| obj.elem.ty == Type::Never && obj.props.is_empty());
            cx.store_elem(site, &Val::proven(element.clone()));
            cx.mark(site, move |obj| {
                obj.length = if (first && !obj.sized && !obj.dynamic) || obj.length == length {
                    length
                } else {
                    None
                };
                obj.sized = true;
            });
            (
                site,
                Val::object(Type::Array(std::sync::Arc::new(element)), site),
            )
        };
        let raw_node = quasi
            .quasis
            .first()
            .map_or(quasi.node_id.get(), |element| element.node_id.get());
        let (_, raw) = array(self, raw_node, Type::String);
        let (strings, value) = array(self, quasi.node_id.get(), cooked);
        self.store_prop(strings, "raw", &raw, true);
        self.mark(strings, |obj| {
            obj.present.insert("raw".to_string());
        });
        value
    }

    fn template(&mut self, template: &'a TemplateLiteral<'a>) -> Val {
        for expression in &template.expressions {
            let value = self.expr(expression);
            self.coerce(&value, false);
        }
        Val::proven(Type::String)
    }

    /// Notes the effects of coercing `value` to a primitive (`ToPrimitive`): user code may run
    /// unless it is one already. `symbol_throws`: whether a symbol makes it throw (string and
    /// number coercion).
    pub fn coerce(&mut self, value: &Val, bigint_throws: bool) {
        let proven = value.is_primitive(false);
        let declared = value.is_primitive(true);
        self.effect(EffectKind::CallsUnknown, proven, declared);
        let no_throw = |declared: bool| {
            value.is_primitive(declared)
                && !value.ty.members().iter().any(|member| {
                    matches!(member, Type::Symbol)
                        || (bigint_throws && matches!(member, Type::BigInt))
                })
        };
        let (proven_free, declared_free) = (no_throw(false), no_throw(true));
        self.effect(EffectKind::Throws, proven_free, declared_free);
        if !proven {
            // `valueOf`, `toString` or `Symbol.toPrimitive` may run, called by the engine (so
            // they are open), and change anything.
            self.escape_value(value, Escape::Unknown);
            self.after_call(false);
            self.note_call();
        }
    }

    fn function_value(&mut self, node: oxc_syntax::node::NodeId) -> Val {
        let site = self.alloc(node, ObjKind::Function(self.node(node)));
        Val::object(Type::opaque_function(Some(site)), site)
    }

    fn array_literal(&mut self, array: &'a oxc_ast::ast::ArrayExpression<'a>) -> Val {
        let site = self.alloc(array.node_id.get(), ObjKind::Array);
        let mut length = Some(0.0);
        let mut holey = false;
        let mut elem = Val::never();
        for element in &array.elements {
            match element {
                ArrayExpressionElement::SpreadElement(spread) => {
                    let value = self.expr(&spread.argument);
                    elem = elem.join(&self.iterate(&value, spread.node_id.get()));
                    length = None;
                }
                ArrayExpressionElement::Elision(_) => {
                    holey = true;
                    length = length.map(|n| n + 1.0);
                }
                element => {
                    if let Some(expression) = element.as_expression() {
                        let value = self.expr(expression);
                        elem = elem.join(&value);
                        length = length.map(|n| n + 1.0);
                    }
                }
            }
        }
        let first = self
            .an
            .peek(site)
            .is_some_and(|obj| obj.elem.ty == Type::Never && obj.props.is_empty());
        self.store_elem(site, &elem);
        self.mark(site, move |obj| {
            obj.holey |= holey;
            obj.length = if (first && !obj.sized && !obj.dynamic) || obj.length == length {
                length
            } else {
                None
            };
            obj.sized = true;
        });
        Val::object(Type::Array(std::sync::Arc::new(Type::Unknown)), site)
    }

    fn object_literal(&mut self, object: &'a oxc_ast::ast::ObjectExpression<'a>) -> Val {
        let site = self.alloc(object.node_id.get(), ObjKind::Object);
        let mut keys = Vec::new();
        let mut dynamic = false;
        let mut accessors = false;
        for property in &object.properties {
            match property {
                ObjectPropertyKind::ObjectProperty(property) => {
                    let key = if property.computed {
                        let key = property.key.as_expression().map(|key| self.expr(key));
                        match &property.key {
                            key if static_key(key, false).is_some()
                                && matches!(
                                    key,
                                    oxc_ast::ast::PropertyKey::StringLiteral(_)
                                        | oxc_ast::ast::PropertyKey::NumericLiteral(_)
                                ) =>
                            {
                                static_key(key, false)
                            }
                            _ => {
                                if let Some(key) = key {
                                    self.coerce(&key, false);
                                }
                                None
                            }
                        }
                    } else {
                        static_key(&property.key, false)
                    };
                    let value = self.expr(&property.value);
                    match (key, property.kind) {
                        (Some(key), PropertyKind::Init)
                            if key != "__proto__" || property.shorthand || property.method =>
                        {
                            self.store_prop(site, &key, &value, true);
                            keys.push(key);
                        }
                        (Some(key), _) if key != "__proto__" => {
                            // A getter or setter: property accesses (which trioxide doesn't
                            // follow for literals) call it.
                            self.escape_value(&value, Escape::Unknown);
                            self.store_prop(site, &key, &Val::unknown(), true);
                            dynamic = true;
                            accessors = true;
                        }
                        (key, kind) => {
                            // `__proto__: x` sets the prototype, whose accessors then run on
                            // property accesses. A computed key may name an accessor, or a
                            // protocol method the engine calls (`[Symbol.iterator]`,
                            // `[Symbol.toPrimitive]`): its value is open either way.
                            let prototype = key.is_some() && kind == PropertyKind::Init;
                            self.escape_value(&value, Escape::Unknown);
                            dynamic = true;
                            accessors |= prototype || kind != PropertyKind::Init;
                        }
                    }
                }
                ObjectPropertyKind::SpreadProperty(spread) => {
                    let value = self.expr(&spread.argument);
                    match value.pts.sites() {
                        Some(sites) if value.trust == Trust::Proven => {
                            for &source in sites {
                                let copied: Vec<(String, Val)> = self
                                    .an
                                    .heap(source)
                                    .filter(|obj| {
                                        obj.kind == ObjKind::Object
                                            && !obj.dynamic
                                            && obj.escape != Escape::Unknown
                                    })
                                    .map(|obj| {
                                        obj.props
                                            .iter()
                                            .map(|(k, v)| (k.clone(), v.clone()))
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                let complete = self.an.heap(source).is_some_and(|obj| {
                                    obj.kind == ObjKind::Object
                                        && !obj.dynamic
                                        && obj.escape != Escape::Unknown
                                });
                                if !complete {
                                    dynamic = true;
                                }
                                // Spreading reads every own property: getters run.
                                let runs_code = self.an.heap(source).is_none_or(|obj| {
                                    obj.accessors
                                        || obj.poisoned
                                        || obj.escape == Escape::Unknown
                                        || obj.kind == ObjKind::Opaque
                                });
                                if runs_code {
                                    self.always(EffectKind::CallsUnknown);
                                    self.note_call();
                                    self.after_call(false);
                                }
                                for (key, value) in copied {
                                    self.store_prop(site, &key, &value, true);
                                }
                            }
                        }
                        _ => {
                            // Copies unknown keys, and may run getters.
                            dynamic = !value.is_primitive(false);
                            self.effect(
                                EffectKind::CallsUnknown,
                                value.is_primitive(false),
                                value.is_primitive(true),
                            );
                        }
                    }
                }
            }
        }
        let first = self
            .an
            .peek(site)
            // Not after a `delete` emptied it: `present` only shrinks from then on.
            .is_some_and(|obj| obj.present.is_empty() && !obj.dynamic && !obj.deleted);
        self.mark(site, move |obj| {
            obj.dynamic |= dynamic;
            obj.accessors |= accessors;
            if first {
                obj.present.extend(keys.iter().cloned());
            } else {
                obj.present.retain(|key| keys.contains(key));
            }
        });
        Val::object(Type::empty_object(), site)
    }

    /// Defines a class: evaluates its heritage, computed keys and static members, and makes
    /// the class and prototype objects.
    pub fn class_definition(&mut self, class: &'a Class<'a>) -> Val {
        self.position = class.span.start;
        // Static blocks and static field initialisers run as the class is defined.
        if class.body.body.iter().any(|element| match element {
            ClassElement::StaticBlock(_) => true,
            ClassElement::PropertyDefinition(property) => {
                property.r#static && property.value.is_some()
            }
            _ => false,
        }) {
            self.note_call();
        }
        let class_site = self.alloc(
            class.node_id.get(),
            ObjKind::Class(self.node(class.node_id.get())),
        );
        let class_ref = class_site;
        let proto = self.alloc(class.body.node_id.get(), ObjKind::Prototype(class_ref));
        if let Some(heritage) = &class.heritage {
            let parent = self.expr(&heritage.expression);
            self.escape_value(&parent, Escape::Known);
            self.an.join_super(class_ref, &parent);
            if parent.pts.sites().is_none() || parent.trust != Trust::Proven {
                // Unknown code's constructor and methods run on our instances.
                self.an.escape(&Pts::site(class_site), Escape::Unknown);
            }
        }
        let class_value = Val::object(Type::opaque_function(Some(class_ref)), class_site);
        // Decorators receive (and may replace or call) the class and its members.
        let decorated = !class.decorators.is_empty()
            || class.body.body.iter().any(|element| match element {
                ClassElement::MethodDefinition(method) => {
                    !method.decorators.is_empty()
                        || method
                            .value
                            .params
                            .items
                            .iter()
                            .any(|param| !param.decorators.is_empty())
                }
                ClassElement::PropertyDefinition(property) => !property.decorators.is_empty(),
                ClassElement::AccessorProperty(property) => !property.decorators.is_empty(),
                _ => false,
            });
        if decorated {
            let mut decorators: Vec<&'a oxc_ast::ast::Decorator<'a>> =
                class.decorators.iter().collect();
            for element in &class.body.body {
                match element {
                    ClassElement::MethodDefinition(method) => {
                        decorators.extend(method.decorators.iter());
                        for param in &method.value.params.items {
                            decorators.extend(param.decorators.iter());
                        }
                    }
                    ClassElement::PropertyDefinition(property) => {
                        decorators.extend(property.decorators.iter());
                    }
                    ClassElement::AccessorProperty(property) => {
                        decorators.extend(property.decorators.iter());
                    }
                    _ => {}
                }
            }
            for decorator in decorators {
                let value = self.expr(&decorator.expression);
                self.escape_value(&value, Escape::Unknown);
            }
            self.an.escape(&Pts::site(class_site), Escape::Unknown);
            self.always(EffectKind::CallsUnknown);
            self.always(EffectKind::Throws);
        }
        if let Some(id) = &class.id
            && let Some(symbol) = id.symbol_id.get()
        {
            self.write_symbol(symbol, class_value.clone(), Some(id.node_id.get()));
        }
        for element in &class.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    if method.computed
                        && let Some(key) = method.key.as_expression()
                    {
                        let key = self.expr(key);
                        self.coerce(&key, false);
                    }
                    let function = self.function_value(method.value.node_id.get());
                    let Some(name) = static_key(&method.key, method.computed) else {
                        self.mark(
                            if method.r#static { class_site } else { proto },
                            move |obj| {
                                obj.dynamic = true;
                            },
                        );
                        self.escape_value(&function, Escape::Unknown);
                        continue;
                    };
                    if method.kind == oxc_ast::ast::MethodDefinitionKind::Constructor {
                        continue;
                    }
                    let target = if method.r#static { class_site } else { proto };
                    self.store_prop(target, &name, &function, true);
                    self.mark(target, move |obj| {
                        obj.present.insert(name.clone());
                    });
                }
                ClassElement::PropertyDefinition(property) => {
                    if property.computed
                        && let Some(key) = property.key.as_expression()
                    {
                        let key = self.expr(key);
                        self.coerce(&key, false);
                        let target = if property.r#static { class_site } else { proto };
                        self.mark(target, move |obj| obj.dynamic = true);
                    }
                    if property.r#static {
                        let saved = std::mem::replace(&mut self.this, class_value.clone());
                        let value = match &property.value {
                            Some(value) => self.expr(value),
                            None => Val::proven(Type::Undefined),
                        };
                        self.this = saved;
                        match static_key(&property.key, property.computed) {
                            Some(name) => {
                                self.store_prop(class_site, &name, &value, true);
                                self.mark(class_site, move |obj| {
                                    obj.present.insert(name.clone());
                                });
                            }
                            None => self.escape_value(&value, Escape::Unknown),
                        }
                    }
                }
                ClassElement::AccessorProperty(property) => {
                    // `accessor x`: a getter/setter pair over a private field.
                    let target = if property.r#static { class_site } else { proto };
                    self.mark(target, move |obj| obj.dynamic = true);
                }
                ClassElement::StaticBlock(block) => {
                    // Static blocks run once, now; they are their own unit.
                    let unit = self.node(block.node_id.get());
                    self.record.edges.push(super::Edge {
                        site: unit,
                        callee: unit,
                        trust: Trust::Proven,
                    });
                }
                ClassElement::TSIndexSignature(_) => {}
            }
        }
        class_value
    }

    /// Instance field initialisers, run with `this` the new instance (the class-init unit).
    pub fn class_fields(&mut self, class: &'a Class<'a>) {
        let class_ref = self.node(class.node_id.get());
        let assigned = crate::shapes::constructor_assigned(self.an.world, class_ref);
        for element in &class.body.body {
            if let ClassElement::PropertyDefinition(property) = element
                && !property.r#static
            {
                let value = match &property.value {
                    Some(value) => self.expr(value),
                    None if property.declare => continue,
                    // The constructor assigns it before anything can read the `undefined`.
                    None if static_key(&property.key, property.computed)
                        .is_some_and(|key| assigned.contains(&key)) =>
                    {
                        continue;
                    }
                    None => Val::proven(Type::Undefined),
                };
                let key = static_key(&property.key, property.computed);
                let this = self.this.clone();
                // Without `useDefineForClassFields`, a field initialiser is an assignment and
                // runs an inherited setter; assume it may.
                if let Some(name) = &key
                    && let Some(parent) = crate::shapes::static_superclass(self.an.world, class_ref)
                    && let Some(setter) =
                        self.class_accessor(parent, name, crate::world::MethodRole::Setter)
                {
                    self.call_unit_as_setter(setter, &this, &value, property.node_id.get());
                }
                self.write_prop(
                    &this,
                    &key.map_or(Key::Unknown, Key::Name),
                    &value,
                    property.node_id.get(),
                    true,
                );
            }
        }
    }

    fn unary(&mut self, unary: &'a UnaryExpression<'a>) -> Val {
        if unary.operator == UnaryOperator::Delete {
            // `delete (o.p)`, `delete (o.p as T)` and `delete o?.p` delete `o.p` too (the last
            // only when `o` isn't nullish, which deleting from every site `o` may be covers).
            let target = match unary.argument.get_inner_expression() {
                Expression::ChainExpression(chain) => chain.expression.as_member_expression(),
                inner => inner.as_member_expression(),
            };
            if let Some(member) = target {
                let object = self.expr(member.object());
                let key = self.member_key(member);
                self.delete(&object, &key);
            } else {
                let _ = self.expr(&unary.argument);
            }
            return Val::proven(Type::Boolean);
        }
        if unary.operator == UnaryOperator::Typeof
            && let Expression::Identifier(ident) = &unary.argument
            && ident.reference_id.get().is_some_and(|reference| {
                self.module
                    .semantic
                    .scoping()
                    .get_reference(reference)
                    .symbol_id()
                    .is_none()
            })
        {
            // `typeof undeclared` doesn't throw.
            return Val::proven(Type::String);
        }
        let value = self.expr(&unary.argument);
        match unary.operator {
            UnaryOperator::LogicalNot => Val::proven(Type::Boolean),
            UnaryOperator::Typeof => Val::proven(Type::String),
            UnaryOperator::Void => Val::proven(Type::Undefined),
            UnaryOperator::UnaryPlus if is_never(&value) => Val::never(),
            UnaryOperator::UnaryPlus => {
                // ToNumber: always a number (it throws on bigints and symbols).
                self.coerce(&value, true);
                // Always a number; its range only as far as the operand is proven.
                if value.trust == Trust::Proven {
                    Val::number(to_number(&value))
                } else {
                    Val::number(NumberType::ANY)
                }
            }
            UnaryOperator::UnaryNegation => self.numeric_unary(&value, numeric::neg),
            UnaryOperator::BitwiseNot => self.numeric_unary(&value, numeric::bit_not),
            UnaryOperator::Delete => unreachable!("handled above"),
        }
    }

    /// A unary operator that maps numbers to numbers (and bigints to bigints).
    fn numeric_unary(&mut self, value: &Val, op: fn(NumberType) -> NumberType) -> Val {
        if is_never(value) {
            return Val::never();
        }
        self.coerce(value, false);
        if let Some(number) = number_operand(value) {
            return Val::number(op(number)).at_most(value.trust);
        }
        if value.trust >= Trust::Declared && value.ty == Type::BigInt {
            return Val::proven(Type::BigInt).at_most(value.trust);
        }
        Val::unknown()
    }

    fn binary(&mut self, binary: &'a BinaryExpression<'a>) -> Val {
        let left = self.expr(&binary.left);
        let right = self.expr(&binary.right);
        self.binary_values(binary.operator, &left, &right)
    }

    /// The value of `left op right`.
    pub fn binary_values(&mut self, operator: BinaryOperator, left: &Val, right: &Val) -> Val {
        use BinaryOperator as B;
        if is_never(left) || is_never(right) {
            // An operand that has no value yet (not reached in this round): neither does this.
            return Val::never();
        }
        match operator {
            B::StrictEquality | B::StrictInequality => Val::proven(Type::Boolean),
            B::Equality
            | B::Inequality
            | B::LessThan
            | B::LessEqualThan
            | B::GreaterThan
            | B::GreaterEqualThan => {
                self.coerce(left, false);
                self.coerce(right, false);
                Val::proven(Type::Boolean)
            }
            B::In | B::Instanceof => {
                self.coerce(left, false);
                // Throws unless the right side is an object (or a callable, for instanceof),
                // and instanceof may call Symbol.hasInstance.
                let object = |declared: bool| {
                    (right.trust == Trust::Proven || (declared && right.trust >= Trust::Declared))
                        && !right.ty.may_be_nullish()
                        && !crate::value::is_primitive(&right.ty)
                        && right.ty.members().iter().all(|member| {
                            matches!(
                                member,
                                Type::Object(_)
                                    | Type::Array(_)
                                    | Type::Tuple(_)
                                    | Type::TypedArray(_)
                                    | Type::Instance(_)
                            )
                        })
                };
                let (proven, declared) = if operator == B::In {
                    (object(false), object(true))
                } else {
                    (false, false)
                };
                self.effect(EffectKind::Throws, proven, declared);
                self.effect(EffectKind::CallsUnknown, proven, declared);
                Val::proven(Type::Boolean)
            }
            B::Addition => {
                self.coerce(left, false);
                self.coerce(right, false);
                // A bigint plus anything but a bigint or a string throws.
                let mixes = |declared: bool| {
                    let bigint = |value: &Val| {
                        !value.is_primitive(declared) || value.ty.members().contains(&Type::BigInt)
                    };
                    let other = |value: &Val| {
                        !value.is_primitive(declared)
                            || value
                                .ty
                                .members()
                                .iter()
                                .any(|member| !matches!(member, Type::BigInt | Type::String))
                    };
                    (bigint(left) && other(right)) || (bigint(right) && other(left))
                };
                let (proven_mixes, declared_mixes) = (mixes(false), mixes(true));
                self.effect(EffectKind::Throws, !proven_mixes, !declared_mixes);
                let string =
                    |value: &Val| value.trust >= Trust::Declared && value.ty == Type::String;
                if (string(left) || string(right))
                    && (left.is_primitive(true) || string(left))
                    && (right.is_primitive(true) || string(right))
                {
                    // Concatenation; a primitive or string-coercible operand on the other side.
                    let trust = if string(left) && string(right) {
                        left.trust.min(right.trust)
                    } else if string(left) {
                        left.trust.min(if right.is_primitive(false) {
                            Trust::Proven
                        } else {
                            Trust::Declared
                        })
                    } else {
                        right.trust.min(if left.is_primitive(false) {
                            Trust::Proven
                        } else {
                            Trust::Declared
                        })
                    };
                    return Val::proven(Type::String).at_most(trust);
                }
                // Both numeric-coercible primitives with no string: a number.
                let numeric = |value: &Val, declared: bool| {
                    value.is_primitive(declared)
                        && value.ty.members().iter().all(|member| {
                            matches!(
                                member,
                                Type::Number(_) | Type::Boolean | Type::Null | Type::Undefined
                            )
                        })
                };
                if left.trust >= Trust::Declared
                    && left.ty == Type::BigInt
                    && right.trust >= Trust::Declared
                    && right.ty == Type::BigInt
                {
                    return Val::proven(Type::BigInt).at_most(left.trust.min(right.trust));
                }
                if numeric(left, true) && numeric(right, true) {
                    let trust = if numeric(left, false) && numeric(right, false) {
                        Trust::Proven
                    } else {
                        Trust::Declared
                    };
                    return Val::number(numeric::add(to_number(left), to_number(right)))
                        .at_most(trust);
                }
                Val::unknown()
            }
            B::Subtraction
            | B::Multiplication
            | B::Division
            | B::Remainder
            | B::Exponential
            | B::ShiftLeft
            | B::ShiftRight
            | B::BitwiseOR
            | B::BitwiseXOR
            | B::BitwiseAnd
            | B::ShiftRightZeroFill => {
                self.coerce(left, true);
                self.coerce(right, true);
                let op: fn(NumberType, NumberType) -> NumberType = match operator {
                    B::Subtraction => numeric::sub,
                    B::Multiplication => numeric::mul,
                    B::Division => numeric::div,
                    B::Remainder => numeric::rem,
                    B::Exponential => numeric::pow,
                    B::ShiftLeft => numeric::shl,
                    B::ShiftRight => numeric::shr,
                    B::BitwiseOR => numeric::bit_or,
                    B::BitwiseXOR => numeric::bit_xor,
                    B::BitwiseAnd => numeric::bit_and,
                    _ => numeric::ushr,
                };
                // Mixing a number with a bigint throws, so a result exists only if both are
                // numbers or both bigints. If either is known to be a non-bigint, it's a number.
                let non_bigint = |value: &Val| {
                    value.trust == Trust::Proven
                        && value.is_primitive(false)
                        && !value.ty.members().contains(&Type::BigInt)
                };
                let declared_non_bigint = |value: &Val| {
                    value.trust >= Trust::Declared
                        && value.is_primitive(true)
                        && !value.ty.members().contains(&Type::BigInt)
                };
                let result = numeric_or_any(op, left, right);
                if operator == B::ShiftRightZeroFill || non_bigint(left) || non_bigint(right) {
                    // `>>>` throws on bigints; otherwise the other operand must be a number too.
                    return Val::number(result_with(
                        op,
                        left,
                        right,
                        operator == B::ShiftRightZeroFill
                            && !(non_bigint(left) || non_bigint(right)),
                        result,
                    ));
                }
                if declared_non_bigint(left) || declared_non_bigint(right) {
                    return Val::number(result).at_most(Trust::Declared);
                }
                if left.trust >= Trust::Declared
                    && left.ty == Type::BigInt
                    && right.trust >= Trust::Declared
                    && right.ty == Type::BigInt
                {
                    return Val::proven(Type::BigInt).at_most(left.trust.min(right.trust));
                }
                Val::unknown()
            }
        }
    }

    fn logical(&mut self, logical: &'a oxc_ast::ast::LogicalExpression<'a>) -> Val {
        let left = self.expr(&logical.left);
        let (truthy, falsy) = self.condition_of_value(&logical.left, &left);
        let (evaluate_right_in, short_circuit_value, short_env) = match logical.operator {
            LogicalOperator::And => (truthy, narrow_falsy(&left), falsy),
            LogicalOperator::Or => (falsy, narrow_truthy(&left), truthy),
            LogicalOperator::Coalesce => {
                let nullish = self.env.clone();
                let non_nullish = left.ty.non_nullish();
                let short = Val {
                    ty: non_nullish,
                    ..left.clone()
                };
                (nullish, short, self.env.clone())
            }
        };
        self.env = evaluate_right_in;
        let right = self.expr(&logical.right);
        self.env = self.env.join(&short_env);
        short_circuit_value.join(&right)
    }

    fn update(&mut self, update: &'a UpdateExpression<'a>) -> Val {
        let old = self.simple_target_value(&update.argument);
        if is_never(&old) {
            return Val::never();
        }
        self.coerce(&old, false);
        let one = NumberType::constant(1.0);
        let new = if let Some(number) = number_operand(&old) {
            let number = if update.operator == UpdateOperator::Increment {
                numeric::add(number, one)
            } else {
                numeric::sub(number, one)
            };
            Val::number(number).at_most(old.trust)
        } else if old.trust >= Trust::Declared && old.ty == Type::BigInt {
            Val::proven(Type::BigInt).at_most(old.trust)
        } else {
            Val::unknown()
        };
        self.assign_simple(&update.argument, new.clone(), update.node_id.get());
        if update.prefix {
            new
        } else {
            // The old value, converted to a numeric.
            if let Some(number) = number_operand(&old) {
                Val::number(number).at_most(old.trust)
            } else {
                new
            }
        }
    }

    fn assignment(&mut self, assign: &'a oxc_ast::ast::AssignmentExpression<'a>) -> Val {
        let operator = assign.operator;
        if operator == AssignmentOperator::Assign {
            // A member target's object and key are evaluated before the right-hand side.
            if let Some(member) = assign
                .left
                .as_simple_assignment_target()
                .and_then(target_member)
            {
                let object = self.expr(member.object());
                let key = self.member_key(member);
                let value = self.expr(&assign.right);
                self.write_prop(&object, &key, &value, assign.node_id.get(), false);
                return value;
            }
            let value = self.expr(&assign.right);
            self.assign_target(&assign.left, value.clone(), assign.node_id.get());
            return value;
        }
        let Some(simple) = assign.left.as_simple_assignment_target() else {
            return Val::unknown();
        };
        // A member target's object and key are evaluated once, for the read and the write.
        let member = simple.as_member_expression();
        let evaluated = member.map(|member| (self.expr(member.object()), self.member_key(member)));
        let old = match (member, &evaluated) {
            (Some(member), Some((object, key))) => {
                let node = member.node_id();
                let value = self.read_prop_at(object, key, node, member);
                self.require_object(object);
                self.or_declared(value, node, member.span())
            }
            _ => self.simple_target_value(simple),
        };
        let store = |cx: &mut Self, value: Val| match &evaluated {
            Some((object, key)) => cx.write_prop(object, key, &value, assign.node_id.get(), false),
            None => cx.assign_simple(simple, value, assign.node_id.get()),
        };
        let value = if let Some(logical) = operator.to_logical_operator() {
            // `a ||= b` etc. assign only on one branch; the result is one of the two.
            let right = self.expr(&assign.right);
            let kept = match logical {
                LogicalOperator::Or => narrow_truthy(&old),
                LogicalOperator::And => narrow_falsy(&old),
                LogicalOperator::Coalesce => Val {
                    ty: old.ty.non_nullish(),
                    ..old.clone()
                },
            };
            let value = kept.join(&right);
            store(self, value.clone());
            return value;
        } else if let Some(binary) = operator.to_binary_operator() {
            let right = self.expr(&assign.right);
            self.binary_values(binary, &old, &right)
        } else {
            Val::unknown()
        };
        store(self, value.clone());
        value
    }

    /// The current value of a simple assignment target (for compound assignment).
    fn simple_target_value(&mut self, target: &'a SimpleAssignmentTarget<'a>) -> Val {
        match target {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) => {
                let value = self.identifier(ident);
                let value = self.or_declared(value, ident.node_id.get(), ident.span);
                self.record_node(ident.node_id.get(), &value);
                value
            }
            SimpleAssignmentTarget::TSAsExpression(cast) => self.expr(&cast.expression),
            SimpleAssignmentTarget::TSSatisfiesExpression(cast) => self.expr(&cast.expression),
            SimpleAssignmentTarget::TSNonNullExpression(cast) => self.expr(&cast.expression),
            SimpleAssignmentTarget::TSTypeAssertion(cast) => self.expr(&cast.expression),
            member => match member.as_member_expression() {
                Some(member) => self.member(member),
                None => Val::unknown(),
            },
        }
    }

    /// Assigns to a simple target whose object (for members) is evaluated again.
    fn assign_simple(
        &mut self,
        target: &'a SimpleAssignmentTarget<'a>,
        value: Val,
        node: oxc_syntax::node::NodeId,
    ) {
        match target {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) => {
                self.assign_identifier(ident, value);
            }
            SimpleAssignmentTarget::TSAsExpression(cast) => {
                self.assign_expression(&cast.expression, value, node);
            }
            SimpleAssignmentTarget::TSSatisfiesExpression(cast) => {
                self.assign_expression(&cast.expression, value, node);
            }
            SimpleAssignmentTarget::TSNonNullExpression(cast) => {
                self.assign_expression(&cast.expression, value, node);
            }
            SimpleAssignmentTarget::TSTypeAssertion(cast) => {
                self.assign_expression(&cast.expression, value, node);
            }
            member => {
                if let Some(member) = member.as_member_expression() {
                    self.assign_member(member, &value, node);
                }
            }
        }
    }

    fn assign_expression(
        &mut self,
        target: &'a Expression<'a>,
        value: Val,
        node: oxc_syntax::node::NodeId,
    ) {
        match target.without_parentheses() {
            Expression::Identifier(ident) => self.assign_identifier(ident, value),
            other => match other.as_member_expression() {
                Some(member) => self.assign_member(member, &value, node),
                None => self.escape_value(&value, Escape::Unknown),
            },
        }
    }

    fn assign_identifier(&mut self, ident: &'a oxc_ast::ast::IdentifierReference<'a>, value: Val) {
        let scoping = self.module.semantic.scoping();
        let symbol = ident
            .reference_id
            .get()
            .and_then(|reference| scoping.get_reference(reference).symbol_id());
        if let Some(symbol) = symbol {
            let flags = scoping.symbol_flags(symbol);
            if flags.contains(oxc_syntax::symbol::SymbolFlags::ConstVariable)
                || self.module.info(symbol).import
            {
                // Assigning a const or an import throws.
                self.always(EffectKind::Throws);
                return;
            }
            self.write_symbol(symbol, value, Some(ident.node_id.get()));
        } else {
            // A global: unknown code can see it.
            self.record_node(ident.node_id.get(), &value);
            self.escape_value(&value, Escape::Unknown);
            self.always(EffectKind::Writes);
            self.always(EffectKind::Throws);
        }
    }

    fn assign_member(
        &mut self,
        member: &'a MemberExpression<'a>,
        value: &Val,
        node: oxc_syntax::node::NodeId,
    ) {
        let object = self.expr(member.object());
        let key = self.member_key(member);
        self.write_prop(&object, &key, value, node, false);
    }

    /// Assigns `value` to any assignment target, destructuring as needed.
    pub fn assign_target(
        &mut self,
        target: &'a AssignmentTarget<'a>,
        value: Val,
        node: oxc_syntax::node::NodeId,
    ) {
        match target {
            AssignmentTarget::ArrayAssignmentTarget(array) => {
                let element = self.iterate(&value, array.node_id.get());
                for item in array.elements.iter().flatten() {
                    self.assign_maybe_default(
                        item,
                        element.join(&Val::proven(Type::Undefined)),
                        node,
                    );
                }
                if let Some(rest) = &array.rest {
                    let site = self.alloc(rest.node_id.get(), ObjKind::Array);
                    self.store_elem(site, &element);
                    self.mark(site, move |obj| obj.length = None);
                    self.assign_target(
                        &rest.target,
                        Val::object(Type::Array(std::sync::Arc::new(Type::Unknown)), site),
                        node,
                    );
                }
            }
            AssignmentTarget::ObjectAssignmentTarget(object) => {
                self.require_object(&value);
                for property in &object.properties {
                    match property {
                        AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(property) => {
                            let read = self.read_prop(
                                &value,
                                &Key::Name(property.binding.name.to_string()),
                                property.node_id.get(),
                            );
                            let read = match &property.init {
                                Some(init) => {
                                    let default = self.expr(init);
                                    Val {
                                        ty: read
                                            .ty
                                            .members()
                                            .iter()
                                            .filter(|m| **m != Type::Undefined)
                                            .cloned()
                                            .fold(Type::Never, |a, b| a.join(&b)),
                                        ..read
                                    }
                                    .join(&default)
                                }
                                None => read,
                            };
                            self.assign_identifier(&property.binding, read);
                        }
                        AssignmentTargetProperty::AssignmentTargetPropertyProperty(property) => {
                            let key = if property.computed {
                                match property.name.as_expression() {
                                    Some(key) => {
                                        let key = self.expr(key);
                                        Key::of_value(&key)
                                    }
                                    None => Key::Unknown,
                                }
                            } else {
                                static_key(&property.name, false).map_or(Key::Unknown, Key::Name)
                            };
                            let read = self.read_prop(&value, &key, property.node_id.get());
                            self.assign_maybe_default(&property.binding, read, node);
                        }
                    }
                }
                if let Some(rest) = &object.rest {
                    // `...rest` copies the remaining own properties into a new object.
                    let site = self.alloc(rest.node_id.get(), ObjKind::Object);
                    self.mark(site, move |obj| obj.dynamic = true);
                    self.escape_value(&value, Escape::Known);
                    self.assign_target(&rest.target, Val::object(Type::empty_object(), site), node);
                }
            }
            simple => {
                if let Some(simple) = simple.as_simple_assignment_target() {
                    self.assign_simple(simple, value, node);
                }
            }
        }
    }

    fn assign_maybe_default(
        &mut self,
        target: &'a AssignmentTargetMaybeDefault<'a>,
        value: Val,
        node: oxc_syntax::node::NodeId,
    ) {
        match target {
            AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(with_default) => {
                let default = self.expr(&with_default.init);
                let value = without_undefined(&value).join(&default);
                self.assign_target(&with_default.binding, value, node);
            }
            target => {
                if let Some(target) = target.as_assignment_target() {
                    self.assign_target(target, value, node);
                }
            }
        }
    }

    /// Notes that destructuring or property access requires `value` to be non-nullish.
    pub fn require_object(&mut self, value: &Val) {
        let proven = value.trust == Trust::Proven && !value.ty.may_be_nullish();
        let declared = value.trust >= Trust::Declared && !value.ty.may_be_nullish();
        self.effect(EffectKind::Throws, proven, declared);
    }

    /// The key a member expression reads, evaluating a computed key.
    pub fn member_key(&mut self, member: &'a MemberExpression<'a>) -> Key {
        match member {
            MemberExpression::StaticMemberExpression(member) => {
                Key::Name(member.property.name.to_string())
            }
            MemberExpression::PrivateFieldExpression(member) => {
                Key::Name(format!("#{}", member.field.name))
            }
            MemberExpression::ComputedMemberExpression(member) => {
                if let Some(name) = self.constant_string(&member.expression) {
                    let value = self.expr(&member.expression);
                    let _ = value;
                    return Key::Name(name);
                }
                let key = self.expr(&member.expression);
                if !key.is_primitive(false) {
                    self.coerce(&key, false);
                }
                Key::of_value(&key)
            }
        }
    }

    /// The value of a string literal, or of a `const` bound to one.
    pub fn constant_string(&self, expression: &Expression<'_>) -> Option<String> {
        match expression.without_parentheses() {
            Expression::StringLiteral(literal) => Some(literal.value.to_string()),
            Expression::Identifier(ident) => {
                let scoping = self.module.semantic.scoping();
                let symbol = scoping
                    .get_reference(ident.reference_id.get()?)
                    .symbol_id()?;
                if !scoping
                    .symbol_flags(symbol)
                    .contains(oxc_syntax::symbol::SymbolFlags::ConstVariable)
                    || self.module.info(symbol).tainted
                {
                    return None;
                }
                let declaration = scoping.symbol_declaration(symbol);
                match self.module.kind(declaration) {
                    oxc_ast::AstKind::VariableDeclarator(declarator) => match &declarator.init {
                        Some(Expression::StringLiteral(literal))
                            if matches!(
                                declarator.id,
                                oxc_ast::ast::BindingPattern::BindingIdentifier(_)
                            ) =>
                        {
                            Some(literal.value.to_string())
                        }
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Evaluates a member read.
    pub fn member(&mut self, member: &'a MemberExpression<'a>) -> Val {
        let node = member.node_id();
        if let Some(value) = self.builtin_member(member) {
            return value;
        }
        let object = self.expr(member.object());
        let key = self.member_key(member);
        let optional = member.optional();
        let mut value = self.read_prop_at(&object, &key, node, member);
        if optional && object.ty.may_be_nullish() {
            value = value.join(&Val::proven(Type::Undefined));
        } else {
            self.require_object(&object);
        }
        let span = member.span();
        self.or_declared(value, node, span)
    }

    /// `Math.PI` and other modelled builtin values, read through an unshadowed global.
    fn builtin_member(&mut self, member: &'a MemberExpression<'a>) -> Option<Val> {
        let MemberExpression::StaticMemberExpression(member) = member else {
            return None;
        };
        let Expression::Identifier(object) = &member.object else {
            return None;
        };
        let name = object.name.as_str();
        if self.an.patched.contains(name) || !self.is_global(object) {
            return None;
        }
        let path = format!("{name}.{}", member.property.name);
        let builtin = builtins::global(&path).filter(|builtin| !builtin.callable)?;
        self.record_node(object.node_id.get(), &Val::unknown());
        Some(self.builtin_ret(builtin, &[], None, member.node_id.get()))
    }

    /// Whether `ident` names a global (not a binding of the program).
    pub fn is_global(&self, ident: &oxc_ast::ast::IdentifierReference<'_>) -> bool {
        ident.reference_id.get().is_some_and(|reference| {
            self.module
                .semantic
                .scoping()
                .get_reference(reference)
                .symbol_id()
                .is_none()
        })
    }

    fn read_prop_at(
        &mut self,
        object: &Val,
        key: &Key,
        node: oxc_syntax::node::NodeId,
        member: &'a MemberExpression<'a>,
    ) -> Val {
        // Indexed reads of arrays: in bounds when a loop condition says so.
        if let MemberExpression::ComputedMemberExpression(computed) = member
            && key.is_index()
            && let Some(trust) = self.in_bounds(&computed.object, &computed.expression, object)
        {
            self.an.output.in_bounds.insert(self.node(node), trust);
            let element = self.element(object, true);
            return element;
        }
        self.read_prop(object, key, node)
    }

    /// Reads property `key` of `object`.
    pub fn read_prop(&mut self, object: &Val, key: &Key, node: oxc_syntax::node::NodeId) -> Val {
        self.note_shared_read();
        if is_never(object) || matches!(key, Key::Never) {
            return Val::never();
        }
        // Primitive receivers.
        if object.trust >= Trust::Declared && object.ty == Type::String {
            return match key.name() {
                Some("length") => Val::number(numeric::int_range(0.0, numeric::MAX_STRING_LENGTH))
                    .at_most(object.trust),
                _ if key.is_index() => {
                    Val::proven(Type::union([Type::String, Type::Undefined])).at_most(object.trust)
                }
                _ => Val::unknown(),
            };
        }
        let Some(sites) = object.pts.sites() else {
            self.untracked_read(object, key);
            return self.declared_container_read(object, key);
        };
        if sites.is_empty() {
            self.untracked_read(object, key);
            return self.declared_container_read(object, key);
        }
        let sites = sites.to_vec();
        let mut value = Val::never();
        for site in sites {
            let read = self.site_prop(site, key, object, node, 0);
            value = value.join(&read);
        }
        // Objects the analysis made: reading their props reads state unless the unit owns
        // them.
        value
    }

    /// Effects of reading a property of something not tracked.
    fn untracked_read(&mut self, object: &Val, key: &Key) {
        self.always(EffectKind::Reads);
        // A getter may run, unless the receiver is a primitive or a builtin container.
        let container = |declared: bool| {
            (object.trust == Trust::Proven || (declared && object.trust >= Trust::Declared))
                && (object.is_primitive(declared)
                    || matches!(
                        object.ty,
                        Type::Array(_) | Type::TypedArray(_) | Type::Tuple(_)
                    )
                    || (declared && matches!(object.ty, Type::Object(_) | Type::Instance(_))))
        };
        self.effect(EffectKind::CallsUnknown, container(false), container(true));
        let _ = key;
    }

    /// A read from an untracked array, typed array or object, using its (declared) type.
    #[expect(
        clippy::unused_self,
        reason = "a method like the other container reads"
    )]
    fn declared_container_read(&mut self, object: &Val, key: &Key) -> Val {
        match (&object.ty, key.name()) {
            (Type::Array(_) | Type::Tuple(_), Some("length")) => {
                Val::number(numeric::int_range(0.0, numeric::MAX_ARRAY_LENGTH))
                    .at_most(object.trust)
            }
            (Type::TypedArray(_), Some("length")) => {
                Val::number(numeric::int_range(0.0, numeric::MAX_TYPED_ARRAY_LENGTH))
                    .at_most(object.trust)
            }
            (Type::TypedArray(kind), _) if key.is_index() => match kind.element() {
                Some(element) => Val::proven(Type::union([Type::Number(element), Type::Undefined]))
                    .at_most(object.trust),
                None => {
                    Val::proven(Type::union([Type::BigInt, Type::Undefined])).at_most(object.trust)
                }
            },
            _ => Val::unknown(),
        }
    }

    /// What reading an array index with no element gives: `undefined`, unless an index
    /// could have been defined on `Array.prototype` or `Object.prototype`.
    fn hole(&self) -> Val {
        if self.an.patched.contains("Array") || self.an.patched.contains("Object") {
            Val::unknown()
        } else {
            Val::proven(Type::Undefined)
        }
    }

    /// The elements of an array value (`in_bounds`: no `undefined` for out-of-range reads).
    pub fn element(&mut self, object: &Val, in_bounds: bool) -> Val {
        self.note_shared_read();
        if is_never(object) {
            return Val::never();
        }
        let undefined = self.hole();
        let mut value = Val::never();
        let Some(sites) = object.pts.sites().filter(|sites| !sites.is_empty()) else {
            // Untracked: what the type says.
            return match &object.ty {
                Type::TypedArray(kind) if object.trust >= Trust::Declared => {
                    let element = kind.element().map_or(Type::BigInt, Type::Number);
                    let element = if in_bounds {
                        element
                    } else {
                        Type::union([element, Type::Undefined])
                    };
                    Val::proven(element).at_most(object.trust)
                }
                _ => Val::unknown(),
            };
        };
        for &site in sites {
            let Some(obj) = self.an.heap(site) else {
                continue;
            };
            if obj.poisoned {
                value = value.join(&Val::poisoned());
                continue;
            }
            let read = match obj.kind {
                ObjKind::TypedArray(kind) => {
                    // Typed array elements always have the kind's type, whoever writes them;
                    // out of range they are `undefined` whatever the prototypes hold.
                    let element = Val::proven(kind.element().map_or(Type::BigInt, Type::Number));
                    // A detached typed array reads `undefined` everywhere.
                    if in_bounds && obj.escape != Escape::Unknown {
                        element
                    } else {
                        element.join(&Val::proven(Type::Undefined))
                    }
                }
                ObjKind::Array if obj.escape != Escape::Unknown && !obj.dynamic => {
                    let mut element = obj.elem.clone();
                    if obj.holey {
                        element = element.join(&undefined);
                    }
                    element
                }
                _ => Val::unknown(),
            };
            value = value.join(&read);
            if obj.unit != self.unit.id || obj.escape != Escape::None {
                self.always(EffectKind::Reads);
            }
        }
        if !in_bounds {
            value = value.join(&undefined);
        }
        value
    }

    fn site_prop(
        &mut self,
        site: AllocId,
        key: &Key,
        receiver: &Val,
        node: oxc_syntax::node::NodeId,
        depth: usize,
    ) -> Val {
        let Some(obj) = self.an.heap(site).cloned() else {
            return Val::unknown();
        };
        if obj.unit != self.unit.id || obj.escape != Escape::None {
            self.always(EffectKind::Reads);
        }
        if obj.poisoned {
            self.always(EffectKind::CallsUnknown);
            return Val::poisoned();
        }
        if obj.escape == Escape::Unknown || depth > 8 {
            self.always(EffectKind::CallsUnknown);
            return Val::unknown();
        }
        match obj.kind {
            ObjKind::Array => {
                return match key {
                    Key::Name(name) if name == "length" => match obj.length {
                        Some(length) => Val::number(NumberType::constant(length)),
                        None => Val::number(numeric::int_range(0.0, numeric::MAX_ARRAY_LENGTH)),
                    },
                    key if key.is_index() => {
                        if obj.dynamic {
                            return Val::unknown();
                        }
                        let in_range = match (key.index_range(), obj.length) {
                            (Some((_, max)), Some(length)) => max < length && !obj.holey,
                            _ => false,
                        };
                        if in_range {
                            obj.elem.clone()
                        } else {
                            obj.elem.join(&self.hole())
                        }
                    }
                    // Array.prototype methods and unknown keys.
                    _ => Val::unknown(),
                };
            }
            ObjKind::TypedArray(kind) => {
                return match key {
                    Key::Name(name) if name == "buffer" => {
                        // Whoever holds the buffer can detach the array.
                        self.an.escape(&Pts::site(site), Escape::Unknown);
                        Val::unknown()
                    }
                    Key::Name(name) if name == "length" => match obj.length {
                        Some(length) => Val::number(NumberType::constant(length)),
                        None => {
                            Val::number(numeric::int_range(0.0, numeric::MAX_TYPED_ARRAY_LENGTH))
                        }
                    },
                    key if key.is_index() => {
                        let element = kind.element().map_or(Type::BigInt, Type::Number);
                        // A typed array's length only changes if code holding it detaches it.
                        let in_range =
                            match (key.name().and_then(|n| n.parse::<f64>().ok()), obj.length) {
                                (Some(index), Some(length)) => index < length,
                                _ => false,
                            };
                        if in_range {
                            Val::proven(element)
                        } else {
                            Val::proven(Type::union([element, Type::Undefined]))
                        }
                    }
                    _ => Val::unknown(),
                };
            }
            ObjKind::Opaque => return Val::unknown(),
            ObjKind::Namespace(module) => {
                let Some(name) = key.name() else {
                    // Any export may be read.
                    self.an.escape(&Pts::site(site), Escape::Unknown);
                    return Val::unknown();
                };
                return self.namespace_member(module, name);
            }
            _ => {}
        }
        let Some(name) = key.name() else {
            self.always(EffectKind::CallsUnknown);
            return Val::unknown();
        };
        // Accessors defined by the class run code.
        let class = match obj.kind {
            ObjKind::Instance(class) | ObjKind::Prototype(class) => Some((class, false)),
            ObjKind::Class(class) => Some((class, true)),
            _ => None,
        };
        if let Some((class, is_static)) = class
            && let getter @ (Accessor::Unit(_) | Accessor::Missing) = self.accessor(
                class,
                name,
                super::super::world::MethodRole::Getter,
                is_static,
            )
        {
            let value = match getter {
                Accessor::Unit(getter) => self.call_unit_as_getter(getter, receiver, node),
                _ => Val::proven(Type::Undefined),
            };
            // A field may define an own property of the same name, shadowing the accessor
            // (`useDefineForClassFields`).
            return match obj.props.get(name) {
                Some(own) => value.join(own),
                None => value,
            };
        }
        if obj.dynamic {
            self.always(EffectKind::CallsUnknown);
            return Val::unknown();
        }
        if let Some(value) = obj.props.get(name) {
            let in_creator = self.inside_creator(&obj);
            if obj.present.contains(name) && !in_creator {
                return value.clone();
            }
            return value.join(&self.inherited(site, &obj, key, receiver, node, depth));
        }
        // A field the class sets up that nothing wrote yet: its initialiser may run in
        // another chunk of this sweep. Until the sweeps settle, nothing is known yet.
        if self.an.optimistic
            && let ObjKind::Instance(class) = obj.kind
            && self
                .an
                .instance_fields
                .get(&class)
                .is_some_and(|fields| fields.contains(name))
        {
            self.an.output.optimistic = true;
            return Val::never();
        }
        self.inherited(site, &obj, key, receiver, node, depth)
    }

    /// The export `name` of `module`, read through its namespace object.
    fn namespace_member(&mut self, module: trioxide_common::ModuleId, name: &str) -> Val {
        let world = self.an.world;
        #[expect(
            clippy::match_same_arms,
            reason = "distinct cases, documented separately"
        )]
        match crate::link::resolve_export(
            world,
            world.module(module),
            name,
            &mut rustc_hash::FxHashSet::default(),
        ) {
            crate::link::Target::Symbol(target) => {
                self.always(EffectKind::Reads);
                if world.module(target.module).info(target.symbol).tainted {
                    return Val::poisoned();
                }
                self.an
                    .symbol_value(target)
                    .cloned()
                    .unwrap_or_else(Val::never)
            }
            crate::link::Target::Namespace(inner) => {
                let unit = self.unit.id;
                self.an.namespace(inner, unit)
            }
            crate::link::Target::Expression(node) if !world.module(node.module).broken => self
                .an
                .default_value(node)
                .cloned()
                .unwrap_or_else(Val::never),
            crate::link::Target::Expression(_) => Val::unknown(),
            // Namespaces have no other properties (and no prototype).
            crate::link::Target::Unresolved => Val::proven(Type::Undefined),
            crate::link::Target::External => Val::unknown(),
        }
    }

    /// Whether the current unit is (in) the code that builds objects of this site, where
    /// properties may not be initialised yet.
    fn inside_creator(&self, obj: &super::Obj) -> bool {
        match obj.kind {
            ObjKind::Instance(class) => {
                let Some(info) = self.an.world.classes.get(&class) else {
                    return true;
                };
                let mut unit = Some(self.unit.id);
                while let Some(id) = unit {
                    if Some(id) == info.constructor || id == info.init {
                        return true;
                    }
                    unit = self.an.world.unit(id).parent;
                }
                false
            }
            _ => false,
        }
    }

    /// A property not own to `obj`: the prototype chain.
    fn inherited(
        &mut self,
        _site: AllocId,
        obj: &super::Obj,
        key: &Key,
        receiver: &Val,
        node: oxc_syntax::node::NodeId,
        depth: usize,
    ) -> Val {
        // Names no prototype in the chain defines give `undefined` only while
        // `Object.prototype` (and `Function.prototype`) can't have been patched.
        let undefined = if self.an.patched.contains("Object") {
            Val::unknown()
        } else {
            Val::proven(Type::Undefined)
        };
        let name = key.name().unwrap_or_default();
        match obj.kind {
            ObjKind::Object => {
                if builtins::OBJECT_PROTOTYPE.contains(&name) {
                    Val::unknown()
                } else {
                    undefined
                }
            }
            ObjKind::Instance(class) => {
                let Some(info) = self.an.world.classes.get(&class) else {
                    return Val::unknown();
                };
                let proto = info.init;
                self.site_prop(proto, key, receiver, node, depth + 1)
            }
            ObjKind::Prototype(class) => {
                if name == "constructor" {
                    return self.an.class_value(class);
                }
                self.super_prop(class, key, receiver, node, depth, true)
            }
            ObjKind::Class(class) => {
                if name == "prototype" {
                    let Some(info) = self.an.world.classes.get(&class) else {
                        return Val::unknown();
                    };
                    return Val::object(Type::Unknown, info.init).at_most(Trust::Proven);
                }
                if matches!(
                    name,
                    "name" | "length" | "call" | "apply" | "bind" | "toString" | "constructor"
                ) {
                    return Val::unknown();
                }
                self.super_prop(class, key, receiver, node, depth, false)
            }
            ObjKind::Function(_) => {
                if matches!(
                    name,
                    "prototype"
                        | "name"
                        | "length"
                        | "call"
                        | "apply"
                        | "bind"
                        | "toString"
                        | "constructor"
                        | "caller"
                        | "arguments"
                ) {
                    Val::unknown()
                } else {
                    undefined
                }
            }
            _ => Val::unknown(),
        }
    }

    /// A property looked up on a class's superclass (its prototype, or itself for statics).
    fn super_prop(
        &mut self,
        class: trioxide_common::NodeRef,
        key: &Key,
        receiver: &Val,
        node: oxc_syntax::node::NodeId,
        depth: usize,
        prototype: bool,
    ) -> Val {
        let has_heritage = self
            .an
            .world
            .classes
            .get(&class)
            .is_some_and(|info| info.has_heritage);
        if !has_heritage {
            let name = key.name().unwrap_or_default();
            return if builtins::OBJECT_PROTOTYPE.contains(&name)
                || !prototype && matches!(name, "name" | "length")
                || self.an.patched.contains("Object")
            {
                Val::unknown()
            } else {
                Val::proven(Type::Undefined)
            };
        }
        let parent = self.an.super_of(class).cloned().unwrap_or_else(Val::never);
        let Some(sites) = parent.pts.sites().filter(|_| parent.trust == Trust::Proven) else {
            return Val::unknown();
        };
        let mut value = Val::never();
        for &site in sites {
            let Some(parent_class) = self.an.heap(site).and_then(|obj| match obj.kind {
                ObjKind::Class(parent) => Some(parent),
                _ => None,
            }) else {
                // `extends null` or a non-class.
                return Val::unknown();
            };
            let target = if prototype {
                self.an
                    .world
                    .classes
                    .get(&parent_class)
                    .map(|info| info.init)
            } else {
                Some(parent_class)
            };
            let Some(target) = target else {
                return Val::unknown();
            };
            let read = self.site_prop(target, key, receiver, node, depth + 1);
            value = value.join(&read);
        }
        value
    }

    /// Writes property `key` of `object`.
    pub fn write_prop(
        &mut self,
        object: &Val,
        key: &Key,
        value: &Val,
        node: oxc_syntax::node::NodeId,
        defining: bool,
    ) {
        // A key with no value: the store can't happen (a run on stale inputs).
        if matches!(key, Key::Never) {
            return;
        }
        self.note_write(true);
        self.require_object(object);
        if matches!(key, Key::Unknown) || key.name() == Some("length") {
            self.forget_bounds_of(object);
        }
        // `length`, or an index that may be past the end.
        if matches!(key, Key::Unknown)
            || key.name() == Some("length")
            || (key.is_index() && !self.an.output.in_bounds.contains_key(&self.node(node)))
        {
            self.note_resize(object);
        }
        let Some(sites) = object
            .pts
            .sites()
            .filter(|sites| !sites.is_empty())
            .map(<[_]>::to_vec)
        else {
            // An untracked object: unknown code may see the value, and setters may run.
            self.escape_value(value, Escape::Unknown);
            self.always(EffectKind::Writes);
            let container = object.trust >= Trust::Declared
                && matches!(
                    object.ty,
                    Type::Array(_) | Type::TypedArray(_) | Type::Tuple(_)
                );
            self.effect(EffectKind::CallsUnknown, false, container);
            return;
        };
        for site in sites {
            let Some(obj) = self.an.heap(site).cloned() else {
                continue;
            };
            if obj.unit != self.unit.id || obj.escape != Escape::None {
                self.always(EffectKind::Writes);
            }
            // Writing to a frozen object throws in strict code.
            if obj.frozen && self.unit.strict && !defining {
                self.always(EffectKind::Throws);
            }
            if obj.escape == Escape::Unknown {
                self.escape_value(value, Escape::Unknown);
                self.always(EffectKind::CallsUnknown);
                continue;
            }
            match obj.kind {
                ObjKind::Array => {
                    if key.name() == Some("length") {
                        self.mark(site, move |obj| {
                            obj.length = None;
                            obj.holey = true;
                        });
                        self.coerce(value, true);
                    } else if key.is_index() {
                        let index_in_bounds =
                            self.an.output.in_bounds.contains_key(&self.node(node));
                        self.store_elem(site, value);
                        if !index_in_bounds {
                            // An index the key's range keeps below the length writes an element
                            // that exists: no hole, no new length.
                            let range = key.index_range();
                            self.mark(site, move |obj| {
                                let within = matches!((range, obj.length), (Some((_, max)), Some(len)) if max < len);
                                if !within {
                                    obj.length = None;
                                    obj.holey = true;
                                }
                            });
                        }
                    } else if matches!(key, Key::Unknown) {
                        // May be `length` or any index: the length is no longer known, and
                        // the value may be an element.
                        self.store_elem(site, value);
                        self.mark(site, |obj| {
                            obj.dynamic = true;
                            obj.length = None;
                            obj.holey = true;
                        });
                        self.coerce(value, true);
                        self.escape_value(value, Escape::Known);
                    } else {
                        self.mark(site, move |obj| obj.dynamic = true);
                        self.escape_value(value, Escape::Known);
                    }
                }
                ObjKind::TypedArray(_) => {
                    // Stored as a number; coercion may run user code.
                    self.coerce(value, false);
                }
                ObjKind::Opaque => {
                    self.escape_value(value, Escape::Unknown);
                    self.always(EffectKind::CallsUnknown);
                }
                ObjKind::Namespace(_) => {
                    // Namespace objects are not writable.
                    self.always(EffectKind::Throws);
                }
                _ => {
                    if obj.accessors && !defining {
                        // A setter (its own, or its prototype's) may run.
                        self.escape_value(value, Escape::Unknown);
                        self.always(EffectKind::CallsUnknown);
                        self.note_call();
                        self.after_call(false);
                    }
                    let Some(name) = key.name() else {
                        self.mark(site, move |obj| obj.dynamic = true);
                        self.escape_value(value, Escape::Known);
                        continue;
                    };
                    let class = match obj.kind {
                        ObjKind::Instance(class) => Some((class, false)),
                        ObjKind::Class(class) => Some((class, true)),
                        _ => None,
                    };
                    if let Some((class, is_static)) = class
                        && let setter @ (Accessor::Unit(_) | Accessor::Missing) = self.accessor(
                            class,
                            name,
                            super::super::world::MethodRole::Setter,
                            is_static,
                        )
                    {
                        match setter {
                            Accessor::Unit(setter) => {
                                self.call_unit_as_setter(setter, object, value, node);
                            }
                            // A getter without a setter: class code is strict, so this throws.
                            _ if !defining => self.always(EffectKind::Throws),
                            _ => {}
                        }
                        if !defining {
                            continue;
                        }
                        // A field definition may define an own property instead (define
                        // semantics) or assign through the setter: both are possible.
                    }
                    let name = name.to_string();
                    let defining_own = defining
                        || (matches!(obj.kind, ObjKind::Instance(_)) && self.inside_creator(&obj));
                    self.store_prop(site, &name, value, defining_own);
                }
            }
        }
    }

    fn delete(&mut self, object: &Val, key: &Key) {
        if matches!(key, Key::Never) {
            return;
        }
        self.note_write(true);
        if let Some(sites) = object.pts.sites().filter(|sites| !sites.is_empty()) {
            // In strict code, deleting a non-configurable own property throws; so does
            // deleting from `null` or `undefined`, in any code.
            let mut throws = object.ty.may_be_nullish() || object.trust != Trust::Proven;
            if self.unit.strict {
                throws |= match key.name() {
                    None => true,
                    Some(name) => sites.iter().any(|&site| {
                        self.an
                            .heap(site)
                            .is_none_or(|obj| non_configurable(obj, name))
                    }),
                };
            }
            if throws {
                self.always(EffectKind::Throws);
            }
            for &site in sites {
                let name = key.name().map(str::to_string);
                self.mark(site, move |obj| {
                    obj.deleted = true;
                    match &name {
                        Some(name) => {
                            obj.present.remove(name);
                        }
                        None => obj.dynamic = true,
                    }
                    if obj.kind == ObjKind::Array {
                        obj.holey = true;
                    }
                });
            }
            self.always(EffectKind::Writes);
        } else {
            self.always(EffectKind::Writes);
            self.always(EffectKind::CallsUnknown);
            self.always(EffectKind::Throws);
        }
    }

    /// Whether iterating `value` is proven to use only the builtin iterators (a proven array,
    /// typed array or string, with their prototypes intact): no program code runs.
    pub fn iterates_natively(&self, value: &Val) -> bool {
        // Their iterators come from the prototypes.
        let intact = ["Array", "String", "Object", super::TYPED_ARRAY]
            .iter()
            .all(|name| !self.an.patched.contains(*name));
        intact
            && value.trust == Trust::Proven
            && value.ty.members().iter().all(|member| {
                matches!(
                    member,
                    Type::Array(_) | Type::TypedArray(_) | Type::Tuple(_) | Type::String
                )
            })
            && !value.ty.may_be_nullish()
    }

    /// The values an iteration over `value` (for-of, spread, array destructuring) produces.
    pub fn iterate(&mut self, value: &Val, node: oxc_syntax::node::NodeId) -> Val {
        let _ = node;
        if is_never(value) {
            return Val::never();
        }
        let intact = ["Array", "String", "Object", super::TYPED_ARRAY]
            .iter()
            .all(|name| !self.an.patched.contains(*name));
        let array_like = self.iterates_natively(value);
        let declared_array_like = intact
            && value.trust >= Trust::Declared
            && value.ty.members().iter().all(|member| {
                matches!(
                    member,
                    Type::Array(_) | Type::TypedArray(_) | Type::Tuple(_) | Type::String
                )
            });
        // Array.prototype[Symbol.iterator] is assumed intact. Whether user iterator code runs
        // never rests on declared types alone (a user iterable passes for `Iterable<T>`).
        self.effect(EffectKind::CallsUnknown, array_like, array_like);
        self.effect(EffectKind::Throws, array_like, array_like);
        if value.trust >= Trust::Declared && value.ty == Type::String {
            return Val::proven(Type::String).at_most(value.trust);
        }
        if !array_like {
            self.escape_value(value, Escape::Unknown);
            if declared_array_like {
                return match &value.ty {
                    Type::Array(element) => Val::declared((**element).clone()),
                    Type::Tuple(items) => Val::declared(Type::union(items.iter().cloned())),
                    Type::TypedArray(kind) => {
                        Val::proven(kind.element().map_or(Type::BigInt, Type::Number))
                            .at_most(Trust::Declared)
                    }
                    _ => Val::unknown(),
                };
            }
            return Val::unknown();
        }
        // Within the length, so only holes read as `undefined`.
        self.element(value, true)
    }
}

/// `ToNumber` of a primitive value.
pub fn to_number(value: &Val) -> NumberType {
    let mut result: Option<NumberType> = None;
    for member in value.ty.members() {
        let number = match member {
            Type::Number(number) => *number,
            Type::Boolean => numeric::boolean(),
            Type::Null => NumberType::constant(0.0),
            _ => NumberType::ANY,
        };
        result = Some(result.map_or(number, |r| r.join(number)));
    }
    result.unwrap_or(NumberType::ANY)
}

/// The operand as a number, if it is (at any trust level) a primitive that converts to one.
fn number_operand(value: &Val) -> Option<NumberType> {
    if value.trust == Trust::Unknown
        || !value.ty.members().iter().all(|member| {
            matches!(
                member,
                Type::Number(_) | Type::Boolean | Type::Null | Type::Undefined
            )
        })
    {
        return None;
    }
    Some(to_number(value))
}

fn numeric_or_any(
    op: fn(NumberType, NumberType) -> NumberType,
    left: &Val,
    right: &Val,
) -> NumberType {
    match (number_operand(left), number_operand(right)) {
        (Some(left), Some(right)) => op(left, right),
        (Some(left), None) => op(left, NumberType::ANY),
        (None, Some(right)) => op(NumberType::ANY, right),
        (None, None) => op(NumberType::ANY, NumberType::ANY),
    }
}

/// The proven result of a numeric operator, at the operands' trust when their ranges came
/// from declared types.
fn result_with(
    op: fn(NumberType, NumberType) -> NumberType,
    left: &Val,
    right: &Val,
    only_kind_proven: bool,
    result: NumberType,
) -> NumberType {
    let proven = |value: &Val| value.trust == Trust::Proven;
    if only_kind_proven
        || !(proven(left) || number_operand(left).is_none())
        || !(proven(right) || number_operand(right).is_none())
    {
        // The result is a number for sure, but its range rests on declared operands: use the
        // range of any number for the unproven side.
        let l = if proven(left) {
            number_operand(left).unwrap_or(NumberType::ANY)
        } else {
            NumberType::ANY
        };
        let r = if proven(right) {
            number_operand(right).unwrap_or(NumberType::ANY)
        } else {
            NumberType::ANY
        };
        return op(l, r);
    }
    result
}

/// `value` narrowed to the truthy case.
pub fn narrow_truthy(value: &Val) -> Val {
    let ty = Type::union(value.ty.members().iter().filter_map(|member| match member {
        Type::Null | Type::Undefined => None,
        Type::Number(number) => {
            let mut number = *number;
            number.non_nan = true;
            if number.int && number.range.min == 0.0 {
                number.range.min = 1.0;
            }
            if number.int && number.range.max == 0.0 {
                number.range.max = -1.0;
            }
            (number.range.min <= number.range.max).then_some(Type::Number(number))
        }
        other => Some(other.clone()),
    }));
    Val {
        ty,
        ..value.clone()
    }
}

/// `value` narrowed to the falsy case.
pub fn narrow_falsy(value: &Val) -> Val {
    let ty = Type::union(value.ty.members().iter().filter_map(|member| match member {
        Type::Object(_)
        | Type::Array(_)
        | Type::Tuple(_)
        | Type::TypedArray(_)
        | Type::Instance(_)
        | Type::Function(..)
        | Type::Symbol => None,
        Type::Number(number) => {
            let zero = number.range.min <= 0.0 && number.range.max >= 0.0;
            match (zero, number.non_nan) {
                (false, true) => None,
                (false, false) => Some(Type::Number(NumberType {
                    range: number.range,
                    ..NumberType::ANY
                })),
                (true, _) => Some(Type::Number(NumberType {
                    range: trioxide_types::Interval::point(0.0),
                    int: true,
                    non_nan: number.non_nan,
                    non_neg_zero: number.non_neg_zero,
                })),
            }
        }
        other => Some(other.clone()),
    }));
    Val {
        ty,
        ..value.clone()
    }
}

/// The value without `undefined` (a default parameter or destructuring default replaces it).
pub fn without_undefined(value: &Val) -> Val {
    Val {
        ty: Type::union(
            value
                .ty
                .members()
                .iter()
                .filter(|member| **member != Type::Undefined)
                .cloned(),
        ),
        ..value.clone()
    }
}

/// Whether the value has no possible value (unreached so far, or always throws).
pub fn is_never(value: &Val) -> bool {
    value.ty == Type::Never && value.pts.is_none()
}

/// Whether `name` is an array index: the canonical form of an integer below 2^32 − 1
/// (`"01"`, `"+1"` and `"1.0"` are ordinary property names).
pub fn is_array_index(name: &str) -> bool {
    name.parse::<u32>()
        .is_ok_and(|index| index != u32::MAX && index.to_string() == name)
}

/// The member expression a simple assignment target writes, through TypeScript casts and
/// parentheses.
fn target_member<'s, 'a>(
    target: &'s SimpleAssignmentTarget<'a>,
) -> Option<&'s MemberExpression<'a>> {
    let inner = match target {
        SimpleAssignmentTarget::TSAsExpression(cast) => &cast.expression,
        SimpleAssignmentTarget::TSSatisfiesExpression(cast) => &cast.expression,
        SimpleAssignmentTarget::TSNonNullExpression(cast) => &cast.expression,
        SimpleAssignmentTarget::TSTypeAssertion(cast) => &cast.expression,
        member => return member.as_member_expression(),
    };
    inner.without_parentheses().as_member_expression()
}

/// Whether `name` may be a non-configurable own property of the objects made at `obj`'s
/// site (so deleting it in strict code throws).
fn non_configurable(obj: &super::Obj, name: &str) -> bool {
    // Code trioxide can't see may have frozen, sealed or redefined it.
    if obj.frozen || obj.poisoned || obj.escape == Escape::Unknown {
        return true;
    }
    match obj.kind {
        ObjKind::Array => name == "length",
        ObjKind::TypedArray(_) => is_array_index(name),
        ObjKind::Class(_) | ObjKind::Function(_) => name == "prototype",
        ObjKind::Namespace(_) | ObjKind::Opaque => true,
        ObjKind::Object | ObjKind::Instance(_) | ObjKind::Prototype(_) => false,
    }
}
