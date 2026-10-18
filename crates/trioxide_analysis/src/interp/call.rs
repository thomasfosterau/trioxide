//! Calls: to units of the program, to modelled builtins, and to unknown code.

use oxc_ast::ast::{Argument, CallExpression, Expression, MemberExpression, NewExpression};
use oxc_syntax::node::NodeId;
use trioxide_common::NodeRef;
use trioxide_types::{NumberType, Type, TypedArrayKind};

use super::{
    Cx, Edge, EffectKind, Escape, ObjKind,
    expr::{Key, to_number},
};
use crate::{
    builtins::{self, Builtin, CallbackParam, MathOp, NumberModel, Receiver, Ret},
    facts::Trust,
    numeric,
    value::{Pts, Val},
    world::{Body, MethodRole},
};

/// The evaluated arguments of a call.
pub struct Args {
    /// Positional values (a spread contributes its elements to every later position).
    pub values: Vec<Val>,
    /// Whether a spread makes positions unknown.
    pub spread: bool,
}

impl<'a> Cx<'_, '_, 'a> {
    fn arguments(&mut self, arguments: &'a [Argument<'a>]) -> Args {
        let mut values = Vec::with_capacity(arguments.len());
        let mut spread = false;
        for argument in arguments {
            match argument {
                Argument::SpreadElement(element) => {
                    let value = self.expr(&element.argument);
                    let item = self.iterate(&value, element.node_id.get());
                    values.push(item.join(&Val::proven(Type::Undefined)));
                    spread = true;
                }
                argument => {
                    if let Some(expression) = argument.as_expression() {
                        values.push(self.expr(expression));
                    }
                }
            }
        }
        Args { values, spread }
    }

    /// A call expression.
    pub fn call(&mut self, call: &'a CallExpression<'a>) -> Val {
        self.call_with(
            call.node_id.get(),
            &call.callee,
            call.optional,
            |cx: &mut Self| cx.arguments(&call.arguments),
        )
    }

    /// A call at `site` of `callee` (a method call when it is a member expression), with
    /// the arguments `args` evaluates (once, after the callee): calls and tagged templates.
    pub fn call_with(
        &mut self,
        site: NodeId,
        callee: &'a Expression<'a>,
        optional: bool,
        args: impl FnOnce(&mut Self) -> Args,
    ) -> Val {
        let whole = callee;
        let callee = callee.without_parentheses();
        if let Expression::Super(_) = callee {
            let args = args(self);
            self.super_call(site, &args);
            return Val::proven(Type::Undefined);
        }
        if let Some(member) = callee.as_member_expression() {
            // Modelled global functions: `Math.floor(x)`.
            if let MemberExpression::StaticMemberExpression(static_member) = member
                && let Expression::Identifier(object) = &static_member.object
                && !self.an.patched.contains(object.name.as_str())
                && self.is_global(object)
                && let Some(builtin) =
                    builtins::global(&format!("{}.{}", object.name, static_member.property.name))
                        .filter(|builtin| builtin.callable)
            {
                self.record_node(object.node_id.get(), &Val::unknown());
                let args = args(self);
                return self.builtin_call(builtin, &args, None, site, Trust::Proven);
            }
            // Reflection that redefines properties or prototypes poisons its target.
            if let MemberExpression::StaticMemberExpression(static_member) = member
                && let Expression::Identifier(object) = &static_member.object
                && self.is_global(object)
                && is_reflective(object.name.as_str(), static_member.property.name.as_str())
            {
                self.record_node(object.node_id.get(), &Val::unknown());
                let args = args(self);
                for value in &args.values {
                    self.an.poison(&value.pts);
                }
                self.always(EffectKind::CallsUnknown);
                self.always(EffectKind::Writes);
                self.always(EffectKind::Throws);
                self.after_call(false);
                let returned = args.values.first().cloned().unwrap_or_else(Val::unknown);
                return Val {
                    poisoned: true,
                    trust: Trust::Unknown,
                    ty: Type::Unknown,
                    ..returned
                };
            }
            let receiver = if let Expression::Super(_) = member.object() {
                self.this.clone()
            } else {
                self.expr(member.object())
            };
            let key = self.member_key(member);
            // Modelled methods of primitive and builtin receivers.
            if let Some(name) = key_name(&key)
                && let Some((kind, trust)) = self.receiver_kind(&receiver)
                && let Some(builtin) = builtins::method_of(kind, name)
            {
                let args = args(self);
                return self.builtin_call(builtin, &args, Some(&receiver), site, trust);
            }
            if !member.optional() {
                self.require_object(&receiver);
            }
            if let Expression::Super(_) = member.object() {
                let function = self.super_method(&key);
                self.record_node(member.node_id(), &function);
                let args = args(self);
                return self.call_value(
                    site,
                    &function,
                    Some(&receiver),
                    &args.values,
                    args.spread,
                );
            }
            // Dispatch per receiver object, so each method sees only the objects it is
            // actually looked up on as `this`.
            let per_site: Vec<Val> = match receiver.pts.sites() {
                Some(sites) if sites.len() > 1 => sites
                    .iter()
                    .map(|&object| {
                        let ty = match self.an.heap(object).map(|obj| obj.kind) {
                            Some(ObjKind::Instance(class)) => Type::Instance(class),
                            _ => receiver.ty.clone(),
                        };
                        Val {
                            ty,
                            pts: Pts::site(object),
                            ..receiver.clone()
                        }
                    })
                    .collect(),
                _ => vec![receiver.clone()],
            };
            let functions: Vec<Val> = per_site
                .iter()
                .map(|receiver| self.read_prop(receiver, &key, member.node_id()))
                .collect();
            let function = functions.iter().fold(Val::never(), |acc, f| acc.join(f));
            self.record_node(member.node_id(), &function);
            let args = args(self);
            let mut value = Val::never();
            for (receiver, function) in per_site.iter().zip(&functions) {
                let result =
                    self.call_value(site, function, Some(receiver), &args.values, args.spread);
                value = value.join(&result);
            }
            if member.optional() && receiver.ty.may_be_nullish() {
                value = value.join(&Val::proven(Type::Undefined));
            }
            return value;
        }
        let function = self.expr(whole);
        let args = args(self);
        let mut value = self.call_value(site, &function, None, &args.values, args.spread);
        if optional && function.ty.may_be_nullish() {
            value = value.join(&Val::proven(Type::Undefined));
        }
        value
    }

    /// Calls whatever `function` may be with `this` (if a method call) and `args`.
    pub fn call_value(
        &mut self,
        site: NodeId,
        function: &Val,
        this: Option<&Val>,
        args: &[Val],
        spread: bool,
    ) -> Val {
        if super::expr::is_never(function) {
            return Val::never();
        }
        let this = this
            .cloned()
            .unwrap_or_else(|| Val::proven(Type::Undefined));
        let mut result = Val::never();
        let mut unknown = false;
        match function.pts.sites() {
            Some(sites) => {
                for &target in sites {
                    match self.an.heap(target).map(|obj| obj.kind) {
                        Some(ObjKind::Function(unit)) => {
                            let value = self.call_unit(site, unit, &this, args, spread);
                            result = result.join(&value);
                        }
                        Some(ObjKind::Class(_)) => {
                            // Calling a class without `new` throws.
                            self.always(EffectKind::Throws);
                        }
                        _ => unknown = true,
                    }
                }
                if sites.is_empty() {
                    // Not an object: calling it throws (or it is a primitive TypeScript says is
                    // callable, which can't be).
                    self.always(EffectKind::Throws);
                    if function.trust != Trust::Proven {
                        unknown = true;
                    }
                }
            }
            None => unknown = true,
        }
        if unknown {
            result = result.join(&self.unknown_call(site, function, &this, args));
        }
        self.after_call(false);
        result
    }

    /// A call to a unit of the program.
    fn call_unit(
        &mut self,
        site: NodeId,
        unit: NodeRef,
        this: &Val,
        args: &[Val],
        spread: bool,
    ) -> Val {
        self.note_call();
        let world = self.an.world;
        let callee = world.unit(unit);
        let open = self.an.is_open(callee);
        let level = if open { Escape::Unknown } else { Escape::Known };
        let count = param_count(callee).max(args.len());
        for index in 0..count {
            let value = args
                .get(index)
                .cloned()
                .unwrap_or_else(|| Val::proven(Type::Undefined));
            self.an.join_param(unit, index, &value);
        }
        for value in args {
            self.escape_value(value, level);
        }
        if spread && self.an.state.spread_called.insert(unit) {
            self.an.touch(super::Dep::Params(unit));
        }
        self.an.join_this(unit, this);
        self.escape_value(this, level);
        self.record.edges.push(Edge {
            site: self.node(site),
            callee: unit,
            trust: Trust::Proven,
        });
        if callee.role == Some(MethodRole::Constructor) {
            self.always(EffectKind::Throws);
            return Val::never();
        }
        if callee.is_async_or_generator() {
            // A promise or a generator object.
            self.always(EffectKind::Allocates);
            return Val::unknown();
        }
        self.an.returned(unit).cloned().unwrap_or_else(Val::never)
    }

    /// A call trioxide can't follow.
    fn unknown_call(&mut self, site: NodeId, function: &Val, this: &Val, args: &[Val]) -> Val {
        self.note_call();
        let site_ref = self.node(site);
        self.record.open_sites.insert(site_ref);
        for value in args {
            self.escape_value(value, Escape::Unknown);
        }
        self.escape_value(this, Escape::Unknown);
        self.escape_value(function, Escape::Unknown);
        // A method TypeScript says the receiver's class has.
        let declared = self.declared_method(this, site);
        let callable =
            function.trust >= Trust::Declared && matches!(function.ty, Type::Function(..));
        self.effect(EffectKind::CallsUnknown, false, declared);
        self.effect(EffectKind::Throws, false, declared && callable);
        if !declared {
            self.effect(EffectKind::Reads, false, false);
            self.effect(EffectKind::Writes, false, false);
            self.effect(EffectKind::Allocates, false, false);
        }
        Val::unknown()
    }

    /// For a call `receiver.m()` where only TypeScript says the receiver is an instance of a
    /// program class: a declared edge to that class's method. Returns whether it found one.
    fn declared_method(&mut self, receiver: &Val, site: NodeId) -> bool {
        let Type::Instance(class) = receiver.ty else {
            return false;
        };
        if receiver.trust < Trust::Declared {
            return false;
        }
        let Some(name) = self.callee_name(site) else {
            return false;
        };
        let Some(unit) = self.class_method(class, &name) else {
            return false;
        };
        self.record.edges.push(Edge {
            site: self.node(site),
            callee: unit,
            trust: Trust::Declared,
        });
        true
    }

    fn callee_name(&self, site: NodeId) -> Option<String> {
        let oxc_ast::AstKind::CallExpression(call) = self.module.kind(site) else {
            return None;
        };
        match call.callee.without_parentheses().as_member_expression()? {
            MemberExpression::StaticMemberExpression(member) => {
                Some(member.property.name.to_string())
            }
            MemberExpression::PrivateFieldExpression(member) => {
                Some(format!("#{}", member.field.name))
            }
            MemberExpression::ComputedMemberExpression(_) => None,
        }
    }

    /// The unit of the method `name` of `class` or its program superclasses (by declaration).
    fn class_method(&self, class: NodeRef, name: &str) -> Option<NodeRef> {
        let mut class = Some(class);
        let mut depth = 0;
        while let Some(id) = class {
            let info = self.an.world.classes.get(&id)?;
            if let Some(member) = info.instance_member(name) {
                return member
                    .unit
                    .filter(|_| member.role == Some(MethodRole::Method));
            }
            class = self.superclass(id);
            depth += 1;
            if depth > 16 {
                return None;
            }
        }
        None
    }

    /// The single program class `class` extends, if known.
    fn superclass(&self, class: NodeRef) -> Option<NodeRef> {
        let parent = self.an.super_of(class)?;
        match parent.pts.sites()? {
            [site] => match self.an.heap(*site)?.kind {
                ObjKind::Class(parent) => Some(parent),
                _ => None,
            },
            _ => None,
        }
    }

    /// The getter or setter `name` of `class` (or a superclass), if it has one.
    pub fn class_accessor(&self, class: NodeRef, name: &str, role: MethodRole) -> Option<NodeRef> {
        match self.accessor(class, name, role, false) {
            Accessor::Unit(unit) => Some(unit),
            Accessor::Data | Accessor::Missing => None,
        }
    }

    /// How `class` (or a superclass) defines `name` as an accessor, among its static members
    /// with `is_static`.
    pub fn accessor(
        &self,
        class: NodeRef,
        name: &str,
        role: MethodRole,
        is_static: bool,
    ) -> Accessor {
        let mut class = Some(class);
        let mut depth = 0;
        while let Some(id) = class {
            let Some(info) = self.an.world.classes.get(&id) else {
                return Accessor::Data;
            };
            let members: Vec<_> = info
                .members
                .iter()
                .filter(|member| member.is_static == is_static && member.name == name)
                .collect();
            if !members.is_empty() {
                let accessor = members.iter().any(|member| {
                    matches!(member.role, Some(MethodRole::Getter | MethodRole::Setter))
                });
                if !accessor {
                    return Accessor::Data;
                }
                return members
                    .iter()
                    .find(|member| member.role == Some(role))
                    .and_then(|member| member.unit)
                    .map_or(Accessor::Missing, Accessor::Unit);
            }
            class = self.superclass(id);
            depth += 1;
            if depth > 16 {
                return Accessor::Data;
            }
        }
        Accessor::Data
    }

    /// Reading a property backed by a getter.
    pub fn call_unit_as_getter(&mut self, unit: NodeRef, receiver: &Val, node: NodeId) -> Val {
        let value = self.call_unit(node, unit, receiver, &[], false);
        self.after_call(false);
        value
    }

    /// Writing a property backed by a setter.
    pub fn call_unit_as_setter(
        &mut self,
        unit: NodeRef,
        receiver: &Val,
        value: &Val,
        node: NodeId,
    ) {
        let _ = self.call_unit(node, unit, receiver, std::slice::from_ref(value), false);
        self.after_call(false);
    }

    /// `super.name` inside a method: the superclass's prototype member.
    fn super_method(&mut self, key: &Key) -> Val {
        let Some(class) = self.unit.class else {
            return Val::unknown();
        };
        let Some(parent) = self.superclass(class) else {
            return Val::unknown();
        };
        let Some(proto) = self.an.world.classes.get(&parent).map(|info| info.init) else {
            return Val::unknown();
        };
        let receiver = Val::object(Type::Unknown, proto);
        self.read_prop(&receiver, key, self.unit.id.node)
    }

    /// `super(...)` in a derived constructor: runs the parent constructor on `this`, then this
    /// class's field initialisers.
    fn super_call(&mut self, site: NodeId, args: &Args) {
        let Some(class) = self.unit.class else {
            return;
        };
        let this = self.this.clone();
        let parent = self
            .an
            .super_of(class)
            .cloned()
            .unwrap_or_else(Val::unknown);
        let mut replaced = Val::never();
        match parent.pts.sites() {
            Some(sites) if parent.trust == Trust::Proven => {
                for &site_ref in sites {
                    let value = if let Some(ObjKind::Class(parent_class)) =
                        self.an.heap(site_ref).map(|obj| obj.kind)
                    {
                        self.construct_class(site, parent_class, &this, args)
                    } else {
                        self.unknown_call(site, &parent, &this, &args.values)
                    };
                    replaced = replaced.join(&value);
                }
            }
            _ => {
                let value = self.unknown_call(site, &parent, &this, &args.values);
                replaced = replaced.join(&value);
            }
        }
        if !is_nothing(&replaced) {
            // The parent constructor returned another object: it is `this` from here on, and
            // what this constructor gives back.
            self.this = self.this.join(&replaced);
            self.returned = self.returned.join(&replaced);
        }
        if let Some(info) = self.an.world.classes.get(&class) {
            let init = info.init;
            self.an.join_this(init, &this);
            self.record.edges.push(Edge {
                site: self.node(site),
                callee: init,
                trust: Trust::Proven,
            });
        }
        self.after_call(false);
    }

    /// Runs `class`'s constructor (or its default one) on `this`. Returns what the
    /// constructor may return instead of `this` (`Never` when it returns nothing else).
    fn construct_class(&mut self, site: NodeId, class: NodeRef, this: &Val, args: &Args) -> Val {
        let Some(info) = self.an.world.classes.get(&class) else {
            return Val::never();
        };
        let (constructor, init, heritage) = (info.constructor, info.init, info.has_heritage);
        if let Some(constructor) = constructor {
            let _ = self.call_unit(site, constructor, this, &args.values, args.spread);
            // `call_unit` treats a direct constructor call as a throw; this one is fine.
            if !heritage {
                self.an.join_this(init, this);
                self.record.edges.push(Edge {
                    site: self.node(site),
                    callee: init,
                    trust: Trust::Proven,
                });
            }
            return self.replacement(constructor);
        }
        let mut replaced = Val::never();
        if heritage {
            // The default derived constructor passes its arguments to `super`.
            let parent = self
                .an
                .super_of(class)
                .cloned()
                .unwrap_or_else(Val::unknown);
            match parent.pts.sites() {
                Some(sites) if parent.trust == Trust::Proven => {
                    for &site_ref in sites {
                        match self.an.heap(site_ref).map(|obj| obj.kind) {
                            Some(ObjKind::Class(parent_class)) if parent_class != class => {
                                let parent = self.construct_class(site, parent_class, this, args);
                                replaced = replaced.join(&parent);
                            }
                            _ => {
                                let value = self.unknown_call(site, &parent, this, &args.values);
                                replaced = replaced.join(&value);
                            }
                        }
                    }
                }
                _ => {
                    let value = self.unknown_call(site, &parent, this, &args.values);
                    replaced = replaced.join(&value);
                }
            }
        }
        self.an.join_this(init, this);
        self.record.edges.push(Edge {
            site: self.node(site),
            callee: init,
            trust: Trust::Proven,
        });
        replaced
    }

    /// What `constructor` may return instead of `this`: anything it returns but `undefined`
    /// (a primitive is ignored for base classes and throws for derived ones; joining it
    /// only loses precision).
    fn replacement(&mut self, constructor: NodeRef) -> Val {
        let returned = self
            .an
            .returned(constructor)
            .cloned()
            .unwrap_or_else(Val::never);
        let value = super::expr::without_undefined(&returned);
        if is_nothing(&value) {
            Val::never()
        } else {
            value
        }
    }

    /// A `new` expression.
    pub fn construct(&mut self, new: &'a NewExpression<'a>) -> Val {
        let site = new.node_id.get();
        if let Expression::Identifier(ident) = new.callee.without_parentheses()
            && self.is_global(ident)
        {
            let name = ident.name.as_str();
            self.record_node(ident.node_id.get(), &Val::unknown());
            let args = self.arguments(&new.arguments);
            return self.builtin_construct(name, &args, site);
        }
        let constructor = self.expr(&new.callee);
        let args = self.arguments(&new.arguments);
        let mut result = Val::never();
        let mut unknown = constructor.pts.sites().is_none();
        for &target in constructor.pts.sites().unwrap_or_default() {
            match self.an.heap(target).map(|obj| obj.kind) {
                Some(ObjKind::Class(class)) => {
                    let instance = self.alloc(site, ObjKind::Instance(class));
                    if self.an.heap(instance).is_some_and(|obj| {
                        obj.present.is_empty() && obj.props.is_empty() && !obj.deleted
                    }) {
                        let present = crate::shapes::definite_fields(self.an.world, class);
                        self.mark(instance, |obj| obj.present.extend(present));
                    }
                    let this = Val::object(Type::Instance(class), instance);
                    let replaced = self.construct_class(site, class, &this, &args);
                    result = result.join(&this).join(&replaced);
                }
                Some(ObjKind::Function(unit)) => {
                    // `new f()` of a plain function: a fresh object as `this`.
                    let object = self.alloc(site, ObjKind::Object);
                    self.mark(object, |obj| obj.dynamic = true);
                    let this = Val::object(Type::empty_object(), object);
                    let _ = self.call_unit(site, unit, &this, &args.values, args.spread);
                    let replaced = self.replacement(unit);
                    result = result.join(&this).join(&replaced);
                }
                _ => unknown = true,
            }
        }
        if unknown {
            let _ = self.unknown_call(site, &constructor, &Val::unknown(), &args.values);
            result = result.join(&Val::unknown());
        }
        self.always(EffectKind::Allocates);
        self.after_call(false);
        result
    }

    /// Whether the array made at `site` is at once the receiver of a whole-array `fill`
    /// (`new Array(n).fill(v)`), with `Array.prototype.fill` intact.
    fn filled_at_once(&self, site: NodeId) -> bool {
        use oxc_ast::AstKind;
        use oxc_span::GetSpan;
        if self.an.patched.contains("Array") {
            return false;
        }
        let nodes = self.module.semantic.nodes();
        let span = nodes.kind(site).span();
        let member = nodes.parent_id(site);
        let AstKind::StaticMemberExpression(fill) = nodes.kind(member) else {
            return false;
        };
        if fill.property.name != "fill" || fill.object.span() != span || fill.optional {
            return false;
        }
        matches!(
            nodes.parent_kind(member),
            AstKind::CallExpression(call)
                if call.callee.span() == fill.span
                    && call.arguments.len() <= 1
                    && !call.arguments.iter().any(Argument::is_spread)
        )
    }

    fn builtin_construct(&mut self, name: &str, args: &Args, site: NodeId) -> Val {
        if self.an.patched.contains(name) {
            return self.unknown_construct(args, site);
        }
        if name == "Proxy" {
            // A proxy forwards to its target through arbitrary traps: neither the target nor
            // the proxy has knowable properties any more.
            for value in &args.values {
                self.an.poison(&value.pts);
            }
            let proxy = self.unknown_construct(args, site);
            self.an.poison(&proxy.pts);
            return Val {
                poisoned: true,
                ..proxy
            };
        }
        let first = args.values.first();
        if let Some(kind) = TypedArrayKind::from_constructor_name(name) {
            let object = self.alloc(site, ObjKind::TypedArray(kind));
            let length = match first {
                None => Some(0.0),
                Some(value) if value.trust == Trust::Proven => {
                    if let Type::Number(number) = &value.ty {
                        // A length must be a valid index.
                        self.effect(
                            EffectKind::Throws,
                            number.int
                                && number.non_nan
                                && number.range.min >= 0.0
                                && number.range.max <= numeric::MAX_TYPED_ARRAY_LENGTH,
                            false,
                        );
                        number.as_constant()
                    } else {
                        // Copies an array-like or iterable: as long as a tracked array.
                        let copied = self.iterate(value, site);
                        self.coerce(&copied, false);
                        self.always(EffectKind::Throws);
                        let lengths: Option<Vec<Option<f64>>> = value.pts.sites().map(|sites| {
                            sites
                                .iter()
                                .map(|site| {
                                    self.an
                                        .heap(*site)
                                        .filter(|obj| {
                                            obj.kind == ObjKind::Array
                                                && !obj.dynamic
                                                && obj.escape != Escape::Unknown
                                        })
                                        .and_then(|obj| obj.length)
                                })
                                .collect()
                        });
                        let copies = value.pts.sites().is_some_and(|sites| {
                            sites.iter().all(|site| {
                                self.an
                                    .heap(*site)
                                    .is_some_and(|obj| obj.kind == ObjKind::Array)
                            })
                        });
                        if !copies {
                            // Maybe a view of a buffer someone else holds (and may detach).
                            self.an.escape(&Pts::site(object), Escape::Unknown);
                        }
                        match lengths.as_deref() {
                            Some([Some(length)]) => Some(*length),
                            _ => None,
                        }
                    }
                }
                Some(value) => {
                    self.escape_value(value, Escape::Unknown);
                    self.always(EffectKind::CallsUnknown);
                    self.always(EffectKind::Throws);
                    self.an.escape(&Pts::site(object), Escape::Unknown);
                    None
                }
            };
            let fresh = self
                .an
                .heap(object)
                .is_some_and(|obj| obj.length.is_none() && obj.props.is_empty() && !obj.holey);
            self.mark(object, |obj| {
                if fresh && !obj.sized && !obj.dynamic {
                    obj.length = length;
                } else if obj.length != length {
                    obj.length = None;
                    obj.dynamic = true;
                }
                obj.sized = true;
            });
            self.always(EffectKind::Allocates);
            return Val::object(Type::TypedArray(kind), object);
        }
        if name == "Array" {
            let object = self.alloc(site, ObjKind::Array);
            match args.values.as_slice() {
                [length]
                    if length.trust == Trust::Proven && matches!(length.ty, Type::Number(_)) =>
                {
                    let number = length.ty.as_number().unwrap_or(NumberType::ANY);
                    self.effect(
                        EffectKind::Throws,
                        number.int
                            && number.non_nan
                            && number.range.min >= 0.0
                            && number.range.max <= numeric::MAX_ARRAY_LENGTH,
                        false,
                    );
                    let exact = number.as_constant();
                    // `new Array(n).fill(v)`: filled before anything else can see a hole.
                    let filled = self.filled_at_once(site);
                    self.mark(object, |obj| {
                        obj.holey |= !filled;
                        obj.length = if !obj.sized || obj.length == exact {
                            exact
                        } else {
                            None
                        };
                        obj.sized = true;
                    });
                }
                // An argument with no value yet (not reached this round): nothing to record.
                values
                    if values
                        .iter()
                        .any(|value| value.ty == Type::Never && value.pts.is_none()) => {}
                values => {
                    for value in values {
                        self.store_elem(object, value);
                    }
                    let count = values.len();
                    #[expect(clippy::cast_precision_loss, reason = "a handful of arguments")]
                    let exact = (!args.spread && count != 1).then_some(count as f64);
                    let filled = self.filled_at_once(site);
                    self.mark(object, |obj| {
                        if count == 1 || args.spread {
                            obj.holey |= !filled;
                            obj.length = None;
                        } else if obj.length != exact {
                            obj.length = if !obj.sized && obj.elem.ty == Type::Never {
                                exact
                            } else {
                                None
                            };
                        }
                        obj.sized = true;
                    });
                }
            }
            self.always(EffectKind::Allocates);
            return Val::object(Type::Array(std::sync::Arc::new(Type::Unknown)), object);
        }
        self.unknown_construct(args, site)
    }

    fn unknown_construct(&mut self, args: &Args, site: NodeId) -> Val {
        // A builtin or host constructor: a fresh object, but what it does with its arguments
        // is unknown (Promise calls its executor, Map iterates, Proxy wraps).
        self.note_call();
        let site_ref = self.node(site);
        self.record.open_sites.insert(site_ref);
        let object = self.alloc(site, ObjKind::Opaque);
        for value in &args.values {
            self.escape_value(value, Escape::Unknown);
        }
        self.always(EffectKind::CallsUnknown);
        self.always(EffectKind::Throws);
        self.always(EffectKind::Allocates);
        self.after_call(false);
        Val {
            ty: Type::Unknown,
            trust: Trust::Unknown,
            pts: Pts::site(object),
            poisoned: false,
        }
    }

    /// Which builtin methods apply to `receiver`, and how far that rests on proof.
    fn receiver_kind(&self, receiver: &Val) -> Option<(Receiver, Trust)> {
        if receiver.trust == Trust::Unknown {
            return None;
        }
        let kind = match &receiver.ty {
            Type::String => Receiver::String,
            Type::Number(_) => Receiver::Number,
            Type::Array(_) | Type::Tuple(_) => Receiver::Array,
            Type::TypedArray(_) => Receiver::TypedArray,
            _ => return None,
        };
        let prototype = match kind {
            Receiver::String => "String",
            Receiver::Number => "Number",
            Receiver::Array => "Array",
            _ => super::TYPED_ARRAY,
        };
        if self.an.patched.contains(prototype) {
            return None;
        }
        if matches!(kind, Receiver::Array | Receiver::TypedArray) {
            // Proven only for objects the analysis made (an unknown object's prototype could be
            // anything); tracked arrays must not have had methods overwritten.
            if receiver.trust == Trust::Proven {
                let sites = receiver.pts.sites()?;
                let intact = sites.iter().all(|site| {
                    self.an.heap(*site).is_some_and(|obj| {
                        matches!(obj.kind, ObjKind::Array | ObjKind::TypedArray(_))
                            && !obj.dynamic
                            && obj.escape != Escape::Unknown
                    })
                });
                return Some((
                    kind,
                    if intact && !sites.is_empty() {
                        Trust::Proven
                    } else {
                        Trust::Declared
                    },
                ));
            }
        }
        Some((kind, receiver.trust))
    }

    /// Calls a modelled builtin. `trust` is how far it is known the builtin is what's called.
    fn builtin_call(
        &mut self,
        builtin: &'static Builtin,
        args: &Args,
        receiver: Option<&Val>,
        site: NodeId,
        trust: Trust,
    ) -> Val {
        let effects = builtin.effects;
        let declared_only = trust != Trust::Proven;
        if effects.writes_receiver || effects.writes_other {
            self.note_write(true);
        }
        // A clock or the PRNG: two calls can differ.
        if effects.writes_other || builtin.name == "Date.now" {
            self.note_volatile();
        }
        // Arguments are coerced to primitives by most builtins (Math.floor(x) runs valueOf).
        let coerces = matches!(builtin.ret, Ret::Number(_)) && receiver.is_none()
            || matches!(
                builtin.name,
                "String"
                    | "isNaN"
                    | "isFinite"
                    | "parseInt"
                    | "parseFloat"
                    | "Number.parseInt"
                    | "Number.parseFloat"
                    | "String.fromCharCode"
                    | "Math.imul"
                    | "Math.clz32"
            );
        // Whether user code may run here (a conversion's `valueOf`, a getter).
        let mut runs_code = false;
        for value in &args.values {
            if (coerces || runs_argument_code(builtin)) && !value.is_primitive(false) {
                runs_code = true;
            }
            if coerces {
                self.coerce(
                    value,
                    matches!(
                        builtin.ret,
                        Ret::Number(NumberModel::Math(_) | NumberModel::Int32)
                    ),
                );
            } else if runs_argument_code(builtin) {
                // Converted (`"a".padStart(n, o)`) or read through getters (`Object.values`),
                // with the engine as caller.
                self.coerce(value, false);
            } else if !builtin.effects.calls_args && !value.is_primitive(false) {
                // Seen by the builtin but not converted (`includes`, `push`).
                self.escape_value(value, Escape::Known);
            }
        }
        // `join` and a comparator-less `sort` convert the elements.
        if let Some(receiver) = receiver
            && builtin.receiver == Receiver::Array
            && (builtin.name == "join" || (builtin.name == "sort" && args.values.is_empty()))
        {
            let element = self.element(receiver, true);
            runs_code |= !element.is_primitive(false);
            self.coerce(&element, false);
        }
        if runs_code {
            let site = self.node(site);
            self.record.open_sites.insert(site);
        }
        let own_receiver = receiver.is_some_and(|receiver| self.owns(receiver));
        let free = |flag: bool| !flag;
        self.effect(
            EffectKind::Reads,
            free(effects.reads) || own_receiver && !effects.writes_other,
            !effects.reads || own_receiver,
        );
        self.effect(
            EffectKind::Writes,
            free(effects.writes_other || effects.writes_receiver && !own_receiver),
            free(effects.writes_other || effects.writes_receiver && !own_receiver),
        );
        self.effect(
            EffectKind::Throws,
            free(effects.throws) && !declared_only,
            free(effects.throws),
        );
        if declared_only {
            self.effect(EffectKind::CallsUnknown, false, true);
            for value in args.values.iter().chain(receiver) {
                self.escape_value(value, Escape::Unknown);
            }
        }
        if let Some(receiver) = receiver {
            self.builtin_mutates(builtin, receiver, &args.values);
        }
        let mut callback_results = Val::never();
        if effects.calls_args {
            callback_results = self.builtin_callbacks(builtin, args, receiver, site, declared_only);
        }
        if effects.allocates {
            self.always(EffectKind::Allocates);
        }
        let value = self.builtin_result(builtin, &args.values, receiver, site, &callback_results);
        self.after_call(!(effects.writes_receiver || effects.writes_other || effects.calls_args));
        if declared_only {
            value.at_most(Trust::Declared)
        } else {
            value
        }
    }

    /// Whether `value` is only objects this unit made and nobody else holds.
    fn owns(&self, value: &Val) -> bool {
        value.pts.sites().is_some_and(|sites| {
            !sites.is_empty()
                && sites.iter().all(|site| {
                    self.an
                        .heap(*site)
                        .is_some_and(|obj| obj.unit == self.unit.id && obj.escape == Escape::None)
                })
        })
    }

    fn builtin_mutates(&mut self, builtin: &Builtin, receiver: &Val, args: &[Val]) {
        if !builtin.effects.writes_receiver {
            return;
        }
        // A mutator throws on a frozen receiver, in sloppy code too.
        if receiver.pts.sites().is_none_or(|sites| {
            sites.iter().any(|&site| {
                self.an
                    .heap(site)
                    .is_none_or(|obj| obj.frozen || obj.poisoned || obj.escape == Escape::Unknown)
            })
        }) {
            self.always(EffectKind::Throws);
        }
        let Some(sites) = receiver.pts.sites().map(<[_]>::to_vec) else {
            return;
        };
        let shrinks = matches!(builtin.name, "pop" | "shift");
        let grows = matches!(builtin.name, "push" | "unshift");
        for site in sites {
            if builtin.effects.retains_args {
                if builtin.name == "fill" {
                    // `fill(value, start, end)` stores only the value.
                    let value = args
                        .first()
                        .cloned()
                        .unwrap_or_else(|| Val::proven(Type::Undefined));
                    self.store_elem(site, &value);
                } else {
                    for value in args {
                        self.store_elem(site, value);
                    }
                }
            }
            if shrinks || grows {
                self.mark(site, |obj| obj.length = None);
            }
        }
        if shrinks || grows {
            self.note_resize(receiver);
        }
        if shrinks {
            self.forget_bounds_of(receiver);
        }
    }

    /// Calls the callback arguments of an iterating builtin.
    fn builtin_callbacks(
        &mut self,
        builtin: &'static Builtin,
        args: &Args,
        receiver: Option<&Val>,
        site: NodeId,
        declared_only: bool,
    ) -> Val {
        let Some(callback) = args.values.first() else {
            return Val::never();
        };
        if builtin.callback.is_empty() || declared_only {
            let _ = self.unknown_call(site, callback, &Val::unknown(), &[]);
            return Val::unknown();
        }
        let element = receiver.map_or_else(Val::unknown, |receiver| self.element(receiver, true));
        let typed = receiver.is_some_and(|receiver| {
            receiver.trust == Trust::Unknown
                || receiver
                    .ty
                    .members()
                    .iter()
                    .any(|member| !matches!(member, Type::Array(_) | Type::Tuple(_)))
        });
        let index = Val::number(numeric::int_range(0.0, numeric::max_length(typed) - 1.0));
        let initial = args.values.get(1).cloned();
        let mut results = Val::never();
        let mut targets = Vec::new();
        let unknown = match callback.pts.sites() {
            Some(sites) => {
                for &target in sites {
                    match self.an.heap(target).map(|obj| obj.kind) {
                        Some(ObjKind::Function(unit)) => targets.push(unit),
                        _ => return self.unknown_callback(site, callback),
                    }
                }
                sites.is_empty()
            }
            None => true,
        };
        if unknown {
            return self.unknown_callback(site, callback);
        }
        // Element callbacks (`forEach`, `map`, …) take a `thisArg`; the others don't.
        let this = if builtin.callback.first() == Some(&CallbackParam::Element)
            && builtin.callback.contains(&CallbackParam::Index)
        {
            args.values
                .get(1)
                .cloned()
                .unwrap_or_else(|| Val::proven(Type::Undefined))
        } else {
            Val::proven(Type::Undefined)
        };
        if args.spread {
            // Where the `thisArg` is isn't known.
            return self.unknown_callback(site, callback);
        }
        for unit in targets {
            let accumulator = self.an.returned(unit).cloned().unwrap_or_else(Val::never);
            let accumulator = match &initial {
                Some(initial) => accumulator.join(initial),
                None => accumulator.join(&element),
            };
            let params: Vec<Val> = builtin
                .callback
                .iter()
                .map(|param| match param {
                    CallbackParam::Element => element.clone(),
                    CallbackParam::Index => index.clone(),
                    CallbackParam::Receiver => receiver.cloned().unwrap_or_else(Val::unknown),
                    CallbackParam::Accumulator => accumulator.clone(),
                })
                .collect();
            let value = self.call_unit(site, unit, &this, &params, false);
            results = results.join(&value);
            if builtin.name == "reduce" {
                results = results.join(&accumulator);
            }
        }
        results
    }

    fn unknown_callback(&mut self, site: NodeId, callback: &Val) -> Val {
        let _ = self.unknown_call(site, callback, &Val::unknown(), &[]);
        Val::unknown()
    }

    /// The value a builtin call or read produces.
    pub fn builtin_ret(
        &mut self,
        builtin: &'static Builtin,
        args: &[Val],
        receiver: Option<&Val>,
        site: NodeId,
    ) -> Val {
        self.builtin_result(builtin, args, receiver, site, &Val::never())
    }

    fn builtin_result(
        &mut self,
        builtin: &'static Builtin,
        args: &[Val],
        receiver: Option<&Val>,
        site: NodeId,
        callback_results: &Val,
    ) -> Val {
        match builtin.ret {
            Ret::Number(model) => {
                // Only arrays are capped at 2^32 − 1 elements.
                let typed = receiver.is_none_or(|receiver| {
                    !receiver
                        .ty
                        .members()
                        .iter()
                        .all(|member| matches!(member, Type::Array(_) | Type::Tuple(_)))
                });
                Val::number(self.number_model(model, args, typed))
            }
            Ret::Boolean => Val::proven(Type::Boolean),
            Ret::String => Val::proven(Type::String),
            Ret::Undefined => Val::proven(Type::Undefined),
            Ret::Unknown => {
                if builtin.name == "reduce" {
                    return callback_results.clone();
                }
                Val::unknown()
            }
            Ret::Receiver => receiver.cloned().unwrap_or_else(Val::unknown),
            Ret::FirstArgument => {
                let first = args
                    .first()
                    .cloned()
                    .unwrap_or_else(|| Val::proven(Type::Undefined));
                if builtin.name == "Object.freeze" {
                    // Freezing a non-empty typed array, a namespace or a Proxy throws.
                    let throws = !first.is_primitive(false)
                        && first.pts.sites().is_none_or(|sites| {
                            sites.iter().any(|&site| {
                                self.an.heap(site).is_none_or(|obj| {
                                    obj.poisoned
                                        || matches!(
                                            obj.kind,
                                            ObjKind::TypedArray(_)
                                                | ObjKind::Namespace(_)
                                                | ObjKind::Opaque
                                        )
                                })
                            })
                        });
                    if throws {
                        self.always(EffectKind::Throws);
                    }
                    for &object in first.pts.sites().unwrap_or_default() {
                        self.mark(object, |obj| obj.frozen = true);
                    }
                }
                first
            }
            Ret::ElementOrUndefined => {
                let element =
                    receiver.map_or_else(Val::unknown, |receiver| self.element(receiver, true));
                element.join(&Val::proven(Type::Undefined))
            }
            Ret::ReceiverElements | Ret::NewArray | Ret::StringArray => {
                let element = match builtin.ret {
                    Ret::ReceiverElements => {
                        receiver.map_or_else(Val::unknown, |receiver| self.element(receiver, true))
                    }
                    Ret::StringArray => Val::proven(Type::String),
                    _ if builtin.name == "map" => callback_results.clone(),
                    _ => Val::unknown(),
                };
                let object = self.alloc(site, ObjKind::Array);
                self.store_elem(object, &element);
                self.mark(object, |obj| obj.length = None);
                Val::object(Type::Array(std::sync::Arc::new(Type::Unknown)), object)
            }
            Ret::ReceiverTypedArray => {
                let Some(Type::TypedArray(kind)) = receiver.map(|receiver| receiver.ty.clone())
                else {
                    return Val::unknown();
                };
                let object = self.alloc(site, ObjKind::TypedArray(kind));
                if builtin.name == "subarray"
                    && let Some(receiver) = receiver
                {
                    // The view shares the receiver's buffer: whoever can detach one
                    // detaches both.
                    match receiver.pts.sites() {
                        Some(sources) => {
                            let sources = sources.to_vec();
                            let mut detachable = false;
                            for &source in &sources {
                                self.mark(source, |obj| {
                                    if !obj.shares_buffer.contains(&object) {
                                        obj.shares_buffer.push(object);
                                    }
                                });
                                detachable |= self
                                    .an
                                    .heap(source)
                                    .is_none_or(|obj| obj.escape == Escape::Unknown);
                            }
                            self.mark(object, |obj| {
                                for source in &sources {
                                    if !obj.shares_buffer.contains(source) {
                                        obj.shares_buffer.push(*source);
                                    }
                                }
                            });
                            detachable |= self
                                .an
                                .heap(object)
                                .is_some_and(|obj| obj.escape == Escape::Unknown);
                            if detachable {
                                self.an.escape(&Pts::site(object), Escape::Unknown);
                                self.an.escape(&receiver.pts, Escape::Unknown);
                            }
                        }
                        None => self.an.escape(&Pts::site(object), Escape::Unknown),
                    }
                }
                Val::object(Type::TypedArray(kind), object)
            }
        }
    }

    #[expect(clippy::unused_self, reason = "a method like the other builtin models")]
    fn number_model(&self, model: NumberModel, args: &[Val], typed: bool) -> NumberType {
        let arg = |index: usize| -> NumberType {
            #[expect(
                clippy::match_same_arms,
                reason = "distinct cases, documented separately"
            )]
            match args.get(index) {
                // A proven primitive converts to a known number; anything else is any number.
                Some(value) if value.trust == Trust::Proven && value.is_primitive(false) => {
                    to_number(value)
                }
                Some(_) => NumberType::ANY,
                // A missing argument is `undefined`: NaN.
                None => NumberType::ANY,
            }
        };
        match model {
            NumberModel::Any => NumberType::ANY,
            NumberModel::NonNaN => NumberType {
                non_nan: true,
                ..NumberType::ANY
            },
            NumberModel::Int32 => numeric::int32(),
            #[expect(clippy::cast_precision_loss, reason = "bounds below 2^53")]
            NumberModel::Int(min, max) => numeric::int_range(min as f64, max as f64),
            NumberModel::IndexOrMinusOne => {
                numeric::int_range(-1.0, numeric::max_length(typed) - 1.0)
            }
            NumberModel::Length => numeric::int_range(0.0, numeric::max_length(typed)),
            NumberModel::CodeUnitOrNaN => NumberType {
                non_nan: false,
                ..numeric::int_range(0.0, 65535.0)
            },
            NumberModel::IntOrNaN => NumberType {
                non_nan: false,
                int: true,
                ..NumberType::ANY
            },
            NumberModel::Unit => NumberType {
                range: trioxide_types::Interval::new(0.0, 1.0),
                int: false,
                non_nan: true,
                non_neg_zero: true,
            },
            NumberModel::Math(op) => match op {
                MathOp::Abs => numeric::abs(arg(0)),
                MathOp::Ceil => numeric::ceil(arg(0)),
                MathOp::Floor => numeric::floor(arg(0)),
                MathOp::Round => numeric::round(arg(0)),
                MathOp::Trunc => numeric::trunc(arg(0)),
                MathOp::Sign => numeric::sign(arg(0)),
                MathOp::Sqrt => numeric::sqrt(arg(0)),
                MathOp::Wave => numeric::unit_wave(arg(0)),
                MathOp::Fround => numeric::fround(arg(0)),
                MathOp::Min | MathOp::Max => {
                    let max = op == MathOp::Max;
                    if args.is_empty() {
                        return NumberType::constant(if max {
                            f64::NEG_INFINITY
                        } else {
                            f64::INFINITY
                        });
                    }
                    (1..args.len())
                        .fold(arg(0), |acc, index| numeric::min_max(acc, arg(index), max))
                }
            },
        }
    }

    /// After a call: relations that callee code could break are forgotten. `pure`: the callee
    /// writes nothing.
    pub fn after_call(&mut self, pure: bool) {
        if pure || (self.env.bounds.is_empty() && self.env.lengths.is_empty()) {
            return;
        }
        let stable = |cx: &Self, array: oxc_syntax::symbol::SymbolId| {
            let value = cx
                .env
                .vars
                .get(&array)
                .cloned()
                .or_else(|| cx.an.symbol_value(cx.module.symbol(array)).cloned());
            value.is_some_and(|value| cx.length_is_stable(&value))
        };
        let bounds = std::mem::take(&mut self.env.bounds);
        let (bounds, dropped): (Vec<_>, Vec<_>) = bounds
            .into_iter()
            .partition(|&(_, array)| stable(self, array));
        let lengths = std::mem::take(&mut self.env.lengths);
        let (lengths, dropped_lengths): (Vec<_>, Vec<_>) = lengths
            .into_iter()
            .partition(|&(_, array)| stable(self, array));
        self.dropped.extend(
            dropped
                .iter()
                .chain(&dropped_lengths)
                .map(|&(_, array)| array),
        );
        self.env.bounds = bounds;
        self.env.lengths = lengths;
    }

    /// Whether only this unit can change the length of the arrays `value` may be.
    fn length_is_stable(&self, value: &Val) -> bool {
        value.pts.sites().is_some_and(|sites| {
            !sites.is_empty()
                && sites.iter().all(|site| {
                    self.an.heap(*site).is_some_and(|obj| match obj.kind {
                        // Only code holding a typed array (or its buffer) can detach it.
                        ObjKind::TypedArray(_) => obj.escape != Escape::Unknown,
                        _ => obj.escape == Escape::None,
                    })
                })
        })
    }

    /// Forgets the length relations of every array `receiver` may be (its length changed).
    pub fn forget_bounds_of(&mut self, receiver: &Val) {
        if receiver.pts.sites().is_none() {
            // Any object: any array whose length others can change.
            self.after_call(false);
        }
        let sites = receiver.pts.sites().map(<[_]>::to_vec);
        let vars = &self.env.vars;
        let unaffected = |array: oxc_syntax::symbol::SymbolId| match (&sites, vars.get(&array)) {
            (Some(sites), Some(value)) => value
                .pts
                .sites()
                .is_some_and(|own| own.iter().all(|site| !sites.contains(site))),
            _ => false,
        };
        let (bounds, dropped): (Vec<_>, Vec<_>) = self
            .env
            .bounds
            .iter()
            .copied()
            .partition(|&(_, array)| unaffected(array));
        let (lengths, dropped_lengths): (Vec<_>, Vec<_>) = self
            .env
            .lengths
            .iter()
            .copied()
            .partition(|&(_, array)| unaffected(array));
        self.env.bounds = bounds;
        self.env.lengths = lengths;
        self.dropped.extend(
            dropped
                .iter()
                .chain(&dropped_lengths)
                .map(|&(_, array)| array),
        );
    }

    /// Whether `object[index]` is in bounds: a tracked local index below the array's length
    /// by a loop condition, and not negative. The trust is the array's.
    pub fn in_bounds(
        &mut self,
        object: &Expression<'_>,
        index: &Expression<'_>,
        object_value: &Val,
    ) -> Option<Trust> {
        let index_symbol = self.tracked_identifier(index)?;
        let array = self.stable_symbol(object)?;
        if !self.env.bounds.contains(&(index_symbol, array)) {
            return None;
        }
        let index_value = self.env.vars.get(&index_symbol)?;
        let number = index_value.ty.as_number()?;
        if !(number.int && number.non_nan && number.range.min >= 0.0) {
            return None;
        }
        let array_kind = |value: &Val| {
            value.ty.members().iter().all(|member| {
                matches!(
                    member,
                    Type::Array(_) | Type::TypedArray(_) | Type::Tuple(_)
                )
            })
        };
        if !array_kind(object_value) || object_value.trust == Trust::Unknown {
            return None;
        }
        // A hole below the length isn't an element.
        let holey = object_value.pts.sites().is_some_and(|sites| {
            sites
                .iter()
                .any(|site| self.an.heap(*site).is_none_or(|obj| obj.holey))
        });
        if holey {
            return None;
        }
        Some(object_value.trust.min(index_value.trust))
    }
}

/// `Object.defineProperty` and friends: calls that can turn a property into an accessor or
/// change an object's prototype.
fn is_reflective(object: &str, method: &str) -> bool {
    matches!(
        (object, method),
        (
            "Object",
            "defineProperty"
                | "defineProperties"
                | "setPrototypeOf"
                | "assign"
                | "__defineGetter__"
        ) | (
            "Reflect",
            "defineProperty" | "setPrototypeOf" | "set" | "deleteProperty" | "apply" | "construct"
        )
    )
}

fn key_name(key: &Key) -> Option<&str> {
    match key {
        Key::Name(name) => Some(name),
        _ => None,
    }
}

/// How many parameters a unit declares (excluding a rest parameter).
fn param_count(unit: &crate::world::Unit<'_>) -> usize {
    match unit.body {
        Body::Function(function) => function.params.items.len(),
        Body::Arrow(arrow) => arrow.params.items.len(),
        _ => 0,
    }
}

/// Whether `value` is no value at all.
fn is_nothing(value: &Val) -> bool {
    value.ty == Type::Never && value.pts.sites().is_some_and(<[_]>::is_empty)
}

/// How a class defines a property, as far as accessors go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accessor {
    /// The nearest definition is a data property or method (or there is none).
    Data,
    /// An accessor without the role asked for: reads give `undefined`, writes throw.
    Missing,
    /// The getter or setter.
    Unit(NodeRef),
}

/// Whether `builtin` converts its arguments to primitives or reads them through getters, so
/// user code (`valueOf`, `toString`, a getter) may run.
fn runs_argument_code(builtin: &Builtin) -> bool {
    matches!(builtin.receiver, Receiver::String | Receiver::Number)
        || matches!(
            (builtin.receiver, builtin.name),
            (
                Receiver::Global,
                "encodeURIComponent" | "decodeURIComponent" | "Object.values" | "Object.entries"
            ) | (
                Receiver::TypedArray,
                "fill" | "set" | "indexOf" | "lastIndexOf" | "includes"
            )
        )
}
