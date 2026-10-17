//! Lowering one function.

mod expr;
mod stmt;

use stmt::completes;

use oxc_ast::{
    AstKind,
    ast::{
        Argument, AssignmentExpression, AssignmentTarget, BinaryExpression, BindingPattern,
        CallExpression, ComputedMemberExpression, Expression, ForOfStatement, ForStatementInit,
        ForStatementLeft, FormalParameter, Function as AstFunction, IdentifierReference,
        LabelIdentifier, LogicalExpression, NewExpression, ObjectExpression, ObjectPropertyKind,
        PropertyKey, PropertyKind, SimpleAssignmentTarget, Statement, StaticMemberExpression,
        UnaryExpression, UpdateExpression, VariableDeclaration, VariableDeclarationKind,
    },
};
use oxc_span::{GetSpan, Span};
use oxc_syntax::{
    node::NodeId,
    operator::{BinaryOperator, LogicalOperator, UnaryOperator, UpdateOperator},
    symbol::SymbolId,
};
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::SymbolRef;
use trioxide_types::{ClassId, FunctionId, NumberType, Type, TypedArrayKind};
use trioxide_wasm::TypedArrayElement;

use super::{FieldRead, Group, Lowerer, Reason, StructSlot};
use crate::{
    kinds::{Kind, StructKey, is_index, is_int32, is_uint32},
    names,
    rir::{
        Access, ArrayTy, BinOp, Conv, Elem, Expr, Function, FunctionKind, Index, Lit, Local,
        LocalId, Loop, LoopId, MathFn, Stmt, StructRef, Ty, UnOp,
    },
    world::{Module, Role},
};

/// What [`Kind::describe`] says for [`Kind::Error`]: a value whose problem was already
/// reported, so reasons mentioning it are dropped.
const CASCADE: &str = "an unknown value";

/// The `this` slot in [`super::Env::params`].
const THIS: usize = usize::MAX;

/// Rust names a function can't take (they would shadow what generated code uses).
const RESERVED_FN_NAMES: &[&str] = &["size_of", "align_of", "new"];

/// A lowered expression.
#[derive(Debug, Clone)]
pub(super) struct V {
    pub expr: Expr,
    pub ty: Ty,
    pub kind: Kind,
}

impl V {
    fn new(expr: Expr, ty: Ty, kind: Kind) -> Self {
        Self { expr, ty, kind }
    }
}

struct LocalInfo {
    symbol: Option<SymbolRef>,
    param: Option<usize>,
    /// Its value failed to lower: its kind is unknown.
    poisoned: bool,
}

#[derive(PartialEq)]
enum FrameKind {
    Loop,
    Block,
}

struct Frame {
    id: LoopId,
    label: Option<String>,
    kind: FrameKind,
    has_continue: bool,
    labelled: bool,
    used: bool,
}

/// Lowers `function`, returning it with the reasons it can't be compiled (kept only in the
/// final round).
pub(super) fn lower_function(
    lw: &mut Lowerer<'_, '_>,
    function: FunctionId,
) -> (Function, Vec<Reason>) {
    let mut cx = FnCx::new(lw, function, None);
    let lowered = cx.run();
    let reasons = if cx.lw.final_round {
        std::mem::take(&mut cx.reasons)
    } else {
        Vec::new()
    };
    (lowered, reasons)
}

struct FnCx<'l, 'w, 'a> {
    lw: &'l mut Lowerer<'w, 'a>,
    id: FunctionId,
    module: &'w Module<'a>,
    class: Option<ClassId>,
    is_ctor: bool,
    locals: Vec<Local>,
    info: Vec<LocalInfo>,
    by_symbol: FxHashMap<SymbolId, LocalId>,
    names: FxHashSet<String>,
    frames: Vec<Frame>,
    next_loop: usize,
    reasons: Vec<Reason>,
    /// Parameter kinds that replace the joined ones (evaluating a constructor for one `new`).
    overrides: Option<Vec<Kind>>,
    ret: Ty,
    /// The field kinds a constructor produced, for [`FnCx::ctor_field_kinds`].
    ctor_kinds: Option<Vec<Kind>>,
    /// Island-local bounds: inside `for (let i = k; i < a.length; i++)` (`i` written only by
    /// the update, `a` never reassigned), `(i, a)`: `a[i]` is in bounds, since array lengths
    /// can't change inside an island.
    bounds: Vec<(SymbolId, SymbolId)>,
    /// Constant locals holding an array created with another array's length.
    same_len: FxHashMap<SymbolId, SymbolId>,
    /// Constant locals holding an array created with a constant length.
    fixed_len: FxHashMap<SymbolId, f64>,
}

impl<'l, 'w, 'a> FnCx<'l, 'w, 'a> {
    fn new(lw: &'l mut Lowerer<'w, 'a>, id: FunctionId, overrides: Option<Vec<Kind>>) -> Self {
        let world = lw.world;
        let module = world.module(id.module).expect("function in a module");
        let (class, is_ctor) = match world.functions.get(&id).map(|info| info.role) {
            Some(Role::Method(class)) => (Some(class), false),
            Some(Role::Constructor(class)) => (Some(class), true),
            None => (Some(id), true),
            _ => (None, false),
        };
        let mut names: FxHashSet<String> = lw.fn_names.values().cloned().collect();
        names.extend(lw.consts.iter().map(|c| c.name.clone()));
        Self {
            lw,
            id,
            module,
            class,
            is_ctor,
            locals: Vec::new(),
            info: Vec::new(),
            by_symbol: FxHashMap::default(),
            names,
            frames: Vec::new(),
            next_loop: 0,
            reasons: Vec::new(),
            overrides,
            ret: Ty::Unit,
            ctor_kinds: None,
            bounds: Vec::new(),
            same_len: FxHashMap::default(),
            fixed_len: FxHashMap::default(),
        }
    }

    fn fail<T>(&mut self, span: Span, message: impl Into<String>) -> Option<T> {
        let message = message.into();
        if message.contains(CASCADE) {
            return None;
        }
        self.reasons.push(Reason {
            function: self.id,
            span,
            message,
        });
        None
    }

    fn final_round(&self) -> bool {
        self.lw.final_round
    }

    fn facts(&self) -> &'w trioxide_analysis::Facts {
        self.lw.facts
    }

    fn node(&self, node: NodeId) -> trioxide_common::NodeRef {
        self.module.node(node)
    }

    /// The usable range fact of an expression node.
    fn num_fact(&self, node: NodeId) -> Option<NumberType> {
        let fact = self.facts().range(self.node(node));
        self.facts().is_usable(fact.trust).then_some(fact.value)
    }

    fn expr_fact(&self, e: &Expression<'_>) -> Option<NumberType> {
        self.num_fact(e.get_inner_expression().node_id())
    }

    fn symbol_fact(&self, symbol: SymbolRef) -> Option<NumberType> {
        let fact = self.facts().symbol_ty(symbol);
        match fact.value {
            Type::Number(n) if self.facts().is_usable(fact.trust) => Some(*n),
            _ => None,
        }
    }

    // ----------------------------------------------------------------------------- setup

    fn run(&mut self) -> Function {
        let name = self.lw.fn_name(self.id);
        if self.is_ctor {
            return self.lower_ctor(name);
        }
        let info = &self.lw.world.functions[&self.id];
        let func: &'a AstFunction<'a> = info.func;
        if func.r#async || func.generator {
            self.fail::<()>(func.span, "is async or a generator");
        }
        let params = self.declare_params(func);
        self.ret = self.ret_ty(self.id);
        if self.final_round() {
            self.check_ret(func);
        }
        let mut body = if let Some(body) = &func.body {
            self.block(&body.statements)
        } else {
            self.fail::<()>(func.span, "has no body");
            Vec::new()
        };
        if self.ret != Ty::Unit && completes(&body) {
            self.fail::<()>(
                func.span,
                "may return undefined (the end of the function is reachable)",
            );
        }
        if self.ret == Ty::Unit && matches!(body.last(), Some(Stmt::Return(None))) {
            body.pop();
        }
        let kind = match self.class {
            Some(class) => {
                FunctionKind::Method(self.struct_ref(StructSlot::Key(StructKey::Class(class))))
            }
            None => FunctionKind::Free,
        };
        Function {
            name,
            kind,
            params,
            locals: std::mem::take(&mut self.locals),
            ret: self.ret,
            body,
        }
    }

    fn declare_params(&mut self, func: &'a AstFunction<'a>) -> Vec<LocalId> {
        if let Some(rest) = &func.params.rest {
            self.fail::<()>(rest.span, "has a rest parameter");
        }
        let mut params = Vec::new();
        for (index, param) in func.params.items.iter().enumerate() {
            let BindingPattern::BindingIdentifier(id) = &param.pattern else {
                self.fail::<()>(param.span, "destructures a parameter");
                continue;
            };
            if param.initializer.is_some() {
                self.fail::<()>(param.span, format!("gives `{}` a default value", id.name));
            }
            if index >= 64 {
                self.fail::<()>(param.span, "has more than 64 parameters");
                continue;
            }
            let symbol = id.symbol_id.get().map(|s| self.module.symbol(s));
            let kind = self.param_kind(index, param, symbol);
            let poisoned = kind == Kind::Error;
            let mutable = id
                .symbol_id
                .get()
                .is_some_and(|s| self.module.semantic.scoping().symbol_is_mutated(s));
            let local = self.new_local(&id.name, symbol, Some(index), &kind, mutable);
            self.info[local.0].poisoned = poisoned;
            if let Some(symbol) = id.symbol_id.get() {
                self.by_symbol.insert(symbol, local);
            }
            params.push(local);
        }
        params
    }

    fn param_kind(
        &mut self,
        index: usize,
        param: &'a FormalParameter<'a>,
        symbol: Option<SymbolRef>,
    ) -> Kind {
        if let Some(overrides) = &self.overrides {
            return normalize_param(overrides.get(index).cloned().unwrap_or(Kind::Bottom), index);
        }
        if self.lw.is_root(self.id) {
            match self.boundary_kind(param, symbol) {
                Ok(kind) => self.lw.env.join_param(self.id, index, &kind),
                Err(message) => {
                    self.fail::<()>(param.span, message);
                    // Unknown from here on: uses of it don't add reasons of their own.
                    return Kind::Error;
                }
            }
        }
        let kind = self
            .lw
            .env
            .params
            .get(&(self.id, index))
            .cloned()
            .unwrap_or(Kind::Bottom);
        if self.final_round() {
            let name = param
                .pattern
                .get_identifier_name()
                .map(|n| n.to_string())
                .unwrap_or_default();
            match &kind {
                Kind::Error if !self.lw.is_root(self.id) => {
                    self.fail::<()>(
                        param.span,
                        format!("parameter `{name}` receives values of different types"),
                    );
                }
                Kind::Unit => {
                    self.fail::<()>(param.span, format!("parameter `{name}` receives undefined"));
                }
                Kind::Bottom if !self.lw.is_root(self.id) => {
                    self.fail::<()>(
                        param.span,
                        format!("parameter `{name}` never receives a value"),
                    );
                }
                _ => {}
            }
        }
        normalize_param(kind, index)
    }

    /// What a parameter of an exported function may receive from JS, from the facts.
    fn boundary_kind(
        &mut self,
        param: &'a FormalParameter<'a>,
        symbol: Option<SymbolRef>,
    ) -> Result<Kind, String> {
        let name = param
            .pattern
            .get_identifier_name()
            .map(|n| n.to_string())
            .unwrap_or_default();
        let Some(symbol) = symbol else {
            return Err(format!("parameter `{name}` has no binding"));
        };
        let fact = self.facts().symbol_ty(symbol);
        if !self.facts().is_usable(fact.trust) {
            return Err(format!(
                "can't prove what parameter `{name}` receives{}",
                trust_hint(fact.trust)
            ));
        }
        let ty = fact.value.clone();
        let members = ty.members();
        let undefined = members.iter().any(|t| matches!(t, Type::Undefined));
        let rest: Vec<&Type> = members
            .iter()
            .filter(|t| !matches!(t, Type::Undefined))
            .collect();
        let what = |t: &Type| -> String { describe_type(t) };
        match rest.as_slice() {
            [Type::Number(_)] => Ok(Kind::Num { undef: undefined }),
            [Type::Boolean] if !undefined => Ok(Kind::Bool),
            [Type::TypedArray(kind)] if !undefined => match typed_element(*kind) {
                Some(element) => Ok(Kind::Array {
                    elem: Elem::Typed(element),
                    fresh: false,
                    params: 0,
                }),
                None => Err(format!("parameter `{name}` is a bigint array")),
            },
            [Type::Array(elem)] if !undefined => match &**elem {
                Type::Number(_) => Ok(Kind::Array {
                    elem: Elem::Number,
                    fresh: false,
                    params: 0,
                }),
                // With `trustTypes`, the annotation itself is the (declared) proof.
                _ if self.facts().trust_types() && is_number_array_annotation(param) => {
                    Ok(Kind::Array {
                        elem: Elem::Number,
                        fresh: false,
                        params: 0,
                    })
                }
                other => Err(format!(
                    "parameter `{name}` is an array of {}, not of numbers",
                    what(other)
                )),
            },
            [Type::Object(_)] if !undefined => Ok(Kind::ParamStruct {
                group: self.group_for(param),
            }),
            [] => Err(format!(
                "parameter `{name}` receives no value the analysis can see (its callers are unknown)"
            )),
            [Type::Instance(_)] => Err(format!(
                "parameter `{name}` is a class instance (only plain objects can cross the boundary)"
            )),
            _ => Err(format!(
                "parameter `{name}` may be {}; only numbers, booleans, typed arrays, `number[]` \
                 and plain-data objects can cross the boundary",
                what(&ty)
            )),
        }
    }

    /// The struct group for an object parameter, named after its TS type.
    fn group_for(&mut self, param: &'a FormalParameter<'a>) -> usize {
        let annotation = param.type_annotation.as_ref().map(|a| &a.type_annotation);
        let type_name = annotation.and_then(names::type_reference_name);
        let name = if let Some(name) = &type_name {
            names::type_name(name)
        } else {
            let function = self.lw.display_name(self.id);
            let param_name = param
                .pattern
                .get_identifier_name()
                .map(|n| n.to_string())
                .unwrap_or_default();
            names::type_name(&format!("{function}_{param_name}"))
        };
        if let Some(index) = self.lw.env.groups.iter().position(|g| g.name == name) {
            return index;
        }
        let order = match (&type_name, annotation) {
            (Some(type_name), _) => interface_order(self.module, type_name),
            (None, Some(oxc_ast::ast::TSType::TSTypeLiteral(literal))) => {
                signature_names(&literal.members)
            }
            _ => Vec::new(),
        };
        self.lw.env.groups.push(Group {
            name,
            order,
            fields: Vec::new(),
        });
        self.lw.env.groups.len() - 1
    }

    fn check_ret(&mut self, func: &'a AstFunction<'a>) {
        let span = func.id.as_ref().map_or(func.span, |id| id.span);
        let kind = self
            .lw
            .env
            .rets
            .get(&self.id)
            .cloned()
            .unwrap_or(Kind::Bottom);
        match &kind {
            Kind::Error => {
                self.fail::<()>(
                    span,
                    "returns values of different types (or a value on some paths only)",
                );
            }
            Kind::Num { undef: true } => {
                self.fail::<()>(span, "may return undefined (an unchecked array read)");
            }
            Kind::Array { params, .. } if *params != 0 => {
                self.fail::<()>(
                    span,
                    "may return one of its array parameters (a returned array must be created by \
                     the call)",
                );
            }
            Kind::ParamStruct { .. } => {
                self.fail::<()>(span, "returns its object parameter");
            }
            Kind::Value {
                key: StructKey::Class(class),
                ..
            } if self.lw.is_root(self.id) => {
                let name = self.lw.class_name(*class);
                self.fail::<()>(
                    span,
                    format!("returns a `{name}` instance (exported functions can return object literals, not class instances)"),
                );
            }
            Kind::Value { fields, .. } if self.lw.is_root(self.id) => {
                if let Some(problem) = boundary_value_problem(fields) {
                    self.fail::<()>(span, format!("returns an object whose fields {problem}"));
                }
            }
            _ => {}
        }
    }

    /// The return type of `function`, from the joined kinds.
    fn ret_ty(&self, function: FunctionId) -> Ty {
        let kind = self
            .lw
            .env
            .rets
            .get(&function)
            .cloned()
            .unwrap_or(Kind::Bottom);
        match kind {
            Kind::Bottom | Kind::Unit => Ty::Unit,
            Kind::Num { undef: false } => {
                match self.lw.env.ret_facts.get(&function).copied().flatten() {
                    Some(n) if is_int32(n) => Ty::I32,
                    Some(n) if is_uint32(n) => Ty::U32,
                    _ => Ty::F64,
                }
            }
            other => self.kind_ty(&other, None, None),
        }
    }

    /// The Rust type for a value of `kind` held by `symbol` (or parameter `param`).
    fn kind_ty(
        &self,
        kind: &Kind,
        symbol: Option<SymbolRef>,
        param: Option<(FunctionId, usize)>,
    ) -> Ty {
        match kind {
            Kind::Bool => Ty::Bool,
            Kind::Num { undef: false } => match symbol.and_then(|s| self.symbol_fact(s)) {
                Some(n) if is_int32(n) => Ty::I32,
                Some(n) if is_uint32(n) => Ty::U32,
                _ => Ty::F64,
            },
            Kind::Array {
                elem,
                fresh,
                params,
            } => {
                let access = match param {
                    Some(key) if self.lw.env.mutated.contains(&key) => Access::Mut,
                    None if *params == 0 && *fresh => Access::Fresh,
                    _ => Access::Shared,
                };
                Ty::Array(ArrayTy {
                    elem: *elem,
                    access,
                })
            }
            Kind::Value { key, .. } => Ty::Struct(self.struct_ref(StructSlot::Key(*key))),
            Kind::ParamStruct { group } => {
                let access = match param {
                    Some(key) if self.lw.env.mutated.contains(&key) => Access::Mut,
                    _ => Access::Shared,
                };
                Ty::StructRef(self.struct_ref(StructSlot::Group(*group)), access)
            }
            Kind::Num { undef: true } | Kind::Bottom | Kind::Unit | Kind::Error => Ty::F64,
        }
    }

    fn struct_ref(&self, slot: StructSlot) -> StructRef {
        self.lw
            .struct_refs
            .get(&slot)
            .copied()
            .unwrap_or(StructRef(0))
    }

    fn new_local(
        &mut self,
        js_name: &str,
        symbol: Option<SymbolRef>,
        param: Option<usize>,
        kind: &Kind,
        mutable: bool,
    ) -> LocalId {
        let ty = self.kind_ty(kind, symbol, param.map(|p| (self.id, p)));
        self.new_local_ty(js_name, symbol, param, ty, mutable)
    }

    fn new_local_ty(
        &mut self,
        js_name: &str,
        symbol: Option<SymbolRef>,
        param: Option<usize>,
        ty: Ty,
        mutable: bool,
    ) -> LocalId {
        let base = names::rust_ident(js_name);
        let mut name = base.clone();
        let mut n = 2;
        while self.names.contains(names::bare(&name))
            || RESERVED_FN_NAMES.contains(&names::bare(&name))
        {
            name = format!("{}_{n}", names::bare(&base));
            if base.starts_with("r#") && names::bare(&name) == names::bare(&base) {
                name = format!("r#{name}");
            }
            n += 1;
        }
        self.names.insert(names::bare(&name).to_string());
        self.locals.push(Local { name, ty, mutable });
        self.info.push(LocalInfo {
            symbol,
            param,
            poisoned: false,
        });
        LocalId(self.locals.len() - 1)
    }

    fn temp(&mut self, name: &str, ty: Ty) -> LocalId {
        self.new_local_ty(name, None, None, ty, false)
    }

    /// The current kind of a local.
    fn local_kind(&self, local: LocalId) -> Kind {
        let info = &self.info[local.0];
        if info.poisoned {
            return Kind::Error;
        }
        if let Some(param) = info.param {
            if let Some(overrides) = &self.overrides {
                return normalize_param(
                    overrides.get(param).cloned().unwrap_or(Kind::Bottom),
                    param,
                );
            }
            let kind = self
                .lw
                .env
                .params
                .get(&(self.id, param))
                .cloned()
                .unwrap_or(Kind::Bottom);
            return normalize_param(kind, param);
        }
        match info.symbol {
            Some(symbol) => self
                .lw
                .env
                .locals
                .get(&symbol)
                .cloned()
                .unwrap_or(Kind::Bottom),
            None => kind_of_ty(self.locals[local.0].ty),
        }
    }

    fn local_value(&self, local: LocalId) -> V {
        V::new(
            Expr::Local(local),
            self.locals[local.0].ty,
            self.local_kind(local),
        )
    }

    // -------------------------------------------------------------------- constructors

    fn lower_ctor(&mut self, name: String) -> Function {
        let class = self.class.expect("constructors have a class");
        let model_ok = self.lw.class_model(class).is_ok();
        let struct_ref = self.struct_ref(StructSlot::Key(StructKey::Class(class)));
        let mut function = Function {
            name,
            kind: FunctionKind::Constructor(struct_ref),
            params: Vec::new(),
            locals: Vec::new(),
            ret: Ty::Struct(struct_ref),
            body: Vec::new(),
        };
        if !model_ok {
            if let Err((span, message)) = self.lw.class_model(class) {
                let message = format!("`{}` {message}", self.lw.class_name(class));
                self.fail::<()>(span, message);
            }
            return function;
        }
        let model = &self.lw.classes[&class];
        let ctor_func = model.ctor_func;
        let fields = model.fields.clone();
        let inits = model.inits.clone();
        // `this` is not readable while the fields are assigned.
        self.class = None;
        if let Some(func) = ctor_func {
            function.params = self.declare_params(func);
        }
        let mut kinds = vec![Kind::Bottom; fields.len()];
        let mut values = Vec::new();
        for (index, init) in inits {
            let Some(value) = self.expr(init) else {
                continue;
            };
            match &value.kind {
                Kind::Num { .. } | Kind::Bool | Kind::Value { .. } => {}
                other => {
                    let message = format!(
                        "stores {} in the field `{}`",
                        other.describe(),
                        fields[index]
                    );
                    self.fail::<()>(init.span(), message);
                    continue;
                }
            }
            kinds[index] = value.kind.clone();
            values.push((index, value));
        }
        if self.overrides.is_none() {
            let slot = self
                .lw
                .env
                .class_fields
                .entry(class)
                .or_insert_with(|| vec![Kind::Bottom; fields.len()]);
            for (slot, kind) in slot.iter_mut().zip(&kinds) {
                super::Env::join_into(&mut self.lw.env.changed, slot, kind);
            }
        }
        let field_tys: Vec<Ty> = if self.final_round() {
            self.lw.structs[struct_ref.0]
                .fields
                .iter()
                .map(|(_, ty)| *ty)
                .collect()
        } else {
            vec![Ty::F64; fields.len()]
        };
        let mut lit = Vec::new();
        for (index, value) in values {
            let ty = field_tys.get(index).copied().unwrap_or(Ty::F64);
            let Some(expr) = self.coerce(value, ty, false, Span::default()) else {
                continue;
            };
            lit.push((fields[index].clone(), expr));
        }
        self.ctor_kinds = Some(kinds);
        function.body = vec![Stmt::Return(Some(Expr::StructLit(struct_ref, lit)))];
        function.locals = std::mem::take(&mut self.locals);
        function
    }
}

/// A parameter's kind as seen inside its function: arrays are "parameter `index`".
fn normalize_param(kind: Kind, index: usize) -> Kind {
    match kind {
        Kind::Array { elem, .. } => Kind::Array {
            elem,
            fresh: false,
            params: 1 << index,
        },
        other => other,
    }
}

/// The kind a value of type `ty` has (for temporaries and constants).
fn kind_of_ty(ty: Ty) -> Kind {
    match ty {
        Ty::Bool => Kind::Bool,
        Ty::Unit => Kind::Unit,
        Ty::Array(array) => Kind::Array {
            elem: array.elem,
            fresh: array.access == Access::Fresh,
            params: 0,
        },
        _ => Kind::NUM,
    }
}

fn trust_hint(trust: trioxide_analysis::Trust) -> &'static str {
    match trust {
        trioxide_analysis::Trust::Declared => " (only TypeScript says so, and `trustTypes` is off)",
        _ => "",
    }
}

fn describe_type(ty: &Type) -> String {
    match ty {
        Type::Unknown => "anything".into(),
        Type::Never => "nothing".into(),
        Type::Number(_) => "a number".into(),
        Type::Boolean => "a boolean".into(),
        Type::String => "a string".into(),
        Type::BigInt => "a bigint".into(),
        Type::Symbol => "a symbol".into(),
        Type::Null => "null".into(),
        Type::Undefined => "undefined".into(),
        Type::Array(_) | Type::Tuple(_) => "an array".into(),
        Type::TypedArray(kind) => format!("a {}", kind.constructor_name()),
        Type::Object(_) => "an object".into(),
        Type::Instance(_) => "a class instance".into(),
        Type::Function(..) => "a function".into(),
        Type::Union(members) => members
            .iter()
            .map(describe_type)
            .collect::<Vec<_>>()
            .join(" or "),
    }
}

/// The ABI element of a typed array kind (`None` for the bigint kinds).
fn typed_element(kind: TypedArrayKind) -> Option<TypedArrayElement> {
    Some(match kind {
        TypedArrayKind::Int8 => TypedArrayElement::I8,
        TypedArrayKind::Uint8 => TypedArrayElement::U8,
        TypedArrayKind::Uint8Clamped => TypedArrayElement::U8Clamped,
        TypedArrayKind::Int16 => TypedArrayElement::I16,
        TypedArrayKind::Uint16 => TypedArrayElement::U16,
        TypedArrayKind::Int32 => TypedArrayElement::I32,
        TypedArrayKind::Uint32 => TypedArrayElement::U32,
        TypedArrayKind::Float32 => TypedArrayElement::F32,
        TypedArrayKind::Float64 => TypedArrayElement::F64,
        TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64 => return None,
    })
}

/// The property order of a local interface or object type alias named `name`.
fn interface_order(module: &Module<'_>, name: &str) -> Vec<String> {
    for statement in &module.program.body {
        let declaration = match statement {
            Statement::ExportDeclaration(export) => Some(&export.declaration),
            other => other.as_declaration(),
        };
        match declaration {
            Some(oxc_ast::ast::Declaration::TSInterfaceDeclaration(interface))
                if interface.id.name == name =>
            {
                return signature_names(&interface.body.body);
            }
            Some(oxc_ast::ast::Declaration::TSTypeAliasDeclaration(alias))
                if alias.id.name == name =>
            {
                if let oxc_ast::ast::TSType::TSTypeLiteral(literal) = &alias.type_annotation {
                    return signature_names(&literal.members);
                }
            }
            _ => {}
        }
    }
    Vec::new()
}

fn signature_names(members: &[oxc_ast::ast::TSSignature<'_>]) -> Vec<String> {
    members
        .iter()
        .filter_map(|member| match member {
            oxc_ast::ast::TSSignature::TSPropertySignature(property) => {
                property.key.static_name().map(|name| name.to_string())
            }
            _ => None,
        })
        .collect()
}

/// Why an object with these field kinds can't be returned across the boundary, if it can't.
fn boundary_value_problem(fields: &[Kind]) -> Option<&'static str> {
    for field in fields {
        match field {
            Kind::Num { undef: false } | Kind::Bool => {}
            Kind::Num { undef: true } => return Some("may be undefined"),
            Kind::Value {
                key: StructKey::Literal(_),
                fields,
            } => {
                if let Some(problem) = boundary_value_problem(fields) {
                    return Some(problem);
                }
            }
            _ => return Some("aren't all numbers, booleans or object literals"),
        }
    }
    None
}

/// Whether a parameter is annotated `number[]` or `Array<number>`.
fn is_number_array_annotation(param: &FormalParameter<'_>) -> bool {
    use oxc_ast::ast::{TSType, TSTypeName};
    let Some(annotation) = &param.type_annotation else {
        return false;
    };
    match &annotation.type_annotation {
        TSType::TSArrayType(array) => matches!(array.element_type, TSType::TSNumberKeyword(_)),
        TSType::TSTypeReference(reference) => {
            matches!(&reference.type_name, TSTypeName::IdentifierReference(id) if id.name == "Array")
                && reference.type_arguments.as_ref().is_some_and(|args| {
                    matches!(args.params.as_slice(), [TSType::TSNumberKeyword(_)])
                })
        }
        _ => false,
    }
}
