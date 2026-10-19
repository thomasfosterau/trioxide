//! The program as the analysis sees it: every module parsed with oxc and indexed into code
//! units, classes and per-symbol facts that don't depend on values.

use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::{
    AstKind,
    ast::{
        ArrowFunctionExpression, Class, ClassElement, Expression, Function, MethodDefinitionKind,
        Program, PropertyKey, StaticBlock,
    },
};
use oxc_parser::Parser;
use oxc_semantic::{Semantic, SemanticBuilder};
use oxc_span::{GetSpan, Span};
use oxc_syntax::{
    module_record::ModuleRecord,
    scope::{ScopeFlags, ScopeId},
    symbol::{SymbolFlags, SymbolId},
};
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::{ModuleId, NodeId, NodeRef, SymbolRef};

use crate::ModuleInput;

/// The code a unit runs.
#[derive(Clone, Copy)]
pub enum Body<'a> {
    /// A module's top level.
    Module(&'a Program<'a>),
    /// A function, method, getter, setter or constructor.
    Function(&'a Function<'a>),
    /// An arrow function.
    Arrow(&'a ArrowFunctionExpression<'a>),
    /// A class `static { }` block.
    StaticBlock(&'a StaticBlock<'a>),
    /// A class's instance field initialisers, run by its constructor.
    ClassInit(&'a Class<'a>),
}

/// How a function unit sits in a class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodRole {
    /// The `constructor`.
    Constructor,
    /// A method.
    Method,
    /// A getter.
    Getter,
    /// A setter.
    Setter,
}

/// A piece of code that runs as a whole: the unit analyses are summarised by.
pub struct Unit<'a> {
    /// The unit's node: the program, function, arrow, static block, or class body.
    pub id: NodeRef,
    /// Its code.
    pub body: Body<'a>,
    /// The unit it is nested in.
    pub parent: Option<NodeRef>,
    /// The class it belongs to (methods, constructors, field initialisers, static blocks).
    pub class: Option<NodeRef>,
    /// Its role in the class, for function units.
    pub role: Option<MethodRole>,
    /// Whether it is a `static` class member.
    #[expect(
        dead_code,
        reason = "descriptive world data, not read by the interpreter yet"
    )]
    pub is_static: bool,
    /// A human-readable name.
    pub name: Option<String>,
    /// The unit's span.
    pub span: Span,
    /// Whether the code is strict-mode.
    pub strict: bool,
    /// Whether it refers to its own `arguments`.
    pub uses_arguments: bool,
}

impl Unit<'_> {
    /// Whether `this` is lexical (arrow functions).
    pub fn lexical_this(&self) -> bool {
        matches!(self.body, Body::Arrow(_))
    }

    /// Whether the unit is a generator or async function.
    pub fn is_async_or_generator(&self) -> bool {
        match self.body {
            Body::Function(function) => function.r#async || function.generator,
            Body::Arrow(arrow) => arrow.r#async,
            _ => false,
        }
    }
}

/// A member of a class body.
pub struct Member {
    /// The key (`#x` for private names).
    pub name: String,
    /// The member's unit, for methods and accessors.
    pub unit: Option<NodeRef>,
    /// Methods, getters and setters have roles; fields don't.
    pub role: Option<MethodRole>,
    /// `static`.
    pub is_static: bool,
    /// The member's node.
    #[expect(
        dead_code,
        reason = "descriptive world data, not read by the interpreter yet"
    )]
    pub node: NodeId,
}

/// A class declaration or expression.
pub struct ClassInfo<'a> {
    /// The class node.
    pub id: NodeRef,
    /// The AST.
    pub class: &'a Class<'a>,
    /// The class's name, if any.
    #[expect(
        dead_code,
        reason = "descriptive world data, not read by the interpreter yet"
    )]
    pub name: Option<String>,
    /// The `constructor` unit, if written.
    pub constructor: Option<NodeRef>,
    /// The unit that runs the instance field initialisers.
    pub init: NodeRef,
    /// Members with static keys.
    pub members: Vec<Member>,
    /// Whether some member has a computed key (its name is unknown).
    pub computed_members: bool,
    /// Whether the class has an `extends` clause.
    pub has_heritage: bool,
    /// Whether the constructor (if written) passes `this` on only in ways the interpreter
    /// follows ([`crate::shapes::keeps_this`]): constructing an instance then doesn't by
    /// itself make it escape.
    pub constructor_keeps_this: bool,
}

impl ClassInfo<'_> {
    /// The non-static member called `name`.
    pub fn instance_member(&self, name: &str) -> Option<&Member> {
        self.members
            .iter()
            .find(|member| !member.is_static && member.name == name)
    }
}

/// Per-symbol facts that don't depend on values.
#[derive(Debug, Clone, Default)]
#[expect(clippy::struct_excessive_bools, reason = "independent flags")]
pub struct SymbolInfo {
    /// The unit whose scope declares the symbol.
    pub unit: Option<NodeRef>,
    /// Every write is in `unit` and nothing else (eval, `with`, sloppy `arguments`) can
    /// change it: the unit can follow its value flow-sensitively.
    pub tracked: bool,
    /// A direct `eval` or `with` can see it: its value is unknowable.
    pub tainted: bool,
    /// Some other unit reads it.
    pub nested_reads: bool,
    /// Function-scoped (`var`, a function declaration or a parameter): readable as
    /// `undefined` before its first write.
    #[expect(
        dead_code,
        reason = "descriptive world data, not read by the interpreter yet"
    )]
    pub hoisted: bool,
    /// An import binding.
    pub import: bool,
    /// Exported from its module.
    pub exported: bool,
    /// A `let`, `const` or `class` binding: reading it before its declaration runs throws.
    pub lexical: bool,
    /// Where its declaration ends (byte offset): reads in its unit before that are in its
    /// temporal dead zone.
    pub declared_at: u32,
    /// Declared in a `switch` case, whose scope spans every case: textual order says nothing
    /// about initialisation there.
    pub in_switch: bool,
}

/// One parsed module.
pub struct Module<'a> {
    /// The module's id.
    pub id: ModuleId,
    /// Its path.
    pub path: &'a Path,
    /// Its source.
    #[expect(
        dead_code,
        reason = "descriptive world data, not read by the interpreter yet"
    )]
    pub source: &'a str,
    /// The AST.
    pub program: &'a Program<'a>,
    /// oxc's semantic model of it.
    pub semantic: Semantic<'a>,
    /// Imports and exports.
    pub record: ModuleRecord<'a>,
    /// Where each import specifier resolved to.
    pub imports: FxHashMap<String, Option<ModuleId>>,
    /// Facts per symbol, indexed by `SymbolId`.
    pub symbols: Vec<SymbolInfo>,
    /// Whether the module had syntax errors (it is then not analysed).
    pub broken: bool,
    /// `unit_of` answers (filled while the world is built).
    unit_cache: std::sync::Mutex<FxHashMap<NodeId, NodeRef>>,
}

impl<'a> Module<'a> {
    /// The node `id`'s kind.
    pub fn kind(&self, id: NodeId) -> AstKind<'a> {
        self.semantic.nodes().kind(id)
    }

    /// The global id of a node in this module.
    pub fn node(&self, node: NodeId) -> NodeRef {
        NodeRef {
            module: self.id,
            node,
        }
    }

    /// The global id of a symbol in this module.
    pub fn symbol(&self, symbol: SymbolId) -> SymbolRef {
        SymbolRef {
            module: self.id,
            symbol,
        }
    }

    /// The facts of `symbol`.
    pub fn info(&self, symbol: SymbolId) -> &SymbolInfo {
        &self.symbols[symbol.index()]
    }

    /// Whether `symbol` is declared only by ambient (`declare`) declarations or function
    /// overload signatures, so its value comes from outside the program.
    pub fn is_ambient(&self, symbol: SymbolId) -> bool {
        let scoping = self.semantic.scoping();
        let nodes = self.semantic.nodes();
        let declaration = scoping.symbol_declaration(symbol);
        std::iter::once(declaration)
            .chain(nodes.ancestor_ids(declaration))
            .take(4)
            .any(|id| match nodes.kind(id) {
                AstKind::Function(function) => function.declare || function.body.is_none(),
                AstKind::VariableDeclaration(declaration) => declaration.declare,
                AstKind::Class(class) => class.declare,
                AstKind::TSEnumDeclaration(declaration) => declaration.declare,
                AstKind::TSNamespaceDeclaration(declaration) => declaration.declare,
                _ => false,
            })
    }

    /// Whether code the compiler generates (rather than the program's own source) imports
    /// this module: the automatic JSX runtime.
    pub fn called_from_generated_code(&self) -> bool {
        self.path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| matches!(stem, "jsx-runtime" | "jsx-dev-runtime"))
    }

    /// The symbol a module-scope binding called `name` refers to.
    pub fn root_binding(&self, name: &str) -> Option<SymbolId> {
        self.semantic.scoping().get_root_binding(name.into())
    }

    /// The unit whose code contains `node` (`node` itself if it is a unit and `include_self`).
    pub fn unit_of(&self, node: NodeId, include_self: bool) -> NodeRef {
        if !include_self && let Some(&unit) = self.cache().get(&node) {
            return unit;
        }
        let nodes = self.semantic.nodes();
        let start = nodes.kind(node).span();
        let mut found = None;
        let candidates = std::iter::once(node)
            .filter(|_| include_self)
            .chain(nodes.ancestor_ids(node));
        let mut previous = node;
        for id in candidates {
            match nodes.kind(id) {
                AstKind::Function(function) if function.body.is_some() => {
                    // A function's own name binding and its params' scope are its own; only its
                    // body and params belong to it, but references in them are inside anyway.
                    found = Some(id);
                }
                AstKind::ArrowFunctionExpression(_)
                | AstKind::StaticBlock(_)
                | AstKind::Program(_) => found = Some(id),
                AstKind::PropertyDefinition(property)
                    if !property.r#static
                        && property
                            .value
                            .as_ref()
                            .is_some_and(|value| value.span().contains_inclusive(start)) =>
                {
                    // Instance field initialisers run in the class's init unit (its body node).
                    found = Some(nodes.parent_id(id));
                }
                AstKind::AccessorProperty(property)
                    if !property.r#static
                        && property
                            .value
                            .as_ref()
                            .is_some_and(|value| value.span().contains_inclusive(start)) =>
                {
                    found = Some(nodes.parent_id(id));
                }
                _ => {}
            }
            if found.is_some() {
                break;
            }
            previous = id;
        }
        let _ = previous;
        let unit = self.node(found.unwrap_or_else(|| self.program.node_id.get()));
        if !include_self {
            self.cache().insert(node, unit);
        }
        unit
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, FxHashMap<NodeId, NodeRef>> {
        self.unit_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The unit whose scope is `scope` (or contains it).
    pub fn unit_of_scope(&self, scope: ScopeId) -> NodeRef {
        let node = self.semantic.scoping().get_node_id(scope);
        self.unit_of(node, true)
    }
}

/// Parses and builds semantic data for one module.
fn parse<'a>(allocator: &'a Allocator, input: &'a ModuleInput<'a>) -> Module<'a> {
    let parsed = Parser::new(allocator, input.source, input.source_type).parse();
    let broken = parsed.fatal_error || !parsed.diagnostics.is_empty();
    let program: &'a Program<'a> = allocator.alloc(parsed.program);
    let semantic = SemanticBuilder::new()
        .with_enum_eval(true)
        .with_build_nodes(true)
        .build(program)
        .semantic;
    let symbols = vec![SymbolInfo::default(); semantic.scoping().symbols_len()];
    Module {
        id: input.id,
        path: input.path,
        source: input.source,
        program,
        semantic,
        record: parsed.module_record,
        imports: input
            .imports
            .iter()
            .map(|(specifier, target)| (specifier.clone(), *target))
            .collect(),
        symbols,
        broken,
        unit_cache: std::sync::Mutex::default(),
    }
}

/// The class and function declarations of `module`, keyed by name span (or node span when
/// anonymous).
fn declarations(module: &Module<'_>) -> Vec<(Span, NodeId)> {
    let mut declarations = Vec::new();
    for node in module.semantic.nodes().iter() {
        let span = match node.kind() {
            AstKind::Function(function) => function.id.as_ref().map_or(function.span, |id| id.span),
            AstKind::ArrowFunctionExpression(arrow) => arrow.span,
            AstKind::Class(class) => class.id.as_ref().map_or(class.span, |id| id.span),
            _ => continue,
        };
        declarations.push((span, node.id()));
    }
    declarations
}

/// What indexing one module found.
struct Indexed<'a> {
    units: Vec<Unit<'a>>,
    classes: Vec<ClassInfo<'a>>,
    symbols: Vec<SymbolInfo>,
    declarations: Vec<(Span, NodeId)>,
    unit_symbols: FxHashMap<NodeRef, Vec<SymbolId>>,
}

/// Modules sent to another thread to be freed.
struct Parsed<'a>(#[expect(dead_code, reason = "held only to be dropped")] Vec<Module<'a>>);

// SAFETY: freeing a module frees only its own heap data (its AST stays in its allocator, which
// outlives it); oxc's semantic data keeps nothing thread-local.
unsafe impl Send for Parsed<'_> {}

/// Modules parsed and indexed on another thread.
struct Built<'a>(Vec<(Module<'a>, Indexed<'a>)>);

// SAFETY: a module's AST lives in an allocator the building thread borrowed exclusively and
// gives up when it returns the module; oxc's semantic data refers only to that AST and its own
// arena, and keeps nothing thread-local; an index refers only to its module's AST.
unsafe impl Send for Built<'_> {}

/// Parses and indexes one module.
fn build_module<'a>(
    allocator: &'a Allocator,
    input: &'a ModuleInput<'a>,
) -> (Module<'a>, Indexed<'a>) {
    let module = parse(allocator, input);
    let indexed = World::index_module(&module);
    (module, indexed)
}

/// Parses and indexes every module, in order: the modules split into as many runs as there
/// are allocators, each run into its allocator, on a thread of its own.
fn build_all<'a>(
    allocators: &'a mut [Allocator],
    inputs: &'a [ModuleInput<'a>],
) -> Vec<(Module<'a>, Indexed<'a>)> {
    if allocators.len() <= 1 {
        let Some(allocator) = allocators.first() else {
            return Vec::new();
        };
        return inputs
            .iter()
            .map(|input| build_module(allocator, input))
            .collect();
    }
    let per_thread = inputs.len().div_ceil(allocators.len());
    std::thread::scope(|scope| {
        let handles: Vec<_> = allocators
            .iter_mut()
            .zip(inputs.chunks(per_thread))
            .map(|(allocator, inputs)| {
                scope.spawn(move || {
                    let allocator: &'a Allocator = allocator;
                    Built(
                        inputs
                            .iter()
                            .map(|input| build_module(allocator, input))
                            .collect(),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("parsing panicked").0)
            .collect()
    })
}

/// How many allocators [`World::build`] wants for `modules` modules on `threads` threads: one
/// per thread it parses on.
pub fn allocators_for(modules: usize, threads: usize) -> usize {
    /// Below this many modules a thread costs more than it saves.
    const MIN_PER_THREAD: usize = 16;
    threads.min(modules / MIN_PER_THREAD).max(1)
}

/// Every module, unit and class of the program.
pub struct World<'a> {
    /// Modules by `ModuleId` index (inputs with no module id gap).
    pub modules: Vec<Module<'a>>,
    /// Units by id.
    pub units: FxHashMap<NodeRef, Unit<'a>>,
    /// Units in source order, module by module.
    pub unit_order: Vec<NodeRef>,
    /// Classes by id.
    pub classes: FxHashMap<NodeRef, ClassInfo<'a>>,
    /// The class each function unit is a member of.
    pub member_of: FxHashMap<NodeRef, NodeRef>,
    /// The symbols each unit's scopes declare.
    pub unit_symbols: FxHashMap<NodeRef, Vec<SymbolId>>,
    /// Each module's range of `unit_order`, by module index.
    module_units: Vec<std::ops::Range<usize>>,
    /// Each module's class and function declarations, by module index: the name span (or
    /// node span when anonymous) and node, as the type oracle wants them.
    pub declarations: Vec<Vec<(Span, NodeId)>>,
}

impl<'a> World<'a> {
    /// Parses and indexes every module, a thread per allocator. The modules' ASTs go in
    /// `allocators`, a run of modules in each (see [`allocators_for`]).
    pub fn build(allocators: &'a mut [Allocator], inputs: &'a [ModuleInput<'a>]) -> Self {
        let (modules, indexed): (Vec<_>, Vec<_>) =
            build_all(allocators, inputs).into_iter().unzip();
        let mut world = Self {
            modules,
            units: FxHashMap::default(),
            unit_order: Vec::new(),
            classes: FxHashMap::default(),
            member_of: FxHashMap::default(),
            unit_symbols: FxHashMap::default(),
            module_units: Vec::new(),
            declarations: Vec::new(),
        };
        for (index, indexed) in indexed.into_iter().enumerate() {
            let start = world.unit_order.len();
            world.add_index(index, indexed);
            world.module_units.push(start..world.unit_order.len());
        }
        world
    }

    /// Frees the world on up to `threads` threads.
    pub fn drop_in_parallel(self, threads: usize) {
        let Self {
            mut modules, units, ..
        } = self;
        drop(units);
        let per_thread = modules.len().div_ceil(threads.max(1)).max(16);
        let mut parts = Vec::new();
        while modules.len() > per_thread {
            parts.push(Parsed(modules.split_off(modules.len() - per_thread)));
        }
        std::thread::scope(|scope| {
            for part in parts {
                scope.spawn(move || drop(part));
            }
            drop(modules);
        });
    }

    /// The module with `id`.
    pub fn module(&self, id: ModuleId) -> &Module<'a> {
        &self.modules[id.index()]
    }

    /// The units of module `id`, in order.
    pub fn module_units(&self, id: ModuleId) -> &[NodeRef] {
        self.module_units
            .get(id.index())
            .map_or(&[], |range| &self.unit_order[range.clone()])
    }

    /// The unit with `id`.
    pub fn unit(&self, id: NodeRef) -> &Unit<'a> {
        &self.units[&id]
    }

    /// Finds a module's units, classes and symbol facts.
    fn index_module(module: &Module<'a>) -> Indexed<'a> {
        let module_id = module.id;
        let nodes = module.semantic.nodes();
        let scoping = module.semantic.scoping();
        let mut units = Vec::new();
        let mut classes = Vec::new();
        for node in nodes.iter() {
            let id = node.id();
            let strict = scoping
                .scope_flags(node.scope_id())
                .contains(ScopeFlags::StrictMode)
                || module.program.source_type.is_module();
            match node.kind() {
                AstKind::Program(program) => {
                    units.push(Unit {
                        id: module.node(id),
                        body: Body::Module(program),
                        parent: None,
                        class: None,
                        role: None,
                        is_static: false,
                        name: None,
                        span: program.span,
                        strict,
                        uses_arguments: false,
                    });
                }
                AstKind::Function(function) if function.body.is_some() => {
                    let (role, is_static, method_name) = method_role(module, id);
                    let name = method_name
                        .or_else(|| function.id.as_ref().map(|id| id.name.to_string()))
                        .or_else(|| binding_name(module, id));
                    units.push(Unit {
                        id: module.node(id),
                        body: Body::Function(function),
                        parent: Some(module.unit_of(id, false)),
                        class: None,
                        role,
                        is_static,
                        name,
                        span: function.span,
                        strict,
                        uses_arguments: false,
                    });
                }
                AstKind::ArrowFunctionExpression(arrow) => {
                    units.push(Unit {
                        id: module.node(id),
                        body: Body::Arrow(arrow),
                        parent: Some(module.unit_of(id, false)),
                        class: None,
                        role: None,
                        is_static: false,
                        name: binding_name(module, id),
                        span: arrow.span,
                        strict,
                        uses_arguments: false,
                    });
                }
                AstKind::StaticBlock(block) => {
                    units.push(Unit {
                        id: module.node(id),
                        body: Body::StaticBlock(block),
                        parent: Some(module.unit_of(id, false)),
                        class: None,
                        role: None,
                        is_static: true,
                        name: Some("static".into()),
                        span: block.span,
                        strict: true,
                        uses_arguments: false,
                    });
                }
                AstKind::Class(class) => classes.push((id, class)),
                _ => {}
            }
        }

        let mut class_infos = Vec::new();
        for (id, class) in classes {
            let class_ref = module.node(id);
            let body_id = class.body.node_id.get();
            units.push(Unit {
                id: module.node(body_id),
                body: Body::ClassInit(class),
                parent: Some(module.unit_of(id, false)),
                class: Some(class_ref),
                role: None,
                is_static: false,
                name: class.id.as_ref().map(|id| format!("{}.<fields>", id.name)),
                span: class.body.span,
                strict: true,
                uses_arguments: false,
            });
            let mut info = ClassInfo {
                id: class_ref,
                class,
                name: class
                    .id
                    .as_ref()
                    .map(|id| id.name.to_string())
                    .or_else(|| binding_name(module, id)),
                constructor: None,
                init: module.node(body_id),
                members: Vec::new(),
                computed_members: false,
                has_heritage: class.heritage.is_some(),
                constructor_keeps_this: true,
            };
            for element in &class.body.body {
                let (key, computed, is_static, role, unit, node) = match element {
                    ClassElement::MethodDefinition(method) => {
                        let role = match method.kind {
                            MethodDefinitionKind::Constructor => MethodRole::Constructor,
                            MethodDefinitionKind::Method => MethodRole::Method,
                            MethodDefinitionKind::Get => MethodRole::Getter,
                            MethodDefinitionKind::Set => MethodRole::Setter,
                        };
                        let unit = module.node(method.value.node_id.get());
                        if role == MethodRole::Constructor {
                            info.constructor = Some(unit);
                            info.constructor_keeps_this = crate::shapes::keeps_this(&method.value);
                        }
                        (
                            &method.key,
                            method.computed,
                            method.r#static,
                            Some(role),
                            Some(unit),
                            method.node_id.get(),
                        )
                    }
                    ClassElement::PropertyDefinition(property) => (
                        &property.key,
                        property.computed,
                        property.r#static,
                        None,
                        None,
                        property.node_id.get(),
                    ),
                    ClassElement::AccessorProperty(property) => (
                        &property.key,
                        property.computed,
                        property.r#static,
                        Some(MethodRole::Getter),
                        None,
                        property.node_id.get(),
                    ),
                    ClassElement::StaticBlock(_) | ClassElement::TSIndexSignature(_) => continue,
                };
                let Some(name) = static_key(key, computed) else {
                    info.computed_members = true;
                    continue;
                };
                if role == Some(MethodRole::Constructor) {
                    continue;
                }
                info.members.push(Member {
                    name,
                    unit,
                    role,
                    is_static,
                    node,
                });
            }
            class_infos.push(info);
        }
        for unit in &mut units {
            if let Body::Function(_) = unit.body
                && unit.role.is_some()
            {
                let parent = nodes.parent_id(unit.id.node);
                // Function -> MethodDefinition -> ClassBody -> Class
                let class = nodes.parent_id(nodes.parent_id(parent));
                unit.class = Some(module.node(class));
            }
            if let Body::StaticBlock(_) = unit.body {
                let class = nodes.parent_id(nodes.parent_id(unit.id.node));
                unit.class = Some(module.node(class));
            }
        }

        // `arguments` use: an unresolved reference called `arguments` inside a non-arrow
        // function belongs to the nearest such function.
        let mut uses_arguments = FxHashSet::default();
        for reference in scoping.root_unresolved_references_ids().flatten() {
            let reference = scoping.get_reference(reference);
            if nodes.kind(reference.node_id()).identifier_name() != Some("arguments".into()) {
                continue;
            }
            let mut unit = module.unit_of(reference.node_id(), false);
            while let Some(found) = units.iter().find(|candidate| candidate.id == unit) {
                if !found.lexical_this() {
                    break;
                }
                unit = found.parent.unwrap_or(unit);
                if found.parent.is_none() {
                    break;
                }
            }
            uses_arguments.insert(unit);
        }
        for unit in &mut units {
            unit.uses_arguments = uses_arguments.contains(&unit.id);
        }

        let symbols = Self::symbol_infos(module, &units);
        let _ = module_id;
        let mut unit_symbols: FxHashMap<NodeRef, Vec<SymbolId>> = FxHashMap::default();
        for (symbol, info) in symbols.iter().enumerate() {
            if let Some(unit) = info.unit {
                unit_symbols
                    .entry(unit)
                    .or_default()
                    .push(SymbolId::from_usize(symbol));
            }
        }
        Indexed {
            units,
            classes: class_infos,
            symbols,
            declarations: declarations(module),
            unit_symbols,
        }
    }

    /// Adds a module's index to the world's tables.
    fn add_index(&mut self, index: usize, indexed: Indexed<'a>) {
        let Indexed {
            units,
            classes: class_infos,
            symbols,
            declarations,
            unit_symbols,
        } = indexed;
        self.declarations.push(declarations);
        self.unit_symbols.extend(unit_symbols);
        self.modules[index].symbols = symbols;
        for info in class_infos {
            for member in &info.members {
                if let Some(unit) = member.unit {
                    self.member_of.insert(unit, info.id);
                }
            }
            if let Some(constructor) = info.constructor {
                self.member_of.insert(constructor, info.id);
            }
            self.classes.insert(info.id, info);
        }
        for unit in units {
            self.unit_order.push(unit.id);
            self.units.insert(unit.id, unit);
        }
    }

    fn symbol_infos(module: &Module<'a>, units: &[Unit<'a>]) -> Vec<SymbolInfo> {
        let scoping = module.semantic.scoping();
        let nodes = module.semantic.nodes();

        // Scopes a direct eval or `with` can see into: the scope itself and its ancestors.
        let mut tainted_scopes = FxHashSet::default();
        for node in nodes.iter() {
            let taint = match node.kind() {
                AstKind::CallExpression(call) => matches!(
                    &call.callee.without_parentheses(),
                    Expression::Identifier(ident) if ident.name == "eval"
                        && ident.reference_id.get().is_some_and(|reference| {
                            scoping.get_reference(reference).symbol_id().is_none()
                        })
                ),
                AstKind::WithStatement(_) => true,
                _ => false,
            };
            if taint {
                for scope in scoping.scope_ancestors(node.scope_id()) {
                    tainted_scopes.insert(scope);
                }
            }
        }

        let unit_strict = |unit: NodeRef| units.iter().find(|u| u.id == unit);
        let exported: FxHashSet<SymbolId> = module
            .record
            .local_export_entries
            .iter()
            .filter_map(|entry| match &entry.local_name {
                oxc_syntax::module_record::ExportLocalName::Name(name)
                | oxc_syntax::module_record::ExportLocalName::Default(name) => {
                    module.root_binding(name.name.as_str())
                }
                oxc_syntax::module_record::ExportLocalName::Null => None,
            })
            .collect();

        scoping
            .symbol_ids()
            .map(|symbol| {
                let flags = scoping.symbol_flags(symbol);
                let scope = scoping.symbol_scope_id(symbol);
                let unit = module.unit_of_scope(scope);
                let tainted = tainted_scopes.contains(&scope);
                let mut writes_elsewhere = false;
                let mut nested_reads = false;
                for reference in scoping.get_resolved_references(symbol) {
                    let at = module.unit_of(reference.node_id(), false);
                    if at != unit {
                        if reference.is_write() {
                            writes_elsewhere = true;
                        }
                        if reference.is_read() {
                            nested_reads = true;
                        }
                    }
                }
                let import = flags.contains(SymbolFlags::Import);
                // A sloppy-mode function that uses `arguments` aliases its parameters.
                let declared_in = unit_strict(unit);
                let aliased_param = declared_in.is_some_and(|u| u.uses_arguments && !u.strict)
                    && flags.contains(SymbolFlags::FunctionScopedVariable);
                let value = flags.intersects(
                    SymbolFlags::Variable
                        | SymbolFlags::Function
                        | SymbolFlags::Class
                        | SymbolFlags::CatchVariable,
                );
                let in_class_init =
                    declared_in.is_some_and(|u| matches!(u.body, Body::ClassInit(_)));
                let tainted = tainted || aliased_param;
                let lexical = flags
                    .intersects(SymbolFlags::BlockScopedVariable | SymbolFlags::Class)
                    && !flags.contains(SymbolFlags::Function);
                let declaration = scoping.symbol_declaration(symbol);
                let declared_at = module.semantic.nodes().kind(declaration).span().end;
                let in_switch = lexical
                    && module
                        .semantic
                        .nodes()
                        .ancestor_kinds(declaration)
                        .take_while(|kind| {
                            !matches!(
                                kind,
                                AstKind::Function(_)
                                    | AstKind::ArrowFunctionExpression(_)
                                    | AstKind::Program(_)
                            )
                        })
                        .any(|kind| matches!(kind, AstKind::SwitchCase(_)));
                SymbolInfo {
                    unit: Some(unit),
                    tracked: value
                        && !tainted
                        && !import
                        && !writes_elsewhere
                        && !aliased_param
                        && !in_class_init,
                    tainted,
                    nested_reads,
                    hoisted: flags
                        .intersects(SymbolFlags::FunctionScopedVariable | SymbolFlags::Function),
                    import,
                    exported: exported.contains(&symbol),
                    lexical,
                    declared_at,
                    in_switch,
                }
            })
            .collect()
    }
}

/// The role and static-ness of a function that is a class member, and the member's name.
fn method_role(
    module: &Module<'_>,
    function: NodeId,
) -> (Option<MethodRole>, bool, Option<String>) {
    let nodes = module.semantic.nodes();
    match nodes.parent_kind(function) {
        AstKind::MethodDefinition(method) => {
            let role = match method.kind {
                MethodDefinitionKind::Constructor => MethodRole::Constructor,
                MethodDefinitionKind::Method => MethodRole::Method,
                MethodDefinitionKind::Get => MethodRole::Getter,
                MethodDefinitionKind::Set => MethodRole::Setter,
            };
            let class_name =
                match nodes.kind(nodes.parent_id(nodes.parent_id(method.node_id.get()))) {
                    AstKind::Class(class) => class.id.as_ref().map(|id| id.name.to_string()),
                    _ => None,
                };
            let key =
                static_key(&method.key, method.computed).unwrap_or_else(|| "[computed]".into());
            let name = match class_name {
                Some(class) => format!("{class}.{key}"),
                None => key,
            };
            (Some(role), method.r#static, Some(name))
        }
        _ => (None, false, None),
    }
}

/// The name a function or class gets from the binding it initialises (`const f = () => {}`)
/// or the property it is the value of.
fn binding_name(module: &Module<'_>, node: NodeId) -> Option<String> {
    let nodes = module.semantic.nodes();
    let mut parent = nodes.parent_id(node);
    while let AstKind::ParenthesizedExpression(_) = nodes.kind(parent) {
        parent = nodes.parent_id(parent);
    }
    match nodes.kind(parent) {
        AstKind::VariableDeclarator(declarator) => declarator
            .id
            .get_binding_identifier()
            .map(|id| id.name.to_string()),
        AstKind::ObjectProperty(property) => static_key(&property.key, property.computed),
        AstKind::PropertyDefinition(property) => static_key(&property.key, property.computed),
        _ => None,
    }
}

/// The name of a property key that is not computed (or is a computed string literal).
pub fn static_key(key: &PropertyKey<'_>, computed: bool) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(ident) if !computed => Some(ident.name.to_string()),
        PropertyKey::PrivateIdentifier(ident) => Some(format!("#{}", ident.name)),
        PropertyKey::StringLiteral(literal) => Some(literal.value.to_string()),
        PropertyKey::NumericLiteral(literal) => Some(number_key(literal.value)),
        _ => None,
    }
}

/// The property name a number key converts to.
pub fn number_key(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e21 {
        #[expect(clippy::cast_possible_truncation, reason = "an integer below 1e21")]
        let int = value as i128;
        int.to_string()
    } else {
        value.to_string()
    }
}
