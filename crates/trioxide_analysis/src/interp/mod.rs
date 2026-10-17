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

use oxc_span::{GetSpan, Span};
use oxc_syntax::symbol::SymbolId;
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::{ModuleId, NodeId, NodeRef, SymbolRef};
use trioxide_types::{Type, TypeOracle, TypedArrayKind};

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

/// Rounds after which numeric facts in the whole-program tables are widened.
const WIDEN_AFTER: usize = 3;
/// Rounds after which the analysis gives up and reports nothing.
const MAX_ROUNDS: usize = 40;

/// After this many rounds, a whole-program entry that still changes is given up: set to "any
/// value" (and its objects to "anything may happen to them"), which nothing can change
/// further, so the run reaches a (less precise) fixed point.
const SATURATE_AFTER: usize = 20;
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

/// Who read what, and which units must run again.
#[derive(Default)]
struct Deps {
    current: Option<NodeRef>,
    /// What the current unit has read in this run.
    current_reads: FxHashSet<Dep>,
    readers: FxHashMap<Dep, Vec<NodeRef>>,
    seen: FxHashSet<(Dep, NodeRef)>,
    dirty: FxHashSet<NodeRef>,
    /// Past [`SATURATE_AFTER`] rounds: the entries that changed since the last saturation.
    saturating: Option<Vec<Dep>>,
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
#[derive(Debug, Clone)]
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
}

impl Obj {
    fn new(kind: ObjKind, unit: NodeRef) -> Self {
        Self {
            kind,
            unit,
            props: FxHashMap::default(),
            order: Vec::new(),
            present: FxHashSet::default(),
            elem: Val::never(),
            length: None,
            holey: false,
            dynamic: false,
            deleted: false,
            poisoned: false,
            added: FxHashSet::default(),
            escape: Escape::None,
            shares_buffer: Vec::new(),
        }
    }
}

/// The whole-program tables, which only grow.
#[derive(Default)]
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
    /// Allocation sites.
    pub heap: FxHashMap<AllocId, Obj>,
    /// The value each class extends.
    pub supers: FxHashMap<NodeRef, Val>,
    /// The value of each `export default <expression>`, by expression node.
    pub defaults: FxHashMap<NodeRef, Val>,
    /// Classes with an instance site that escapes to unknown code.
    pub open_instances: FxHashSet<NodeRef>,
    /// `let`, `const` and `class` bindings that may be read before their declaration runs:
    /// their unit made a call (which may read them) before reaching it.
    pub may_be_uninit: FxHashSet<SymbolRef>,
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
    /// Each evaluated expression's own effects and the units it calls, where it has any.
    pub expr_effects: FxHashMap<NodeRef, ExprRecord>,
    /// What each loop does.
    pub loops: FxHashMap<NodeRef, LoopRecord>,
    /// Statements reached with a reachable environment.
    pub reached: FxHashSet<NodeRef>,
    /// Statements interpreted (or skipped) with an unreachable one.
    pub skipped: FxHashSet<NodeRef>,
}

/// An expression's own effects (without its callees') and the units it calls.
#[derive(Debug, Clone, Default)]
pub struct ExprRecord {
    /// Counting only proven facts.
    pub proven: Effects,
    /// Counting declared facts as proof.
    pub declared: Effects,
    /// Units it calls (directly or through implicit calls).
    pub callees: Vec<NodeRef>,
}

impl ExprRecord {
    fn join(&mut self, other: &Self) {
        self.proven = union(self.proven, other.proven);
        self.declared = union(self.declared, other.declared);
        for callee in &other.callees {
            if !self.callees.contains(callee) {
                self.callees.push(*callee);
            }
        }
    }
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
}

impl LoopRecord {
    fn join(&mut self, other: &Self) {
        self.proven = union(self.proven, other.proven);
        self.declared = union(self.declared, other.declared);
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
}

impl Frame {
    /// A frame whose edges start at `edges`.
    pub fn new(edges: usize) -> Self {
        Self {
            proven: Effects::PURE,
            declared: Effects::PURE,
            edges,
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
}

impl Output {
    fn merge(&mut self, other: Self) {
        for (node, value) in other.nodes {
            match self.nodes.entry(node) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let joined = entry.get().join(&value);
                    entry.insert(joined);
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(value);
                }
            }
        }
        for (node, trust) in other.in_bounds {
            self.in_bounds
                .entry(node)
                .and_modify(|old| *old = (*old).min(trust))
                .or_insert(trust);
        }
        self.units.extend(other.units);
    }
}

/// The analysis's results once the tables are stable.
pub struct Results {
    /// The stable tables.
    pub state: State,
    /// Every unit's last recordings, merged.
    pub output: Output,
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
    /// Type oracle queries (cache misses).
    pub oracle_queries: usize,
    /// Time spent in the oracle.
    pub oracle_time: std::time::Duration,
    /// Total interpretation time.
    pub total_time: std::time::Duration,
    /// Whole-program entries given up (set to "any value") because they kept changing.
    pub saturated: usize,
}

/// Runs the interpreter to a fixed point.
pub fn run(
    world: &World<'_>,
    links: &Links,
    oracle: &mut dyn TypeOracle,
    options: AnalysisOptions,
) -> Results {
    let open_modules = open_modules(world, &options);
    let lexicals = lexicals(world);
    let import_cycles = import_cycles(world);
    let mut analyzer = Analyzer {
        world,
        links,
        oracle,
        options,
        state: State::default(),
        output: Output::default(),
        round: 0,
        deps: std::cell::RefCell::default(),
        outputs: FxHashMap::default(),
        ts_cache: FxHashMap::default(),
        patched: patched_builtins(world),
        open_modules,
        lexicals,
        import_cycles,
        saturated: 0,
        stats: Stats::default(),
    };
    let started = std::time::Instant::now();
    let mut converged = false;
    analyzer
        .deps
        .get_mut()
        .dirty
        .extend(world.unit_order.iter().copied());
    analyzer.stats.units = world.unit_order.len();
    while analyzer.round < MAX_ROUNDS {
        if analyzer.round == SATURATE_AFTER {
            analyzer.deps.get_mut().saturating = Some(Vec::new());
        }
        for &unit in &world.unit_order {
            if !analyzer.deps.get_mut().dirty.remove(&unit) {
                continue;
            }
            let unit = world.unit(unit);
            if world.module(unit.id.module).broken {
                continue;
            }
            analyzer.stats.interpretations += 1;
            analyzer.interpret(unit);
            analyzer.saturate();
        }
        analyzer.round += 1;
        if analyzer.deps.get_mut().dirty.is_empty() {
            converged = true;
            break;
        }
    }
    analyzer.stats.rounds = analyzer.round;
    analyzer.stats.saturated = analyzer.saturated;
    analyzer.stats.total_time = started.elapsed();
    let mut output = Output::default();
    for unit_output in analyzer.outputs.into_values() {
        output.merge(unit_output);
    }
    Results {
        state: analyzer.state,
        output,
        converged,
        stats: analyzer.stats,
    }
}

/// Builtin namespaces (`Math`, …) whose models can't be trusted: the program shadows them,
/// writes to them, or lets the namespace object (or `NS.prototype`, or the global object)
/// flow anywhere other than a plain `NS.name` read, call or construction, where it could be
/// patched. `"TypedArray"` stands for the typed array constructors and their shared
/// prototype. Any way of reaching a builtin prototype without naming it (`__proto__`,
/// `Object.getPrototypeOf`, `.constructor`, `X.prototype` of anything but a program class,
/// `Reflect`, `Function`, `eval`) turns every model off.
fn patched_builtins(world: &World<'_>) -> FxHashSet<String> {
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
fn open_modules(world: &World<'_>, options: &AnalysisOptions) -> FxHashSet<ModuleId> {
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

pub(crate) struct Analyzer<'w, 'a> {
    pub world: &'w World<'a>,
    pub links: &'w Links,
    pub oracle: &'w mut dyn TypeOracle,
    pub options: AnalysisOptions,
    pub state: State,
    pub output: Output,
    pub round: usize,
    deps: std::cell::RefCell<Deps>,
    outputs: FxHashMap<NodeRef, Output>,
    ts_cache: FxHashMap<NodeRef, Option<Type>>,
    pub patched: FxHashSet<String>,
    /// Modules whose exports code outside the program may use (see [`open_modules`]).
    pub open_modules: FxHashSet<ModuleId>,
    /// Each unit's own `let`, `const` and `class` bindings.
    pub lexicals: FxHashMap<NodeRef, Vec<SymbolId>>,
    /// Each module's strongly connected component of the import graph (by module index).
    pub import_cycles: Vec<usize>,
    /// How many entries were given up (see [`SATURATE_AFTER`]).
    saturated: usize,
    stats: Stats,
}

/// A unit's flow-sensitive state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Env {
    /// Whether this point is reachable.
    pub reachable: bool,
    /// Values of tracked locals.
    pub vars: FxHashMap<SymbolId, Val>,
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
        for (symbol, value) in &other.vars {
            vars.entry(*symbol)
                .and_modify(|existing| {
                    if existing != value {
                        *existing = existing.join(value);
                    }
                })
                .or_insert_with(|| value.clone());
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
        for (symbol, value) in &mut joined.vars {
            if let Some(old) = self.vars.get(symbol) {
                value.ty = numeric::widen_type(&old.ty, &value.ty);
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
    /// Arrays whose length relations were dropped, in order.
    pub dropped: Vec<SymbolId>,
}

impl<'w, 'a> Analyzer<'w, 'a> {
    fn interpret(&mut self, unit: &'w Unit<'a>) {
        let deps = self.deps.get_mut();
        deps.current = Some(unit.id);
        deps.current_reads.clear();
        // Losses from outside an interpretation (summaries, merges) don't concern this unit.
        let _ = crate::value::take_lost();
        let capacity = self
            .outputs
            .get(&unit.id)
            .map_or(0, |output| output.nodes.len());
        self.output = Output::default();
        self.output.nodes.reserve(capacity);
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
            dropped: Vec::new(),
        };
        cx.enter();
        match unit.body {
            Body::Module(program) => {
                cx.hoist(&program.body);
                cx.statements(&program.body);
            }
            Body::Function(function) => {
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
        let output = std::mem::take(&mut self.output);
        self.outputs.insert(unit.id, output);
        self.deps.get_mut().current = None;
    }

    /// Notes that the current unit read `dep`.
    pub fn read(&self, dep: Dep) {
        let mut deps = self.deps.borrow_mut();
        let Some(unit) = deps.current else { return };
        if deps.current_reads.insert(dep) && deps.seen.insert((dep, unit)) {
            deps.readers.entry(dep).or_default().push(unit);
        }
    }

    /// Gives up the entries that changed while saturating (see [`SATURATE_AFTER`]).
    fn saturate(&mut self) {
        loop {
            let Some(changed) = self.deps.get_mut().saturating.as_mut() else {
                return;
            };
            if changed.is_empty() {
                return;
            }
            let mut changed = std::mem::take(changed);
            changed.sort_unstable();
            changed.dedup();
            for dep in changed {
                self.give_up(dep);
            }
        }
    }

    /// Sets the entry `dep` to the top of the lattice (its old objects escape).
    fn give_up(&mut self, dep: Dep) {
        let top = |slot: &mut Val, lost: &mut Vec<Pts>| {
            if slot.trust != Trust::Unknown || slot.pts.sites().is_some() {
                lost.push(std::mem::take(&mut slot.pts));
                *slot = Val::unknown();
                true
            } else {
                false
            }
        };
        let mut lost = Vec::new();
        let changed = match dep {
            Dep::Symbol(symbol) => self
                .state
                .symbols
                .get_mut(&symbol)
                .is_some_and(|slot| top(slot, &mut lost)),
            Dep::Params(unit) => {
                let mut changed = self.state.spread_called.insert(unit);
                if let Some(params) = self.state.params.get_mut(&unit) {
                    for slot in params {
                        changed |= top(slot, &mut lost);
                    }
                }
                changed
            }
            Dep::This(unit) => self
                .state
                .this
                .get_mut(&unit)
                .is_some_and(|slot| top(slot, &mut lost)),
            Dep::Return(unit) => self
                .state
                .returns
                .get_mut(&unit)
                .is_some_and(|slot| top(slot, &mut lost)),
            Dep::Super(class) => self
                .state
                .supers
                .get_mut(&class)
                .is_some_and(|slot| top(slot, &mut lost)),
            Dep::Default(node) => self
                .state
                .defaults
                .get_mut(&node)
                .is_some_and(|slot| top(slot, &mut lost)),
            Dep::Heap(site) => {
                lost.push(Pts::site(site));
                self.state.heap.get_mut(&site).is_some_and(|obj| {
                    let changed = !obj.dynamic || obj.length.is_some() || !obj.holey;
                    obj.dynamic = true;
                    obj.length = None;
                    obj.holey = true;
                    changed
                })
            }
        };
        if changed {
            self.saturated += 1;
            self.touch(dep);
        }
        for pts in lost {
            self.escape(&pts, Escape::Unknown);
        }
    }

    /// Notes that `dep` changed: every unit that read it runs again.
    pub fn touch(&self, dep: Dep) {
        let mut deps = self.deps.borrow_mut();
        let Deps {
            current,
            current_reads,
            readers,
            dirty,
            saturating,
            ..
        } = &mut *deps;
        if let Some(changed) = saturating {
            changed.push(dep);
        }
        if let Some(units) = readers.get(&dep) {
            // The current unit's result depends only on what this run read so far.
            let skip = current.filter(|_| !current_reads.contains(&dep));
            dirty.extend(units.iter().copied().filter(|&unit| Some(unit) != skip));
        }
    }

    /// The heap entry of `site`, as a dependency of the current unit.
    pub fn heap(&self, site: AllocId) -> Option<&Obj> {
        self.read(Dep::Heap(site));
        self.state.heap.get(&site)
    }

    /// Every value `symbol` holds, as a dependency of the current unit.
    pub fn symbol_value(&self, symbol: SymbolRef) -> Option<&Val> {
        self.read(Dep::Symbol(symbol));
        self.state.symbols.get(&symbol)
    }

    /// What `unit` returns, as a dependency of the current unit.
    pub fn returned(&self, unit: NodeRef) -> Option<&Val> {
        self.read(Dep::Return(unit));
        self.state.returns.get(&unit)
    }

    /// What `class` extends, as a dependency of the current unit.
    pub fn super_of(&self, class: NodeRef) -> Option<&Val> {
        self.read(Dep::Super(class));
        self.state.supers.get(&class)
    }

    /// The value of an `export default` expression, as a dependency of the current unit.
    pub fn default_value(&self, node: NodeRef) -> Option<&Val> {
        self.read(Dep::Default(node));
        self.state.defaults.get(&node)
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
            .state
            .this
            .get(&unit.id)
            .cloned()
            .unwrap_or_else(Val::never);
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
        unit_open(self.world, &self.state, unit)
    }

    /// Joins into a whole-program table entry, noting whether it changed.
    fn join_into(round: usize, slot: &mut Val, value: &Val) -> bool {
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

    fn join_symbol(&mut self, symbol: SymbolRef, value: &Val) {
        let slot = self.state.symbols.entry(symbol).or_insert_with(Val::never);
        if Self::join_into(self.round, slot, value) {
            self.touch(Dep::Symbol(symbol));
        }
    }

    fn join_return(&mut self, unit: NodeRef, value: &Val) {
        let slot = self.state.returns.entry(unit).or_insert_with(Val::never);
        if Self::join_into(self.round, slot, value) {
            self.touch(Dep::Return(unit));
        }
    }

    fn join_this(&mut self, unit: NodeRef, value: &Val) {
        let slot = self.state.this.entry(unit).or_insert_with(Val::never);
        if Self::join_into(self.round, slot, value) {
            self.touch(Dep::This(unit));
        }
    }

    fn join_param(&mut self, unit: NodeRef, index: usize, value: &Val) {
        let params = self.state.params.entry(unit).or_default();
        let mut changed = false;
        if params.len() <= index {
            params.resize_with(index + 1, Val::never);
            changed = true;
        }
        if Self::join_into(self.round, &mut params[index], value) || changed {
            self.touch(Dep::Params(unit));
        }
    }

    fn join_super(&mut self, class: NodeRef, value: &Val) {
        let slot = self.state.supers.entry(class).or_insert_with(Val::never);
        if Self::join_into(self.round, slot, value) {
            self.touch(Dep::Super(class));
        }
    }

    /// Joins into the value of an `export default` expression.
    pub fn join_default(&mut self, node: NodeRef, value: &Val) {
        let slot = self.state.defaults.entry(node).or_insert_with(Val::never);
        if Self::join_into(self.round, slot, value) {
            self.touch(Dep::Default(node));
        }
    }

    /// The heap entry of `site`, created (empty) on first use.
    fn obj(&mut self, site: AllocId, kind: ObjKind, unit: NodeRef) -> &mut Obj {
        if !self.state.heap.contains_key(&site) {
            self.touch(Dep::Heap(site));
        }
        self.state
            .heap
            .entry(site)
            .or_insert_with(|| Obj::new(kind, unit))
    }

    /// Marks every site in `pts` (and what it reaches) as reaching code that may redefine its
    /// properties arbitrarily.
    pub fn poison(&mut self, pts: &Pts) {
        self.escape(pts, Escape::Unknown);
        for &site in pts.sites().unwrap_or_default() {
            if let Some(obj) = self.state.heap.get_mut(&site)
                && !obj.poisoned
            {
                obj.poisoned = true;
                self.touch(Dep::Heap(site));
            }
        }
    }

    /// Marks every site in `pts` (and what it reaches) as escaping at least to `level`.
    pub fn escape(&mut self, pts: &Pts, level: Escape) {
        let Some(sites) = pts.sites() else {
            return;
        };
        let mut pending: Vec<AllocId> = sites.to_vec();
        while let Some(site) = pending.pop() {
            let Some(obj) = self.state.heap.get_mut(&site) else {
                continue;
            };
            if obj.escape >= level {
                continue;
            }
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
            self.touch(Dep::Heap(site));
            if level == Escape::Unknown
                && let ObjKind::Instance(class) = kind
                && self.state.open_instances.insert(class)
            {
                self.touch(Dep::Heap(class));
            }
            // A namespace object reaches every export.
            if let ObjKind::Namespace(module) = kind {
                let mut exported = Vec::new();
                self.exported_values(module, &mut exported, &mut FxHashSet::default());
                pending.extend(exported);
                continue;
            }
            // A class's prototype, constructor and methods go with it, and so do an
            // instance's (`instance.constructor` reaches the class).
            if let ObjKind::Class(class) | ObjKind::Instance(class) | ObjKind::Prototype(class) =
                kind
                && let Some(info) = self.world.classes.get(&class)
            {
                pending.push(class);
                pending.push(info.init);
                pending.extend(info.members.iter().filter_map(|member| member.unit));
                pending.extend(info.constructor);
            }
        }
    }

    /// The allocation sites every export of `module` (and of what it re-exports) may hold.
    fn exported_values(
        &self,
        module: ModuleId,
        out: &mut Vec<AllocId>,
        seen: &mut FxHashSet<ModuleId>,
    ) {
        if !seen.insert(module) {
            return;
        }
        let info = self.world.module(module);
        for (index, symbol) in info.symbols.iter().enumerate() {
            if symbol.exported
                && let Some(value) = self.symbol_value(info.symbol(SymbolId::from_usize(index)))
                && let Some(sites) = value.pts.sites()
            {
                out.extend_from_slice(sites);
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
                self.exported_values(target, out, seen);
            }
        }
        // `export default <expression>` values and re-exported imports.
        for (import, target) in self.links.imports() {
            if import.module == module
                && info.info(import.symbol).exported
                && let crate::link::Target::Symbol(target) = target
                && let Some(sites) = self
                    .symbol_value(target)
                    .and_then(|value| value.pts.sites())
            {
                out.extend_from_slice(sites);
            }
        }
        if let crate::link::Target::Expression(node) =
            crate::link::resolve_export(self.world, info, "default", &mut FxHashSet::default())
            && let Some(sites) = self.default_value(node).and_then(|value| value.pts.sites())
        {
            out.extend_from_slice(sites);
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
        let ty = self.oracle.type_at(node.module, span);
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
        self.an
            .output
            .nodes
            .entry(key)
            .and_modify(|existing| *existing = existing.join(value))
            .or_insert_with(|| value.clone());
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
            EffectKind::CallsUnknown => effects.calls_unknown = true,
            EffectKind::Allocates => effects.allocates = true,
        };
        // An expression's own effects count whether or not a `try` catches them.
        for frame in &mut self.frames {
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
            // `declare`d: defined by code outside the program.
            self.always(EffectKind::Reads);
            return Val::unknown();
        }
        if info.import {
            return self.read_import(self.module.symbol(symbol));
        }
        if info.lexical {
            self.note_tdz_read(symbol);
        }
        if info.tracked && info.unit == Some(self.unit.id) {
            if let Some(value) = self.env.vars.get(&symbol) {
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
    /// elsewhere, when its unit may call out before reaching the declaration.
    fn note_tdz_read(&mut self, symbol: SymbolId) {
        let info = self.module.info(symbol);
        let throws = if info.unit == Some(self.unit.id) {
            info.in_switch || self.position < info.declared_at
        } else {
            let symbol = self.module.symbol(symbol);
            self.an.read(Dep::Symbol(symbol));
            self.an.state.may_be_uninit.contains(&symbol)
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
            if self.an.state.may_be_uninit.insert(symbol) {
                self.an.touch(Dep::Symbol(symbol));
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
        self.an.join_symbol(self.module.symbol(symbol), &value);
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
        site
    }

    /// Writes `value` to property `key` of every object in `site`'s heap entry.
    pub fn store_prop(&mut self, site: AllocId, key: &str, value: &Val, creating: bool) {
        let round = self.an.round;
        let Some(obj) = self.an.state.heap.get_mut(&site) else {
            return;
        };
        let mut changed = false;
        if !obj.props.contains_key(key) {
            obj.order.push(key.to_string());
            changed = true;
        }
        let slot = obj.props.entry(key.to_string()).or_insert_with(Val::never);
        changed |= Analyzer::join_into(round, slot, value);
        // A key the object didn't have when it was made (even if the code that made it adds
        // it, it does so later): its shape changes.
        if !creating && !obj.present.contains(key) && obj.added.insert(key.to_string()) {
            changed = true;
        }
        let level = obj.escape.max(Escape::Known);
        if changed {
            self.an.touch(Dep::Heap(site));
        }
        self.an.escape(&value.pts, level);
    }

    /// Joins `value` into the elements of the array at `site`.
    pub fn store_elem(&mut self, site: AllocId, value: &Val) {
        let round = self.an.round;
        let Some(obj) = self.an.state.heap.get_mut(&site) else {
            return;
        };
        let changed = Analyzer::join_into(round, &mut obj.elem, value);
        let level = obj.escape.max(Escape::Known);
        if changed {
            self.an.touch(Dep::Heap(site));
        }
        self.an.escape(&value.pts, level);
    }

    /// Updates a heap entry's flags.
    pub fn mark(&mut self, site: AllocId, update: impl FnOnce(&mut Obj)) {
        self.an.read(Dep::Heap(site));
        if let Some(obj) = self.an.state.heap.get_mut(&site) {
            let before = (
                obj.dynamic,
                obj.deleted,
                obj.holey,
                obj.length,
                obj.present.len(),
                obj.shares_buffer.len(),
            );
            update(obj);
            if before
                != (
                    obj.dynamic,
                    obj.deleted,
                    obj.holey,
                    obj.length,
                    obj.present.len(),
                    obj.shares_buffer.len(),
                )
            {
                self.an.touch(Dep::Heap(site));
            }
        }
    }
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

/// Whether code trioxide does not see may call `unit`.
pub fn unit_open(world: &World<'_>, state: &State, unit: &Unit<'_>) -> bool {
    let escaped = |site: AllocId| {
        state
            .heap
            .get(&site)
            .is_some_and(|obj| obj.escape == Escape::Unknown)
    };
    match unit.body {
        Body::Module(_) | Body::StaticBlock(_) => false,
        Body::ClassInit(_) => unit
            .class
            .is_none_or(|class| class_open(world, state, class)),
        Body::Function(_) | Body::Arrow(_) => {
            if escaped(unit.id) {
                return true;
            }
            if let Some(&class) = world.member_of.get(&unit.id) {
                return class_open(world, state, class);
            }
            false
        }
    }
}

/// Whether unknown code may construct the class or reach its instances or prototype.
pub fn class_open(world: &World<'_>, state: &State, class: NodeRef) -> bool {
    let Some(info) = world.classes.get(&class) else {
        return true;
    };
    let escaped = |site: &AllocId| {
        state
            .heap
            .get(site)
            .is_some_and(|obj| obj.escape == Escape::Unknown)
    };
    escaped(&class) || escaped(&info.init) || state.open_instances.contains(&class)
}
