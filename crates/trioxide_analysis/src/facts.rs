use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use trioxide_common::{ModuleId, NodeRef, Span, SymbolRef};
use trioxide_types::{ClassId, FunctionId, NumberType, Type};

/// How far a fact can be relied on (PLAN D1).
///
/// Ordered from weakest to strongest, so `trust >= Trust::Declared` reads naturally.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Trust {
    /// Nothing is known; the value is a placeholder (e.g. [`Type::Unknown`]).
    #[default]
    Unknown,
    /// TypeScript says so, but the claim rests on an annotation, cast, `any` or an open-world
    /// boundary. Only usable as proof with `trustTypes: true`.
    Declared,
    /// Derived from literals, operators, builtin models and flow narrowing with no unsound
    /// construct on the path. Always usable as proof.
    Proven,
}

/// A value together with how far it can be trusted.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Fact<T> {
    /// What is claimed.
    pub value: T,
    /// How far the claim can be relied on.
    pub trust: Trust,
}

impl<T> Fact<T> {
    /// A fact proven sound.
    pub const fn proven(value: T) -> Self {
        Self {
            value,
            trust: Trust::Proven,
        }
    }

    /// A fact that rests on TS declarations.
    pub const fn declared(value: T) -> Self {
        Self {
            value,
            trust: Trust::Declared,
        }
    }

    /// Borrows the value.
    pub const fn as_ref(&self) -> Fact<&T> {
        Fact {
            value: &self.value,
            trust: self.trust,
        }
    }
}

impl<T: Default> Fact<T> {
    /// The "nothing known" fact: the default value at [`Trust::Unknown`].
    pub fn unknown() -> Self {
        Self {
            value: T::default(),
            trust: Trust::Unknown,
        }
    }
}

/// What a function may do when called (PLAN §2 step 4, "effect and purity summaries").
///
/// Every flag is conservative: `true` means "may", `false` means "proven not to" at the
/// fact's trust level. [`Effects::default`] is the "anything" summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(clippy::struct_excessive_bools, reason = "independent effect flags")]
pub struct Effects {
    /// Reads mutable state outside its own locals.
    pub reads: bool,
    /// Writes state outside its own locals.
    pub writes: bool,
    /// Allocates objects that may outlive the call.
    pub allocates: bool,
    /// May throw.
    pub throws: bool,
    /// Calls a function whose effects are unknown.
    pub calls_unknown: bool,
}

impl Default for Effects {
    fn default() -> Self {
        Self::ANY
    }
}

impl Effects {
    /// May do anything.
    pub const ANY: Self = Self {
        reads: true,
        writes: true,
        allocates: true,
        throws: true,
        calls_unknown: true,
    };
    /// Does nothing observable.
    pub const PURE: Self = Self {
        reads: false,
        writes: false,
        allocates: false,
        throws: false,
        calls_unknown: false,
    };

    /// No writes, throws or unknown calls: removable when the result is unused.
    pub const fn is_side_effect_free(self) -> bool {
        !self.writes && !self.throws && !self.calls_unknown
    }
}

/// How far an allocated object may travel from the code that creates it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Escape {
    /// Only the creating function (or module) ever holds it: it is never returned, stored
    /// outside its locals, passed to a call or captured by a closure that could outlive it.
    None,
    /// Other analysed code may hold it, but nothing trioxide can't see.
    Known,
    /// Code outside the analysis may hold it. The default: nothing is known.
    #[default]
    Unknown,
}

/// What kind of object an allocation site creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AllocKind {
    /// An object literal.
    Object,
    /// An array literal, `new Array` or an array a builtin returns.
    Array,
    /// A typed array.
    TypedArray(trioxide_types::TypedArrayKind),
    /// An instance of a program class.
    Instance(ClassId),
    /// A function or arrow.
    Function,
    /// A class.
    Class,
    /// Something else (a regexp, `new Map`, …).
    Other,
}

/// Facts about one allocation site.
#[derive(Debug, Clone, PartialEq)]
pub struct Allocation {
    /// What it creates.
    pub kind: AllocKind,
    /// How far its objects travel. Always `Proven`: escape levels never rest on declared
    /// types (a call TypeScript resolves to a program method is still an unknown call, which
    /// escapes everything it is given), so neither do shapes.
    pub escape: Fact<Escape>,
    /// For object literals and arrays: the exact shape every object from the site keeps (no
    /// property added after creation, none deleted, no computed keys), if there is one.
    pub stable_shape: Fact<bool>,
}

/// Hidden-class facts for a class (PLAN §4 pass 10).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClassShape {
    /// Fields every instance has before any code can observe it, in definition order (class
    /// fields, then constructor assignments).
    pub fields: Vec<String>,
    /// Properties instances get after construction, outside the constructor.
    pub added_later: Vec<String>,
    /// Some instance property is deleted.
    pub deletes: bool,
    /// Instance properties are written with computed keys or redefined.
    pub dynamic: bool,
}

impl ClassShape {
    /// Whether instances keep one shape for life.
    pub fn is_stable(&self) -> bool {
        self.added_later.is_empty() && !self.deletes && !self.dynamic
    }
}

/// A call edge between code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CallEdge {
    /// The call (or `new`, accessor use, or builtin callback) site.
    pub site: NodeRef,
    /// The unit containing the site.
    pub caller: FunctionId,
    /// The unit called.
    pub callee: FunctionId,
    /// `Proven` from points-to; `Declared` from the receiver's TypeScript class.
    pub trust: Trust,
}

/// The call graph over code units: functions, methods, arrows, module top levels, class field
/// initialisers and static blocks.
#[derive(Debug, Clone, Default)]
pub struct CallGraph {
    units: Vec<FunctionId>,
    edges: Vec<CallEdge>,
    sccs: Vec<Vec<FunctionId>>,
    by_caller: FxHashMap<FunctionId, Vec<usize>>,
    by_site: FxHashMap<NodeRef, Vec<usize>>,
    by_callee: FxHashMap<FunctionId, Vec<usize>>,
}

impl CallGraph {
    /// A call graph over `units` with `edges` and their strongly connected components.
    pub fn new(units: Vec<FunctionId>, edges: Vec<CallEdge>, sccs: Vec<Vec<FunctionId>>) -> Self {
        let mut by_caller: FxHashMap<FunctionId, Vec<usize>> = FxHashMap::default();
        let mut by_site: FxHashMap<NodeRef, Vec<usize>> = FxHashMap::default();
        let mut by_callee: FxHashMap<FunctionId, Vec<usize>> = FxHashMap::default();
        for (index, edge) in edges.iter().enumerate() {
            by_caller.entry(edge.caller).or_default().push(index);
            by_site.entry(edge.site).or_default().push(index);
            by_callee.entry(edge.callee).or_default().push(index);
        }
        Self {
            units,
            edges,
            sccs,
            by_caller,
            by_site,
            by_callee,
        }
    }

    /// Every unit.
    pub fn units(&self) -> &[FunctionId] {
        &self.units
    }

    /// Every edge, sorted by caller and site.
    pub fn edges(&self) -> &[CallEdge] {
        &self.edges
    }

    /// The edges out of `caller`.
    pub fn edges_from(&self, caller: FunctionId) -> impl Iterator<Item = &CallEdge> {
        self.by_caller
            .get(&caller)
            .into_iter()
            .flatten()
            .map(|&index| &self.edges[index])
    }

    /// The edges into `callee`: every site known to call it.
    pub fn callers(&self, callee: FunctionId) -> impl Iterator<Item = &CallEdge> {
        self.by_callee
            .get(&callee)
            .into_iter()
            .flatten()
            .map(|&index| &self.edges[index])
    }

    /// The units a call site may call.
    pub fn callees(&self, site: NodeRef) -> impl Iterator<Item = &CallEdge> {
        self.by_site
            .get(&site)
            .into_iter()
            .flatten()
            .map(|&index| &self.edges[index])
    }

    /// Strongly connected components, callees before callers.
    pub fn sccs(&self) -> &[Vec<FunctionId>] {
        &self.sccs
    }
}

/// What the analysis knows about one code unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionInfo {
    /// Its name, if it has one.
    pub name: Option<String>,
    /// Its span.
    pub span: Span,
    /// Whether code trioxide does not see may call it (its parameters are then only
    /// `Declared`).
    pub open: bool,
}

/// The analysis side tables for a whole project (PLAN §2 step 4).
///
/// Tables are keyed by [`NodeRef`] or [`SymbolRef`], i.e. `(ModuleId, NodeId/SymbolId)`. A
/// missing entry means "nothing known" and is returned as [`Fact::unknown`], so passes never
/// have to distinguish "not analysed" from "analysed, unknown".
///
/// Passes must check trust before acting: use [`Facts::is_usable`], which honours
/// `trustTypes`.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    trust_types: bool,
    node_types: FxHashMap<NodeRef, Fact<Type>>,
    symbol_types: FxHashMap<SymbolRef, Fact<Type>>,
    effects: FxHashMap<FunctionId, (Effects, Effects)>,
    in_bounds: FxHashMap<NodeRef, Trust>,
    allocations: FxHashMap<NodeRef, Allocation>,
    class_shapes: FxHashMap<ClassId, Fact<ClassShape>>,
    functions: FxHashMap<FunctionId, FunctionInfo>,
    imports: FxHashMap<SymbolRef, SymbolRef>,
    call_graph: CallGraph,
    /// Call sites where unknown code may run as well.
    open_sites: FxHashSet<NodeRef>,
    /// Effects of evaluated expressions that have any (with their callees'): `(proven, declared)`.
    expr_effects: FxHashMap<NodeRef, (Effects, Effects)>,
    loops: FxHashMap<NodeRef, LoopFacts>,
    points_to: FxHashMap<NodeRef, Vec<NodeRef>>,
    /// Nodes no execution reaches.
    unreachable: FxHashSet<NodeRef>,
}

/// What a call site may call (see [`Facts::site_resolution`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SiteResolution {
    /// The program units it may call.
    pub callees: Vec<FunctionId>,
    /// Whether those are all it may call: no code trioxide doesn't see runs there.
    pub complete: Fact<bool>,
}

/// What a loop (its test, body and update, every iteration) does.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopFacts {
    /// Its effects, including what it calls: `(proven, declared)`.
    pub effects: (Effects, Effects),
    /// The function's own locals it assigns. Anything else it writes shows in `effects`.
    pub written: Vec<SymbolRef>,
    /// Arrays whose length the loop condition bounds and the loop may change (so `a[i]` is
    /// not in bounds through them).
    pub length_changed: Vec<SymbolRef>,
    /// Numeric locals it assigns, with their range at the loop head (loop counters among
    /// them).
    pub induction: Vec<(SymbolRef, Fact<NumberType>)>,
    /// The largest expressions inside it whose value is the same on every iteration and that
    /// can be evaluated once before it: proven pure, allocating nothing, reading none of
    /// `written`, and reading no outside state when the loop writes any. (Identifiers and
    /// literals are left out.)
    pub invariant: Vec<NodeRef>,
}

impl Facts {
    /// Empty tables. `trust_types` decides whether `Declared` facts are usable as proof.
    pub fn new(trust_types: bool) -> Self {
        Self {
            trust_types,
            ..Self::default()
        }
    }

    /// Whether `Declared` facts count as proof here.
    pub const fn trust_types(&self) -> bool {
        self.trust_types
    }

    /// Whether a fact at `trust` may justify a semantics-changing rewrite: always for
    /// `Proven`, for `Declared` only with `trustTypes`, never for `Unknown`.
    pub const fn is_usable(&self, trust: Trust) -> bool {
        match trust {
            Trust::Proven => true,
            Trust::Declared => self.trust_types,
            Trust::Unknown => false,
        }
    }

    /// The type of the value an expression or binding node evaluates to.
    pub fn ty(&self, node: NodeRef) -> Fact<&Type> {
        self.node_types
            .get(&node)
            .map_or(UNKNOWN_TYPE.as_ref(), Fact::as_ref)
    }

    /// The numeric range of an expression, if it is a number.
    pub fn range(&self, node: NodeRef) -> Fact<NumberType> {
        let fact = self.ty(node);
        match fact.value {
            Type::Number(number) => Fact {
                value: *number,
                trust: fact.trust,
            },
            _ => Fact {
                value: NumberType::ANY,
                trust: Trust::Unknown,
            },
        }
    }

    /// The type of every value a symbol (binding) can hold.
    pub fn symbol_ty(&self, symbol: SymbolRef) -> Fact<&Type> {
        self.symbol_types
            .get(&symbol)
            .map_or(UNKNOWN_TYPE.as_ref(), Fact::as_ref)
    }

    /// What calling `function` may do, including its callees. With `trustTypes` this is the
    /// summary that takes declared types as proof (`Declared` unless it agrees with the proven
    /// one); otherwise the proven summary.
    pub fn effects(&self, function: FunctionId) -> Fact<Effects> {
        match self.effects.get(&function) {
            Some(&(proven, declared)) if self.trust_types && declared != proven => {
                Fact::declared(declared)
            }
            Some(&(proven, _)) => Fact::proven(proven),
            None => Fact::unknown(),
        }
    }

    /// Both effect summaries of `function`: `(proven, declared)`.
    pub fn effect_summaries(&self, function: FunctionId) -> Option<(Effects, Effects)> {
        self.effects.get(&function).copied()
    }

    /// Whether the element access at `node` (`a[i]`) is within the array's bounds.
    pub fn in_bounds(&self, node: NodeRef) -> Fact<bool> {
        match self.in_bounds.get(&node) {
            Some(&trust) => Fact { value: true, trust },
            None => Fact {
                value: false,
                trust: Trust::Unknown,
            },
        }
    }

    /// How far objects created at `site` (an object or array literal, `new` or function)
    /// travel.
    pub fn escapes(&self, site: NodeRef) -> Fact<Escape> {
        self.allocations
            .get(&site)
            .map_or(Fact::unknown(), |allocation| allocation.escape.clone())
    }

    /// Everything known about the allocation site `site`.
    pub fn allocation(&self, site: NodeRef) -> Option<&Allocation> {
        self.allocations.get(&site)
    }

    /// Every allocation site, sorted.
    pub fn allocations(&self) -> Vec<(NodeRef, &Allocation)> {
        let mut all: Vec<_> = self.allocations.iter().map(|(k, v)| (*k, v)).collect();
        all.sort_by_key(|(site, _)| *site);
        all
    }

    /// Hidden-class facts for a class.
    pub fn class_shape(&self, class: ClassId) -> Fact<&ClassShape> {
        self.class_shapes
            .get(&class)
            .map_or(UNKNOWN_SHAPE.as_ref(), Fact::as_ref)
    }

    /// The call graph.
    pub fn call_graph(&self) -> &CallGraph {
        &self.call_graph
    }

    /// What the analysis knows about a code unit.
    pub fn function(&self, function: FunctionId) -> Option<&FunctionInfo> {
        self.functions.get(&function)
    }

    /// The exported binding an import binding refers to (the linker's answer).
    pub fn resolve_import(&self, symbol: SymbolRef) -> Option<SymbolRef> {
        self.imports.get(&symbol).copied()
    }

    /// What the call (or `new`, or implicit-call) site `site` may call. Its callees are the
    /// call graph's; it is complete, at the edges' trust, unless code trioxide doesn't see may
    /// run there too. A site never evaluated is `Unknown`.
    pub fn site_resolution(&self, site: NodeRef) -> SiteResolution {
        let edges: Vec<&CallEdge> = self.call_graph.callees(site).collect();
        let mut callees: Vec<FunctionId> = edges.iter().map(|edge| edge.callee).collect();
        callees.sort_unstable();
        callees.dedup();
        let complete = if !self.node_types.contains_key(&site) {
            Fact::unknown()
        } else if self.open_sites.contains(&site) {
            Fact::proven(false)
        } else {
            let trust = edges
                .iter()
                .map(|edge| edge.trust)
                .min()
                .unwrap_or(Trust::Proven);
            Fact { value: true, trust }
        };
        SiteResolution { callees, complete }
    }

    /// What evaluating `node` may do, including its subexpressions, the units it calls and
    /// implicit calls (getters, `valueOf`, …). With `trustTypes` the summary that takes
    /// declared types as proof (`Declared` unless it agrees with the proven one). A node never
    /// evaluated is `Unknown`.
    pub fn expr_effects(&self, node: NodeRef) -> Fact<Effects> {
        let Some(&(proven, declared)) = self.expr_effects.get(&node) else {
            return if self.node_types.contains_key(&node) {
                Fact::proven(Effects::PURE)
            } else {
                Fact::unknown()
            };
        };
        if self.trust_types && declared != proven {
            Fact::declared(declared)
        } else {
            Fact::proven(proven)
        }
    }

    /// Whether `node` can be removed when its value is unused: evaluating it writes nothing,
    /// throws nothing and runs no unknown code (it may read and allocate).
    pub fn is_pure(&self, node: NodeRef) -> Fact<bool> {
        let effects = self.expr_effects(node);
        Fact {
            value: !effects.value.writes && !effects.value.throws && !effects.value.calls_unknown,
            trust: effects.trust,
        }
    }

    /// What the loop statement `node` does, if it was evaluated.
    pub fn loop_facts(&self, node: NodeRef) -> Option<&LoopFacts> {
        self.loops.get(&node)
    }

    /// The allocation sites the value of `node` may be (`Proven`: if it is an object, it was
    /// made at one of them), or `Unknown` when it may be an object trioxide doesn't track.
    pub fn points_to(&self, node: NodeRef) -> Fact<Option<&[NodeRef]>> {
        match self.points_to.get(&node) {
            Some(sites) => Fact::proven(Some(sites.as_slice())),
            None => Fact::unknown(),
        }
    }

    /// Whether `node` (a unit, statement or expression) may be executed. `Proven` false for
    /// units nothing calls or can reach and for statements after every path left; otherwise
    /// `Unknown` true. Distinct from a `never` type, which also means "no value seen yet".
    pub fn reachable(&self, node: NodeRef) -> Fact<bool> {
        if self.unreachable.contains(&node) {
            Fact::proven(false)
        } else {
            Fact {
                value: true,
                trust: Trust::Unknown,
            }
        }
    }

    /// Drops every node-keyed fact of `module` (so they all read as `Unknown`), e.g. when its
    /// AST changed in a way the facts can't follow.
    pub fn forget_module(&mut self, module: ModuleId) {
        let other = |node: &NodeRef| node.module != module;
        self.node_types.retain(|node, _| other(node));
        self.in_bounds.retain(|node, _| other(node));
        self.allocations.retain(|node, _| other(node));
        self.expr_effects.retain(|node, _| other(node));
        self.loops.retain(|node, _| other(node));
        self.points_to.retain(|node, _| other(node));
        self.open_sites.retain(other);
        // Unknown reachability is "may run".
        self.unreachable.retain(other);
        self.effects.retain(|unit, _| other(unit));
        self.functions.retain(|unit, _| other(unit));
        self.class_shapes.retain(|class, _| other(class));
        self.symbol_types
            .retain(|symbol, _| symbol.module != module);
        self.imports
            .retain(|import, target| import.module != module && target.module != module);
        let graph = std::mem::take(&mut self.call_graph);
        let units = graph.units.into_iter().filter(other).collect();
        let edges = graph
            .edges
            .into_iter()
            .filter(|edge| other(&edge.site) && other(&edge.caller) && other(&edge.callee))
            .collect();
        let sccs = graph
            .sccs
            .into_iter()
            .map(|scc| scc.into_iter().filter(other).collect::<Vec<_>>())
            .filter(|scc| !scc.is_empty())
            .collect();
        self.call_graph = CallGraph::new(units, edges, sccs);
    }

    pub(crate) fn set_open_sites(&mut self, sites: FxHashSet<NodeRef>) {
        self.open_sites = sites;
    }

    pub(crate) fn set_expr_effects(&mut self, node: NodeRef, proven: Effects, declared: Effects) {
        self.expr_effects.insert(node, (proven, declared));
    }

    pub(crate) fn set_loop_facts(&mut self, node: NodeRef, facts: LoopFacts) {
        self.loops.insert(node, facts);
    }

    pub(crate) fn set_points_to(&mut self, node: NodeRef, sites: Vec<NodeRef>) {
        self.points_to.insert(node, sites);
    }

    pub(crate) fn set_unreachable(&mut self, nodes: FxHashSet<NodeRef>) {
        self.unreachable = nodes;
    }

    /// Every node with a type fact, sorted.
    pub fn node_types(&self) -> Vec<(NodeRef, &Fact<Type>)> {
        let mut all: Vec<_> = self.node_types.iter().map(|(k, v)| (*k, v)).collect();
        all.sort_by_key(|(node, _)| *node);
        all
    }

    /// Records the type of a node. Replaces any previous fact.
    pub fn set_ty(&mut self, node: NodeRef, fact: Fact<Type>) {
        self.node_types.insert(node, fact);
    }

    /// Records the type of a symbol. Replaces any previous fact.
    pub fn set_symbol_ty(&mut self, symbol: SymbolRef, fact: Fact<Type>) {
        self.symbol_types.insert(symbol, fact);
    }

    /// Records a function's effect summary. Replaces any previous fact.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "public API, like the other setters"
    )]
    pub fn set_effects(&mut self, function: FunctionId, fact: Fact<Effects>) {
        let declared = fact.value;
        let proven = if fact.trust == Trust::Proven {
            fact.value
        } else {
            Effects::ANY
        };
        self.effects.insert(function, (proven, declared));
    }

    pub(crate) fn set_effect_summaries(
        &mut self,
        function: FunctionId,
        proven: Effects,
        declared: Effects,
    ) {
        self.effects.insert(function, (proven, declared));
    }

    pub(crate) fn set_in_bounds(&mut self, node: NodeRef, trust: Trust) {
        self.in_bounds.insert(node, trust);
    }

    pub(crate) fn set_allocation(&mut self, site: NodeRef, allocation: Allocation) {
        self.allocations.insert(site, allocation);
    }

    pub(crate) fn set_class_shape(&mut self, class: ClassId, shape: Fact<ClassShape>) {
        self.class_shapes.insert(class, shape);
    }

    pub(crate) fn set_function(&mut self, function: FunctionId, info: FunctionInfo) {
        self.functions.insert(function, info);
    }

    pub(crate) fn set_import(&mut self, import: SymbolRef, target: SymbolRef) {
        self.imports.insert(import, target);
    }

    pub(crate) fn set_call_graph(&mut self, graph: CallGraph) {
        self.call_graph = graph;
    }
}

static UNKNOWN_SHAPE: Fact<ClassShape> = Fact {
    value: ClassShape {
        fields: Vec::new(),
        added_later: Vec::new(),
        deletes: true,
        dynamic: true,
    },
    trust: Trust::Unknown,
};

static UNKNOWN_TYPE: Fact<Type> = Fact {
    value: Type::Unknown,
    trust: Trust::Unknown,
};

#[cfg(test)]
mod tests {
    use trioxide_common::{ModuleId, NodeId};
    use trioxide_types::NumberType;

    use super::*;

    fn node(index: u32) -> NodeRef {
        NodeRef {
            module: ModuleId::new(0),
            node: NodeId::from_usize(index as usize),
        }
    }

    #[test]
    fn missing_facts_are_unknown() {
        let facts = Facts::new(false);
        assert_eq!(
            facts.ty(node(1)),
            Fact {
                value: &Type::Unknown,
                trust: Trust::Unknown
            }
        );
        assert_eq!(
            facts.effects(node(1)),
            Fact {
                value: Effects::ANY,
                trust: Trust::Unknown
            }
        );
    }

    #[test]
    fn trust_types_gates_declared_facts() {
        let mut facts = Facts::new(false);
        facts.set_ty(node(1), Fact::declared(Type::Number(NumberType::default())));
        let fact = facts.ty(node(1));
        assert!(!facts.is_usable(fact.trust));
        assert!(facts.is_usable(Trust::Proven));
        assert!(Facts::new(true).is_usable(fact.trust));
        assert!(!Facts::new(true).is_usable(Trust::Unknown));
    }
}
