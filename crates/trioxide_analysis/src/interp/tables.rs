//! The whole-program tables as one chunk of a sweep sees and changes them.
//!
//! A chunk reads a snapshot of the tables (`base`) and writes to its own copy-on-write
//! overlay (`own`), so its units see each other's writes as they would running alone. Every
//! write is also logged as an [`Op`]; merging a sweep replays the chunks' logs, in chunk order,
//! onto the real tables with the same code (a `Tables` without a base). Replaying rather than
//! merging overlays keeps the tables' updates exactly what interpreting the units one after
//! another would do, whatever the order of the stale reads; the worklist re-runs every unit
//! whose reads a merge changed, so the fixed point is the same kind of fixed point.

use oxc_syntax::symbol::SymbolId;
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::{ModuleId, NodeRef, SymbolRef};
use trioxide_types::Type;

use super::{Dep, Escape, Obj, ObjKind, State, WIDEN_AFTER};
use crate::{
    facts::Trust,
    link::Links,
    numeric,
    value::{AllocId, Pts, Val},
    world::World,
};

/// One write to the whole-program tables.
pub enum Op {
    JoinSymbol(SymbolRef, Val),
    JoinReturn(NodeRef, Val),
    JoinThis(NodeRef, Val),
    JoinParam(NodeRef, usize, Val),
    JoinSuper(NodeRef, Val),
    JoinDefault(NodeRef, Val),
    SpreadCalled(NodeRef),
    Create(AllocId, ObjKind, NodeRef),
    StoreProp {
        site: AllocId,
        key: String,
        value: Val,
        creating: bool,
    },
    StoreElem(AllocId, Val),
    Mark(AllocId, Box<dyn Fn(&mut Obj) + Send>),
    Escape(Pts, Escape),
    Poison(Pts),
    /// A lexical binding may be read before its declaration runs.
    MayBeUninit(SymbolRef),
    /// Sets the entry to the top of the lattice (see `SATURATE_AFTER`); its objects escape.
    GiveUp(Dep),
}

/// What applying an op needs besides the tables.
#[derive(Clone, Copy)]
pub struct OpCx<'w, 'a> {
    pub world: &'w World<'a>,
    pub links: &'w Links,
    pub round: usize,
}

/// The tables, possibly as an overlay on a snapshot.
pub struct Tables<'s> {
    base: Option<&'s State>,
    /// The tables themselves (no base), or the entries this chunk wrote.
    pub own: State,
    /// Entries whose value changed, in order, since last drained.
    pub touched: Vec<Dep>,
    /// Entries read while applying ops (escaping a namespace reads its exports).
    pub reads: Vec<Dep>,
    /// Entries given up so far.
    pub given_up: usize,
}

impl<'s> Tables<'s> {
    /// The real tables.
    pub fn global(state: State) -> Self {
        Self {
            base: None,
            own: state,
            touched: Vec::new(),
            reads: Vec::new(),
            given_up: 0,
        }
    }

    /// An empty overlay on `base`.
    pub fn overlay(base: &'s State) -> Self {
        Self {
            base: Some(base),
            own: State::default(),
            touched: Vec::new(),
            reads: Vec::new(),
            given_up: 0,
        }
    }

    pub fn obj(&self, site: AllocId) -> Option<&Obj> {
        lookup(&self.own.heap, self.base.map(|base| &base.heap), &site).map(|obj| &**obj)
    }

    pub fn symbol(&self, symbol: SymbolRef) -> Option<&Val> {
        lookup(
            &self.own.symbols,
            self.base.map(|base| &base.symbols),
            &symbol,
        )
    }

    pub fn returned(&self, unit: NodeRef) -> Option<&Val> {
        lookup(
            &self.own.returns,
            self.base.map(|base| &base.returns),
            &unit,
        )
    }

    pub fn this(&self, unit: NodeRef) -> Option<&Val> {
        lookup(&self.own.this, self.base.map(|base| &base.this), &unit)
    }

    pub fn params(&self, unit: NodeRef) -> Option<&Vec<Val>> {
        lookup(&self.own.params, self.base.map(|base| &base.params), &unit)
    }

    pub fn super_of(&self, class: NodeRef) -> Option<&Val> {
        lookup(&self.own.supers, self.base.map(|base| &base.supers), &class)
    }

    pub fn default_value(&self, node: NodeRef) -> Option<&Val> {
        lookup(
            &self.own.defaults,
            self.base.map(|base| &base.defaults),
            &node,
        )
    }

    pub fn spread_called(&self, unit: NodeRef) -> bool {
        self.own.spread_called.contains(&unit)
            || self
                .base
                .is_some_and(|base| base.spread_called.contains(&unit))
    }

    pub fn may_be_uninit(&self, symbol: SymbolRef) -> bool {
        self.own.may_be_uninit.contains(&symbol)
            || self
                .base
                .is_some_and(|base| base.may_be_uninit.contains(&symbol))
    }

    pub fn open_instance(&self, class: NodeRef) -> bool {
        self.own.open_instances.contains(&class)
            || self
                .base
                .is_some_and(|base| base.open_instances.contains(&class))
    }

    /// The heap entry of `site` for writing, copied into the overlay first.
    fn obj_mut(&mut self, site: AllocId) -> Option<&mut Obj> {
        if !self.own.heap.contains_key(&site) {
            let copy = self.base?.heap.get(&site)?.clone();
            self.own.heap.insert(site, copy);
        }
        self.own.heap.get_mut(&site).map(|obj| &mut **obj)
    }

    /// Applies `op`, noting what changed in `touched`.
    pub fn apply(&mut self, cx: OpCx<'_, '_>, op: &Op) {
        match op {
            Op::JoinSymbol(symbol, value) => {
                let base = self.base;
                let slot = self.own.symbols.entry(*symbol).or_insert_with(|| {
                    base.and_then(|base| base.symbols.get(symbol).cloned())
                        .unwrap_or_else(Val::never)
                });
                if join_into(cx.round, slot, value) {
                    self.touched.push(Dep::Symbol(*symbol));
                }
            }
            Op::JoinReturn(unit, value) => {
                let base = self.base;
                let slot = self.own.returns.entry(*unit).or_insert_with(|| {
                    base.and_then(|base| base.returns.get(unit).cloned())
                        .unwrap_or_else(Val::never)
                });
                if join_into(cx.round, slot, value) {
                    self.touched.push(Dep::Return(*unit));
                }
            }
            Op::JoinThis(unit, value) => {
                let base = self.base;
                let slot = self.own.this.entry(*unit).or_insert_with(|| {
                    base.and_then(|base| base.this.get(unit).cloned())
                        .unwrap_or_else(Val::never)
                });
                if join_into(cx.round, slot, value) {
                    self.touched.push(Dep::This(*unit));
                }
            }
            Op::JoinParam(unit, index, value) => {
                let base = self.base;
                let params = self.own.params.entry(*unit).or_insert_with(|| {
                    base.and_then(|base| base.params.get(unit).cloned())
                        .unwrap_or_default()
                });
                let mut changed = false;
                if params.len() <= *index {
                    params.resize_with(index + 1, Val::never);
                    changed = true;
                }
                if join_into(cx.round, &mut params[*index], value) || changed {
                    self.touched.push(Dep::Params(*unit));
                }
            }
            Op::JoinSuper(class, value) => {
                let base = self.base;
                let slot = self.own.supers.entry(*class).or_insert_with(|| {
                    base.and_then(|base| base.supers.get(class).cloned())
                        .unwrap_or_else(Val::never)
                });
                if join_into(cx.round, slot, value) {
                    self.touched.push(Dep::Super(*class));
                    // A class unknown code already reaches exposes its new parent too.
                    let level = self.obj(*class).map_or(Escape::None, |obj| obj.escape);
                    let level = if self.open_instance(*class) {
                        Escape::Unknown
                    } else {
                        level
                    };
                    if level > Escape::None {
                        let parent = value.pts.clone();
                        self.escape(cx, &parent, level);
                    }
                }
            }
            Op::JoinDefault(node, value) => {
                let base = self.base;
                let slot = self.own.defaults.entry(*node).or_insert_with(|| {
                    base.and_then(|base| base.defaults.get(node).cloned())
                        .unwrap_or_else(Val::never)
                });
                if join_into(cx.round, slot, value) {
                    self.touched.push(Dep::Default(*node));
                }
            }
            Op::SpreadCalled(unit) => {
                if !self.spread_called(*unit) {
                    self.own.spread_called.insert(*unit);
                    self.touched.push(Dep::Params(*unit));
                }
            }
            Op::Create(site, kind, unit) => {
                if self.obj(*site).is_none() {
                    self.own
                        .heap
                        .insert(*site, Box::new(Obj::new(*kind, *unit)));
                    self.touched.push(Dep::Heap(*site));
                }
            }
            Op::StoreProp {
                site,
                key,
                value,
                creating,
            } => self.store_prop(cx, *site, key, value, *creating),
            Op::StoreElem(site, value) => {
                let Some(obj) = self.obj_mut(*site) else {
                    return;
                };
                let changed = join_into(cx.round, &mut obj.elem, value);
                let level = obj.escape.max(Escape::Known);
                if changed {
                    self.touched.push(Dep::Heap(*site));
                }
                self.escape(cx, &value.pts, level);
            }
            Op::Mark(site, update) => {
                let Some(obj) = self.obj_mut(*site) else {
                    return;
                };
                let flags = |obj: &Obj| {
                    (
                        obj.dynamic,
                        obj.deleted,
                        obj.holey,
                        obj.length,
                        obj.sized,
                        obj.present.len(),
                        obj.shares_buffer.len(),
                        obj.accessors,
                        obj.frozen,
                    )
                };
                let before = flags(obj);
                update(obj);
                // A given-up entry stays at the top, so an update that would lower a flag
                // again (and then be given up again, round after round) changes nothing.
                if obj.given_up {
                    obj.give_up();
                }
                if before != flags(obj) {
                    self.touched.push(Dep::Heap(*site));
                }
            }
            Op::Escape(pts, level) => self.escape(cx, pts, *level),
            Op::MayBeUninit(symbol) => {
                if !self.may_be_uninit(*symbol) {
                    self.own.may_be_uninit.insert(*symbol);
                    self.touched.push(Dep::Symbol(*symbol));
                }
            }
            Op::GiveUp(dep) => self.give_up(cx, *dep),
            Op::Poison(pts) => {
                self.escape(cx, pts, Escape::Unknown);
                for &site in pts.sites().unwrap_or_default() {
                    if self.obj(site).is_some_and(|obj| !obj.poisoned)
                        && let Some(obj) = self.obj_mut(site)
                    {
                        obj.poisoned = true;
                        self.touched.push(Dep::Heap(site));
                    }
                }
            }
        }
    }

    fn store_prop(
        &mut self,
        cx: OpCx<'_, '_>,
        site: AllocId,
        key: &str,
        value: &Val,
        creating: bool,
    ) {
        let Some(obj) = self.obj_mut(site) else {
            return;
        };
        let mut changed = false;
        if !obj.props.contains_key(key) {
            obj.order.push(key.to_string());
            changed = true;
        }
        let slot = obj.props.entry(key.to_string()).or_insert_with(Val::never);
        changed |= join_into(cx.round, slot, value);
        // A key the object didn't have when it was made (even if the code that made it adds
        // it, it does so later): its shape changes.
        if !creating && !obj.present.contains(key) && obj.added.insert(key.to_string()) {
            changed = true;
        }
        let level = obj.escape.max(Escape::Known);
        if changed {
            self.touched.push(Dep::Heap(site));
        }
        self.escape(cx, &value.pts, level);
    }

    /// Sets the entry `dep` to the top of the lattice: "any value" for a value entry, flags
    /// that say anything may happen for a heap entry. The objects it held escape.
    fn give_up(&mut self, cx: OpCx<'_, '_>, dep: Dep) {
        fn top(slot: &mut Val, lost: &mut Vec<Pts>) -> bool {
            if slot.trust != Trust::Unknown || slot.pts.sites().is_some() {
                lost.push(std::mem::take(&mut slot.pts));
                *slot = Val::unknown();
                true
            } else {
                false
            }
        }
        let mut lost = Vec::new();
        let changed = match dep {
            Dep::Symbol(symbol) => match self.symbol(symbol).cloned() {
                Some(mut slot) => {
                    let changed = top(&mut slot, &mut lost);
                    self.own.symbols.insert(symbol, slot);
                    changed
                }
                None => false,
            },
            Dep::Params(unit) => {
                let mut changed = !self.spread_called(unit);
                self.own.spread_called.insert(unit);
                if let Some(params) = self.params(unit).cloned() {
                    let mut params = params;
                    for slot in &mut params {
                        changed |= top(slot, &mut lost);
                    }
                    self.own.params.insert(unit, params);
                }
                changed
            }
            Dep::This(unit) => match self.this(unit).cloned() {
                Some(mut slot) => {
                    let changed = top(&mut slot, &mut lost);
                    self.own.this.insert(unit, slot);
                    changed
                }
                None => false,
            },
            Dep::Return(unit) => match self.returned(unit).cloned() {
                Some(mut slot) => {
                    let changed = top(&mut slot, &mut lost);
                    self.own.returns.insert(unit, slot);
                    changed
                }
                None => false,
            },
            Dep::Super(class) => match self.super_of(class).cloned() {
                Some(mut slot) => {
                    let changed = top(&mut slot, &mut lost);
                    self.own.supers.insert(class, slot);
                    changed
                }
                None => false,
            },
            Dep::Default(node) => match self.default_value(node).cloned() {
                Some(mut slot) => {
                    let changed = top(&mut slot, &mut lost);
                    self.own.defaults.insert(node, slot);
                    changed
                }
                None => false,
            },
            Dep::Heap(site) => {
                lost.push(Pts::site(site));
                self.obj_mut(site).is_some_and(Obj::give_up)
            }
        };
        if changed {
            self.touched.push(dep);
            self.given_up += 1;
        }
        for pts in lost {
            self.escape(cx, &pts, Escape::Unknown);
        }
    }

    /// Marks every site in `pts` (and what it reaches) as escaping at least to `level`.
    fn escape(&mut self, cx: OpCx<'_, '_>, pts: &Pts, level: Escape) {
        let Some(sites) = pts.sites() else {
            return;
        };
        let mut pending: Vec<AllocId> = sites.to_vec();
        while let Some(site) = pending.pop() {
            if self.obj(site).is_none_or(|obj| obj.escape >= level) {
                continue;
            }
            let obj = self.obj_mut(site).expect("the site exists");
            obj.escape = level;
            let kind = obj.kind;
            for value in obj.props.values().chain([&obj.elem]) {
                if let Some(inner) = value.pts.sites() {
                    pending.extend_from_slice(inner);
                }
            }
            if level == Escape::Unknown {
                pending.extend_from_slice(&obj.shares_buffer);
            }
            self.touched.push(Dep::Heap(site));
            if level == Escape::Unknown
                && let ObjKind::Instance(class) = kind
                && !self.open_instance(class)
            {
                self.own.open_instances.insert(class);
                self.touched.push(Dep::Heap(class));
            }
            // A namespace object reaches every export.
            if let ObjKind::Namespace(module) = kind {
                let mut exported = Vec::new();
                self.exported_values(cx, module, &mut exported, &mut FxHashSet::default());
                pending.extend(exported);
                continue;
            }
            // A class's prototype, constructor and methods go with it, and so do an
            // instance's (`instance.constructor` reaches the class).
            if let ObjKind::Class(class) | ObjKind::Instance(class) | ObjKind::Prototype(class) =
                kind
                && let Some(info) = cx.world.classes.get(&class)
            {
                pending.push(class);
                pending.push(info.init);
                pending.extend(info.members.iter().filter_map(|member| member.unit));
                pending.extend(info.constructor);
                // Inherited members are reachable through it too.
                self.reads.push(Dep::Super(class));
                if let Some(sites) = self.super_of(class).and_then(|parent| parent.pts.sites()) {
                    pending.extend_from_slice(sites);
                }
            }
        }
    }

    /// The allocation sites every export of `module` (and of what it re-exports) may hold.
    fn exported_values(
        &mut self,
        cx: OpCx<'_, '_>,
        module: ModuleId,
        out: &mut Vec<AllocId>,
        seen: &mut FxHashSet<ModuleId>,
    ) {
        if !seen.insert(module) {
            return;
        }
        let info = cx.world.module(module);
        for (index, symbol) in info.symbols.iter().enumerate() {
            if symbol.exported {
                let symbol = info.symbol(SymbolId::from_usize(index));
                self.reads.push(Dep::Symbol(symbol));
                if let Some(sites) = self.symbol(symbol).and_then(|value| value.pts.sites()) {
                    out.extend_from_slice(sites);
                }
            }
        }
        for entry in info
            .record
            .star_export_entries
            .iter()
            .chain(&info.record.indirect_export_entries)
        {
            if let Some(target) = entry
                .module_request
                .as_ref()
                .and_then(|request| info.imports.get(request.name.as_str()).copied().flatten())
            {
                self.exported_values(cx, target, out, seen);
            }
        }
        // `export default <expression>` values and re-exported imports.
        for (import, target) in cx.links.imports() {
            if import.module == module
                && info.info(import.symbol).exported
                && let crate::link::Target::Symbol(target) = target
            {
                self.reads.push(Dep::Symbol(target));
                if let Some(sites) = self.symbol(target).and_then(|value| value.pts.sites()) {
                    out.extend_from_slice(sites);
                }
            }
        }
        if let crate::link::Target::Expression(node) =
            crate::link::resolve_export(cx.world, info, "default", &mut FxHashSet::default())
        {
            self.reads.push(Dep::Default(node));
            if let Some(sites) = self.default_value(node).and_then(|value| value.pts.sites()) {
                out.extend_from_slice(sites);
            }
        }
    }
}

/// Every entry an overlay holds.
pub fn entries(own: &State) -> impl Iterator<Item = Dep> + '_ {
    own.symbols
        .keys()
        .map(|&symbol| Dep::Symbol(symbol))
        .chain(own.params.keys().map(|&unit| Dep::Params(unit)))
        .chain(own.spread_called.iter().map(|&unit| Dep::Params(unit)))
        .chain(own.this.keys().map(|&unit| Dep::This(unit)))
        .chain(own.returns.keys().map(|&unit| Dep::Return(unit)))
        .chain(own.heap.keys().map(|&site| Dep::Heap(site)))
        .chain(own.open_instances.iter().map(|&class| Dep::Heap(class)))
        .chain(own.supers.keys().map(|&class| Dep::Super(class)))
        .chain(own.defaults.keys().map(|&node| Dep::Default(node)))
        .chain(own.may_be_uninit.iter().map(|&symbol| Dep::Symbol(symbol)))
}

/// Moves an overlay's entries into the tables, replacing theirs: what replaying its writes
/// does when no one else wrote the entries since its snapshot.
pub fn install(own: State, global: &mut State) {
    global.symbols.extend(own.symbols);
    global.params.extend(own.params);
    global.spread_called.extend(own.spread_called);
    global.this.extend(own.this);
    global.returns.extend(own.returns);
    global.heap.extend(own.heap);
    global.supers.extend(own.supers);
    global.defaults.extend(own.defaults);
    global.open_instances.extend(own.open_instances);
    global.may_be_uninit.extend(own.may_be_uninit);
}

/// [`install`] for several overlays, in order, whose entries are all distinct: the big
/// tables on threads of their own.
pub fn install_all(mut owns: Vec<State>, global: &mut State) {
    fn take<T: Default>(owns: &mut [State], table: impl Fn(&mut State) -> &mut T) -> Vec<T> {
        owns.iter_mut()
            .map(|own| std::mem::take(table(own)))
            .collect()
    }
    fn extend<K: std::hash::Hash + Eq, V>(
        global: &mut FxHashMap<K, V>,
        parts: Vec<FxHashMap<K, V>>,
    ) {
        global.reserve(parts.iter().map(FxHashMap::len).sum());
        for part in parts {
            global.extend(part);
        }
    }
    let symbols = take(&mut owns, |own| &mut own.symbols);
    let heap = take(&mut owns, |own| &mut own.heap);
    let params = take(&mut owns, |own| &mut own.params);
    let this = take(&mut owns, |own| &mut own.this);
    let returns = take(&mut owns, |own| &mut own.returns);
    let State {
        symbols: global_symbols,
        heap: global_heap,
        params: global_params,
        this: global_this,
        returns: global_returns,
        spread_called,
        supers,
        defaults,
        open_instances,
        may_be_uninit,
    } = global;
    std::thread::scope(|scope| {
        scope.spawn(|| extend(global_symbols, symbols));
        scope.spawn(|| extend(global_heap, heap));
        scope.spawn(|| {
            extend(global_params, params);
            extend(global_this, this);
            extend(global_returns, returns);
        });
        for own in owns {
            spread_called.extend(own.spread_called);
            supers.extend(own.supers);
            defaults.extend(own.defaults);
            open_instances.extend(own.open_instances);
            may_be_uninit.extend(own.may_be_uninit);
        }
    });
}

/// Whether a chunk's overlay `own` ended up with the value `global` now holds for `dep`: the
/// chunk wrote the entry, and the merge left it as the chunk saw it.
pub fn agrees(own: &State, dep: Dep, global: &State) -> bool {
    match dep {
        Dep::Symbol(symbol) => {
            (own.symbols.contains_key(&symbol) || own.may_be_uninit.contains(&symbol))
                && own.symbols.get(&symbol) == global.symbols.get(&symbol)
                && own.may_be_uninit.contains(&symbol) == global.may_be_uninit.contains(&symbol)
        }
        Dep::Params(unit) => {
            same(own.params.get(&unit), global.params.get(&unit))
                && own.spread_called.contains(&unit) == global.spread_called.contains(&unit)
        }
        Dep::This(unit) => same(own.this.get(&unit), global.this.get(&unit)),
        Dep::Return(unit) => same(own.returns.get(&unit), global.returns.get(&unit)),
        Dep::Heap(site) => {
            same(own.heap.get(&site), global.heap.get(&site))
                && own.open_instances.contains(&site) == global.open_instances.contains(&site)
        }
        Dep::Super(class) => same(own.supers.get(&class), global.supers.get(&class)),
        Dep::Default(node) => same(own.defaults.get(&node), global.defaults.get(&node)),
    }
}

/// `key` in the overlay `own`, else in `base`.
fn lookup<'m, K: std::hash::Hash + Eq, V>(
    own: &'m FxHashMap<K, V>,
    base: Option<&'m FxHashMap<K, V>>,
    key: &K,
) -> Option<&'m V> {
    let own = if own.is_empty() { None } else { own.get(key) };
    own.or_else(|| base?.get(key))
}

/// Both present and equal.
fn same<T: PartialEq>(own: Option<&T>, global: Option<&T>) -> bool {
    own.is_some() && own == global
}

/// Joins into a whole-program table entry, noting whether it changed.
pub fn join_into(round: usize, slot: &mut Val, value: &Val) -> bool {
    if slot == value || (value.ty == Type::Never && value.pts.is_none()) {
        return false;
    }
    let mut joined = slot.join(value);
    if round >= WIDEN_AFTER {
        joined.ty = numeric::widen_type(&slot.ty, &joined.ty);
    }
    if joined == *slot {
        return false;
    }
    *slot = joined;
    true
}
