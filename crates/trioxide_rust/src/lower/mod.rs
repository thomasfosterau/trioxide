//! Lowering an island from the oxc AST plus [`Facts`] to [`rir`](crate::rir).
//!
//! Lowering doubles as the subset checker: a function is eligible exactly when it lowers.
//! Every construct the lowering can't translate with JS-exact semantics records a [`Reason`]
//! with the span of the construct.
//!
//! It runs in rounds. Each round lowers every member of the island against the kinds the
//! previous round inferred ([`Env`]): the kinds of locals, parameters, returns and fields
//! are joined over every assignment, argument and return, so they only grow. When a round
//! changes nothing, a final round chooses Rust types (consulting range facts for `i32` and
//! `u32`) and produces the RIR.

mod class;
mod func;

use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_analysis::Facts;
use trioxide_common::{NodeRef, Span, SymbolRef};
use trioxide_types::{ClassId, FunctionId, NumberType};
use trioxide_wasm::AbiType;

pub(crate) use class::ClassModel;

use crate::{
    ExtraFacts,
    kinds::{Kind, StructKey},
    rir::{self, FuncRef, StructRef, Ty},
    world::World,
};

/// Why a function can't be compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    /// The function.
    pub function: FunctionId,
    /// The offending construct.
    pub span: Span,
    /// What is wrong, as a sentence fragment ("calls `console.log`, a host API").
    pub message: String,
}

/// The most rounds before giving up on inference.
const MAX_ROUNDS: usize = 32;

/// What a boundary struct parameter group knows about one field.
#[derive(Debug, Clone, Default)]
pub(crate) struct GroupField {
    pub name: String,
    /// The join of the facts at every read (`None` once one is unusable or not a number).
    pub read: Option<FieldRead>,
    pub reads: usize,
    /// The join of every written value's kind.
    pub written: Option<Kind>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FieldRead {
    Number(NumberType),
    Bool,
}

/// One boundary struct type: parameters that share it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Group {
    pub name: String,
    /// Declared field order, from the TS type when it is a local interface or type literal.
    pub order: Vec<String>,
    pub fields: Vec<GroupField>,
}

impl Group {
    pub fn field_mut(&mut self, name: &str) -> &mut GroupField {
        if let Some(index) = self.fields.iter().position(|f| f.name == name) {
            return &mut self.fields[index];
        }
        self.fields.push(GroupField {
            name: name.to_string(),
            ..GroupField::default()
        });
        self.fields.last_mut().expect("just pushed")
    }

    /// Fields in declaration order, then first-use order.
    pub fn ordered(&self) -> Vec<&GroupField> {
        let mut fields: Vec<&GroupField> = self.fields.iter().collect();
        fields.sort_by_key(|f| {
            self.order
                .iter()
                .position(|name| *name == f.name)
                .unwrap_or(usize::MAX)
        });
        fields
    }

    /// A field's Rust type.
    pub fn field_ty(field: &GroupField) -> Ty {
        match (field.read, &field.written) {
            (Some(FieldRead::Bool), None | Some(Kind::Bool)) => Ty::Bool,
            (Some(FieldRead::Number(n)), None) if crate::kinds::is_int32(n) => Ty::I32,
            _ => Ty::F64,
        }
    }
}

/// An object literal site.
#[derive(Debug, Clone, Default)]
pub(crate) struct LiteralModel {
    pub fields: Vec<String>,
    pub kinds: Vec<Kind>,
    /// The function that returns it directly, for naming.
    pub returned_by: Option<FunctionId>,
}

/// Inferred kinds, joined across the island.
#[derive(Default)]
pub(crate) struct Env {
    /// Parameter kinds; index `usize::MAX` is `this`.
    pub params: FxHashMap<(FunctionId, usize), Kind>,
    pub rets: FxHashMap<FunctionId, Kind>,
    /// The join of the range facts of every returned number (`None`: some is unknown).
    pub ret_facts: FxHashMap<FunctionId, Option<NumberType>>,
    pub locals: FxHashMap<SymbolRef, Kind>,
    pub class_fields: FxHashMap<ClassId, Vec<Kind>>,
    pub literals: FxHashMap<NodeRef, LiteralModel>,
    pub groups: Vec<Group>,
    /// Parameters written through directly.
    pub writes: FxHashSet<(FunctionId, usize)>,
    /// `(caller, parameter, callee, parameter)`: the caller passes its parameter on.
    pub passes: FxHashSet<(FunctionId, usize, FunctionId, usize)>,
    /// Parameters the island may write to, directly or through callees.
    pub mutated: FxHashSet<(FunctionId, usize)>,
    /// Call edges between members.
    pub calls: FxHashSet<(FunctionId, FunctionId, Span)>,
    pub changed: bool,
}

impl Env {
    /// Joins `kind` into `slot`, noting whether anything changed.
    pub fn join_into(changed: &mut bool, slot: &mut Kind, kind: &Kind) {
        let joined = slot.join(kind);
        if joined != *slot {
            *slot = joined;
            *changed = true;
        }
    }

    pub fn join_param(&mut self, function: FunctionId, index: usize, kind: &Kind) {
        let slot = self.params.entry((function, index)).or_insert(Kind::Bottom);
        Self::join_into(&mut self.changed, slot, kind);
    }

    pub fn join_local(&mut self, symbol: SymbolRef, kind: &Kind) {
        let slot = self.locals.entry(symbol).or_insert(Kind::Bottom);
        Self::join_into(&mut self.changed, slot, kind);
    }

    pub fn join_ret(&mut self, function: FunctionId, kind: &Kind, fact: Option<NumberType>) {
        let slot = self.rets.entry(function).or_insert(Kind::Bottom);
        Self::join_into(&mut self.changed, slot, kind);
        let joined = match self.ret_facts.get(&function) {
            None => fact,
            Some(None) => None,
            Some(Some(previous)) => fact.map(|fact| previous.join(fact)),
        };
        if self.ret_facts.get(&function) != Some(&joined) {
            self.ret_facts.insert(function, joined);
            self.changed = true;
        }
    }

    fn compute_mutated(&mut self) {
        let mut mutated = self.writes.clone();
        loop {
            let before = mutated.len();
            for &(caller, index, callee, callee_index) in &self.passes {
                if mutated.contains(&(callee, callee_index)) {
                    mutated.insert((caller, index));
                }
            }
            if mutated.len() == before {
                break;
            }
        }
        if mutated != self.mutated {
            self.mutated = mutated;
            self.changed = true;
        }
    }
}

/// Island-wide lowering state.
pub(crate) struct Lowerer<'w, 'a> {
    pub world: &'w World<'a>,
    pub facts: &'w Facts,
    pub extra: &'w dyn ExtraFacts,
    pub env: Env,
    /// Exported functions with their boundary parameter kinds.
    pub roots: Vec<FunctionId>,
    pub members: Vec<FunctionId>,
    member_index: FxHashMap<FunctionId, usize>,
    pub classes: FxHashMap<ClassId, ClassModel<'a>>,
    /// Whether this is the final round (types chosen, reasons kept, RIR produced).
    pub final_round: bool,
    /// Struct indices, assigned before the final round.
    pub struct_refs: FxHashMap<StructSlot, StructRef>,
    pub structs: Vec<rir::StructDef>,
    pub consts: Vec<rir::ConstDef>,
    pub const_refs: FxHashMap<SymbolRef, rir::ConstRef>,
    /// Rust names of functions.
    pub fn_names: FxHashMap<FunctionId, String>,
    /// Classes that can't be compiled, and why.
    pub bad_classes: FxHashMap<ClassId, (Span, String)>,
    /// Nesting of constructor evaluations for one `new` ([`func`]).
    pub ctor_depth: usize,
}

/// What a Rust struct stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum StructSlot {
    Group(usize),
    Key(StructKey),
}

/// The outcome of lowering one island.
pub(crate) struct Lowered {
    /// The island, if any root survived.
    pub island: Option<rir::Island>,
    /// Roots that could not be compiled, with every reason (their own and their callees').
    pub failed: Vec<(FunctionId, Vec<Reason>)>,
    /// Every member of the island that was compiled.
    pub members: Vec<FunctionId>,
}

/// Lowers the island made of `roots` (exported) and every function they reach.
pub(crate) fn lower_island(
    world: &World<'_>,
    facts: &Facts,
    extra: &dyn ExtraFacts,
    id: &str,
    module_label: &str,
    roots: &[FunctionId],
) -> Lowered {
    let mut roots = roots.to_vec();
    let mut failed = Vec::new();
    loop {
        if roots.is_empty() {
            return Lowered {
                island: None,
                failed,
                members: Vec::new(),
            };
        }
        let mut lowerer = Lowerer::new(world, facts, extra, roots.clone());
        let converged = lowerer.infer();
        let (functions, reasons) = if converged {
            lowerer.assign_structs();
            lowerer.final_round = true;
            lowerer.lower_all()
        } else {
            let reasons = roots
                .iter()
                .map(|&root| Reason {
                    function: root,
                    span: world.functions[&root].func.span,
                    message: "the types of its values didn't settle (the island is too complex \
                              for the backend)"
                        .into(),
                })
                .collect();
            (Vec::new(), reasons)
        };
        if reasons.is_empty() {
            let island = lowerer.finish(id, module_label, functions);
            return Lowered {
                island: Some(island),
                failed,
                members: lowerer.members.clone(),
            };
        }
        // A function fails with its own reasons or those of a member it calls.
        let mut failing: FxHashSet<FunctionId> = reasons.iter().map(|r| r.function).collect();
        loop {
            let before = failing.len();
            for &(caller, callee, _) in &lowerer.env.calls {
                if failing.contains(&callee) {
                    failing.insert(caller);
                }
            }
            if failing.len() == before {
                break;
            }
        }
        let mut next = Vec::new();
        for root in roots {
            if !failing.contains(&root) {
                next.push(root);
                continue;
            }
            let mut own = Vec::new();
            let mut seen = FxHashSet::default();
            collect_reasons(&lowerer, &reasons, root, &mut seen, &mut own);
            failed.push((root, own));
        }
        roots = next;
    }
}

fn collect_reasons(
    lowerer: &Lowerer<'_, '_>,
    reasons: &[Reason],
    function: FunctionId,
    seen: &mut FxHashSet<FunctionId>,
    out: &mut Vec<Reason>,
) {
    if !seen.insert(function) {
        return;
    }
    out.extend(reasons.iter().filter(|r| r.function == function).cloned());
    let mut callees: Vec<(FunctionId, Span)> = lowerer
        .env
        .calls
        .iter()
        .filter(|(caller, _, _)| *caller == function)
        .map(|&(_, callee, span)| (callee, span))
        .collect();
    callees.sort_by_key(|(_, span)| span.start);
    for (callee, span) in callees {
        if reasons.iter().any(|r| r.function == callee) || {
            let mut probe = Vec::new();
            let mut probe_seen = seen.clone();
            collect_reasons(lowerer, reasons, callee, &mut probe_seen, &mut probe);
            !probe.is_empty()
        } {
            out.push(Reason {
                function,
                span,
                message: format!(
                    "calls `{}`, which can't be compiled to WASM",
                    lowerer.display_name(callee)
                ),
            });
            collect_reasons(lowerer, reasons, callee, seen, out);
        }
    }
}

impl<'w, 'a> Lowerer<'w, 'a> {
    fn new(
        world: &'w World<'a>,
        facts: &'w Facts,
        extra: &'w dyn ExtraFacts,
        roots: Vec<FunctionId>,
    ) -> Self {
        let mut lowerer = Self {
            world,
            facts,
            extra,
            env: Env::default(),
            roots: roots.clone(),
            members: Vec::new(),
            member_index: FxHashMap::default(),
            classes: FxHashMap::default(),
            final_round: false,
            struct_refs: FxHashMap::default(),
            structs: Vec::new(),
            consts: Vec::new(),
            const_refs: FxHashMap::default(),
            fn_names: FxHashMap::default(),
            bad_classes: FxHashMap::default(),
            ctor_depth: 0,
        };
        for root in roots {
            lowerer.member(root);
        }
        lowerer
    }

    /// A human-readable name for messages.
    pub fn display_name(&self, function: FunctionId) -> String {
        if let Some(info) = self.world.functions.get(&function) {
            let name = info.name.clone().unwrap_or_else(|| "<anonymous>".into());
            return match info.role {
                crate::world::Role::Constructor(class) | crate::world::Role::Method(class) => {
                    format!("{}.{name}", self.class_name(class))
                }
                _ => name,
            };
        }
        // A synthesised constructor is keyed by its class.
        format!("{}.constructor", self.class_name(function))
    }

    pub fn class_name(&self, class: ClassId) -> String {
        self.world
            .classes
            .get(&class)
            .and_then(|info| info.name.clone())
            .unwrap_or_else(|| "<class>".into())
    }

    /// The island index of `function`, adding it as a member.
    pub fn member(&mut self, function: FunctionId) -> FuncRef {
        if let Some(&index) = self.member_index.get(&function) {
            return FuncRef(index);
        }
        let index = self.members.len();
        self.members.push(function);
        self.member_index.insert(function, index);
        self.env.changed = true;
        FuncRef(index)
    }

    /// The class's model, built on first use.
    pub fn class_model(&mut self, class: ClassId) -> Result<&ClassModel<'a>, (Span, String)> {
        if let Some(error) = self.bad_classes.get(&class) {
            return Err(error.clone());
        }
        if !self.classes.contains_key(&class) {
            match ClassModel::build(self.world, self.facts, class) {
                Ok(model) => {
                    self.classes.insert(class, model);
                    self.env.changed = true;
                }
                Err(error) => {
                    self.bad_classes.insert(class, error.clone());
                    return Err(error);
                }
            }
        }
        Ok(&self.classes[&class])
    }

    /// The Rust name of a member function, unique in the island (methods within their
    /// class).
    pub fn fn_name(&mut self, function: FunctionId) -> String {
        if let Some(name) = self.fn_names.get(&function) {
            return name.clone();
        }
        let (base, method_of) = match self.world.functions.get(&function) {
            Some(info) => match info.role {
                crate::world::Role::Method(class) => (
                    crate::names::rust_ident(info.name.as_deref().unwrap_or("method")),
                    Some(class),
                ),
                crate::world::Role::Constructor(class) => ("new".to_string(), Some(class)),
                _ => (
                    crate::names::rust_ident(info.name.as_deref().unwrap_or("anonymous")),
                    None,
                ),
            },
            None => ("new".to_string(), Some(function)),
        };
        let taken = |lowerer: &Self, name: &str| {
            lowerer.fn_names.iter().any(|(other, existing)| {
                crate::names::bare(existing) == crate::names::bare(name)
                    && match (method_of, lowerer.method_class(*other)) {
                        (Some(a), Some(b)) => a == b,
                        (None, None) => true,
                        _ => false,
                    }
            }) || (method_of.is_none()
                && ["size_of", "align_of", "new"].contains(&crate::names::bare(name)))
                || name.starts_with("__abi_")
        };
        let mut name = base.clone();
        let mut n = 2;
        while taken(self, &name) {
            name = format!("{}_{n}", crate::names::bare(&base));
            n += 1;
        }
        self.fn_names.insert(function, name.clone());
        name
    }

    fn method_class(&self, function: FunctionId) -> Option<ClassId> {
        match self.world.functions.get(&function).map(|info| info.role) {
            Some(crate::world::Role::Method(class) | crate::world::Role::Constructor(class)) => {
                Some(class)
            }
            Some(_) => None,
            None => Some(function),
        }
    }

    pub fn is_root(&self, function: FunctionId) -> bool {
        self.roots.contains(&function)
    }

    /// Runs rounds until nothing changes. Returns whether it converged.
    fn infer(&mut self) -> bool {
        for _ in 0..MAX_ROUNDS {
            self.env.changed = false;
            let _ = self.lower_all();
            self.env.compute_mutated();
            if !self.env.changed {
                return true;
            }
        }
        false
    }

    /// Lowers every member (members found on the way included).
    fn lower_all(&mut self) -> (Vec<rir::Function>, Vec<Reason>) {
        let mut functions = Vec::new();
        let mut reasons = Vec::new();
        let mut index = 0;
        while index < self.members.len() {
            let function = self.members[index];
            let (lowered, mut own) = func::lower_function(self, function);
            functions.push(lowered);
            reasons.append(&mut own);
            index += 1;
        }
        (functions, reasons)
    }

    /// Assigns every struct an index: boundary structs first (parameter groups, then returned
    /// literals, which become the spec's structs in this order), then the rest.
    fn assign_structs(&mut self) {
        let mut slots: Vec<StructSlot> =
            (0..self.env.groups.len()).map(StructSlot::Group).collect();
        let mut abi_literals: Vec<NodeRef> = Vec::new();
        for root in &self.roots {
            if let Some(Kind::Value {
                key: StructKey::Literal(site),
                fields,
            }) = self.env.rets.get(root)
            {
                collect_literals(*site, fields, &mut abi_literals);
            }
        }
        abi_literals.dedup();
        let mut seen = FxHashSet::default();
        abi_literals.retain(|site| seen.insert(*site));
        slots.extend(
            abi_literals
                .iter()
                .map(|&site| StructSlot::Key(StructKey::Literal(site))),
        );
        let mut others: Vec<StructSlot> = self
            .env
            .literals
            .keys()
            .filter(|site| !abi_literals.contains(site))
            .map(|&site| StructSlot::Key(StructKey::Literal(site)))
            .chain(
                self.classes
                    .keys()
                    .map(|&class| StructSlot::Key(StructKey::Class(class))),
            )
            .collect();
        others.sort();
        slots.extend(others);

        let mut names: FxHashSet<String> = FxHashSet::default();
        for (index, slot) in slots.iter().enumerate() {
            let base = match slot {
                StructSlot::Group(group) => self.env.groups[*group].name.clone(),
                StructSlot::Key(StructKey::Class(class)) => {
                    crate::names::type_name(&self.class_name(*class))
                }
                StructSlot::Key(StructKey::Literal(site)) => {
                    let model = &self.env.literals[site];
                    model.returned_by.map_or_else(
                        || "Object".to_string(),
                        |function| self.return_type_name(function),
                    )
                }
            };
            let mut name = base.clone();
            let mut n = 2;
            while names.contains(&name) || crate::names::is_reserved_type(&name) {
                name = format!("{base}{n}");
                n += 1;
            }
            names.insert(name.clone());
            let abi = match slot {
                StructSlot::Group(_) => Some(index),
                StructSlot::Key(StructKey::Literal(site)) => abi_literals
                    .iter()
                    .position(|s| s == site)
                    .map(|position| self.env.groups.len() + position),
                StructSlot::Key(StructKey::Class(_)) => None,
            };
            self.struct_refs.insert(*slot, StructRef(index));
            self.structs.push(rir::StructDef {
                name,
                fields: Vec::new(),
                doc: String::new(),
                abi,
                methods: Vec::new(),
            });
        }
        // Fields, now that every struct has an index.
        for (slot, &StructRef(index)) in &self.struct_refs.clone() {
            let (fields, doc) = match slot {
                StructSlot::Group(group) => {
                    let group = &self.env.groups[*group];
                    (
                        group
                            .ordered()
                            .into_iter()
                            .map(|field| (field.name.clone(), Group::field_ty(field)))
                            .collect(),
                        format!("`{}`, a plain-data object parameter", group.name),
                    )
                }
                StructSlot::Key(StructKey::Literal(site)) => {
                    let model = &self.env.literals[site];
                    (
                        model
                            .fields
                            .iter()
                            .zip(&model.kinds)
                            .map(|(name, kind)| (name.clone(), self.value_field_ty(kind)))
                            .collect(),
                        format!(
                            "`{{ {} }}`, the object literal{}",
                            model.fields.join(", "),
                            model.returned_by.map_or_else(String::new, |f| format!(
                                " `{}` returns",
                                self.display_name(f)
                            ))
                        ),
                    )
                }
                StructSlot::Key(StructKey::Class(class)) => {
                    let model = &self.classes[class];
                    let kinds = self
                        .env
                        .class_fields
                        .get(class)
                        .cloned()
                        .unwrap_or_default();
                    (
                        model
                            .fields
                            .iter()
                            .enumerate()
                            .map(|(i, name)| {
                                (
                                    name.clone(),
                                    self.value_field_ty(kinds.get(i).unwrap_or(&Kind::NUM)),
                                )
                            })
                            .collect(),
                        format!(
                            "The class `{}`, used only inside the island",
                            self.class_name(*class)
                        ),
                    )
                }
            };
            self.structs[index].fields = fields;
            self.structs[index].doc = doc;
        }
    }

    /// The Rust type of a value field of the given kind.
    pub fn value_field_ty(&self, kind: &Kind) -> Ty {
        match kind {
            Kind::Bool => Ty::Bool,
            Kind::Value { key, .. } => self
                .struct_refs
                .get(&StructSlot::Key(*key))
                .map_or(Ty::F64, |&s| Ty::Struct(s)),
            _ => Ty::F64,
        }
    }

    /// The name for the struct a function returns.
    fn return_type_name(&self, function: FunctionId) -> String {
        if let Some(info) = self.world.functions.get(&function)
            && let Some(annotation) = &info.func.return_type
            && let Some(name) = crate::names::type_reference_name(&annotation.type_annotation)
        {
            return crate::names::type_name(&name);
        }
        crate::names::type_name(&self.display_name(function))
    }

    /// Assembles the island from the final round.
    fn finish(&mut self, id: &str, module: &str, mut functions: Vec<rir::Function>) -> rir::Island {
        for (index, function) in functions.iter().enumerate() {
            if let rir::FunctionKind::Method(StructRef(s))
            | rir::FunctionKind::Constructor(StructRef(s)) = function.kind
            {
                self.structs[s].methods.push(FuncRef(index));
            }
        }
        let mut exports = Vec::new();
        for &root in &self.roots {
            let func = FuncRef(self.member_index[&root]);
            let lowered = &functions[func.0];
            let info = &self.world.functions[&root];
            let name = info.name.clone().unwrap_or_default();
            let mut params = Vec::new();
            for (index, &local) in lowered.params.iter().enumerate() {
                let local = &lowered.locals[local.0];
                let mutated = self.env.mutated.contains(&(root, index));
                let ty = abi_type(local.ty, &self.structs).expect("checked by the final round");
                let is_aggregate = !ty.is_scalar();
                params.push((
                    crate::names::abi_param_name(&info.func.params.items[index], index),
                    ty,
                    mutated && is_aggregate,
                ));
            }
            exports.push(rir::Export {
                func,
                name,
                params,
                ret: abi_type(lowered.ret, &self.structs),
            });
        }
        for function in &mut functions {
            function.body.shrink_to_fit();
        }
        rir::Island {
            id: id.to_string(),
            module: module.to_string(),
            structs: std::mem::take(&mut self.structs),
            consts: std::mem::take(&mut self.consts),
            functions,
            exports,
        }
    }
}

fn collect_literals(site: NodeRef, fields: &[Kind], out: &mut Vec<NodeRef>) {
    out.push(site);
    for field in fields {
        if let Kind::Value {
            key: StructKey::Literal(nested),
            fields,
        } = field
        {
            collect_literals(*nested, fields, out);
        }
    }
}

/// The ABI type of a boundary value, if it can cross.
pub(crate) fn abi_type(ty: Ty, structs: &[rir::StructDef]) -> Option<AbiType> {
    Some(match ty {
        Ty::F64 => AbiType::F64,
        Ty::I32 => AbiType::I32,
        Ty::U32 => AbiType::U32,
        Ty::Bool => AbiType::Bool,
        Ty::Array(array) => match array.elem {
            rir::Elem::Typed(element) => AbiType::TypedArray { element },
            rir::Elem::Number => AbiType::NumberArray,
        },
        Ty::Struct(s) | Ty::StructRef(s, _) => AbiType::Struct {
            index: structs[s.0].abi?,
        },
        Ty::Index | Ty::Unit => return None,
    })
}
