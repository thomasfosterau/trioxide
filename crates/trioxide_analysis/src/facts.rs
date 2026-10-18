use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use trioxide_common::{ModuleId, NodeId, NodeRef, Span, SymbolRef};
use trioxide_types::{ClassId, FunctionId, NumberType, Type};

use crate::constants::Constant;

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
///
/// The queries:
///
/// | Query | Answers |
/// |---|---|
/// | [`ty`](Self::ty), [`range`](Self::range), [`symbol_ty`](Self::symbol_ty) | types and numeric ranges of nodes and bindings |
/// | [`in_bounds`](Self::in_bounds) | an element access is below the length (and not a hole) |
/// | [`escapes`](Self::escapes), [`allocation`](Self::allocation), [`class_shape`](Self::class_shape) | how far objects travel; object and class shapes |
/// | [`effects`](Self::effects), [`expr_effects`](Self::expr_effects), [`is_pure`](Self::is_pure) | what a unit or an expression may do |
/// | [`call_graph`](Self::call_graph) (with [`CallGraph::callers`]), [`site_resolution`](Self::site_resolution) | who calls what, and whether a site's callees are all it may run |
/// | [`points_to`](Self::points_to) | the allocation sites a value may be |
/// | [`loop_facts`](Self::loop_facts) | a loop's effects, written locals, length changes, induction ranges, invariants |
/// | [`reachable`](Self::reachable) | whether code may run at all (unlike a `never` type) |
/// | [`function`](Self::function), [`resolve_import`](Self::resolve_import) | units and linked imports |
/// | [`symbol_constant`](Self::symbol_constant), [`enum_member`](Self::enum_member) | bindings and enum members with literal values |
/// | [`export_used`](Self::export_used) | whether any module can use an export |
/// | [`builtin_intact`](Self::builtin_intact), [`may_have_holes`](Self::may_have_holes), [`instance_keys_observed`](Self::instance_keys_observed) | unpatched builtins, packed arrays, classes whose instances' keys nothing observes |
///
/// Every answer carries a [`Trust`]; escape and shape facts are always `Proven` (see
/// [`Allocation::escape`]).
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
    /// `const` bindings with literal values.
    constants: FxHashMap<SymbolRef, Constant>,
    /// Members of enums nothing can change, by enum and member name.
    enum_members: FxHashMap<(SymbolRef, String), (Constant, Trust)>,
    /// Exports no module can use.
    unused_exports: FxHashSet<SymbolRef>,
    /// Builtin namespaces and prototypes the program may patch, once the analysis ran.
    patched: Option<FxHashSet<String>>,
    /// Array allocation sites none of whose arrays ever has a hole.
    packed: FxHashSet<NodeRef>,
    /// Classes no code can observe the own keys of.
    unobserved_classes: FxHashSet<ClassId>,
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
    /// Its effects, including what it calls: `(proven, declared)`. `writes` counts every
    /// write except assignments to the function's own tracked locals (those are `written`):
    /// writes to objects, local ones included, and to locals a closure can see.
    pub effects: (Effects, Effects),
    /// The function's own locals it assigns.
    pub written: Vec<SymbolRef>,
    /// Arrays whose length the loop condition bounds and the loop may change (so `a[i]` is
    /// not in bounds through them).
    pub length_changed: Vec<SymbolRef>,
    /// Allocation sites of arrays whose length the loop, or a unit it calls, changes through
    /// a modelled operation (`push`, `pop`, `shift`, `unshift`, `length = …`, a write past
    /// the end). Only complete when `resizes_untracked` is false.
    pub resized: Vec<NodeRef>,
    /// Any array may have been resized, listed in `resized` or not: the loop runs unknown
    /// code, or resizes arrays in ways trioxide doesn't attribute to a site (`splice`, `sort`,
    /// `Array.prototype.push.call`, `defineProperty(a, "length")`, `Reflect.set`,
    /// `Object.assign`, …).
    pub resizes_untracked: bool,
    /// Numeric locals of the loop's function that it assigns and that live across its
    /// iterations (not ones declared in its body or a nested loop).
    pub induction: Vec<Induction>,
    /// The largest expressions inside it whose value is the same on every iteration and that
    /// can be evaluated once, before the first iteration's body, after the loop's first test
    /// passes. Each is in the test or the body (not a `for` loop's update, nor a `for-in`/`of`
    /// head), runs on every iteration that gets that far (no `if`, `?:`, `&&`, `switch`,
    /// `try`, nested loop or function around it, and the loop has no `break`, `continue`,
    /// `return`, `throw`, `await` or `yield`), is `Proven` free of writes, throws, unknown
    /// calls and allocation, reads no clock or random number, reads none of `written`, reads
    /// no object or shared local when the loop writes any, and is not a callee, a write
    /// target or a `delete` operand. Identifiers and literals are left out.
    pub invariant: Vec<NodeRef>,
}

/// A numeric local a loop assigns (see [`LoopFacts::induction`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Induction {
    /// The local.
    pub symbol: SymbolRef,
    /// Its range at the loop head: on entry and before every iteration's test.
    pub range: Fact<NumberType>,
    /// What it changes by per iteration, when the loop's only write to it is one `++`, `--`,
    /// `+= c`, `-= c` or `= x ± c` with a constant integer `c`, run once per iteration (a
    /// `for` loop's update, or a statement of the body when nothing `continue`s past it), and
    /// no nested function writes it. Negative when it counts down.
    ///
    /// The step is nominal: the update is exact only while the value stays an integer within
    /// ±2^53 (from 2^53 − 2, `i++` stalls). Trip-count arithmetic must check `range` first.
    pub step: Option<f64>,
    /// It is the loop's counter: it has a `step` and the loop's test reads it.
    pub counter: bool,
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

    /// Whether [`CallGraph::callers`] lists every site that may call `function`: `Proven`
    /// true when no code trioxide doesn't see can reach it (it is not
    /// [`open`](FunctionInfo::open)), so every call is one trioxide followed, accessor uses
    /// and modelled builtins' callbacks (`arr.map(f)`) included; `Proven` false when it is
    /// open (passed to code trioxide doesn't model, such as `f.call(…)`, or exported in an
    /// open world); `Unknown` for a unit the analysis has nothing on. Check this before
    /// relying on a function having a single caller.
    pub fn callers_complete(&self, function: FunctionId) -> Fact<bool> {
        match self.functions.get(&function) {
            None => Fact::unknown(),
            Some(info) => Fact::proven(!info.open),
        }
    }

    /// What the analysis knows about a code unit.
    pub fn function(&self, function: FunctionId) -> Option<&FunctionInfo> {
        self.functions.get(&function)
    }

    /// The exported binding an import binding refers to (the linker's answer).
    pub fn resolve_import(&self, symbol: SymbolRef) -> Option<SymbolRef> {
        self.imports.get(&symbol).copied()
    }

    /// What the call (or `new`, or implicit-call) site `site` may call: the call graph's
    /// callees there, including the callbacks a modelled builtin calls at the site
    /// (`arr.map(f)` lists `f`) and the constructor and field initialisers of a `new`.
    ///
    /// `complete` says whether those are all the program code that may run at the site:
    /// - `true` at `Proven`: nothing else runs there (a site in code that never runs is
    ///   vacuously complete; [`reachable`](Self::reachable) tells those apart);
    /// - `true` at `Declared`: so, if TypeScript's types hold (some callee was resolved
    ///   through a declared class);
    /// - `false` at `Proven`: code trioxide doesn't see may run there (an unknown callee, an
    ///   unmodelled builtin or constructor such as `new Promise(executor)`, or a conversion
    ///   that may call `valueOf`/`toString`);
    /// - `Unknown`: the site was never evaluated.
    ///
    /// A generator's (or async generator's) body doesn't run at the call: the call creates
    /// the generator, and the body runs at its `next()` calls. Its unit is listed as the
    /// callee all the same; an inliner must check the callee's kind.
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
    /// declared types as proof (`Declared` unless it agrees with the proven one). Unlike a
    /// unit's [`Facts::effects`], `writes` and `reads` count the function's own locals and
    /// objects too (`i++` writes), and `allocates` every object it makes (`[]` allocates). A node with no recorded evaluation (never evaluated, or a
    /// callee or optional-chain part evaluated as part of its call) is `Unknown`.
    pub fn expr_effects(&self, node: NodeRef) -> Fact<Effects> {
        let Some(&(proven, declared)) = self.expr_effects.get(&node) else {
            return Fact::unknown();
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
    /// `Unknown` true, which passes must never treat as dead. Distinct from a `never` type,
    /// which also means "no value seen yet".
    ///
    /// Facts about code that never runs are vacuous (inside an unreachable function, `a.pop()`
    /// may read as invariant and pure): passes must check `reachable` before acting on them.
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

    /// The literal value of a binding (or of the binding an import resolves to): `Proven` for a
    /// `const` declared with a primitive literal (through parentheses and TypeScript casts),
    /// otherwise `Unknown` with no value. A read in the binding's temporal dead zone throws
    /// instead, so check the read's [`is_pure`](Self::is_pure) too.
    pub fn symbol_constant(&self, symbol: SymbolRef) -> Fact<Option<&Constant>> {
        let symbol = self.resolve_import(symbol).unwrap_or(symbol);
        match self.constants.get(&symbol) {
            Some(constant) => Fact::proven(Some(constant)),
            None => Fact {
                value: None,
                trust: Trust::Unknown,
            },
        }
    }

    /// The value every evaluation of the member read `E.name` (or `E["name"]`) yields, where
    /// `enum_symbol` is the enum `E` or an import of it. Only for an enum declared once, at the
    /// top of a module in no import cycle, after code that can't call anything (so the object
    /// exists before any read runs, and the read doesn't throw). `Proven` when every use of it,
    /// through imports too, is a member read (so nothing can change the object) and it isn't
    /// visible outside the program; `Declared` when only TypeScript says the members are
    /// read-only (the enum object is used as a value, or exported to unseen code). Otherwise
    /// `Unknown` with no value.
    pub fn enum_member(&self, enum_symbol: SymbolRef, name: &str) -> Fact<Option<&Constant>> {
        let symbol = self.resolve_import(enum_symbol).unwrap_or(enum_symbol);
        match self.enum_members.get(&(symbol, name.to_string())) {
            Some((constant, trust)) => Fact {
                value: Some(constant),
                trust: *trust,
            },
            None => Fact {
                value: None,
                trust: Trust::Unknown,
            },
        }
    }

    /// Whether the export of the module-level binding `symbol` may be used by another module
    /// or the host: `Proven` false in a closed world when its module is not an entry, not
    /// re-exported by one, its namespace object is never taken (`import * as`, `export * as`,
    /// `import()`), and no import resolves to it; otherwise `Unknown` true.
    pub fn export_used(&self, symbol: SymbolRef) -> Fact<bool> {
        if self.unused_exports.contains(&symbol) {
            Fact::proven(false)
        } else {
            Fact {
                value: true,
                trust: Trust::Unknown,
            }
        }
    }

    /// Whether the program leaves the builtin `name` alone (a namespace such as `"Array"` or
    /// `"Object"`, whose prototype goes with it, or `"TypedArray"`): `Proven` true when no
    /// code shadows, reassigns or hands out the namespace or reaches a builtin prototype
    /// without naming it, so the builtin models (methods, iteration, holes reading through
    /// the prototype) describe what runs (PLAN §3, "Builtin integrity"); `Proven` false when
    /// it may be patched; `Unknown` before the analysis ran.
    pub fn builtin_intact(&self, name: &str) -> Fact<bool> {
        match &self.patched {
            Some(patched) => Fact::proven(!patched.contains(name)),
            None => Fact::unknown(),
        }
    }

    /// Whether an array made at `site` may ever have a hole (a missing index below its
    /// length): `Proven` false when no array from the site ever has one (no `delete`, sparse
    /// write, `length` growth or holey literal), otherwise `Unknown` true.
    pub fn may_have_holes(&self, site: NodeRef) -> Fact<bool> {
        if self.packed.contains(&site) {
            Fact::proven(false)
        } else {
            Fact {
                value: true,
                trust: Trust::Unknown,
            }
        }
    }

    /// Whether code may observe which own properties instances of `class` have, or in which
    /// order (`Object.keys`, `for…in`, `JSON.stringify`, spread, `in`, `hasOwnProperty`, …).
    /// `Proven` false when the class is not open, no program class extends it, its
    /// prototype gets no properties outside its body, and no such observer may see any of its
    /// instances; otherwise `Unknown` true.
    pub fn instance_keys_observed(&self, class: ClassId) -> Fact<bool> {
        if self.unobserved_classes.contains(&class) {
            Fact::proven(false)
        } else {
            Fact {
                value: true,
                trust: Trust::Unknown,
            }
        }
    }

    /// Drops every node-keyed fact of `module` (so they all read as `Unknown`), e.g. when a
    /// pass rewrote it in a way its node ids can't follow. The rewrite must keep its meaning:
    /// facts elsewhere about values and effects stay. Facts elsewhere that name its nodes go:
    /// call sites that called into it become open (their callees are gone) and points-to sets
    /// that include its allocation sites read as `Unknown`.
    pub fn forget_module(&mut self, module: ModuleId) {
        let other = |node: &NodeRef| node.module != module;
        for edge in &self.call_graph.edges {
            if other(&edge.site) && !other(&edge.callee) {
                self.open_sites.insert(edge.site);
            }
        }
        self.points_to
            .retain(|node, sites| other(node) && sites.iter().all(other));
        self.node_types.retain(|node, _| other(node));
        self.in_bounds.retain(|node, _| other(node));
        self.allocations.retain(|node, _| other(node));
        self.expr_effects.retain(|node, _| other(node));
        self.loops.retain(|node, _| other(node));
        for facts in self.loops.values_mut() {
            let count = facts.resized.len();
            facts.resized.retain(other);
            facts.resizes_untracked |= facts.resized.len() != count;
        }
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
        self.constants.retain(|symbol, _| symbol.module != module);
        self.enum_members
            .retain(|(symbol, _), _| symbol.module != module);
        self.unused_exports.retain(|symbol| symbol.module != module);
        self.packed.retain(other);
        self.unobserved_classes.retain(other);
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

    /// Renumbers `module`'s nodes, e.g. after a pass rewrote it and the facts were computed
    /// from its printed text: `map` takes each node id the facts use to the one it now has.
    /// Facts about nodes `map` doesn't cover are dropped, and so are facts elsewhere whose
    /// values name them (a type naming a dropped function, a points-to set with a dropped
    /// site); call sites in other modules that called a dropped unit become open.
    pub fn remap_module(&mut self, module: ModuleId, map: &FxHashMap<NodeId, NodeId>) {
        fn table<V>(
            table: &mut FxHashMap<NodeRef, V>,
            node: &impl Fn(NodeRef) -> Option<NodeRef>,
            value: impl Fn(V) -> Option<V>,
        ) {
            *table = std::mem::take(table)
                .into_iter()
                .filter_map(|(key, entry)| Some((node(key)?, value(entry)?)))
                .collect();
        }
        let node = |node: NodeRef| -> Option<NodeRef> {
            if node.module == module {
                map.get(&node.node).map(|&id| NodeRef { module, node: id })
            } else {
                Some(node)
            }
        };
        let ty = |fact: Fact<Type>| -> Option<Fact<Type>> {
            Some(Fact {
                value: fact.value.try_map_ids(&node)?,
                trust: fact.trust,
            })
        };
        table(&mut self.node_types, &node, ty);
        self.symbol_types = std::mem::take(&mut self.symbol_types)
            .into_iter()
            .filter_map(|(symbol, fact)| Some((symbol, ty(fact)?)))
            .collect();
        table(&mut self.effects, &node, Some);
        table(&mut self.in_bounds, &node, Some);
        table(&mut self.allocations, &node, |allocation| {
            Some(Allocation {
                kind: match allocation.kind {
                    AllocKind::Instance(class) => AllocKind::Instance(node(class)?),
                    kind => kind,
                },
                ..allocation
            })
        });
        table(&mut self.class_shapes, &node, Some);
        table(&mut self.functions, &node, Some);
        table(&mut self.expr_effects, &node, Some);
        table(&mut self.loops, &node, |mut facts| {
            facts.invariant = facts.invariant.into_iter().filter_map(node).collect();
            // A resized array whose site is gone is one trioxide no longer tracks.
            let resized = std::mem::take(&mut facts.resized);
            let count = resized.len();
            facts.resized = resized.into_iter().filter_map(node).collect();
            facts.resizes_untracked |= facts.resized.len() != count;
            Some(facts)
        });
        table(&mut self.points_to, &node, |sites| {
            sites.into_iter().map(node).collect()
        });
        let nodes = |set: FxHashSet<NodeRef>| set.into_iter().filter_map(node).collect();
        self.unreachable = nodes(std::mem::take(&mut self.unreachable));
        self.packed = nodes(std::mem::take(&mut self.packed));
        self.unobserved_classes = nodes(std::mem::take(&mut self.unobserved_classes));
        let mut open_sites: FxHashSet<NodeRef> = nodes(std::mem::take(&mut self.open_sites));
        let graph = std::mem::take(&mut self.call_graph);
        let units = graph.units.into_iter().filter_map(node).collect();
        let mut edges = Vec::new();
        for edge in graph.edges {
            match (node(edge.site), node(edge.caller), node(edge.callee)) {
                (Some(site), Some(caller), Some(callee)) => edges.push(CallEdge {
                    site,
                    caller,
                    callee,
                    ..edge
                }),
                (Some(site), Some(_), None) => {
                    open_sites.insert(site);
                }
                _ => {}
            }
        }
        let sccs = graph
            .sccs
            .into_iter()
            .map(|scc| scc.into_iter().filter_map(node).collect::<Vec<_>>())
            .filter(|scc| !scc.is_empty())
            .collect();
        self.call_graph = CallGraph::new(units, edges, sccs);
        self.open_sites = open_sites;
    }

    /// Gives `alias` the facts about the value of `node` (a parenthesized expression and its
    /// contents, for example).
    pub fn alias(&mut self, alias: NodeRef, node: NodeRef) {
        if let Some(fact) = self.node_types.get(&node).cloned() {
            self.node_types.insert(alias, fact);
        }
        if let Some(&effects) = self.expr_effects.get(&node) {
            self.expr_effects.insert(alias, effects);
        }
        if let Some(sites) = self.points_to.get(&node).cloned() {
            self.points_to.insert(alias, sites);
        }
        if self.unreachable.contains(&node) {
            self.unreachable.insert(alias);
        }
    }

    /// Sets the span recorded for `function` (after [`remap_module`](Self::remap_module), its
    /// span in the source it now has).
    pub fn set_function_span(&mut self, function: FunctionId, span: Span) {
        if let Some(info) = self.functions.get_mut(&function) {
            info.span = span;
        }
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

    pub(crate) fn set_symbol_constant(&mut self, symbol: SymbolRef, constant: Constant) {
        self.constants.insert(symbol, constant);
    }

    pub(crate) fn set_enum_member(
        &mut self,
        symbol: SymbolRef,
        name: String,
        constant: Constant,
        trust: Trust,
    ) {
        self.enum_members.insert((symbol, name), (constant, trust));
    }

    pub(crate) fn set_export_unused(&mut self, symbol: SymbolRef) {
        self.unused_exports.insert(symbol);
    }

    pub(crate) fn set_patched(&mut self, patched: FxHashSet<String>) {
        self.patched = Some(patched);
    }

    pub(crate) fn set_packed(&mut self, site: NodeRef) {
        self.packed.insert(site);
    }

    pub(crate) fn set_unobserved_classes(&mut self, classes: FxHashSet<ClassId>) {
        self.unobserved_classes = classes;
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
