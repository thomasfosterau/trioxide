//! The whole-program abstract interpreter behind the value facts (types, trust, ranges,
//! points-to and escape), PLAN §2 step 4 and §3.
//!
//! Every code unit (module top level, function, arrow, static block, class field
//! initialisers) is interpreted over abstract values. Inside a unit, *tracked* locals (every
//! write is in the unit itself) are followed flow-sensitively: assignments update an
//! environment, branches narrow and join it, loops iterate to a fixed point. Everything else
//! (module variables read from functions, captured variables, object properties, parameters,
//! return values, `this`) lives in whole-program tables that only ever grow by joins. The
//! analysis repeats every unit until no table changes; numbers are widened after a few rounds
//! so it terminates.
//!
//! Objects are abstracted by allocation site. A value's points-to set says which sites it may
//! be; property reads and writes go through the sites' heap entries. An object that reaches
//! code trioxide can't see (an unknown callee, a library export, a parameter of a function
//! unknown code may call) *escapes*: its contents and the parameters of the functions it holds
//! become unknown. A function is only called from known sites while its function object has
//! not escaped, which is how parameter facts can be `Proven` (PLAN §3: "function parameters
//! are `Proven` only when every call site is known").

mod call;
mod expr;
mod narrow;
mod stmt;
mod sweep;
mod tables;

pub use sweep::run;

use oxc_span::{GetSpan, Span};
use oxc_syntax::symbol::SymbolId;
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::{ModuleId, NodeId, NodeRef, SymbolRef};
use trioxide_types::{CheckerQueries, SharedTypeQueries, Type, TypeOracle, TypedArrayKind};

use crate::{
    AnalysisOptions,
    facts::{Effects, Trust},
    link::Links,
    numeric,
    value::{AllocId, Pts, Val},
    world::{Body, Module, Unit, World},
};

/// Global objects the lib declares with interface types (which the oracle answers `Unknown`
/// for), when not shadowed.
const LIB_OBJECTS: [&str; 16] = [
    "console",
    "Math",
    "JSON",
    "Object",
    "Number",
    "String",
    "Array",
    "Date",
    "Promise",
    "Reflect",
    "Symbol",
    "Map",
    "Set",
    "globalThis",
    "Intl",
    "Error",
];

/// Merges an entry of the whole-program tables may change in before its numbers are widened.
const WIDEN_AFTER: u32 = 3;
/// Rounds after which the analysis gives up and reports nothing.
const MAX_ROUNDS: usize = 40;

/// A whole-program entry that changes in more merges than this is given up: set to "any
/// value" (and its objects to "anything may happen to them"), which nothing can change
/// further, so the run reaches a (less precise) fixed point. Counted per entry, like
/// [`WIDEN_AFTER`], so neither depends on how many sweeps the chunking takes.
const SATURATE_AFTER: u32 = 20;
/// [`SATURATE_AFTER`], or what the options say instead.
fn saturate_after(options: &AnalysisOptions) -> u32 {
    match options.saturate_after {
        0 => SATURATE_AFTER,
        merges => u32::try_from(merges).unwrap_or(u32::MAX),
    }
}

/// Loop iterations after which loop-carried numbers are widened.
const LOOP_WIDEN_AFTER: usize = 2;
/// Loop iterations after which the loop's environment is abandoned to `Unknown`.
const MAX_LOOP_ITERATIONS: usize = 12;

/// A whole-program table entry a unit's interpretation read. When one changes, the units
/// that read it are interpreted again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dep {
    Symbol(SymbolRef),
    Params(NodeRef),
    This(NodeRef),
    Return(NodeRef),
    Heap(AllocId),
    Super(NodeRef),
    Default(NodeRef),
}

/// Who read what in a chunk, and which of its units must run (again).
struct ChunkDeps<'w> {
    current: Option<NodeRef>,
    /// What the current unit has read in this run.
    current_reads: FxHashSet<Dep>,
    /// Readers recorded in earlier sweeps.
    readers: &'w FxHashMap<Dep, Vec<NodeRef>>,
    /// Readers in this sweep.
    reads: FxHashMap<Dep, Vec<NodeRef>>,
    seen: FxHashSet<(Dep, NodeRef)>,
    /// The chunk's units.
    members: &'w FxHashSet<NodeRef>,
    dirty: FxHashSet<NodeRef>,
}

/// The type oracle as a chunk reaches it.
pub(crate) enum Types<'w> {
    /// The only user of the oracle (chunks run one after another).
    Exclusive(&'w mut dyn TypeOracle),
    /// A view several chunks query at once.
    Shared(&'w dyn SharedTypeQueries),
    /// A worker's own queries (a checker it holds).
    Worker(&'w mut dyn CheckerQueries),
}

impl Types<'_> {
    /// The same oracle, borrowed for less long.
    fn reborrow(&mut self) -> Types<'_> {
        match self {
            Types::Exclusive(oracle) => Types::Exclusive(&mut **oracle),
            Types::Shared(oracle) => Types::Shared(*oracle),
            Types::Worker(oracle) => Types::Worker(&mut **oracle),
        }
    }
}

/// How far an object may be seen by code other than the unit that created it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Escape {
    /// Only the creating unit uses it.
    #[default]
    None,
    /// Other analysed code may hold it (it was returned, stored, passed or captured).
    Known,
    /// Code trioxide does not see may hold it: its contents are unknown.
    Unknown,
}

/// What kind of object an allocation site makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjKind {
    /// An object literal.
    Object,
    /// An array literal or `new Array`.
    Array,
    /// `new Float64Array(…)` and friends.
    TypedArray(TypedArrayKind),
    /// `new C(…)` of a class in the program.
    Instance(NodeRef),
    /// A function or arrow (the unit).
    Function(NodeRef),
    /// A class (its constructor function).
    Class(NodeRef),
    /// A class's prototype object (keyed by the class body).
    Prototype(NodeRef),
    /// A module namespace object (keyed by the module's program node).
    Namespace(ModuleId),
    /// Some other object (`new Map`, a regexp, …).
    Opaque,
}

/// The heap entry of an allocation site: everything ever stored in objects made there.
#[derive(Debug, Clone, PartialEq)]
pub struct Obj {
    /// What it is.
    pub kind: ObjKind,
    /// The unit whose code creates it.
    pub unit: NodeRef,
    /// Every value ever written to each property (static keys only).
    pub props: FxHashMap<String, Val>,
    /// Property names in the order they were first written.
    pub order: Vec<String>,
    /// Properties every object from this site has as soon as anyone else can see it.
    pub present: FxHashSet<String>,
    /// Array and typed array elements.
    pub elem: Val,
    /// The array's length, while it is fixed and known.
    pub length: Option<f64>,
    /// Whether the creating code has set `length` yet: from then on it only goes from known
    /// to unknown, never back.
    pub sized: bool,
    /// Whether an array may have holes (reads of them give `undefined`).
    pub holey: bool,
    /// Whether properties are written with computed keys, deleted, or redefined, so `props`
    /// is not the whole story.
    pub dynamic: bool,
    /// Whether a property was deleted.
    pub deleted: bool,
    /// Whether the object reached a proxy, `Object.defineProperty` or the like: its
    /// properties may be anything, whatever TypeScript says.
    pub poisoned: bool,
    /// Properties added after creation, outside the creating code.
    pub added: FxHashSet<String>,
    /// Escape state.
    pub escape: Escape,
    /// Typed arrays sharing this one's buffer (`subarray` views and what they view): whoever
    /// can detach one detaches them all.
    pub shares_buffer: Vec<AllocId>,
    /// An object literal with a getter or setter, or a `__proto__` (whose prototype may have
    /// them): reading, writing or spreading its properties may run user code.
    pub accessors: bool,
    /// Given up on (see [`SATURATE_AFTER`]): its flags stay at the top whatever runs later.
    pub given_up: bool,
    /// Passed to `Object.freeze`: in strict code, writing or deleting its properties throws.
    pub frozen: bool,
}

impl Obj {
    /// The values its properties and elements hold that are objects.
    pub fn held(&self) -> Vec<Val> {
        self.props
            .values()
            .chain([&self.elem])
            .filter(|value| value.pts.sites().is_some_and(|sites| !sites.is_empty()))
            .cloned()
            .collect()
    }

    /// Sets the flags to "anything may happen" for good; whether that changed them.
    fn give_up(&mut self) -> bool {
        let changed = !self.given_up
            || !self.dynamic
            || !self.deleted
            || self.length.is_some()
            || !self.sized
            || !self.holey
            || !self.present.is_empty()
            || !self.accessors
            || !self.frozen;
        self.given_up = true;
        self.dynamic = true;
        self.deleted = true;
        self.length = None;
        self.sized = true;
        self.holey = true;
        self.present.clear();
        self.accessors = true;
        self.frozen = true;
        changed
    }

    fn new(kind: ObjKind, unit: NodeRef) -> Self {
        Self {
            kind,
            unit,
            props: FxHashMap::default(),
            order: Vec::new(),
            present: FxHashSet::default(),
            elem: Val::never(),
            length: None,
            sized: false,
            holey: false,
            dynamic: false,
            deleted: false,
            poisoned: false,
            added: FxHashSet::default(),
            escape: Escape::None,
            shares_buffer: Vec::new(),
            accessors: false,
            given_up: false,
            frozen: false,
        }
    }
}

/// The whole-program tables, which only grow.
#[derive(Default, Clone)]
pub struct State {
    /// Every value each symbol ever holds.
    pub symbols: FxHashMap<SymbolRef, Val>,
    /// Join of the arguments each callable unit receives from known call sites, by position.
    pub params: FxHashMap<NodeRef, Vec<Val>>,
    /// Units called with a spread or more arguments than recorded positions understand.
    pub spread_called: FxHashSet<NodeRef>,
    /// `this` values passed to each unit.
    pub this: FxHashMap<NodeRef, Val>,
    /// Return values of each unit.
    pub returns: FxHashMap<NodeRef, Val>,
    /// Allocation sites, boxed so the merges move pointers rather than whole entries.
    pub heap: FxHashMap<AllocId, Box<Obj>>,
    /// The value each class extends.
    pub supers: FxHashMap<NodeRef, Val>,
    /// The value of each `export default <expression>`, by expression node.
    pub defaults: FxHashMap<NodeRef, Val>,
    /// Classes with an instance site that escapes to unknown code.
    pub open_instances: FxHashSet<NodeRef>,
    /// `let`, `const` and `class` bindings that may be read before their declaration runs:
    /// their unit made a call (which may read them) before reaching it.
    pub may_be_uninit: FxHashSet<SymbolRef>,
    /// Whether the program can replace the prototype of an object the analysis doesn't
    /// follow ([`may_reprototype`]): instances unknown code holds may then be of no class.
    pub reprototypes: bool,
}

/// A call edge recorded while interpreting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    /// The call (or `new`, getter access, implicit callback) site.
    pub site: NodeRef,
    /// The unit called.
    pub callee: NodeRef,
    /// `Proven` when the callee is known from points-to; `Declared` when only the receiver's
    /// TypeScript type says which method it is.
    pub trust: Trust,
}

/// What one unit does itself, before adding its callees.
#[derive(Debug, Clone, Default)]
pub struct UnitRecord {
    /// Effects counting only proven facts.
    pub proven: Effects,
    /// Effects counting declared facts as proof.
    pub declared: Effects,
    /// Calls to analysed units.
    pub edges: Vec<Edge>,
    /// Allocation sites the unit evaluated.
    pub allocations: Vec<AllocId>,
    /// Call sites (and implicit-call sites) where code trioxide doesn't see may run.
    pub open_sites: FxHashSet<NodeRef>,
    /// Each visit of an expression with effects or calls: its node, its own effects and the
    /// range of `edges` it made (merged per node by the summary).
    pub expr_effects: Vec<ExprVisit>,
    /// What each loop does.
    pub loops: FxHashMap<NodeRef, LoopRecord>,
    /// Statements reached with a reachable environment.
    pub reached: FxHashSet<NodeRef>,
    /// Statements interpreted (or skipped) with an unreachable one.
    pub skipped: FxHashSet<NodeRef>,
    /// Arrays whose length the unit may change.
    pub resized: FxHashSet<AllocId>,
    /// It may change the length of an array it doesn't track.
    pub resizes_untracked: bool,
}

/// One evaluation of an expression: its own effects (without its callees') and the edges
/// (calls, implicit ones included) it made.
#[derive(Debug, Clone, Copy)]
pub struct ExprVisit {
    /// The expression.
    pub node: NodeId,
    /// Counting only proven facts.
    pub proven: Effects,
    /// Counting declared facts as proof.
    pub declared: Effects,
    /// It writes something, the unit's own locals and objects included.
    pub local_writes: bool,
    /// It reads a clock or a random number: its value can differ between evaluations.
    pub volatile: bool,
    /// It reads an object, or a local other code can write.
    pub shared_reads: bool,
    /// Its edges: `edges[start..end]` of the unit's record.
    pub edges: (u32, u32),
}

/// What a loop (its test, body and update) does.
#[derive(Debug, Clone, Default)]
pub struct LoopRecord {
    /// Its own effects, counting only proven facts.
    pub proven: Effects,
    /// Counting declared facts as proof.
    pub declared: Effects,
    /// Units it calls.
    pub callees: Vec<NodeRef>,
    /// Tracked locals it assigns.
    pub written: Vec<SymbolId>,
    /// Arrays whose length relations (`i < a.length`) it broke.
    pub length_changed: Vec<SymbolId>,
    /// Numeric locals it assigns, with their values at the loop head.
    pub induction: Vec<(SymbolId, Val)>,
    /// It writes an object or a local a closure can see (writes to tracked locals are in
    /// `written`).
    pub shared_writes: bool,
    /// Arrays whose length it may change (not counting callees).
    pub resized: Vec<AllocId>,
    /// It may change the length of an array it doesn't track.
    pub resizes_untracked: bool,
}

impl LoopRecord {
    fn join(&mut self, other: &Self) {
        self.proven = union(self.proven, other.proven);
        self.declared = union(self.declared, other.declared);
        self.shared_writes |= other.shared_writes;
        self.resizes_untracked |= other.resizes_untracked;
        for site in &other.resized {
            if !self.resized.contains(site) {
                self.resized.push(*site);
            }
        }
        for (into, from) in [(&mut self.callees, &other.callees)] {
            for item in from {
                if !into.contains(item) {
                    into.push(*item);
                }
            }
        }
        for (into, from) in [
            (&mut self.written, &other.written),
            (&mut self.length_changed, &other.length_changed),
        ] {
            for item in from {
                if !into.contains(item) {
                    into.push(*item);
                }
            }
        }
        for (symbol, value) in &other.induction {
            match self.induction.iter_mut().find(|(own, _)| own == symbol) {
                Some((_, own)) => *own = own.join(value),
                None => self.induction.push((*symbol, value.clone())),
            }
        }
    }
}

/// Either effect set.
fn union(a: Effects, b: Effects) -> Effects {
    Effects {
        reads: a.reads || b.reads,
        writes: a.writes || b.writes,
        allocates: a.allocates || b.allocates,
        throws: a.throws || b.throws,
        calls_unknown: a.calls_unknown || b.calls_unknown,
    }
}

/// Effects being collected for an expression or loop under interpretation.
pub struct Frame {
    /// Counting only proven facts.
    pub proven: Effects,
    /// Counting declared facts as proof.
    pub declared: Effects,
    /// Where its edges start in the unit's record.
    pub edges: usize,
    /// Something is written, the unit's own locals and objects included.
    pub local_writes: bool,
    /// An object or a local other code can see is written.
    pub shared_writes: bool,
    /// A clock or a random number is read.
    pub volatile: bool,
    /// An object, or a local other code can write, is read.
    pub shared_reads: bool,
}

impl Frame {
    /// A frame whose edges start at `edges`.
    pub fn new(edges: usize) -> Self {
        Self {
            proven: Effects::PURE,
            declared: Effects::PURE,
            edges,
            local_writes: false,
            shared_writes: false,
            volatile: false,
            shared_reads: false,
        }
    }
}

/// What interpretation recorded (for one unit, or merged for all).
#[derive(Default)]
pub struct Output {
    /// The value of each expression and binding node.
    pub nodes: FxHashMap<NodeRef, Val>,
    /// Element accesses proven (or declared) in bounds.
    pub in_bounds: FxHashMap<NodeRef, Trust>,
    /// Per-unit effects and edges.
    pub units: FxHashMap<NodeRef, UnitRecord>,
    /// Every value the unit's private locals held in its last run (they never reach the
    /// shared tables). That run saw the final tables, so it is all they can hold; earlier runs
    /// on less complete tables don't count.
    pub symbols: FxHashMap<SymbolRef, Val>,
    /// The last run read a declared instance field before anything wrote it, as `never`, or
    /// skipped a builtin call with a `never` argument: another chunk may write the field, or
    /// reach the argument, in the same sweep. If nothing does, the unit runs again once the
    /// sweeps settle, reading the field as missing and calling the builtin.
    pub optimistic: bool,
    /// Where each of the unit's loops settled in its last run, by loop node: the next run
    /// starts them there.
    loop_heads: FxHashMap<NodeId, Settled>,
}

impl Output {
    fn clear(&mut self) {
        self.nodes.clear();
        self.in_bounds.clear();
        self.units.clear();
        self.symbols.clear();
        self.optimistic = false;
    }
}

impl Results {
    /// What `unit` does itself, from its last run.
    pub fn record(&self, unit: NodeRef) -> Option<&UnitRecord> {
        self.outputs.get(&unit)?.units.get(&unit)
    }
}

/// The analysis's results once the tables are stable.
pub struct Results {
    /// The stable tables.
    pub state: State,
    /// Every unit's last recordings, by unit.
    pub outputs: FxHashMap<NodeRef, Output>,
    /// Whether a fixed point was reached (if not, nothing may be trusted).
    pub converged: bool,
    /// How the run went.
    pub stats: Stats,
}

/// Counters for one analysis run.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    /// Sweeps over the units with changed inputs until there were none.
    pub rounds: usize,
    /// Units (module bodies, functions, class initialisers, static blocks).
    pub units: usize,
    /// Unit interpretations, over all sweeps.
    pub interpretations: usize,
    /// Chunks the units were split into (each sweep runs those with work in parallel).
    pub chunks: usize,
    /// Type oracle queries (cache misses).
    pub oracle_queries: usize,
    /// Time spent in the oracle.
    pub oracle_time: std::time::Duration,
    /// Time spent merging sweeps (single-threaded).
    pub merge_time: std::time::Duration,
    /// Total interpretation time.
    pub total_time: std::time::Duration,
    /// Parsing and indexing the modules, registering declarations and linking.
    pub build_time: std::time::Duration,
    /// Summarising the tables as facts.
    pub summary_time: std::time::Duration,
    /// Whole-program entries given up (set to "any value") because they kept changing.
    pub saturated: usize,
}

/// Builtin namespaces (`Math`, …) whose models can't be trusted: the program shadows them,
/// writes to them, or lets the namespace object (or `NS.prototype`, or the global object)
/// flow anywhere other than a plain `NS.name` read, call or construction, where it could be
/// patched. `"TypedArray"` stands for the typed array constructors and their shared
/// prototype. Any way of reaching a builtin prototype without naming it (`__proto__`,
/// `Object.getPrototypeOf`, `.constructor`, `X.prototype` of anything but a program class,
/// `Reflect`, `Function`, `eval`) turns every model off.
pub(crate) fn patched_builtins(world: &World<'_>) -> FxHashSet<String> {
    use oxc_ast::AstKind;
    const GLOBAL_OBJECTS: [&str; 4] = ["globalThis", "window", "self", "global"];
    /// Globals that reach any object, the global object or a builtin prototype.
    const HATCH_GLOBALS: [&str; 3] = ["Reflect", "Function", "eval"];
    /// Properties that reach a prototype (or define accessors on one).
    const HATCH_PROPERTIES: [&str; 10] = [
        "__proto__",
        "constructor",
        "getPrototypeOf",
        "setPrototypeOf",
        "__defineGetter__",
        "__defineSetter__",
        "__lookupGetter__",
        "__lookupSetter__",
        "getOwnPropertyDescriptor",
        "getOwnPropertyDescriptors",
    ];
    let mut patched = FxHashSet::default();
    let all = || {
        crate::builtins::NAMESPACES
            .iter()
            .map(|name| (*name).to_string())
            .chain(std::iter::once(TYPED_ARRAY.to_string()))
    };
    for module in &world.modules {
        let scoping = module.semantic.scoping();
        let nodes = module.semantic.nodes();
        for name in crate::builtins::NAMESPACES {
            // A binding of the same name anywhere shadows it somewhere.
            if scoping
                .symbol_ids()
                .any(|symbol| scoping.symbol_name(symbol) == name)
            {
                patched.insert(name.to_string());
            }
        }
        if scoping.symbol_ids().any(|symbol| {
            TypedArrayKind::from_constructor_name(scoping.symbol_name(symbol)).is_some()
        }) {
            patched.insert(TYPED_ARRAY.to_string());
        }
        for reference in scoping.root_unresolved_references_ids().flatten() {
            let reference = scoping.get_reference(reference);
            if !reference.is_value() {
                // A type annotation (`a: Uint8Array`) hands nothing out.
                continue;
            }
            let node = reference.node_id();
            let AstKind::IdentifierReference(ident) = nodes.kind(node) else {
                continue;
            };
            let name = ident.name.as_str();
            if HATCH_GLOBALS.contains(&name) {
                patched.extend(all());
                continue;
            }
            let typed_array = TypedArrayKind::from_constructor_name(name).is_some();
            let namespace = crate::builtins::NAMESPACES.contains(&name);
            let global_object = GLOBAL_OBJECTS.contains(&name);
            if !namespace && !global_object && !typed_array {
                continue;
            }
            let safe = safe_namespace_use(nodes, node, name, global_object);
            if !safe {
                if global_object {
                    patched.extend(all());
                } else if typed_array {
                    patched.insert(TYPED_ARRAY.to_string());
                } else {
                    patched.insert(name.to_string());
                }
            }
        }
        let hatch = nodes.iter().any(|node| match node.kind() {
            AstKind::StaticMemberExpression(member) => {
                let property = member.property.name.as_str();
                HATCH_PROPERTIES.contains(&property)
                    || (property == "prototype" && !names_program_symbol(module, &member.object))
            }
            AstKind::ComputedMemberExpression(member) => match &member.expression {
                oxc_ast::ast::Expression::StringLiteral(key) => {
                    let key = key.value.as_str();
                    HATCH_PROPERTIES.contains(&key) || key == "prototype"
                }
                _ => false,
            },
            AstKind::ObjectProperty(property) => {
                !property.computed
                    && property
                        .key
                        .static_name()
                        .is_some_and(|key| key == "__proto__")
            }
            AstKind::BindingProperty(property) => property
                .key
                .static_name()
                .is_some_and(|key| HATCH_PROPERTIES.contains(&key.as_ref())),
            AstKind::AssignmentTargetPropertyProperty(property) => property
                .name
                .static_name()
                .is_some_and(|key| HATCH_PROPERTIES.contains(&key.as_ref())),
            _ => false,
        });
        if hatch {
            patched.extend(all());
        }
    }
    patched
}

/// Every unit's own lexical bindings.
/// The instance fields of each class: its own (non-`declare`) field declarations, its
/// parameter properties and what its constructor assigns.
fn instance_fields(world: &World<'_>) -> FxHashMap<NodeRef, FxHashSet<String>> {
    world
        .classes
        .iter()
        .map(|(&class, info)| {
            let mut fields: FxHashSet<String> = info
                .class
                .body
                .body
                .iter()
                .filter_map(|element| match element {
                    oxc_ast::ast::ClassElement::PropertyDefinition(property)
                        if !property.r#static && !property.declare =>
                    {
                        crate::world::static_key(&property.key, property.computed)
                    }
                    _ => None,
                })
                .collect();
            fields.extend(crate::shapes::definite_fields(world, class));
            fields.extend(crate::shapes::constructor_assigned(world, class));
            (class, fields)
        })
        .collect()
}

fn lexicals(world: &World<'_>) -> FxHashMap<NodeRef, Vec<SymbolId>> {
    let mut lexicals: FxHashMap<NodeRef, Vec<SymbolId>> = FxHashMap::default();
    for module in &world.modules {
        for (index, info) in module.symbols.iter().enumerate() {
            if info.lexical
                && let Some(unit) = info.unit
            {
                lexicals
                    .entry(unit)
                    .or_default()
                    .push(SymbolId::from_usize(index));
            }
        }
    }
    lexicals
}

/// The strongly connected component of every module in the import graph (Tarjan's
/// algorithm): modules in one component may run each other's code before they finish.
fn import_cycles(world: &World<'_>) -> Vec<usize> {
    let count = world.modules.len();
    let edges: Vec<Vec<usize>> = world
        .modules
        .iter()
        .map(|module| {
            module
                .imports
                .values()
                .copied()
                .flatten()
                .map(ModuleId::index)
                .filter(|&target| target < count)
                .collect()
        })
        .collect();
    let mut index = vec![usize::MAX; count];
    let mut low = vec![0; count];
    let mut on_stack = vec![false; count];
    let mut stack = Vec::new();
    let mut component = vec![usize::MAX; count];
    let mut next = 0;
    let mut components = 0;
    for root in 0..count {
        if index[root] != usize::MAX {
            continue;
        }
        let mut frames: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&mut (node, ref mut at)) = frames.last_mut() {
            if let Some(&successor) = edges[node].get(*at) {
                *at += 1;
                if index[successor] == usize::MAX {
                    index[successor] = next;
                    low[successor] = next;
                    next += 1;
                    stack.push(successor);
                    on_stack[successor] = true;
                    frames.push((successor, 0));
                } else if on_stack[successor] {
                    low[node] = low[node].min(index[successor]);
                }
                continue;
            }
            frames.pop();
            if let Some(&(parent, _)) = frames.last() {
                low[parent] = low[parent].min(low[node]);
            }
            if low[node] == index[node] {
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component[member] = components;
                    if member == node {
                        break;
                    }
                }
                components += 1;
            }
        }
    }
    component
}

/// The modules whose exports the host can use: the entries (or, without any, the modules no
/// other module imports) and every module they re-export from, transitively.
pub(crate) fn open_modules(world: &World<'_>, options: &AnalysisOptions) -> FxHashSet<ModuleId> {
    let mut roots: Vec<ModuleId> = options.entries.clone();
    if roots.is_empty() {
        let imported: FxHashSet<ModuleId> = world
            .modules
            .iter()
            .flat_map(|module| module.imports.values().copied().flatten())
            .filter(|target| target.index() < world.modules.len())
            .collect();
        roots = world
            .modules
            .iter()
            .map(|module| module.id)
            .filter(|id| !imported.contains(id))
            .collect();
        if roots.is_empty()
            && let Some(first) = world.modules.first()
        {
            roots.push(first.id);
        }
    }
    let mut open = FxHashSet::default();
    while let Some(module) = roots.pop() {
        if module.index() >= world.modules.len() || !open.insert(module) {
            continue;
        }
        let info = world.module(module);
        let record = &info.record;
        let requests = record
            .star_export_entries
            .iter()
            .chain(&record.indirect_export_entries)
            .filter_map(|entry| entry.module_request.as_ref())
            .map(|request| request.name.as_str());
        // `import { a } from "x"; export { a }` re-exports too.
        let reexported_imports = record
            .import_entries
            .iter()
            .filter(|entry| {
                info.semantic
                    .scoping()
                    .get_root_binding(entry.local_name.name.as_str().into())
                    .is_some_and(|symbol| info.info(symbol).exported)
            })
            .map(|entry| entry.module_request.name.as_str());
        for request in requests.chain(reexported_imports) {
            if let Some(target) = info.imports.get(request).copied().flatten() {
                roots.push(target);
            }
        }
    }
    open
}

/// The name [`patched_builtins`] uses for the typed array constructors and prototypes.
pub const TYPED_ARRAY: &str = "TypedArray";

/// Whether `expression` is an identifier bound by the program (so `X.prototype` is a program
/// function's or class's).
/// Whether the program names a way to replace an object's prototype: `__proto__`,
/// `setPrototypeOf`, or a global that reaches anything (`Reflect`, `Function`, `eval`). Writes
/// to followed objects poison them; this covers the objects the analysis doesn't follow.
pub(crate) fn may_reprototype(world: &World<'_>) -> bool {
    use oxc_ast::AstKind;
    const NAMES: [&str; 2] = ["__proto__", "setPrototypeOf"];
    world.modules.iter().any(|module| {
        let scoping = module.semantic.scoping();
        let nodes = module.semantic.nodes();
        let global = scoping
            .root_unresolved_references_ids()
            .flatten()
            .any(|reference| {
                let reference = scoping.get_reference(reference);
                reference.is_value()
                    && matches!(
                        nodes.kind(reference.node_id()),
                        AstKind::IdentifierReference(ident)
                            if matches!(ident.name.as_str(), "Reflect" | "Function" | "eval")
                    )
            });
        global
            || nodes.iter().any(|node| match node.kind() {
                AstKind::StaticMemberExpression(member) => {
                    NAMES.contains(&member.property.name.as_str())
                }
                AstKind::ComputedMemberExpression(member) => matches!(
                    &member.expression,
                    oxc_ast::ast::Expression::StringLiteral(key)
                        if NAMES.contains(&key.value.as_str())
                ),
                _ => false,
            })
    })
}

fn names_program_symbol(module: &Module<'_>, expression: &oxc_ast::ast::Expression<'_>) -> bool {
    let oxc_ast::ast::Expression::Identifier(ident) = expression.without_parentheses() else {
        return false;
    };
    ident.reference_id.get().is_some_and(|reference| {
        module
            .semantic
            .scoping()
            .get_reference(reference)
            .symbol_id()
            .is_some()
    })
}

/// Whether the reference `node` to a namespace (or the global object) only reads a member
/// without handing anything out (`Math.floor(x)`, `Math.PI`, `globalThis.foo`), or calls or
/// constructs it (`new Array(n)`, `x instanceof Uint8Array`).
fn safe_namespace_use(
    nodes: &oxc_semantic::AstNodes<'_>,
    node: NodeId,
    name: &str,
    global_object: bool,
) -> bool {
    use oxc_ast::AstKind;
    let parent = nodes.parent_id(node);
    let span = nodes.kind(node).span();
    if !global_object {
        match nodes.kind(parent) {
            AstKind::CallExpression(call) if call.callee.span() == span => return true,
            AstKind::NewExpression(new) if new.callee.span() == span => return true,
            AstKind::BinaryExpression(binary)
                if binary.operator == oxc_syntax::operator::BinaryOperator::Instanceof
                    && binary.right.span() == span =>
            {
                return true;
            }
            _ => {}
        }
    }
    let AstKind::StaticMemberExpression(member) = nodes.kind(parent) else {
        return false;
    };
    if member.object.span() != span {
        return false;
    }
    let property = member.property.name.as_str();
    if global_object && crate::builtins::NAMESPACES.contains(&property) {
        // `globalThis.Math` hands out the namespace.
        return false;
    }
    let grandparent = nodes.parent_id(parent);
    match nodes.kind(grandparent) {
        // A call of the member: `Math.floor(x)` (its arguments are not the namespace).
        AstKind::CallExpression(call) if call.callee.span() == member.span => true,
        // Any further member access (`Array.prototype.x`, `Math.floor.call`) or a write.
        AstKind::StaticMemberExpression(_)
        | AstKind::ComputedMemberExpression(_)
        | AstKind::PrivateFieldExpression(_)
        | AstKind::AssignmentExpression(_)
        | AstKind::UpdateExpression(_)
        | AstKind::ArrayAssignmentTarget(_)
        | AstKind::ObjectAssignmentTarget(_)
        | AstKind::AssignmentTargetWithDefault(_)
        | AstKind::AssignmentTargetPropertyProperty(_)
        | AstKind::ForInStatement(_)
        | AstKind::ForOfStatement(_) => false,
        AstKind::UnaryExpression(unary) => !unary.operator.is_delete(),
        // A plain read: fine for values the models know (`Math.PI`); a builtin function or
        // `NS.prototype` read as a value could be patched by whoever receives it.
        _ => {
            let path = format!("{name}.{property}");
            global_object || crate::builtins::global(&path).is_some_and(|builtin| !builtin.callable)
        }
    }
}

/// One chunk's interpreter for one sweep.
pub(crate) struct Analyzer<'w, 'a> {
    pub world: &'w World<'a>,
    pub links: &'w Links,
    pub options: &'w AnalysisOptions,
    /// Builtin namespaces whose models are off.
    pub patched: &'w FxHashSet<String>,
    /// How many merges changed each entry so far.
    changes: &'w FxHashMap<Dep, u32>,
    /// How many merges changed each heap property so far.
    slot_changes: &'w tables::SlotChanges,
    tables: tables::Tables<'w>,
    log: Vec<tables::Op>,
    /// Entries the logged writes read (other than the ones they wrote).
    op_reads: FxHashSet<Dep>,
    /// Entries the logged writes changed.
    changed: FxHashSet<Dep>,
    /// Heap properties they changed.
    changed_slots: FxHashSet<tables::Slot>,
    /// The current unit's recordings.
    pub output: Output,
    /// Each of the chunk's units' last recordings.
    outputs: &'w mut FxHashMap<NodeRef, Output>,
    deps: std::cell::RefCell<ChunkDeps<'w>>,
    /// TypeScript's answers so far, for this chunk's nodes.
    ts_cache: &'w mut FxHashMap<NodeRef, Option<Type>>,
    types: Types<'w>,
    /// Modules whose exports code outside the program may use (see [`open_modules`]).
    pub open_modules: &'w FxHashSet<ModuleId>,
    /// Each unit's own `let`, `const` and `class` bindings.
    pub lexicals: &'w FxHashMap<NodeRef, Vec<SymbolId>>,
    /// Each module's strongly connected component of the import graph (by module index).
    pub import_cycles: &'w [usize],
    /// The instance fields each class declares or its constructor assigns.
    pub instance_fields: &'w FxHashMap<NodeRef, FxHashSet<String>>,
    /// Whether a declared instance field nobody has written yet reads as `never`, not as
    /// missing, and builtin calls with a `never` argument are skipped: true until the sweeps
    /// first settle (see [`Output::optimistic`]).
    pub optimistic: bool,
    stats: Stats,
}

/// A unit's flow-sensitive state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Env {
    /// Whether this point is reachable.
    pub reachable: bool,
    /// Values of tracked locals.
    pub vars: Vars,
    /// `(index, array)`: the local `index` is below `array.length`.
    pub bounds: Vec<(SymbolId, SymbolId)>,
    /// `(n, array)`: the local `n` holds `array.length`.
    pub lengths: Vec<(SymbolId, SymbolId)>,
}

impl Env {
    fn reachable() -> Self {
        Self {
            reachable: true,
            ..Self::default()
        }
    }

    fn unreachable() -> Self {
        Self::default()
    }

    /// Both paths: values join, relations must hold on both.
    #[must_use]
    pub fn join(&self, other: &Self) -> Self {
        if !self.reachable {
            return other.clone();
        }
        if !other.reachable {
            return self.clone();
        }
        let mut vars = self.vars.clone();
        if !vars.shares(&other.vars) {
            for (symbol, value) in other.vars.iter() {
                match vars.get(*symbol) {
                    Some(existing) if existing == value => {}
                    Some(existing) => {
                        let joined = existing.join(value);
                        vars.insert(*symbol, joined);
                    }
                    None => vars.insert(*symbol, value.clone()),
                }
            }
        }
        Self {
            reachable: true,
            vars,
            bounds: self
                .bounds
                .iter()
                .filter(|bound| other.bounds.contains(bound))
                .copied()
                .collect(),
            lengths: self
                .lengths
                .iter()
                .filter(|length| other.lengths.contains(length))
                .copied()
                .collect(),
        }
    }

    /// `next` widened against `self` (loop heads).
    fn widen(&self, next: &Self) -> Self {
        let mut joined = self.join(next);
        let widened: Vec<(SymbolId, Type)> = joined
            .vars
            .iter()
            .filter_map(|(&symbol, value)| {
                let old = self.vars.get(symbol)?;
                let ty = numeric::widen_type(&old.ty, &value.ty);
                (ty != value.ty).then_some((symbol, ty))
            })
            .collect();
        for (symbol, ty) in widened {
            if let Some(value) = joined.vars.get_mut(symbol) {
                value.ty = ty;
            }
        }
        joined
    }

    fn forget(&mut self, symbol: SymbolId) {
        self.bounds
            .retain(|(index, array)| *index != symbol && *array != symbol);
        self.lengths
            .retain(|(n, array)| *n != symbol && *array != symbol);
    }
}

/// An [`Env`] kept between runs (its values not shared, so it can move between threads).
#[derive(Debug)]
struct Settled {
    reachable: bool,
    vars: FxHashMap<SymbolId, Val>,
    bounds: Vec<(SymbolId, SymbolId)>,
    lengths: Vec<(SymbolId, SymbolId)>,
}

impl Settled {
    fn from_env(env: Env) -> Self {
        Self {
            reachable: env.reachable,
            vars: std::rc::Rc::unwrap_or_clone(env.vars.0),
            bounds: env.bounds,
            lengths: env.lengths,
        }
    }

    fn into_env(self) -> Env {
        Env {
            reachable: self.reachable,
            vars: Vars(std::rc::Rc::new(self.vars)),
            bounds: self.bounds,
            lengths: self.lengths,
        }
    }
}

/// The values of a unit's tracked locals: shared between copies of an environment until one
/// of them changes (branches copy environments far more often than they change them).
#[derive(Debug, Clone, Default)]
pub struct Vars(std::rc::Rc<FxHashMap<SymbolId, Val>>);

impl PartialEq for Vars {
    fn eq(&self, other: &Self) -> bool {
        self.shares(other) || *self.0 == *other.0
    }
}

impl Vars {
    /// Whether both are the same copy.
    fn shares(&self, other: &Self) -> bool {
        std::rc::Rc::ptr_eq(&self.0, &other.0)
    }

    /// The value of `symbol`.
    pub fn get(&self, symbol: SymbolId) -> Option<&Val> {
        self.0.get(&symbol)
    }

    /// Sets `symbol`'s value (copying the table if another environment shares it and the
    /// value changes).
    pub fn insert(&mut self, symbol: SymbolId, value: Val) {
        if self.0.get(&symbol) != Some(&value) {
            std::rc::Rc::make_mut(&mut self.0).insert(symbol, value);
        }
    }

    /// `symbol`'s value, to change in place.
    pub fn get_mut(&mut self, symbol: SymbolId) -> Option<&mut Val> {
        std::rc::Rc::make_mut(&mut self.0).get_mut(&symbol)
    }

    /// Every value, to change in place.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Val> {
        std::rc::Rc::make_mut(&mut self.0).values_mut()
    }

    /// Every local and its value.
    pub fn iter(&self) -> impl Iterator<Item = (&SymbolId, &Val)> {
        self.0.iter()
    }
}

/// Interpretation of one unit.
pub(crate) struct Cx<'c, 'w, 'a> {
    pub an: &'c mut Analyzer<'w, 'a>,
    pub unit: &'w Unit<'a>,
    pub module: &'w Module<'a>,
    pub env: Env,
    /// The value of `this`.
    pub this: Val,
    /// Join of returned values.
    pub returned: Val,
    /// Environments at `break`s, by label (`None` for unlabelled).
    pub breaks: Vec<(Option<&'a str>, Env)>,
    /// Environments at `continue`s.
    pub continues: Vec<(Option<&'a str>, Env)>,
    /// Labels of the statement being entered (for labelled loops).
    pub pending_labels: Vec<&'a str>,
    /// How many enclosing `try` blocks with a `catch` there are.
    pub catching: usize,
    /// Symbols assigned anywhere (for `catch` entry), while inside a `try`.
    pub try_writes: Vec<FxHashSet<SymbolId>>,
    /// A `const n = array.length` binding about to be written.
    pub pending_length: Option<(SymbolId, SymbolId)>,
    pub record: UnitRecord,
    /// Where interpretation is (the start of the expression last entered).
    pub position: u32,
    /// Effects of the expressions and loops being interpreted, innermost last.
    pub frames: Vec<Frame>,
    /// Locals assigned in each loop being interpreted.
    pub loop_writes: Vec<FxHashSet<SymbolId>>,
    /// For each loop being interpreted: the arrays it resizes, and whether it resizes one
    /// trioxide doesn't track.
    pub loop_resized: Vec<(FxHashSet<AllocId>, bool)>,
    /// Where each loop settled when it last ran, in this run or the unit's last: a loop that
    /// runs again starts from there.
    pub loop_heads: FxHashMap<NodeId, Env>,
    /// Arrays whose length relations were dropped, in order.
    pub dropped: Vec<SymbolId>,
}

impl<'w, 'a> Analyzer<'w, 'a> {
    fn interpret(&mut self, unit: &'w Unit<'a>) {
        let deps = self.deps.get_mut();
        deps.current = Some(unit.id);
        deps.current_reads.clear();
        // Reuse the last run's tables: they are about the right size.
        self.output = self.outputs.remove(&unit.id).unwrap_or_default();
        self.output.clear();
        let loop_heads = std::mem::take(&mut self.output.loop_heads)
            .into_iter()
            .map(|(node, settled)| (node, settled.into_env()))
            .collect();
        // Losses from outside an interpretation (merges) don't concern this unit.
        let _ = crate::value::take_lost();
        let module = self.world.module(unit.id.module);
        let this = self.this_for(unit);
        let mut cx = Cx {
            an: self,
            unit,
            module,
            env: Env::reachable(),
            this,
            returned: Val::never(),
            breaks: Vec::new(),
            continues: Vec::new(),
            pending_labels: Vec::new(),
            catching: 0,
            try_writes: Vec::new(),
            pending_length: None,
            record: UnitRecord::default(),
            position: 0,
            frames: Vec::new(),
            loop_writes: Vec::new(),
            loop_resized: Vec::new(),
            loop_heads,
            dropped: Vec::new(),
        };
        cx.enter();
        match unit.body {
            Body::Module(program) => {
                cx.hoist(&program.body);
                cx.statements(&program.body);
            }
            Body::Function(function) => {
                // A function expression's own name, in its body, is the function itself.
                if function.is_expression()
                    && let Some(id) = &function.id
                    && let Some(symbol) = id.symbol_id.get()
                {
                    let site = unit.id;
                    // Made by the code that evaluates the expression.
                    cx.an
                        .obj(site, ObjKind::Function(site), unit.parent.unwrap_or(site));
                    let value = Val::object(Type::opaque_function(Some(site)), site);
                    cx.write_symbol(symbol, value, Some(id.node_id.get()));
                }
                cx.params(&function.params);
                if let Some(body) = &function.body {
                    cx.hoist(&body.statements);
                    cx.statements(&body.statements);
                }
            }
            Body::Arrow(arrow) => {
                cx.params(&arrow.params);
                match &arrow.body {
                    oxc_ast::ast::ArrowFunctionBody::FunctionBody(body) => {
                        cx.hoist(&body.statements);
                        cx.statements(&body.statements);
                    }
                    body => {
                        if let Some(expression) = body.as_expression() {
                            let value = cx.expr(expression);
                            cx.escape_value(&value, Escape::Known);
                            cx.returned = cx.returned.join(&value);
                            cx.env = Env::unreachable();
                        }
                    }
                }
            }
            Body::StaticBlock(block) => {
                cx.hoist(&block.body);
                cx.statements(&block.body);
            }
            Body::ClassInit(class) => cx.class_fields(class),
        }
        if cx.env.reachable {
            cx.returned = cx.returned.join(&Val::proven(Type::Undefined));
        }
        let returned = std::mem::replace(&mut cx.returned, Val::never());
        let record = std::mem::take(&mut cx.record);
        let loop_heads = std::mem::take(&mut cx.loop_heads)
            .into_iter()
            .map(|(node, env)| (node, Settled::from_env(env)))
            .collect();
        self.join_return(unit.id, &returned);
        // A site joined with an unknown object is no longer followed: anything may happen to
        // it from here, so it escapes.
        loop {
            let mut lost = crate::value::take_lost();
            if lost.is_empty() {
                break;
            }
            lost.sort_unstable();
            lost.dedup();
            for site in lost {
                self.escape(&Pts::site(site), Escape::Unknown);
            }
        }
        self.output.units.insert(unit.id, record);
        self.output.loop_heads = loop_heads;
        let output = std::mem::take(&mut self.output);
        self.outputs.insert(unit.id, output);
        self.deps.get_mut().current = None;
    }

    /// Notes that the current unit read `dep`.
    pub fn read(&self, dep: Dep) {
        let mut deps = self.deps.borrow_mut();
        let Some(unit) = deps.current else { return };
        if deps.current_reads.insert(dep) && deps.seen.insert((dep, unit)) {
            deps.reads.entry(dep).or_default().push(unit);
        }
    }

    /// Notes that `dep` changed: this chunk's units that read it run (again). Units of other
    /// chunks are found when the sweep is merged.
    fn touch(&self, dep: Dep) {
        let mut deps = self.deps.borrow_mut();
        let ChunkDeps {
            current,
            current_reads,
            readers,
            reads,
            members,
            dirty,
            ..
        } = &mut *deps;
        // The current unit's result depends only on what this run read so far.
        let skip = current.filter(|_| !current_reads.contains(&dep));
        let earlier = readers.get(&dep).into_iter().flatten();
        let now = reads.get(&dep).into_iter().flatten();
        for &unit in earlier.chain(now) {
            if Some(unit) != skip && members.contains(&unit) {
                dirty.insert(unit);
            }
        }
    }

    /// Applies a write to this chunk's view and logs it for the merge.
    fn write(&mut self, op: tables::Op) {
        let cx = tables::OpCx {
            world: self.world,
            links: self.links,
            changes: self.changes,
            slot_changes: self.slot_changes,
        };
        self.tables.apply(cx, &op);
        self.changed_slots
            .extend(self.tables.touched_slots.drain(..));
        // A write that changed nothing here changes nothing in the merged tables either: they
        // hold at least what this chunk saw. Flag updates depend on the entry, so they always
        // replay.
        if !self.tables.touched.is_empty() || matches!(op, tables::Op::Mark(..)) {
            self.log.push(op);
        }
        if !self.tables.reads.is_empty() {
            let reads = std::mem::take(&mut self.tables.reads);
            for &dep in &reads {
                self.read(dep);
                self.op_reads.insert(dep);
            }
            self.tables.reads = reads;
            self.tables.reads.clear();
        }
        if !self.tables.touched.is_empty() {
            let touched = std::mem::take(&mut self.tables.touched);
            for &dep in &touched {
                self.touch(dep);
                self.changed.insert(dep);
            }
            self.tables.touched = touched;
            self.tables.touched.clear();
        }
    }

    /// The heap entry of `site`, *not* as a dependency: only for asking whether the creating
    /// code is making the object for the first time, which decides how a [`Cx::mark`] update
    /// initialises it. Once initialised, the update's other branch leaves the entry as it is,
    /// so a later answer can't change anything.
    pub fn peek(&self, site: AllocId) -> Option<&Obj> {
        self.tables.obj(site)
    }

    /// The heap entry of `site`, as a dependency of the current unit.
    pub fn heap(&self, site: AllocId) -> Option<&Obj> {
        self.read(Dep::Heap(site));
        self.tables.obj(site)
    }

    /// Every value `symbol` holds, as a dependency of the current unit.
    pub fn symbol_value(&self, symbol: SymbolRef) -> Option<&Val> {
        self.read(Dep::Symbol(symbol));
        self.tables.symbol(symbol)
    }

    /// What `unit` returns, as a dependency of the current unit.
    pub fn returned(&self, unit: NodeRef) -> Option<&Val> {
        self.read(Dep::Return(unit));
        self.tables.returned(unit)
    }

    /// What `class` extends, as a dependency of the current unit.
    pub fn super_of(&self, class: NodeRef) -> Option<&Val> {
        self.read(Dep::Super(class));
        self.tables.super_of(class)
    }

    /// The value of an `export default` expression, as a dependency of the current unit.
    pub fn default_value(&self, node: NodeRef) -> Option<&Val> {
        self.read(Dep::Default(node));
        self.tables.default_value(node)
    }

    /// The arguments known callers pass `unit`, by position, and whether one passes a spread.
    pub fn params_of(&self, unit: NodeRef) -> (Vec<Val>, bool) {
        self.read(Dep::Params(unit));
        (
            self.tables.params(unit).cloned().unwrap_or_default(),
            self.tables.spread_called(unit),
        )
    }

    /// Notes a call of `unit` with a spread (so positions say nothing).
    pub fn called_with_spread(&mut self, unit: NodeRef) {
        self.write(tables::Op::SpreadCalled(unit));
    }

    /// `this` for a unit: what known callers passed, or what TypeScript says if unknown code
    /// may call it.
    fn this_for(&mut self, unit: &Unit<'a>) -> Val {
        match unit.body {
            Body::Module(_) => Val::proven(Type::Undefined),
            Body::Arrow(_) => {
                // Lexical: the enclosing non-arrow unit's `this`, as recorded.
                let mut parent = unit.parent;
                while let Some(id) = parent {
                    let enclosing = self.world.unit(id);
                    if !enclosing.lexical_this() {
                        return match enclosing.body {
                            Body::Module(_) => Val::proven(Type::Undefined),
                            _ => self.recorded_this(enclosing),
                        };
                    }
                    parent = enclosing.parent;
                }
                Val::unknown()
            }
            Body::StaticBlock(_) => match unit.class {
                Some(class) => self.class_value(class),
                None => Val::unknown(),
            },
            _ => self.recorded_this(unit),
        }
    }

    fn recorded_this(&mut self, unit: &Unit<'a>) -> Val {
        self.read(Dep::This(unit.id));
        let recorded = self
            .tables
            .this(unit.id)
            .cloned()
            .unwrap_or_else(Val::never);
        // Joining with unknown drops the recorded sites, which escapes them: unknown code
        // may call the unit on them.
        if self.is_open(unit) {
            recorded.join(&Val::unknown())
        } else {
            recorded
        }
    }

    /// The value of the class object for `class`.
    #[expect(clippy::unused_self, reason = "a method like the other value builders")]
    fn class_value(&self, class: NodeRef) -> Val {
        Val::object(Type::opaque_function(Some(class)), class)
    }

    /// Whether code trioxide does not see may call `unit` (so its parameters and `this` are
    /// whatever TypeScript declares).
    /// Whether `unit` is a function that no known caller has called yet and that unknown
    /// code can't call.
    fn uncalled(&self, unit: &Unit<'a>) -> bool {
        uncalled(self.world, &self.tables, unit)
    }

    pub fn is_open(&self, unit: &Unit<'a>) -> bool {
        self.read(Dep::Heap(unit.id));
        if let Some(class) = unit
            .class
            .or_else(|| self.world.member_of.get(&unit.id).copied())
        {
            self.read(Dep::Heap(class));
            if let Some(info) = self.world.classes.get(&class) {
                self.read(Dep::Heap(info.init));
            }
        }
        unit_open(self.world, &self.tables, unit)
    }

    fn join_symbol(&mut self, symbol: SymbolRef, value: &Val) {
        if unchanged(self.tables.symbol(symbol), value) {
            return;
        }
        self.write(tables::Op::JoinSymbol(symbol, value.clone()));
    }

    fn join_return(&mut self, unit: NodeRef, value: &Val) {
        if unchanged(self.tables.returned(unit), value) {
            return;
        }
        self.write(tables::Op::JoinReturn(unit, value.clone()));
    }

    fn join_this(&mut self, unit: NodeRef, value: &Val) {
        if unchanged(self.tables.this(unit), value) {
            return;
        }
        self.write(tables::Op::JoinThis(unit, value.clone()));
    }

    fn join_param(&mut self, unit: NodeRef, index: usize, value: &Val) {
        let slot = self
            .tables
            .params(unit)
            .and_then(|params| params.get(index));
        if slot.is_some() && unchanged(slot, value) {
            return;
        }
        self.write(tables::Op::JoinParam(unit, index, value.clone()));
    }

    fn join_super(&mut self, class: NodeRef, value: &Val) {
        if unchanged(self.tables.super_of(class), value) {
            return;
        }
        self.write(tables::Op::JoinSuper(class, value.clone()));
    }

    /// Joins into the value of an `export default` expression.
    pub fn join_default(&mut self, node: NodeRef, value: &Val) {
        if unchanged(self.tables.default_value(node), value) {
            return;
        }
        self.write(tables::Op::JoinDefault(node, value.clone()));
    }

    /// Creates the (empty) heap entry of `site` on first use.
    fn obj(&mut self, site: AllocId, kind: ObjKind, unit: NodeRef) {
        if self.tables.obj(site).is_none() {
            self.write(tables::Op::Create(site, kind, unit));
        }
    }

    /// Marks every site in `pts` (and what it reaches) as reaching code that may redefine its
    /// properties arbitrarily.
    pub fn poison(&mut self, pts: &Pts) {
        if pts.sites().is_some_and(|sites| !sites.is_empty()) {
            self.write(tables::Op::Poison(pts.clone()));
        }
    }

    /// Marks every site in `pts` (and what it reaches) as escaping at least to `level`.
    pub fn escape(&mut self, pts: &Pts, level: Escape) {
        // A namespace that escaped already still takes the exports written since.
        let needed = pts.sites().is_some_and(|sites| {
            sites.iter().any(|&site| {
                self.tables.obj(site).is_some_and(|obj| {
                    obj.escape < level || matches!(obj.kind, ObjKind::Namespace(_))
                })
            })
        });
        if needed {
            self.write(tables::Op::Escape(pts.clone(), level));
        }
    }

    /// The namespace object of `module`.
    pub fn namespace(&mut self, module: ModuleId, unit: NodeRef) -> Val {
        let program = self.world.module(module).program.node_id.get();
        let site = NodeRef {
            module,
            node: program,
        };
        self.obj(site, ObjKind::Namespace(module), unit);
        Val::object(Type::empty_object(), site)
    }

    /// TypeScript's type at `node`, as a declared value (cached).
    fn declared_type(&mut self, node: NodeRef, span: Span) -> Option<Type> {
        if let Some(cached) = self.ts_cache.get(&node) {
            return cached.clone();
        }
        let started = std::time::Instant::now();
        let ty = match &mut self.types {
            Types::Exclusive(oracle) => oracle.type_at(node.module, span),
            Types::Shared(oracle) => oracle.type_at(node.module, span),
            Types::Worker(oracle) => oracle.type_at(node.module, span),
        };
        self.stats.oracle_queries += 1;
        self.stats.oracle_time += started.elapsed();
        self.ts_cache.insert(node, ty.clone());
        ty
    }
}

impl Cx<'_, '_, '_> {
    /// The node `id` of this module as a global ref.
    pub fn node(&self, id: NodeId) -> NodeRef {
        self.module.node(id)
    }

    /// Records the value of `node` for this round, joining with earlier visits.
    pub fn record_node(&mut self, node: NodeId, value: &Val) {
        let key = self.node(node);
        match self.an.output.nodes.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                // Loops revisit nodes, mostly with the value they had.
                if entry.get() != value {
                    let joined = entry.get().join(value);
                    entry.insert(joined);
                }
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(value.clone());
            }
        }
    }

    /// The value with TypeScript's declared type filled in if the analysis knows nothing.
    pub fn or_declared(&mut self, value: Val, node: NodeId, span: Span) -> Val {
        if value.trust != Trust::Unknown || value.poisoned || self.declared_type_unused(node) {
            return value;
        }
        let key = self.node(node);
        match self.an.declared_type(key, span) {
            Some(ty) if ty != Type::Unknown => {
                let mut declared = Val::declared(ty);
                // An object TypeScript knows about may still be one trioxide tracks.
                declared.pts = value.pts;
                declared
            }
            _ => value,
        }
    }

    /// Whether asking TypeScript about `node` can't tell anything: the value of an expression
    /// statement is discarded, and the lib's global objects (`console`, `Math`) have types the
    /// oracle answers `Unknown` for.
    fn declared_type_unused(&self, node: NodeId) -> bool {
        use oxc_ast::AstKind;
        let nodes = self.module.semantic.nodes();
        match nodes.kind(node) {
            AstKind::CallExpression(_) => {
                matches!(nodes.parent_kind(node), AstKind::ExpressionStatement(_))
            }
            AstKind::IdentifierReference(ident) => {
                LIB_OBJECTS.contains(&ident.name.as_str())
                    && ident.reference_id.get().is_some_and(|reference| {
                        self.module
                            .semantic
                            .scoping()
                            .get_reference(reference)
                            .symbol_id()
                            .is_none()
                    })
            }
            _ => false,
        }
    }

    /// Marks the objects `value` may be as escaping to `level`. Values that leave the unit
    /// escape to at least `Known`.
    pub fn escape_value(&mut self, value: &Val, level: Escape) {
        self.an.escape(&value.pts, level);
    }

    /// Notes an effect. `proven` and `declared` say whether the operation is known *not* to
    /// have it at each trust level.
    pub fn effect(&mut self, kind: EffectKind, proven_free: bool, declared_free: bool) {
        let set = |effects: &mut Effects| match kind {
            EffectKind::Reads => effects.reads = true,
            EffectKind::Writes => effects.writes = true,
            EffectKind::Throws => effects.throws = true,
            // Code trioxide doesn't see may read and write anything.
            EffectKind::CallsUnknown => {
                effects.calls_unknown = true;
                effects.reads = true;
                effects.writes = true;
            }
            EffectKind::Allocates => effects.allocates = true,
        };
        // An expression's own effects count whether or not a `try` catches them. Only the
        // innermost frame is marked: frames pass theirs on as they close ([`Cx::pop_frame`]).
        if let Some(frame) = self.frames.last_mut() {
            if !proven_free {
                set(&mut frame.proven);
            }
            if !declared_free {
                set(&mut frame.declared);
            }
        }
        if kind == EffectKind::Throws && self.catching > 0 {
            return;
        }
        if !proven_free {
            set(&mut self.record.proven);
        }
        if !declared_free {
            set(&mut self.record.declared);
        }
    }

    /// Notes a write for the expressions and loops being interpreted: `shared` when it is
    /// to an object or a local other code can see.
    pub fn note_write(&mut self, shared: bool) {
        if let Some(frame) = self.frames.last_mut() {
            frame.local_writes = true;
            frame.shared_writes |= shared;
        }
    }

    /// Notes that the length of the arrays `object` may be can change.
    pub fn note_resize(&mut self, object: &Val) {
        if let Some(sites) = object.pts.sites() {
            self.record.resized.extend(sites.iter().copied());
            for (resized, _) in &mut self.loop_resized {
                resized.extend(sites.iter().copied());
            }
        } else {
            self.record.resizes_untracked = true;
            for (_, untracked) in &mut self.loop_resized {
                *untracked = true;
            }
        }
    }

    /// Notes a read of an object, or of a local other code can write.
    pub fn note_shared_read(&mut self) {
        if let Some(frame) = self.frames.last_mut() {
            frame.shared_reads = true;
        }
    }

    /// Notes that a clock or a random number was read.
    pub fn note_volatile(&mut self) {
        if let Some(frame) = self.frames.last_mut() {
            frame.volatile = true;
        }
    }

    /// Closes the innermost frame, passing what it saw on to the enclosing one.
    pub fn pop_frame(&mut self) -> Frame {
        let frame = self.frames.pop().expect("a frame is open");
        if let Some(parent) = self.frames.last_mut() {
            parent.proven = union(parent.proven, frame.proven);
            parent.declared = union(parent.declared, frame.declared);
            parent.local_writes |= frame.local_writes;
            parent.shared_writes |= frame.shared_writes;
            parent.volatile |= frame.volatile;
            parent.shared_reads |= frame.shared_reads;
        }
        frame
    }

    /// The units called since edge `start` of this unit's record.
    pub fn callees_since(&self, start: usize) -> Vec<NodeRef> {
        let mut callees: Vec<NodeRef> = self.record.edges[start..]
            .iter()
            .map(|edge| edge.callee)
            .collect();
        callees.sort_unstable();
        callees.dedup();
        callees
    }

    /// Notes an effect that happens regardless of what types say.
    pub fn always(&mut self, kind: EffectKind) {
        self.effect(kind, false, false);
    }

    fn enter(&mut self) {
        self.record.proven = Effects::PURE;
        self.record.declared = Effects::PURE;
        // Hoisted `var`s start out `undefined`; block-scoped bindings are in their TDZ.
        if let Some(symbols) = self.an.world.unit_symbols.get(&self.unit.id) {
            let scoping = self.module.semantic.scoping();
            for &symbol in symbols {
                let flags = scoping.symbol_flags(symbol);
                if flags.contains(oxc_syntax::symbol::SymbolFlags::FunctionScopedVariable)
                    && !is_parameter(self.module, symbol)
                {
                    self.write_symbol(symbol, Val::proven(Type::Undefined), None);
                }
            }
        }
    }

    /// Reads a symbol of this module.
    pub fn read_symbol(&mut self, symbol: SymbolId) -> Val {
        let info = self.module.info(symbol);
        if info.tainted {
            return Val::poisoned();
        }
        let flags = self.module.semantic.scoping().symbol_flags(symbol);
        if flags.intersects(oxc_syntax::symbol::SymbolFlags::EnumMember) {
            // Enum members are properties of the enum object: TypeScript's types say what.
            return Val::unknown();
        }
        if flags.contains(oxc_syntax::symbol::SymbolFlags::Ambient)
            || self.module.is_ambient(symbol)
        {
            // `declare`d: defined by code outside the program, if at all. Only TypeScript says
            // it exists, so reading it may throw a ReferenceError unless declared types count.
            self.always(EffectKind::Reads);
            self.effect(EffectKind::Throws, false, true);
            return Val::unknown();
        }
        if info.import {
            return self.read_import(self.module.symbol(symbol));
        }
        if info.lexical {
            self.note_tdz_read(symbol);
        }
        if info.tracked && info.unit == Some(self.unit.id) {
            if let Some(value) = self.env.vars.get(symbol) {
                return value.clone();
            }
            // Not yet initialised on any path that got here.
            return Val::never();
        }
        let scoping = self.module.semantic.scoping();
        let mutable = !scoping.symbol_flags(symbol).intersects(
            oxc_syntax::symbol::SymbolFlags::ConstVariable
                | oxc_syntax::symbol::SymbolFlags::Function,
        ) || scoping.symbol_is_mutated(symbol);
        if mutable {
            self.always(EffectKind::Reads);
            self.note_shared_read();
        }
        self.an
            .symbol_value(self.module.symbol(symbol))
            .cloned()
            .unwrap_or_else(Val::never)
    }

    /// The value an import binding refers to.
    pub fn read_import(&mut self, symbol: SymbolRef) -> Val {
        match self.an.links.import(symbol) {
            Some(crate::link::Target::Symbol(target)) => {
                let target_module = self.an.world.module(target.module);
                if target_module.broken {
                    return Val::unknown();
                }
                // In an import cycle the exporting module may not have run its declaration.
                if target_module.info(target.symbol).lexical
                    && self.an.import_cycles.get(target.module.index())
                        == self.an.import_cycles.get(self.module.id.index())
                {
                    self.always(EffectKind::Throws);
                }
                if target_module.info(target.symbol).tainted {
                    return Val::poisoned();
                }
                self.always(EffectKind::Reads);
                self.an
                    .symbol_value(target)
                    .cloned()
                    .unwrap_or_else(Val::never)
            }
            Some(crate::link::Target::Expression(node))
                if !self.an.world.module(node.module).broken =>
            {
                self.an
                    .default_value(node)
                    .cloned()
                    .unwrap_or_else(Val::never)
            }
            Some(crate::link::Target::Namespace(module)) => {
                let unit = self.unit.id;
                self.an.namespace(module, unit)
            }
            _ => Val::unknown(),
        }
    }

    /// A read of the lexical binding `symbol` throws if its declaration hasn't run: in its own
    /// unit, when the read comes before the declaration's end (or anywhere in a `switch`);
    /// elsewhere, when its unit may call out before reaching the declaration, or when it is
    /// module-level in a module that is in an import cycle.
    fn note_tdz_read(&mut self, symbol: SymbolId) {
        let info = self.module.info(symbol);
        let throws = if info.unit == Some(self.unit.id) {
            info.in_switch || self.position < info.declared_at
        } else {
            // In an import cycle another module may call this code before the module's own
            // top level has declared it.
            let module_level =
                info.unit == Some(self.module.node(self.module.program.node_id.get()));
            let cyclic = module_level && {
                let component = self.an.import_cycles.get(self.module.id.index());
                self.an
                    .import_cycles
                    .iter()
                    .filter(|&other| Some(other) == component)
                    .count()
                    > 1
            };
            let symbol = self.module.symbol(symbol);
            self.an.read(Dep::Symbol(symbol));
            cyclic || self.an.tables.may_be_uninit(symbol)
        };
        if throws {
            self.always(EffectKind::Throws);
        }
    }

    /// Code this unit doesn't control may run now (a call, a getter, a coercion): any of the
    /// unit's own lexical bindings not declared yet may be read in their dead zone.
    pub fn note_call(&mut self) {
        let Some(lexicals) = self.an.lexicals.get(&self.unit.id) else {
            return;
        };
        let position = self.position;
        let pending: Vec<SymbolRef> = lexicals
            .iter()
            .filter(|&&symbol| {
                let info = self.module.info(symbol);
                info.in_switch || info.declared_at > position
            })
            .map(|&symbol| self.module.symbol(symbol))
            .collect();
        for symbol in pending {
            if !self.an.tables.may_be_uninit(symbol) {
                self.an.write(tables::Op::MayBeUninit(symbol));
            }
        }
    }

    /// Writes a symbol of this module. `node` is the binding or reference, for its fact.
    pub fn write_symbol(&mut self, symbol: SymbolId, value: Val, node: Option<NodeId>) {
        let info = self.module.info(symbol).clone();
        let value = if info.tainted {
            // Code trioxide can't see (eval, `with`) may read the binding.
            self.escape_value(&value, Escape::Unknown);
            Val::poisoned()
        } else {
            value
        };
        if let Some(node) = node {
            self.record_node(node, &value);
        }
        if let Some(writes) = self.try_writes.last_mut() {
            writes.insert(symbol);
        }
        let own = info.unit == Some(self.unit.id);
        if own {
            for writes in &mut self.loop_writes {
                writes.insert(symbol);
            }
        }
        // An assignment (a declaration initialises instead).
        if node.is_some_and(|node| {
            matches!(
                self.module.semantic.nodes().kind(node),
                oxc_ast::AstKind::IdentifierReference(_)
            )
        }) {
            // A local a closure reads is state other code can see.
            self.note_write(!(own && info.tracked && !info.nested_reads));
        }
        if info.tracked && own {
            self.env.vars.insert(symbol, value.clone());
            self.env.forget(symbol);
        } else if !own {
            self.always(EffectKind::Writes);
        }
        if !own || info.nested_reads {
            // Other units can see it.
            self.escape_value(&value, Escape::Known);
        }
        if info.exported && self.exports_open() {
            // Importers outside the program can read it.
            self.escape_value(&value, Escape::Unknown);
        }
        let symbol = self.module.symbol(symbol);
        if private_local(&info, self.unit.id) {
            // Only this unit (and the facts) ever read it.
            let slot = self
                .an
                .output
                .symbols
                .entry(symbol)
                .or_insert_with(Val::never);
            if slot != &value {
                *slot = slot.join(&value);
            }
        } else {
            self.an.join_symbol(symbol, &value);
        }
    }

    /// Every value `symbol` (of this module) holds: what this run wrote to it if it is a
    /// private local, else what the shared tables say.
    pub fn every_value(&self, symbol: SymbolId) -> Option<Val> {
        let info = self.module.info(symbol);
        let symbol = self.module.symbol(symbol);
        if private_local(info, self.unit.id) {
            return self.an.output.symbols.get(&symbol).cloned();
        }
        self.an.symbol_value(symbol).cloned()
    }

    /// Whether code outside the program may use this module's exports: in an open world, for
    /// an entry (or a module an entry re-exports from), and for runtimes generated code calls.
    pub fn exports_open(&self) -> bool {
        !self.an.options.closed_world
            || self.an.open_modules.contains(&self.module.id)
            || self.module.called_from_generated_code()
    }

    /// Allocates (or revisits) the object made at `site`.
    pub fn alloc(&mut self, site: NodeId, kind: ObjKind) -> AllocId {
        let site = self.node(site);
        let unit = self.unit.id;
        self.an.obj(site, kind, unit);
        if !self.record.allocations.contains(&site) {
            self.record.allocations.push(site);
        }
        // The expressions being evaluated allocate, whether or not the object outlives the
        // unit (which is all the unit's own summary counts).
        if let Some(frame) = self.frames.last_mut() {
            frame.proven.allocates = true;
            frame.declared.allocates = true;
        }
        site
    }

    /// Writes `value` to property `key` of every object in `site`'s heap entry.
    pub fn store_prop(&mut self, site: AllocId, key: &str, value: &Val, creating: bool) {
        self.an.write(tables::Op::StoreProp {
            site,
            key: key.to_string(),
            value: value.clone(),
            creating,
        });
    }

    /// Joins `value` into the elements of the array at `site`.
    pub fn store_elem(&mut self, site: AllocId, value: &Val) {
        self.an.write(tables::Op::StoreElem(site, value.clone()));
    }

    /// Updates a heap entry's flags. `update` runs again when the sweep is merged, on the
    /// merged entry, so it must be idempotent. The update depends on the entry, so a change
    /// to it (the unit's own included) runs the unit again.
    pub fn mark(&mut self, site: AllocId, update: impl Fn(&mut Obj) + Send + 'static) {
        self.an.read(Dep::Heap(site));
        self.an.write(tables::Op::Mark(site, Box::new(update)));
    }
}

/// Whether joining `value` into `slot` can't change it.
fn unchanged(slot: Option<&Val>, value: &Val) -> bool {
    (value.ty == Type::Never && value.pts.is_none()) || slot == Some(value)
}

/// Kinds of effect, for [`Cx::effect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKind {
    Reads,
    Writes,
    Throws,
    CallsUnknown,
    Allocates,
}

/// Whether only `unit` reads the symbol: its own flow-sensitive local, not read by nested
/// units or importers.
fn private_local(info: &crate::world::SymbolInfo, unit: NodeRef) -> bool {
    info.tracked && info.unit == Some(unit) && !info.nested_reads && !info.exported
}

/// Whether `symbol` is a parameter (bound by the call, not hoisted as `undefined`).
fn is_parameter(module: &Module<'_>, symbol: SymbolId) -> bool {
    use oxc_ast::AstKind;
    let declaration = module.semantic.scoping().symbol_declaration(symbol);
    let nodes = module.semantic.nodes();
    std::iter::once(declaration)
        .chain(nodes.ancestor_ids(declaration))
        .take(8)
        .any(|id| {
            matches!(
                nodes.kind(id),
                AstKind::FormalParameter(_) | AstKind::FormalParameterRest(_)
            )
        })
}

/// The escape facts openness depends on.
pub trait EscapeView {
    /// Whether code trioxide does not see may reach the object made at `site`.
    fn escaped(&self, site: AllocId) -> bool;
    /// Whether an instance of `class` may reach code trioxide does not see.
    fn instance_open(&self, class: NodeRef) -> bool;
}

impl EscapeView for State {
    fn escaped(&self, site: AllocId) -> bool {
        self.heap
            .get(&site)
            .is_some_and(|obj| obj.escape == Escape::Unknown)
    }

    fn instance_open(&self, class: NodeRef) -> bool {
        self.open_instances.contains(&class)
    }
}

impl EscapeView for tables::Tables<'_> {
    fn escaped(&self, site: AllocId) -> bool {
        self.obj(site)
            .is_some_and(|obj| obj.escape == Escape::Unknown)
    }

    fn instance_open(&self, class: NodeRef) -> bool {
        self.open_instance(class)
    }
}

/// Whether code trioxide does not see may call `unit`.
pub fn unit_open(world: &World<'_>, view: &impl EscapeView, unit: &Unit<'_>) -> bool {
    match unit.body {
        Body::Module(_) | Body::StaticBlock(_) => false,
        Body::ClassInit(_) => unit
            .class
            .is_none_or(|class| class_open(world, view, class)),
        Body::Function(_) | Body::Arrow(_) => {
            if view.escaped(unit.id) {
                return true;
            }
            if let Some(&class) = world.member_of.get(&unit.id) {
                return class_open(world, view, class);
            }
            false
        }
    }
}

/// Whether `unit` is a function (or a class's field initialisers) that no known caller (or
/// `new`) has reached yet and that unknown code can't call, as `tables` say.
fn uncalled(world: &World<'_>, tables: &tables::Tables<'_>, unit: &Unit<'_>) -> bool {
    matches!(
        unit.body,
        Body::Function(_) | Body::Arrow(_) | Body::ClassInit(_)
    ) && tables.this(unit.id).is_none()
        && tables.params(unit.id).is_none()
        && !tables.spread_called(unit.id)
        && !unit_open(world, tables, unit)
}

/// Whether unknown code may construct the class or reach its instances or prototype.
pub fn class_open(world: &World<'_>, view: &impl EscapeView, class: NodeRef) -> bool {
    let Some(info) = world.classes.get(&class) else {
        return true;
    };
    view.escaped(class) || view.escaped(info.init) || view.instance_open(class)
}
