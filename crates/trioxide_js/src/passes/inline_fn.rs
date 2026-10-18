//! PLAN §4 pass 5: function inlining.
//!
//! A call `f(…)` to a function declaration of the same module is replaced by `f`'s body when
//! the facts say `f` is all that runs there (`site_resolution(site).complete`, usable) and
//! `f` is small ([`SMALL`] nodes) or this is its only call site (`callers_complete` usable and
//! one caller edge; up to [`LARGE`] nodes).
//!
//! **Forms.** A body that is one `return e` becomes an expression wherever the call is:
//! `(x = arg, e')`, its temporaries declared (`let x;`) just before the statement holding the
//! call. When the call is that statement's first evaluation (`const r = f(a)` or `return f(a)`)
//! the temporaries are declared with their arguments instead (`const x = a;`). Other
//! bodies are inlined where the call is a statement of its own (`f(a);`), a declaration's
//! initialiser (`const x = f(a);`), a `return`'s argument, or the right side of an assignment
//! statement to a name: the parameters become `const`s (`let` when `f` reassigns them), the
//! body follows, and its `return`s become assignments to the target, with a labeled block and
//! `break` when a `return` isn't the body's last statement.
//!
//! **Arguments** are evaluated once and in order. A literal, or a name that is never reassigned
//! and whose read can't throw, is copied into each use; anything else gets a fresh local (or
//! temporary). A default parameter takes its default when the argument is missing (evaluated
//! after the earlier parameters, as in the callee) or a literal argument, and later arguments
//! must then be literals or such names, since their evaluation could otherwise move past the
//! default's. Rest parameters become an array literal of the remaining arguments; extra
//! arguments are still evaluated.
//!
//! **Names.** The callee's parameters and locals get fresh names that nothing visible at the
//! call site has ([`Namer`]), keeping their own where possible; the callee's other references
//! must mean the same binding at the call site.
//!
//! **Not inlined:** callees that use `this`, `arguments`, `new.target`, `super`, a direct
//! `eval`, `with`, labels or `var`; generators and async functions (a generator's body runs at
//! `next()`, not at the call); recursive ones; ones containing a function or class (its
//! identity and what it captures would change); callees and callers that are WASM island
//! functions ([`Keep`]); calls with spread arguments; calls in code proven never to run.
//!
//! **Removal.** A function every call of which was inlined goes when it has no other reference,
//! `callers_complete` is usable-true, and it is neither exported nor an island function.
//!
//! **Specialisation** (`passes.specialize`, the [`Params`] pass, which runs whether or not
//! inlining does): a parameter every caller passes the same literal is removed from the
//! signature and from every call, and becomes a `const` at the top of the body, so signatures
//! don't keep dead parameters. (Pass 1, run earlier in the group, has already folded the
//! parameter's reads where specialisation allows; the syntactic cleanup drops the `const` when
//! nothing reads it any more.) This needs every caller known (`callers_complete`), all of them
//! direct calls in this module, and no use of `arguments`.
//!
//! **Off by default** (`passes: { "inline-fn": true }` turns it on): V8's optimising tier
//! already inlines hot calls, so the measured wins are in code that isn't hot yet.

use oxc_allocator::GetAllocator;
use oxc_allocator::{Allocator, ArenaVec, CloneIn, TakeIn};
use oxc_ast::ast::Ident;
use oxc_ast::{
    AstKind,
    ast::{
        Argument, ArrayExpressionElement, AssignmentOperator, AssignmentTarget, BindingPattern,
        CallExpression, Declaration, ExportDefaultDeclarationKind, Expression, Function,
        IdentifierReference, LabelIdentifier, ObjectProperty, Program, Statement,
        VariableDeclaration, VariableDeclarationKind, VariableDeclarator,
    },
    builder::AstBuilder,
};
use oxc_ast_visit::{Visit, VisitMut, walk, walk_mut};
use oxc_semantic::Scoping;
use oxc_span::{GetSpan, Span};
use oxc_syntax::{operator::UnaryOperator, scope::ScopeFlags, scope::ScopeId, symbol::SymbolId};
use oxc_traverse::{Ancestor, Traverse, TraverseCtx};
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::{NodeId, NodeRef};

use super::{
    Comments, Decisions, Position,
    speed::{ForgetIds, Namer, is_island, rescope, unnamed},
    track_position,
    util::{Query, Slot, has_dynamic_scope, is_global_name, live_references, small_enough, weak},
};
use crate::{Keep, Pass, PassContext};

/// Callees with more call sites than one are inlined up to this many AST nodes.
pub const SMALL: usize = 40;
/// A callee's only call site takes at most this many nodes.
pub const LARGE: usize = 400;

/// PLAN §4 pass 5 (`"inline-fn"`), last in the foundation group.
pub struct InlineFn;

impl Pass for InlineFn {
    fn name(&self) -> &'static str {
        "inline-fn"
    }

    /// Off unless `passes` turns it on: V8's optimising tier already inlines and replaces
    /// non-escaping objects in hot code, where this pass measured no faster (PLAN §4).
    fn enabled_by_default(&self) -> bool {
        false
    }

    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
        let query = Query {
            facts: ctx.facts,
            module: ctx.module,
        };
        if has_dynamic_scope(program, ctx.scoping()) {
            return false;
        }
        let decisions = Decisions::new(self.name());
        let tagged = super::wasm_tags(program);
        let index = Index::build(
            program,
            ctx.scoping(),
            query,
            ctx.keep,
            &tagged,
            ctx.allocator,
        );
        if index.callees.is_empty() {
            return decisions.flush(ctx.log);
        }
        let namer = Namer::new(ctx.scoping());
        let mut visitor = Visitor {
            query,
            position: Position::default(),
            decisions,
            index,
            namer,
            frames: Vec::new(),
            inlined: FxHashSet::default(),
            allocator: ctx.allocator,
        };
        ctx.traverse(&mut visitor, program, ());
        let Visitor {
            mut decisions,
            index,
            inlined,
            ..
        } = visitor;
        if decisions.applied > 0 {
            *ctx.scoping_mut() = rescope(program);
            if remove_inlined(program, ctx, &index, &inlined, &mut decisions) {
                *ctx.scoping_mut() = rescope(program);
            }
        }
        decisions.flush(ctx.log)
    }
}

/// Specialisation's parameter removal (`"params"`, PLAN §4 pass 5): with `passes.specialize`, a
/// parameter every caller passes the same literal leaves the signature and the calls. On by
/// default, but it does nothing unless specialisation is on; it runs before [`InlineFn`].
pub struct Params;

impl Pass for Params {
    fn name(&self) -> &'static str {
        "params"
    }

    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
        if !ctx.specialize || has_dynamic_scope(program, ctx.scoping()) {
            return false;
        }
        let query = Query {
            facts: ctx.facts,
            module: ctx.module,
        };
        let mut decisions = Decisions::new(self.name());
        if drop_constant_parameters(program, ctx, query, &mut decisions) {
            *ctx.scoping_mut() = rescope(program);
        }
        decisions.flush(ctx.log)
    }
}

/// A function declaration of the module, as a callee.
struct Callee<'a> {
    node: NodeId,
    name: String,
    /// Why it can never be inlined, if so.
    problem: Option<String>,
    /// A copy of it, with the semantic ids it had (to resolve its names).
    template: Option<&'a Function<'a>>,
    size: usize,
    /// Its body is one `return` (or nothing).
    single_return: bool,
    /// Its only `return` is its last statement (or it has none).
    simple_returns: bool,
    /// References to bindings declared outside it: name and binding (`None`: a global).
    free: Vec<(String, Option<SymbolId>)>,
    exported: bool,
    island: bool,
    /// A class method (called on a receiver, which is its `this`).
    method: bool,
    /// The span of its declaration statement.
    statement: Span,
}

/// The module's inlinable function declarations, and the units call sites are in.
struct Index<'a> {
    /// By the function's node.
    callees: FxHashMap<NodeId, Callee<'a>>,
    /// Function declarations' nodes, by binding.
    by_symbol: FxHashMap<SymbolId, NodeId>,
    /// Call sites inside island functions (where nothing is inlined).
    in_island: FxHashSet<NodeId>,
    /// Call sites where `this` may not be bound yet (derived-class constructors, before or
    /// after `super()`), so a read of it can throw.
    unbound_this: FxHashSet<NodeId>,
    /// Units in recursive cycles.
    recursive: FxHashSet<NodeRef>,
}

impl<'a> Index<'a> {
    #[expect(
        clippy::items_after_statements,
        reason = "visitors used only here, next to their use"
    )]
    fn build(
        program: &Program<'a>,
        scoping: &Scoping,
        query: Query<'_>,
        keep: &Keep,
        tagged: &FxHashSet<u32>,
        allocator: &'a Allocator,
    ) -> Self {
        let mut recursive = FxHashSet::default();
        let graph = query.facts.call_graph();
        for scc in graph.sccs() {
            if scc.len() > 1 {
                recursive.extend(scc.iter().copied());
            }
        }
        for edge in graph.edges() {
            if edge.caller == edge.callee {
                recursive.insert(edge.callee);
            }
        }
        let mut exported = FxHashSet::default();
        let mut export_starts: FxHashMap<NodeId, u32> = FxHashMap::default();
        for statement in &program.body {
            match statement {
                Statement::ExportDeclaration(export) => {
                    if let Declaration::FunctionDeclaration(function) = &export.declaration {
                        exported.insert(function.node_id());
                        export_starts.insert(function.node_id(), export.span.start);
                    }
                }
                Statement::ExportDefaultDeclaration(export) => {
                    if let ExportDefaultDeclarationKind::FunctionDeclaration(function) =
                        &export.declaration
                    {
                        exported.insert(function.node_id());
                        export_starts.insert(function.node_id(), export.span.start);
                    }
                }
                _ => {}
            }
        }
        struct Collect<'c, 'a> {
            index: Index<'a>,
            scoping: &'c Scoping,
            query: Query<'c>,
            keep: &'c Keep,
            tagged: &'c FxHashSet<u32>,
            exported: FxHashSet<NodeId>,
            export_starts: FxHashMap<NodeId, u32>,
            allocator: &'a Allocator,
            /// Whether each enclosing function is an island.
            islands: Vec<bool>,
            statements: Vec<Span>,
            /// Whether each enclosing class extends another.
            derived: Vec<bool>,
            /// Whether `this` may be unbound in each enclosing function.
            unbound: Vec<bool>,
            /// The constructor of a derived class about to be visited.
            constructor: Option<NodeId>,
        }
        impl<'a> Visit<'a> for Collect<'_, 'a> {
            fn visit_statement(&mut self, statement: &Statement<'a>) {
                self.statements.push(statement.span());
                walk::walk_statement(self, statement);
                self.statements.pop();
            }

            fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
                let node = function.node_id();
                let mut starts = vec![function.span.start];
                if let Some(&start) = self.export_starts.get(&node) {
                    starts.push(start);
                }
                let island = is_island(
                    self.keep,
                    self.query.node(node),
                    super::forced_island(function, self.tagged, &starts),
                );
                if function.is_declaration()
                    && let Some(id) = &function.id
                {
                    let callee = self.callee(function, Some(id.symbol_id()), island, None);
                    self.index.callees.insert(node, callee);
                    self.index.by_symbol.insert(id.symbol_id(), node);
                }
                let outer = self.islands.last().copied().unwrap_or(false);
                self.islands.push(outer || island);
                self.unbound.push(self.constructor == Some(node));
                walk::walk_function(self, function, flags);
                self.unbound.pop();
                self.islands.pop();
            }

            fn visit_class(&mut self, class: &oxc_ast::ast::Class<'a>) {
                self.derived.push(class.heritage.is_some());
                walk::walk_class(self, class);
                self.derived.pop();
            }

            fn visit_arrow_function_expression(
                &mut self,
                arrow: &oxc_ast::ast::ArrowFunctionExpression<'a>,
            ) {
                let outer = self.islands.last().copied().unwrap_or(false);
                let island = is_island(self.keep, self.query.node(arrow.node_id()), false);
                self.islands.push(outer || island);
                let unbound = self.unbound.last().copied().unwrap_or(false);
                self.unbound.push(unbound);
                walk::walk_arrow_function_expression(self, arrow);
                self.unbound.pop();
                self.islands.pop();
            }

            fn visit_method_definition(&mut self, method: &oxc_ast::ast::MethodDefinition<'a>) {
                if method.kind == oxc_ast::ast::MethodDefinitionKind::Method
                    && !method.computed
                    && let Some(name) = method.key.static_name()
                {
                    let function = &method.value;
                    let island = is_island(self.keep, self.query.node(function.node_id()), false);
                    let callee = self.callee(function, None, island, Some(&name));
                    self.index.callees.insert(function.node_id(), callee);
                }
                if method.kind == oxc_ast::ast::MethodDefinitionKind::Constructor
                    && self.derived.last().copied().unwrap_or(false)
                {
                    self.constructor = Some(method.value.node_id());
                }
                walk::walk_method_definition(self, method);
            }

            fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
                if self.islands.last().copied().unwrap_or(false) {
                    self.index.in_island.insert(call.node_id());
                }
                if self.unbound.last().copied().unwrap_or(false) {
                    self.index.unbound_this.insert(call.node_id());
                }
                walk::walk_call_expression(self, call);
            }
        }
        impl<'a> Collect<'_, 'a> {
            fn callee(
                &self,
                function: &Function<'a>,
                symbol: Option<SymbolId>,
                island: bool,
                method: Option<&str>,
            ) -> Callee<'a> {
                let node = function.node_id();
                let scope = function.scope_id();
                let name = match (method, symbol) {
                    (Some(name), _) => name.to_string(),
                    (None, Some(symbol)) => self.scoping.symbol_name(symbol).to_string(),
                    (None, None) => "function".to_string(),
                };
                let mut scan = Scan {
                    scoping: self.scoping,
                    scope,
                    own: symbol,
                    method: method.is_some(),
                    problem: None,
                    size: 0,
                    free: Vec::new(),
                    returns: 0,
                    depth: 1,
                };
                scan.visit_formal_parameters(&function.params);
                if let Some(body) = &function.body {
                    scan.visit_function_body(body);
                }
                let mut problem = scan.problem.take();
                let body = function.body.as_ref();
                if problem.is_none() {
                    problem = if function.generator {
                        Some("is a generator (its body runs at `next()`, not at the call)".into())
                    } else if function.r#async {
                        Some("is async".into())
                    } else if function.declare || body.is_none() {
                        Some("has no body".into())
                    } else if body.is_some_and(|body| !body.directives.is_empty()) {
                        Some("has directives".into())
                    } else if self
                        .query
                        .node(node)
                        .is_some_and(|unit| self.index.recursive.contains(&unit))
                    {
                        Some("is recursive".into())
                    } else if !simple_parameters(function) {
                        Some("destructures its parameters".into())
                    } else {
                        None
                    }
                }
                let statements = body.map_or(&[][..], |body| body.statements.as_slice());
                let single_return = matches!(statements, [] | [Statement::ReturnStatement(_)]);
                let last_returns = matches!(statements.last(), Some(Statement::ReturnStatement(_)));
                let simple_returns = scan.returns == 0 || (scan.returns == 1 && last_returns);
                let template = problem.is_none().then(|| {
                    &*self
                        .allocator
                        .alloc(function.clone_in_with_semantic_ids(self.allocator))
                });
                Callee {
                    node,
                    name,
                    problem,
                    template,
                    size: scan.size,
                    single_return,
                    simple_returns,
                    free: scan.free,
                    exported: self.exported.contains(&node),
                    island,
                    method: method.is_some(),
                    statement: self.statements.last().copied().unwrap_or(function.span),
                }
            }
        }
        let mut collect = Collect {
            index: Index {
                callees: FxHashMap::default(),
                by_symbol: FxHashMap::default(),
                in_island: FxHashSet::default(),
                unbound_this: FxHashSet::default(),
                recursive,
            },
            scoping,
            query,
            keep,
            tagged,
            exported,
            export_starts,
            allocator,
            islands: Vec::new(),
            statements: Vec::new(),
            derived: Vec::new(),
            unbound: Vec::new(),
            constructor: None,
        };
        collect.visit_program(program);
        collect.index
    }
}

/// Whether every parameter is a plain name (rest included).
fn simple_parameters(function: &Function<'_>) -> bool {
    function
        .params
        .items
        .iter()
        .all(|parameter| matches!(parameter.pattern, BindingPattern::BindingIdentifier(_)))
        && function
            .params
            .rest
            .as_ref()
            .is_none_or(|rest| matches!(rest.rest.argument, BindingPattern::BindingIdentifier(_)))
}

/// Walks a callee: what stops inlining it, its size, its returns and free references.
struct Scan<'s> {
    scoping: &'s Scoping,
    scope: ScopeId,
    own: Option<SymbolId>,
    /// A method, whose `this` is its receiver.
    method: bool,
    problem: Option<String>,
    size: usize,
    free: Vec<(String, Option<SymbolId>)>,
    returns: usize,
    depth: usize,
}

impl Scan<'_> {
    fn stop(&mut self, why: &str) {
        if self.problem.is_none() {
            self.problem = Some(why.to_string());
        }
    }
}

impl<'a> Visit<'a> for Scan<'_> {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        self.size += 1;
        match kind {
            AstKind::ThisExpression(_) if !self.method => self.stop("uses `this`"),
            AstKind::PrivateIdentifier(_) => self.stop("uses private names"),
            AstKind::Super(_) => self.stop("uses `super`"),
            AstKind::NewTarget(_) => self.stop("uses `new.target`"),
            AstKind::WithStatement(_) => self.stop("uses `with`"),
            AstKind::LabeledStatement(_) => self.stop("uses labels"),
            AstKind::ReturnStatement(_) => self.returns += 1,
            AstKind::YieldExpression(_) | AstKind::AwaitExpression(_) => {
                self.stop("suspends (`yield`/`await`)");
            }
            AstKind::Class(_) => self.stop("contains a class"),
            AstKind::ArrowFunctionExpression(_) => {
                self.stop("contains a closure (its identity and what it captures would change)");
            }
            _ => {}
        }
    }

    fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
        if self.depth > 0 {
            self.stop("contains a closure (its identity and what it captures would change)");
            return;
        }
        self.depth += 1;
        walk::walk_function(self, function, flags);
        self.depth -= 1;
    }

    fn visit_variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
        match declaration.kind {
            VariableDeclarationKind::Var => self.stop("declares a `var`"),
            VariableDeclarationKind::Using | VariableDeclarationKind::AwaitUsing => {
                self.stop("declares a `using` binding");
            }
            _ => {}
        }
        walk::walk_variable_declaration(self, declaration);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if let Expression::Identifier(ident) = &call.callee
            && ident.name == "eval"
        {
            self.stop("calls `eval`");
        }
        walk::walk_call_expression(self, call);
    }

    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        self.size += 1;
        if ident.name == "arguments" {
            self.stop("uses `arguments`");
        }
        let Some(reference) = ident.reference_id.get() else {
            self.stop("has a name the scoping doesn't know");
            return;
        };
        let reference = self.scoping.get_reference(reference);
        if !reference.is_value() {
            return;
        }
        match reference.symbol_id() {
            Some(symbol) if Some(symbol) == self.own => self.stop("is recursive"),
            Some(symbol) => {
                let scope = self.scoping.symbol_scope_id(symbol);
                if scope != self.scope && !self.scoping.scope_is_descendant_of(scope, self.scope) {
                    self.free.push((ident.name.to_string(), Some(symbol)));
                }
            }
            None => self.free.push((ident.name.to_string(), None)),
        }
    }
}

/// One call site's plan.
struct Site {
    callee: NodeId,
}

/// A statement list being visited: its scope, where the visit is in it, and the declarations
/// of temporaries to put before its items.
struct Frame<'a> {
    scope: ScopeId,
    /// The index of the item being visited (in the list as it was), if any yet.
    item: Option<usize>,
    /// How many statements deep inside that item the visit is.
    depth: usize,
    /// The call whose arguments the item evaluates first, if it has one.
    leading: Option<Span>,
    /// Uninitialised temporaries, by item.
    temporaries: Vec<(usize, Vec<String>)>,
    /// Temporaries declared with their values, by item (after the uninitialised ones, which
    /// the values may assign).
    declarations: Vec<(usize, Statement<'a>)>,
}

struct Visitor<'a, 'f> {
    query: Query<'f>,
    position: Position,
    decisions: Decisions,
    index: Index<'a>,
    namer: Namer,
    frames: Vec<Frame<'a>>,
    /// Callees inlined somewhere.
    inlined: FxHashSet<NodeId>,
    allocator: &'a Allocator,
}

/// What a parameter is in the inlined code.
enum Value<'a> {
    /// Copied into each use: a literal or a name.
    Copy(Expression<'a>),
    /// A fresh local (or temporary) holding the argument.
    Local {
        name: String,
        init: Expression<'a>,
        mutable: bool,
        /// The value is the callee's code (a default), so its names are renamed; an argument
        /// is the caller's, and must not be (it may hold earlier inlined copies of the callee).
        callee_code: bool,
    },
}

/// The parameters' values, plus arguments evaluated only for their effects.
struct Arguments<'a> {
    values: FxHashMap<SymbolId, Value<'a>>,
    order: Vec<SymbolId>,
    extra: Vec<Expression<'a>>,
}

/// How a parameter gets its value at one call.
enum How {
    /// Copy the argument at this position into each use.
    CopyArgument(usize),
    /// Copy `undefined` (no argument).
    CopyUndefined,
    /// A local holding the argument at this position.
    LocalArgument(usize),
    /// A local holding `undefined`, which the callee reassigns.
    LocalUndefined,
    /// A local holding the parameter's default (no argument).
    LocalDefault,
    /// A local holding an array of the arguments from this position on.
    Rest(usize),
}

/// How a call's parameters get their values: `(parameter, its name, reassigned, how)`, and the
/// positions of extra arguments evaluated for their effects.
struct Plan {
    parameters: Vec<(SymbolId, String, bool, How)>,
    extra: Vec<usize>,
}

impl Plan {
    fn needs_locals(&self) -> bool {
        self.parameters
            .iter()
            .any(|(_, _, _, how)| !matches!(how, How::CopyArgument(_) | How::CopyUndefined))
    }
}

/// How a statement uses the call it inlines.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// `f(a);`
    Discard,
    /// `const x = f(a);`
    Declare,
    /// `return f(a);`
    Return,
    /// `x = f(a);`
    Assign,
}

impl<'a> Traverse<'a, ()> for Visitor<'a, '_> {
    track_position!(before_statement);

    fn enter_statements(
        &mut self,
        _statements: &mut ArenaVec<'a, Statement<'a>>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.frames.push(Frame {
            scope: ctx.current_scope_id(),
            item: None,
            depth: 0,
            leading: None,
            temporaries: Vec::new(),
            declarations: Vec::new(),
        });
    }

    fn exit_statements(
        &mut self,
        statements: &mut ArenaVec<'a, Statement<'a>>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        // Where each item of the list as it was starts now.
        let mut starts = Vec::with_capacity(statements.len());
        if self.position.live() {
            let mut index = 0;
            while index < statements.len() {
                starts.push(index);
                index += self.statement(statements, index, ctx);
            }
        } else {
            starts.extend(0..statements.len());
        }
        let frame = self.frames.pop().expect("a frame per statement list");
        let mut before: Vec<(usize, Statement<'a>)> = frame.declarations;
        for (item, names) in frame.temporaries.into_iter().rev() {
            let declarators = names.iter().map(|name| {
                VariableDeclarator::new(
                    Span::default(),
                    BindingPattern::new_binding_identifier(
                        Span::default(),
                        Ident::from_str_in(name, ctx),
                        ctx,
                    ),
                    None,
                    None,
                    false,
                    ctx,
                )
            });
            let declaration = Statement::new_variable_declaration(
                Span::default(),
                VariableDeclarationKind::Let,
                ArenaVec::from_iter_in(declarators, ctx),
                false,
                ctx,
            );
            let at = before.partition_point(|(other, _)| *other < item);
            before.insert(at, (item, declaration));
        }
        for (item, mut declaration) in before.into_iter().rev() {
            // At the statement's start, so its leading comments stay above its temporaries.
            let start = statements[starts[item]].span().start;
            if let Statement::VariableDeclaration(declaration) = &mut declaration {
                declaration.span = Span::empty(start);
            }
            statements.insert(starts[item], declaration);
        }
    }

    fn exit_expression(&mut self, expression: &mut Expression<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        if !self.position.live() {
            return;
        }
        let Expression::CallExpression(call) = expression else {
            return;
        };
        let slot = Slot::of(ctx);
        if matches!(slot, Slot::Statement | Slot::Delete) {
            return;
        }
        let statement_form = matches!(
            ctx.parent(),
            Ancestor::VariableDeclaratorInit(_)
                | Ancestor::ReturnStatementArgument(_)
                | Ancestor::AssignmentExpressionRight(_)
        );
        let span = call.span;
        let scope = ctx.current_scope_id();
        let site = match self.check(call, scope, ctx.scoping()) {
            Ok(site) => site,
            Err(Some(reason)) => {
                self.decisions.skipped(span, reason);
                return;
            }
            Err(None) => return,
        };
        let callee = &self.index.callees[&site.callee];
        if !callee.single_return {
            if !statement_form {
                self.decisions.skipped(
                    span,
                    "the body isn't a single `return`, and the call isn't a statement, \
                     declaration, `return` or assignment of its own",
                );
            }
            return;
        }
        if slot == Slot::Typeof {
            return;
        }
        let template = callee.template.expect("eligible callees have a template");
        let Some(frame_scope) = self.frames.last().map(|frame| frame.scope) else {
            return;
        };
        let plan = match self.plan(call, site.callee, ctx.scoping()) {
            Ok(plan) => plan,
            Err(reason) => {
                self.decisions.skipped(span, reason);
                return;
            }
        };
        if plan.needs_locals() && !list_in_same_function(ctx) {
            self.decisions.skipped(
                span,
                "an argument needs a temporary, and there is no statement list in the same \
                 function to declare it in",
            );
            return;
        }
        let leading = self
            .frames
            .last()
            .is_some_and(|frame| frame.depth == 1 && frame.leading == Some(span));
        let receiver = receiver_of(call, self.allocator);
        let mut arguments = self.apply(call, site.callee, plan, frame_scope, ctx);
        let mut sequence = Vec::new();
        let mut copies = FxHashMap::default();
        let mut renames = FxHashMap::default();
        for symbol in &arguments.order {
            match arguments.values.remove(symbol) {
                Some(Value::Copy(expression)) => {
                    copies.insert(*symbol, expression);
                }
                Some(Value::Local {
                    name,
                    init,
                    mutable,
                    callee_code,
                }) => {
                    renames.insert(*symbol, name.clone());
                    sequence.push((name, init, mutable, callee_code));
                }
                None => {}
            }
        }
        let mut parts: Vec<Expression<'a>> = Vec::new();
        let mut declarations = Vec::new();
        for (name, mut init, mutable, callee_code) in sequence {
            if callee_code {
                Rename {
                    scoping: ctx.scoping(),
                    copies: &copies,
                    renames: &renames,
                    this: receiver.as_ref(),
                    ast: AstBuilder::new(self.allocator),
                    mode: None,
                }
                .visit_expression(&mut init);
            }
            if leading {
                declarations.push((name, init, mutable));
                continue;
            }
            let target = AssignmentTarget::new_assignment_target_identifier(
                span,
                Ident::from_str_in(&name, ctx),
                ctx,
            );
            parts.push(Expression::new_assignment_expression(
                span,
                AssignmentOperator::Assign,
                target,
                init,
                ctx,
            ));
            if let Some(frame) = self.frames.last_mut()
                && let Some(item) = frame.item
            {
                match frame.temporaries.last_mut() {
                    Some((last, names)) if *last == item => names.push(name),
                    _ => frame.temporaries.push((item, vec![name])),
                }
            }
        }
        parts.append(&mut arguments.extra);
        let body = template.body.as_ref().expect("callees have bodies");
        let mut result = match body.statements.first() {
            Some(Statement::ReturnStatement(ret)) if ret.argument.is_some() => ret
                .argument
                .as_ref()
                .expect("checked")
                .clone_in_with_semantic_ids(self.allocator),
            _ => undefined(span, frame_scope, ctx),
        };
        let mut rename = Rename {
            scoping: ctx.scoping(),
            copies: &copies,
            renames: &renames,
            this: receiver.as_ref(),
            ast: AstBuilder::new(self.allocator),
            mode: None,
        };
        ForgetIds.visit_expression(&result);
        rename.visit_expression(&mut result);
        parts.push(result);
        let replacement = if parts.len() == 1 {
            parts.pop().expect("one part")
        } else {
            let sequence =
                Expression::new_sequence_expression(span, ArenaVec::from_iter_in(parts, ctx), ctx);
            Expression::new_parenthesized_expression(span, sequence, ctx)
        };
        if !slot.admits(&replacement, false) {
            self.decisions
                .skipped(span, "the body would put a bare name under `typeof`");
            return;
        }
        ForgetNames.visit_expression(&replacement);
        *expression = slot.wrap(replacement, ctx);
        if let Some(frame) = self.frames.last_mut()
            && let Some(item) = frame.item
        {
            let mut declarations = declarations.into_iter().peekable();
            while let Some(&(_, _, mutable)) = declarations.peek() {
                let mut declarators = ArenaVec::new_in(ctx);
                while let Some((name, init, _)) =
                    declarations.next_if(|(_, _, other)| *other == mutable)
                {
                    ForgetNames.visit_expression(&init);
                    declarators.push(VariableDeclarator::new(
                        Span::default(),
                        BindingPattern::new_binding_identifier(
                            Span::default(),
                            Ident::from_str_in(&name, ctx),
                            ctx,
                        ),
                        None,
                        Some(init),
                        false,
                        ctx,
                    ));
                }
                let kind = if mutable {
                    VariableDeclarationKind::Let
                } else {
                    VariableDeclarationKind::Const
                };
                let declaration = Statement::new_variable_declaration(
                    Span::default(),
                    kind,
                    declarators,
                    false,
                    ctx,
                );
                frame.declarations.push((item, declaration));
            }
        }
        self.inlined.insert(site.callee);
        self.decisions.applied(span);
    }
}

impl<'a> Visitor<'a, '_> {
    fn before_statement(&mut self, statement: &Statement<'a>, _ctx: &TraverseCtx<'a, ()>) {
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        if frame.depth == 0 {
            frame.item = Some(frame.item.map_or(0, |item| item + 1));
            frame.leading = leading_call(statement);
        }
        frame.depth += 1;
    }

    fn after_statement(&mut self, _statement: &Statement<'a>, _ctx: &TraverseCtx<'a, ()>) {
        if let Some(frame) = self.frames.last_mut() {
            frame.depth -= 1;
        }
    }
}

/// The call a statement evaluates before anything else, when it is a whole initialiser of its
/// first declaration (`const r = f(a)`) or a `return` argument: the call's arguments can be
/// evaluated by declarations just before the statement.
fn leading_call(statement: &Statement<'_>) -> Option<Span> {
    let call = match statement {
        Statement::VariableDeclaration(declaration) if !declaration.declare => {
            declaration.declarations.first()?.init.as_ref()?
        }
        Statement::ReturnStatement(ret) => ret.argument.as_ref()?,
        _ => return None,
    };
    match call {
        Expression::CallExpression(call) => Some(call.span),
        _ => None,
    }
}

/// Whether the expression being visited is inside a statement list of its own function (not a
/// concise arrow body, a parameter default or a class member).
fn list_in_same_function(ctx: &TraverseCtx<'_, ()>) -> bool {
    for ancestor in ctx.ancestors() {
        match ancestor {
            Ancestor::FunctionBodyStatements(_)
            | Ancestor::ProgramBody(_)
            | Ancestor::BlockStatementBody(_)
            | Ancestor::SwitchCaseConsequent(_)
            | Ancestor::StaticBlockBody(_) => return true,
            Ancestor::FunctionParams(_)
            | Ancestor::ArrowFunctionExpressionParams(_)
            | Ancestor::ArrowFunctionExpressionBody(_)
            | Ancestor::PropertyDefinitionValue(_)
            | Ancestor::PropertyDefinitionKey(_)
            | Ancestor::AccessorPropertyValue(_)
            | Ancestor::ClassBody(_) => return false,
            _ => {}
        }
    }
    false
}

/// `undefined`, or `void 0` where it is shadowed.
fn undefined<'a>(span: Span, scope: ScopeId, ctx: &TraverseCtx<'a, ()>) -> Expression<'a> {
    if is_global_name(ctx.scoping(), scope, "undefined") {
        Expression::new_identifier(span, Ident::from_str_in("undefined", ctx), ctx)
    } else {
        Expression::new_unary_expression(
            span,
            UnaryOperator::Void,
            Expression::new_numeric_literal(
                span,
                0.0,
                None,
                oxc_ast::ast::NumberBase::Decimal,
                ctx,
            ),
            ctx,
        )
    }
}

impl<'a> Visitor<'a, '_> {
    /// Whether the call may be inlined: the callee, the facts and the names. `Err(None)` for
    /// calls that aren't candidates at all (not to a function declaration of the module).
    fn check(
        &self,
        call: &CallExpression<'a>,
        scope: ScopeId,
        scoping: &Scoping,
    ) -> Result<Site, Option<String>> {
        if call.optional {
            return Err(None);
        }
        let node = match &call.callee {
            Expression::Identifier(ident) => {
                let symbol = ident
                    .reference_id
                    .get()
                    .and_then(|reference| scoping.get_reference(reference).symbol_id())
                    .ok_or(None)?;
                *self.index.by_symbol.get(&symbol).ok_or(None)?
            }
            // A method of a receiver: what the facts say runs there.
            Expression::StaticMemberExpression(member) if !member.optional => {
                let site = self.query.node(call.node_id()).ok_or(None)?;
                let resolution = self.query.facts.site_resolution(site);
                let [unit] = resolution.callees.as_slice() else {
                    return Err(None);
                };
                let callee = self.index.callees.get(&unit.node).ok_or(None)?;
                if unit.module != self.query.module || !callee.method {
                    return Err(None);
                }
                // The call throws on a nullish receiver; the inlined body might not.
                let receiver = self.query.usable_ty(member.object.node_id());
                if !receiver.is_some_and(super::util::never_nullish) {
                    return Err(Some(format!(
                        "the receiver of `{}` isn't proven to be an object (the call would \
                         throw on `null` or `undefined`)",
                        callee.name
                    )));
                }
                if !self.receiver_copyable(&member.object, scoping, call.node_id()) {
                    return Err(Some(format!(
                        "the receiver of `{}` isn't a name that stays the same or `this`",
                        callee.name
                    )));
                }
                unit.node
            }
            _ => return Err(None),
        };
        let callee = self.index.callees.get(&node).ok_or(None)?;
        if callee.method != !matches!(call.callee, Expression::Identifier(_)) {
            return Err(None);
        }
        if let Some(problem) = &callee.problem {
            return Err(Some(format!("`{}` {problem}", callee.name)));
        }
        if callee.island {
            return Err(Some(format!(
                "`{}` is a WASM island function (an explicit request stays)",
                callee.name
            )));
        }
        if self.index.in_island.contains(&call.node_id()) {
            return Err(Some("the call is inside a WASM island function".into()));
        }
        if call
            .arguments
            .iter()
            .any(|argument| matches!(argument, Argument::SpreadElement(_)))
        {
            return Err(Some("the call spreads its arguments".into()));
        }
        let Some(site) = self.query.node(call.node_id()) else {
            return Err(None);
        };
        let facts = self.query.facts;
        let resolution = facts.site_resolution(site);
        if !self.query.usable(resolution.complete.trust) {
            return Err(Some(weak(
                resolution.complete.trust,
                "whether the callee is all that runs at the call",
            )));
        }
        let Some(unit) = self.query.node(callee.node) else {
            return Err(Some(format!(
                "`{}` was made by an earlier pass and has no facts",
                callee.name
            )));
        };
        let unit = Some(unit);
        if !resolution.complete.value
            || resolution.callees.len() != 1
            || Some(resolution.callees[0]) != unit
        {
            return Err(Some(
                "other code than the callee may run at the call".into(),
            ));
        }
        for (name, binding) in &callee.free {
            if scoping.find_binding(scope, name.as_str().into()) != *binding {
                return Err(Some(format!(
                    "`{name}` means something else at the call than in `{}`",
                    callee.name
                )));
            }
        }
        let only_site = unit.is_some_and(|unit| {
            let complete = facts.callers_complete(unit);
            complete.value
                && self.query.usable(complete.trust)
                && facts.call_graph().callers(unit).count() == 1
        });
        if callee.size > LARGE || (!only_site && callee.size > SMALL) {
            return Err(Some(format!(
                "`{}` is too big to copy ({} nodes; the budget is {SMALL}, or {LARGE} for an \
                 only call site)",
                callee.name, callee.size
            )));
        }
        Ok(Site { callee: node })
    }

    /// How each parameter gets its value at this call, without changing anything.
    fn plan(
        &self,
        call: &CallExpression<'a>,
        callee: NodeId,
        scoping: &Scoping,
    ) -> Result<Plan, String> {
        let template = self.index.callees[&callee]
            .template
            .expect("eligible callees have a template");
        let parameters = &template.params.items;
        let count = parameters.len();
        let arguments: Vec<&Expression<'a>> = call
            .arguments
            .iter()
            .map(|argument| argument.as_expression().expect("no spread arguments"))
            .collect();
        let copyable: Vec<bool> = arguments
            .iter()
            .map(|argument| self.copyable(argument, scoping, call.node_id()))
            .collect();
        let mut parameters_plan = Vec::new();
        for (position, parameter) in parameters.iter().enumerate() {
            let BindingPattern::BindingIdentifier(id) = &parameter.pattern else {
                return Err("destructured parameter".into());
            };
            let symbol = id.symbol_id();
            let mutable = scoping.symbol_is_mutated(symbol);
            let argument = arguments.get(position);
            let how = if parameter.initializer.is_some() {
                if !copyable.iter().skip(position + 1).all(|&copyable| copyable) {
                    return Err(
                        "a default parameter is followed by computed arguments, whose \
                                evaluation would move past the default's"
                            .into(),
                    );
                }
                match argument {
                    None => How::LocalDefault,
                    Some(argument) if is_defined_literal(argument) => {
                        if mutable {
                            How::LocalArgument(position)
                        } else {
                            How::CopyArgument(position)
                        }
                    }
                    Some(_) => return Err("a default parameter gets a computed argument".into()),
                }
            } else if argument.is_some() {
                if copyable[position] && !mutable {
                    How::CopyArgument(position)
                } else {
                    How::LocalArgument(position)
                }
            } else if mutable {
                How::LocalUndefined
            } else {
                How::CopyUndefined
            };
            parameters_plan.push((symbol, id.name.to_string(), mutable, how));
        }
        // One computed argument whose parameter the body reads once, as the first thing it
        // evaluates, can go straight into that use: nothing moves past it.
        let callee_info = &self.index.callees[&callee];
        if callee_info.single_return && template.params.rest.is_none() {
            // Every other parameter must be copied: a local (even of a reassigned parameter)
            // is assigned before the body, so it would run before the moved argument.
            let locals: Vec<usize> = parameters_plan
                .iter()
                .enumerate()
                .filter(|(_, (_, _, _, how))| {
                    !matches!(how, How::CopyArgument(_) | How::CopyUndefined)
                })
                .map(|(index, _)| index)
                .collect();
            let defaults = parameters
                .iter()
                .any(|parameter| parameter.initializer.is_some());
            let extra_computed = (count..arguments.len()).any(|position| !copyable[position]);
            if let [index] = locals.as_slice()
                && !defaults
                && !extra_computed
                && parameters_plan
                    .iter()
                    .all(|(_, _, _, how)| !matches!(how, How::LocalUndefined))
            {
                let symbol = parameters_plan[*index].0;
                let body = template.body.as_ref().expect("callees have bodies");
                let value = match body.statements.first() {
                    Some(Statement::ReturnStatement(ret)) => ret.argument.as_ref(),
                    _ => None,
                };
                if let Some(value) = value
                    && uses(value, symbol, scoping) == 1
                    && first_read(value, scoping) == Some(symbol)
                    && !parameters_plan[*index].2
                    && let How::LocalArgument(position) = parameters_plan[*index].3
                {
                    parameters_plan[*index].3 = How::CopyArgument(position);
                }
            }
        }
        let mut extra = Vec::new();
        if let Some(rest) = &template.params.rest {
            let BindingPattern::BindingIdentifier(id) = &rest.rest.argument else {
                return Err("destructured rest parameter".into());
            };
            let symbol = id.symbol_id();
            parameters_plan.push((
                symbol,
                id.name.to_string(),
                scoping.symbol_is_mutated(symbol),
                How::Rest(count),
            ));
        } else {
            extra.extend((count..arguments.len()).filter(|&position| !copyable[position]));
        }
        Ok(Plan {
            parameters: parameters_plan,
            extra,
        })
    }

    /// Carries out `plan`: moves the arguments that get locals out of the call (which goes).
    fn apply(
        &mut self,
        call: &mut CallExpression<'a>,
        callee: NodeId,
        plan: Plan,
        scope: ScopeId,
        ctx: &TraverseCtx<'a, ()>,
    ) -> Arguments<'a> {
        let template = self.index.callees[&callee]
            .template
            .expect("eligible callees have a template");
        let scoping = ctx.scoping();
        let mut taken: Vec<Option<Expression<'a>>> = call
            .arguments
            .iter_mut()
            .map(|argument| argument.as_expression_mut().map(|e| e.take_in(&ctx.ast)))
            .collect();
        let mut values = FxHashMap::default();
        let mut order = Vec::new();
        for (position, (symbol, base, mutable, how)) in plan.parameters.into_iter().enumerate() {
            let local = |init: Expression<'a>, namer: &mut Namer| Value::Local {
                name: namer.fresh(&base, scope, scoping),
                init,
                mutable,
                callee_code: false,
            };
            let value = match how {
                How::CopyArgument(at) => Value::Copy(unnamed(
                    taken[at].take().expect("present"),
                    &AstBuilder::new(self.allocator),
                )),
                How::CopyUndefined => Value::Copy(undefined(call.span, scope, ctx)),
                How::LocalArgument(at) => local(
                    unnamed(
                        taken[at].take().expect("present"),
                        &AstBuilder::new(self.allocator),
                    ),
                    &mut self.namer,
                ),
                How::LocalUndefined => local(undefined(call.span, scope, ctx), &mut self.namer),
                How::LocalDefault => {
                    let default = template.params.items[position]
                        .initializer
                        .as_ref()
                        .expect("a default")
                        .clone_in_with_semantic_ids(self.allocator);
                    ForgetIds.visit_expression(&default);
                    match local(default.unbox(), &mut self.namer) {
                        Value::Local {
                            name,
                            init,
                            mutable,
                            ..
                        } => Value::Local {
                            name,
                            init,
                            mutable,
                            callee_code: true,
                        },
                        copy @ Value::Copy(_) => copy,
                    }
                }
                How::Rest(from) => {
                    let elements = taken
                        .iter_mut()
                        .skip(from)
                        .filter_map(Option::take)
                        .map(ArrayExpressionElement::from);
                    let array = Expression::new_array_expression(
                        call.span,
                        ArenaVec::from_iter_in(elements, ctx),
                        ctx,
                    );
                    local(array, &mut self.namer)
                }
            };
            values.insert(symbol, value);
            order.push(symbol);
        }
        let extra = plan
            .extra
            .iter()
            .filter_map(|&position| taken[position].take())
            .collect();
        Arguments {
            values,
            order,
            extra,
        }
    }

    /// Whether a method call's receiver may stand for `this` in each use: `this`, or a name that
    /// is never reassigned and whose read can't throw.
    fn receiver_copyable(
        &self,
        receiver: &Expression<'_>,
        scoping: &Scoping,
        call: NodeId,
    ) -> bool {
        matches!(
            receiver,
            Expression::ThisExpression(_) | Expression::Identifier(_)
        ) && self.copyable(receiver, scoping, call)
    }

    /// Whether `argument` may be copied into each use of its parameter: a literal, a name that
    /// is never reassigned and whose read can't throw, or `this` where it is bound.
    fn copyable(&self, argument: &Expression<'_>, scoping: &Scoping, call: NodeId) -> bool {
        match argument.without_parentheses() {
            Expression::ThisExpression(_) => !self.index.unbound_this.contains(&call),
            Expression::NumericLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_) => true,
            Expression::StringLiteral(literal) => {
                !literal.lone_surrogates
                    && small_enough(&trioxide_analysis::Constant::String(
                        literal.value.to_string(),
                    ))
            }
            Expression::UnaryExpression(unary) => {
                unary.operator == UnaryOperator::UnaryNegation
                    && matches!(unary.argument, Expression::NumericLiteral(_))
            }
            Expression::Identifier(ident) => {
                let Some(symbol) = ident
                    .reference_id
                    .get()
                    .and_then(|reference| scoping.get_reference(reference).symbol_id())
                else {
                    return super::util::is_global_constant(ident, scoping);
                };
                !scoping.symbol_is_mutated(symbol) && self.query.pure(ident.node_id())
            }
            _ => false,
        }
    }

    /// Inlines the call in `statements[index]` if it is a statement form. Returns how many
    /// statements to move past.
    fn statement(
        &mut self,
        statements: &mut ArenaVec<'a, Statement<'a>>,
        index: usize,
        ctx: &mut TraverseCtx<'a, ()>,
    ) -> usize {
        let statement = &mut statements[index];
        if self.query.dead(statement.node_id()) {
            return 1;
        }
        let (mode, call) = match statement {
            Statement::ExpressionStatement(expression) => match &mut expression.expression {
                Expression::CallExpression(call) => (Mode::Discard, call),
                Expression::AssignmentExpression(assignment)
                    if assignment.operator == AssignmentOperator::Assign
                        && matches!(
                            assignment.left,
                            AssignmentTarget::AssignmentTargetIdentifier(_)
                        ) =>
                {
                    match &mut assignment.right {
                        Expression::CallExpression(call) => (Mode::Assign, call),
                        _ => return 1,
                    }
                }
                _ => return 1,
            },
            Statement::VariableDeclaration(declaration)
                if declaration.declarations.len() == 1
                    && !declaration.declare
                    && matches!(
                        declaration.kind,
                        VariableDeclarationKind::Const
                            | VariableDeclarationKind::Let
                            | VariableDeclarationKind::Var
                    ) =>
            {
                let declarator = &mut declaration.declarations[0];
                match (&declarator.id, &mut declarator.init) {
                    (
                        BindingPattern::BindingIdentifier(_),
                        Some(Expression::CallExpression(call)),
                    ) => (Mode::Declare, call),
                    _ => return 1,
                }
            }
            Statement::ReturnStatement(ret) => match &mut ret.argument {
                Some(Expression::CallExpression(call)) => (Mode::Return, call),
                _ => return 1,
            },
            _ => return 1,
        };
        let scope = ctx.current_scope_id();
        let span = call.span;
        let site = match self.check(call, scope, ctx.scoping()) {
            Ok(site) => site,
            Err(Some(reason)) => {
                self.decisions.skipped(span, reason);
                return 1;
            }
            Err(None) => return 1,
        };
        let callee = &self.index.callees[&site.callee];
        if mode != Mode::Discard && callee.single_return {
            // The expression form took it (or logged why not).
            return 1;
        }
        let simple = callee.simple_returns;
        let template = callee.template.expect("eligible callees have a template");
        let label_base = callee.name.clone();
        let call = match &mut statements[index] {
            Statement::ExpressionStatement(expression) => match &mut expression.expression {
                Expression::CallExpression(call) => call,
                Expression::AssignmentExpression(assignment) => match &mut assignment.right {
                    Expression::CallExpression(call) => call,
                    _ => unreachable!("matched above"),
                },
                _ => unreachable!("matched above"),
            },
            Statement::VariableDeclaration(declaration) => {
                match &mut declaration.declarations[0].init {
                    Some(Expression::CallExpression(call)) => call,
                    _ => unreachable!("matched above"),
                }
            }
            Statement::ReturnStatement(ret) => match &mut ret.argument {
                Some(Expression::CallExpression(call)) => call,
                _ => unreachable!("matched above"),
            },
            _ => unreachable!("matched above"),
        };
        let plan = match self.plan(call, site.callee, ctx.scoping()) {
            Ok(plan) => plan,
            Err(reason) => {
                self.decisions.skipped(span, reason);
                return 1;
            }
        };
        let receiver = receiver_of(call, self.allocator);
        let mut arguments = self.apply(call, site.callee, plan, scope, ctx);
        let mut out: Vec<Statement<'a>> = Vec::new();
        let mut copies = FxHashMap::default();
        let mut renames = FxHashMap::default();
        let mut locals_in_order = Vec::new();
        for symbol in &arguments.order {
            match arguments.values.remove(symbol) {
                Some(Value::Copy(expression)) => {
                    copies.insert(*symbol, expression);
                }
                Some(Value::Local {
                    name,
                    init,
                    mutable,
                    callee_code,
                }) => {
                    renames.insert(*symbol, name.clone());
                    locals_in_order.push((name, init, mutable, callee_code));
                }
                None => {}
            }
        }
        for (name, mut init, mutable, callee_code) in locals_in_order {
            if callee_code {
                Rename {
                    scoping: ctx.scoping(),
                    copies: &copies,
                    renames: &renames,
                    this: receiver.as_ref(),
                    ast: AstBuilder::new(self.allocator),
                    mode: None,
                }
                .visit_expression(&mut init);
            }
            out.push(declare(
                if mutable {
                    VariableDeclarationKind::Let
                } else {
                    VariableDeclarationKind::Const
                },
                &name,
                Some(init),
                span,
                ctx,
            ));
        }
        for extra in arguments.extra.drain(..) {
            out.push(Statement::new_expression_statement(span, extra, ctx));
        }
        // The callee's own locals.
        let body = template.body.as_ref().expect("callees have bodies");
        for symbol in locals(body) {
            let name = ctx.scoping().symbol_name(symbol).to_string();
            renames.insert(symbol, self.namer.fresh(&name, scope, ctx.scoping()));
        }
        let statement = &mut statements[index];
        let target = match (mode, &mut *statement) {
            (Mode::Declare, Statement::VariableDeclaration(declaration)) => declaration
                .declarations[0]
                .id
                .get_binding_identifier()
                .map(|id| id.name.to_string()),
            (Mode::Assign, Statement::ExpressionStatement(expression)) => {
                match &expression.expression {
                    Expression::AssignmentExpression(assignment) => match &assignment.left {
                        AssignmentTarget::AssignmentTargetIdentifier(ident) => {
                            Some(ident.name.to_string())
                        }
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        };
        let ends_with_return =
            matches!(body.statements.last(), Some(Statement::ReturnStatement(_)));
        let label =
            (!simple && mode != Mode::Return).then(|| enclosing_free_label(&label_base, ctx));
        // `x = f()` with several returns goes through a result local, assigned at the end.
        let result = match (mode, &label) {
            (Mode::Assign | Mode::Declare, Some(_)) => Some(self.namer.fresh(
                &format!("{label_base}_result"),
                scope,
                ctx.scoping(),
            )),
            (Mode::Declare | Mode::Assign, _) => target.clone(),
            _ => None,
        };
        let mut body_statements: Vec<Statement<'a>> = body
            .statements
            .iter()
            .map(|statement| statement.clone_in_with_semantic_ids(self.allocator))
            .collect();
        let final_return = if ends_with_return {
            match body_statements.pop() {
                Some(Statement::ReturnStatement(mut ret)) => Some(ret.argument.take()),
                _ => None,
            }
        } else {
            None
        };
        let mut rename = Rename {
            scoping: ctx.scoping(),
            copies: &copies,
            renames: &renames,
            this: receiver.as_ref(),
            ast: AstBuilder::new(self.allocator),
            mode: Some(Returns {
                mode,
                target: result.clone(),
                label: label.clone(),
            }),
        };
        for statement in &mut body_statements {
            ForgetIds.visit_statement(statement);
            rename.visit_statement(statement);
        }
        let final_value: Option<Option<Expression<'a>>> = final_return.map(|value| {
            value.map(|mut value| {
                ForgetIds.visit_expression(&value);
                rename.mode = None;
                rename.visit_expression(&mut value);
                value
            })
        });
        let span_of = statements[index].span();
        let mut prefix = Vec::new();
        let mut suffix = Vec::new();
        match mode {
            Mode::Discard => {
                if let Some(Some(value)) = final_value {
                    body_statements.push(Statement::new_expression_statement(span, value, ctx));
                }
            }
            Mode::Return => match final_value {
                Some(value) => {
                    body_statements.push(Statement::new_return_statement(span, value, ctx));
                }
                None => suffix.push(Statement::new_return_statement(span, None, ctx)),
            },
            Mode::Declare => {
                let Statement::VariableDeclaration(declaration) = &mut statements[index] else {
                    unreachable!("matched above");
                };
                let mut declaration = declaration.take_in(&ctx.ast);
                let value = if label.is_some() {
                    // The block assigns a fresh `let`; the declaration stays after it, so the
                    // binding is in its temporal dead zone while the body runs, as before.
                    let result = result.clone().expect("a result local");
                    prefix.push(declare(
                        VariableDeclarationKind::Let,
                        &result,
                        None,
                        span,
                        ctx,
                    ));
                    if let Some(Some(value)) = final_value {
                        body_statements.push(assign(&result, value, span, ctx));
                    }
                    Expression::new_identifier(span, Ident::from_str_in(&result, ctx), ctx)
                } else {
                    match final_value {
                        Some(Some(value)) => value,
                        _ => undefined(span, scope, ctx),
                    }
                };
                declaration.declarations[0].init = Some(value);
                suffix.push(Statement::VariableDeclaration(ctx.alloc(declaration)));
            }
            Mode::Assign => {
                let target = target.clone().expect("assignments have a target");
                if label.is_some() {
                    let result = result.clone().expect("a result local");
                    prefix.push(declare(
                        VariableDeclarationKind::Let,
                        &result,
                        None,
                        span,
                        ctx,
                    ));
                    if let Some(Some(value)) = final_value {
                        body_statements.push(assign(&result, value, span, ctx));
                    }
                    let value =
                        Expression::new_identifier(span, Ident::from_str_in(&result, ctx), ctx);
                    suffix.push(assign(&target, value, span, ctx));
                } else {
                    let value = match final_value {
                        Some(Some(value)) => value,
                        _ => undefined(span, scope, ctx),
                    };
                    suffix.push(assign(&target, value, span, ctx));
                }
            }
        }
        out.append(&mut body_statements);
        let mut replacement = prefix;
        match &label {
            Some(label) => {
                let block =
                    Statement::new_block_statement(span, ArenaVec::from_iter_in(out, ctx), ctx);
                replacement.push(Statement::new_labeled_statement(
                    span,
                    LabelIdentifier::new(span, Ident::from_str_in(label, ctx), ctx),
                    block,
                    ctx,
                ));
            }
            None => replacement.append(&mut out),
        }
        replacement.append(&mut suffix);
        // The first statement takes the original's place (and its comments).
        if let Some(first) = replacement.first_mut() {
            set_statement_span(first, span_of);
        }
        for statement in &replacement {
            ForgetNames.visit_statement(statement);
        }
        let count = replacement.len();
        statements.remove(index);
        for (offset, statement) in replacement.into_iter().enumerate() {
            statements.insert(index + offset, statement);
        }
        self.inlined.insert(site.callee);
        self.decisions.applied(span);
        count.max(1)
    }
}

/// `base`, or `base_1`, `base_2`, … if a label around the visited statement list has it.
fn enclosing_free_label(base: &str, ctx: &TraverseCtx<'_, ()>) -> String {
    let labels: FxHashSet<String> = ctx
        .ancestors()
        .filter_map(|ancestor| match ancestor {
            Ancestor::LabeledStatementBody(labeled) => Some(labeled.label().name.to_string()),
            _ => None,
        })
        .collect();
    let mut name = base.to_string();
    let mut counter = 1;
    while labels.contains(&name) {
        name = format!("{base}_{counter}");
        counter += 1;
    }
    name
}

/// A copy of a method call's receiver (`p` in `p.m()`), if the callee is a member.
fn receiver_of<'a>(call: &CallExpression<'a>, allocator: &'a Allocator) -> Option<Expression<'a>> {
    match &call.callee {
        Expression::StaticMemberExpression(member) => Some(member.object.clone_in(allocator)),
        _ => None,
    }
}

/// How many times `expression` reads `symbol`.
fn uses(expression: &Expression<'_>, symbol: SymbolId, scoping: &Scoping) -> usize {
    struct Count<'s> {
        scoping: &'s Scoping,
        symbol: SymbolId,
        found: usize,
    }
    impl<'a> Visit<'a> for Count<'_> {
        fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
            if ident
                .reference_id
                .get()
                .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
                == Some(self.symbol)
            {
                self.found += 1;
            }
        }
    }
    let mut count = Count {
        scoping,
        symbol,
        found: 0,
    };
    count.visit_expression(expression);
    count.found
}

/// The binding `expression` reads before evaluating anything else, if it starts with a name.
fn first_read(expression: &Expression<'_>, scoping: &Scoping) -> Option<SymbolId> {
    match expression {
        Expression::Identifier(ident) => ident
            .reference_id
            .get()
            .and_then(|reference| scoping.get_reference(reference).symbol_id()),
        Expression::ParenthesizedExpression(e) => first_read(&e.expression, scoping),
        Expression::TSAsExpression(e) => first_read(&e.expression, scoping),
        Expression::TSSatisfiesExpression(e) => first_read(&e.expression, scoping),
        Expression::TSNonNullExpression(e) => first_read(&e.expression, scoping),
        Expression::StaticMemberExpression(member) => first_read(&member.object, scoping),
        Expression::ComputedMemberExpression(member) => first_read(&member.object, scoping),
        // A name called directly would bind `this` differently once it is a member.
        Expression::CallExpression(call) if !matches!(call.callee, Expression::Identifier(_)) => {
            first_read(&call.callee, scoping)
        }
        Expression::BinaryExpression(binary) => first_read(&binary.left, scoping),
        Expression::LogicalExpression(logical) => first_read(&logical.left, scoping),
        Expression::ConditionalExpression(conditional) => first_read(&conditional.test, scoping),
        Expression::UnaryExpression(unary)
            if !matches!(
                unary.operator,
                UnaryOperator::Typeof | UnaryOperator::Delete
            ) =>
        {
            first_read(&unary.argument, scoping)
        }
        Expression::SequenceExpression(sequence) => sequence
            .expressions
            .first()
            .and_then(|first| first_read(first, scoping)),
        _ => None,
    }
}

/// Clears the reference and symbol ids under a node the inliner made, so a later inlining in
/// the same run (of a call whose argument holds this code) can't take its names for the
/// callee's; the scoping is rebuilt after the pass.
struct ForgetNames;

impl<'a> Visit<'a> for ForgetNames {
    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        ident.reference_id.set(None);
    }

    fn visit_binding_identifier(&mut self, ident: &oxc_ast::ast::BindingIdentifier<'a>) {
        ident.symbol_id.set(None);
    }
}

/// Gives a statement made by the inliner the span of the one it replaces, so that one's
/// comments stay with it.
fn set_statement_span(statement: &mut Statement<'_>, span: Span) {
    use oxc_span::GetSpanMut;
    *statement.span_mut() = span;
}

/// The bindings a function body declares (outside nested functions), in order.
fn locals(body: &oxc_ast::ast::FunctionBody<'_>) -> Vec<SymbolId> {
    struct Collect(Vec<SymbolId>);
    impl<'a> Visit<'a> for Collect {
        fn visit_binding_identifier(&mut self, ident: &oxc_ast::ast::BindingIdentifier<'a>) {
            if let Some(symbol) = ident.symbol_id.get() {
                self.0.push(symbol);
            }
        }
    }
    let mut collect = Collect(Vec::new());
    collect.visit_function_body(body);
    collect.0
}

/// `kind name = init;`, with an empty span at the start of `span` (references made with
/// `span` must not count as inside the declaration).
fn declare<'a>(
    kind: VariableDeclarationKind,
    name: &str,
    init: Option<Expression<'a>>,
    span: Span,
    ctx: &TraverseCtx<'a, ()>,
) -> Statement<'a> {
    let span = Span::empty(span.start);
    let declarator = VariableDeclarator::new(
        span,
        BindingPattern::new_binding_identifier(span, Ident::from_str_in(name, ctx), ctx),
        None,
        init,
        false,
        ctx,
    );
    Statement::new_variable_declaration(
        span,
        kind,
        ArenaVec::from_iter_in([declarator], ctx),
        false,
        ctx,
    )
}

/// `name = value;`
fn assign<'a>(
    name: &str,
    value: Expression<'a>,
    span: Span,
    ast: &(impl oxc_ast::builder::GetAstBuilder<'a> + GetAllocator<'a>),
) -> Statement<'a> {
    let target = AssignmentTarget::new_assignment_target_identifier(
        span,
        Ident::from_str_in(name, ast),
        ast,
    );
    Statement::new_expression_statement(
        span,
        Expression::new_assignment_expression(span, AssignmentOperator::Assign, target, value, ast),
        ast,
    )
}

/// Whether `expression` is a literal that isn't `undefined`.
fn is_defined_literal(expression: &Expression<'_>) -> bool {
    match expression.without_parentheses() {
        Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_) => true,
        Expression::StringLiteral(literal) => !literal.lone_surrogates,
        Expression::UnaryExpression(unary) => {
            unary.operator == UnaryOperator::UnaryNegation
                && matches!(unary.argument, Expression::NumericLiteral(_))
        }
        _ => false,
    }
}

/// What the callee's `return`s become.
struct Returns {
    mode: Mode,
    /// The name returns assign (declarations and assignments).
    target: Option<String>,
    /// The block returns break out of, when one is needed.
    label: Option<String>,
}

/// Renames a copy of the callee's code: parameters to their values, locals to fresh names,
/// and (in statement forms) `return`s to assignments and `break`s.
struct Rename<'s, 'a> {
    scoping: &'s Scoping,
    copies: &'s FxHashMap<SymbolId, Expression<'a>>,
    renames: &'s FxHashMap<SymbolId, String>,
    /// A method call's receiver, which `this` becomes.
    this: Option<&'s Expression<'a>>,
    ast: AstBuilder<'a>,
    mode: Option<Returns>,
}

impl<'a> Rename<'_, 'a> {
    fn symbol(&self, ident: &IdentifierReference<'a>) -> Option<SymbolId> {
        ident
            .reference_id
            .get()
            .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
    }

    /// What a callee `return` becomes.
    fn returned(&self, span: Span, value: Option<Expression<'a>>) -> Option<Statement<'a>> {
        let returns = self.mode.as_ref()?;
        let ast = &self.ast;
        let mut statements = Vec::new();
        match returns.mode {
            Mode::Return => return None,
            Mode::Discard => {
                if let Some(value) = value {
                    statements.push(Statement::new_expression_statement(span, value, ast));
                }
            }
            Mode::Declare | Mode::Assign => {
                let target = returns.target.as_ref()?;
                let value = value.unwrap_or_else(|| {
                    Expression::new_unary_expression(
                        span,
                        UnaryOperator::Void,
                        Expression::new_numeric_literal(
                            span,
                            0.0,
                            None,
                            oxc_ast::ast::NumberBase::Decimal,
                            ast,
                        ),
                        ast,
                    )
                });
                statements.push(assign(target, value, span, ast));
            }
        }
        if let Some(label) = &returns.label {
            statements.push(Statement::new_break_statement(
                span,
                Some(LabelIdentifier::new(
                    span,
                    Ident::from_str_in(label, ast),
                    ast,
                )),
                ast,
            ));
        }
        Some(Statement::new_block_statement(
            span,
            ArenaVec::from_iter_in(statements, ast),
            ast,
        ))
    }
}

impl<'a> VisitMut<'a> for Rename<'_, 'a> {
    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        if let (Expression::ThisExpression(_), Some(receiver)) = (&*expression, self.this) {
            *expression = receiver.clone_in(self.ast.allocator());
            return;
        }
        if let Expression::Identifier(ident) = expression
            && let Some(symbol) = self.symbol(ident)
            && let Some(copy) = self.copies.get(&symbol)
        {
            let copy = copy.clone_in(self.ast.allocator());
            let primary = matches!(
                copy,
                Expression::Identifier(_)
                    | Expression::NumericLiteral(_)
                    | Expression::StringLiteral(_)
                    | Expression::BooleanLiteral(_)
                    | Expression::NullLiteral(_)
                    | Expression::ParenthesizedExpression(_)
                    | Expression::ArrayExpression(_)
                    | Expression::ObjectExpression(_)
                    | Expression::StaticMemberExpression(_)
                    | Expression::ComputedMemberExpression(_)
                    | Expression::CallExpression(_)
            );
            *expression = if primary {
                copy
            } else {
                Expression::new_parenthesized_expression(ident.span, copy, &self.ast)
            };
            return;
        }
        walk_mut::walk_expression(self, expression);
    }

    fn visit_identifier_reference(&mut self, ident: &mut IdentifierReference<'a>) {
        if let Some(symbol) = self.symbol(ident)
            && let Some(name) = self.renames.get(&symbol)
        {
            ident.name = Ident::from_str_in(name, &self.ast);
        }
    }

    fn visit_binding_identifier(&mut self, ident: &mut oxc_ast::ast::BindingIdentifier<'a>) {
        if let Some(symbol) = ident.symbol_id.get()
            && let Some(name) = self.renames.get(&symbol)
        {
            ident.name = Ident::from_str_in(name, &self.ast);
        }
    }

    fn visit_object_property(&mut self, property: &mut ObjectProperty<'a>) {
        walk_mut::walk_object_property(self, property);
        if property.shorthand {
            let same = match (&property.key, &property.value) {
                (
                    oxc_ast::ast::PropertyKey::StaticIdentifier(key),
                    Expression::Identifier(value),
                ) => key.name == value.name,
                _ => false,
            };
            property.shorthand = same;
        }
    }

    fn visit_statement(&mut self, statement: &mut Statement<'a>) {
        walk_mut::walk_statement(self, statement);
        // In the `return f(…)` form, the callee's returns stay returns.
        let converts = self
            .mode
            .as_ref()
            .is_some_and(|returns| returns.mode != Mode::Return);
        if converts && let Statement::ReturnStatement(ret) = statement {
            let span = ret.span;
            let value = ret.argument.take();
            if let Some(replacement) = self.returned(span, value) {
                *statement = replacement;
            }
        }
    }
}

/// Removes the functions every call of which was inlined, where nothing else needs them.
/// Returns whether it removed any.
#[expect(
    clippy::items_after_statements,
    reason = "visitors used only here, next to their use"
)]
fn remove_inlined<'a>(
    program: &mut Program<'a>,
    ctx: &mut PassContext<'a, '_>,
    index: &Index<'a>,
    inlined: &FxHashSet<NodeId>,
    decisions: &mut Decisions,
) -> bool {
    let references = live_references(program, ctx.scoping());
    let mut remove: FxHashMap<Span, Span> = FxHashMap::default();
    for node in inlined {
        let callee = &index.callees[node];
        if callee.exported || callee.island || callee.method {
            continue;
        }
        // The scoping was rebuilt, so the symbol is looked up again (node ids stay).
        let Some((statement, symbol)) = find_function(program, callee.node) else {
            continue;
        };
        if references.get(&symbol).is_some_and(|spans| {
            spans
                .iter()
                .any(|span| !statement.contains_inclusive(*span))
        }) {
            continue;
        }
        let unit = ctx.facts.callers_complete(NodeRef {
            module: ctx.module,
            node: callee.node,
        });
        if !(unit.value && ctx.facts.is_usable(unit.trust)) {
            decisions.skipped(
                statement,
                "every call was inlined, but code trioxide doesn't see may call it",
            );
            continue;
        }
        remove.insert(statement, callee.statement);
    }
    if remove.is_empty() {
        return false;
    }
    struct Remove<'r> {
        targets: &'r FxHashMap<Span, Span>,
        comments: Comments,
        removed: Vec<Span>,
    }
    impl<'a> VisitMut<'a> for Remove<'_> {
        fn visit_statements(&mut self, statements: &mut ArenaVec<'a, Statement<'a>>) {
            walk_mut::walk_statements(self, statements);
            let keep: Vec<bool> = statements
                .iter()
                .map(|statement| match statement {
                    Statement::FunctionDeclaration(function) => {
                        let gone = self.targets.contains_key(&function.span);
                        if gone {
                            self.removed.push(function.span);
                        }
                        !gone
                    }
                    _ => true,
                })
                .collect();
            if keep.iter().any(|kept| !kept) {
                self.comments.remove(statements, &keep);
            }
        }
    }
    let mut visitor = Remove {
        targets: &remove,
        comments: Comments::default(),
        removed: Vec::new(),
    };
    visitor.visit_program(program);
    visitor.comments.apply(program);
    for span in visitor.removed {
        decisions.applied_with(span, "removed: every call was inlined");
    }
    true
}

/// The span and binding of the function declaration `node`, if it is still in the program.
fn find_function(program: &Program<'_>, node: NodeId) -> Option<(Span, SymbolId)> {
    struct Find {
        node: NodeId,
        found: Option<(Span, SymbolId)>,
    }
    impl<'a> Visit<'a> for Find {
        fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
            if function.node_id() == self.node
                && let Some(id) = &function.id
            {
                self.found = Some((function.span, id.symbol_id()));
            }
            walk::walk_function(self, function, flags);
        }
    }
    let mut find = Find { node, found: None };
    find.visit_program(program);
    find.found
}

/// Specialisation: drops parameters every caller passes the same literal. Returns whether
/// anything changed.
#[expect(
    clippy::items_after_statements,
    reason = "visitors used only here, next to their use"
)]
fn drop_constant_parameters<'a>(
    program: &mut Program<'a>,
    ctx: &mut PassContext<'a, '_>,
    query: Query<'_>,
    decisions: &mut Decisions,
) -> bool {
    let scoping = ctx.scoping();
    // Every function declaration's references, by kind of use.
    struct Uses {
        callees: FxHashMap<SymbolId, Vec<NodeId>>,
        other: FxHashSet<SymbolId>,
        arguments: FxHashSet<ScopeId>,
        depth: Vec<ScopeId>,
    }
    struct Collect<'s> {
        scoping: &'s Scoping,
        uses: Uses,
    }
    impl<'a> Visit<'a> for Collect<'_> {
        fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
            if let Expression::Identifier(ident) = &call.callee
                && let Some(symbol) = ident
                    .reference_id
                    .get()
                    .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
                && !call.optional
            {
                self.uses
                    .callees
                    .entry(symbol)
                    .or_default()
                    .push(call.node_id());
                for argument in &call.arguments {
                    self.visit_argument(argument);
                }
                return;
            }
            walk::walk_call_expression(self, call);
        }

        fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
            if ident.name == "arguments"
                && let Some(&scope) = self.uses.depth.last()
            {
                self.uses.arguments.insert(scope);
            }
            if let Some(symbol) = ident
                .reference_id
                .get()
                .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
            {
                self.uses.other.insert(symbol);
            }
        }

        fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
            self.uses.depth.push(function.scope_id());
            walk::walk_function(self, function, flags);
            self.uses.depth.pop();
        }
    }
    let mut collect = Collect {
        scoping,
        uses: Uses {
            callees: FxHashMap::default(),
            other: FxHashSet::default(),
            arguments: FxHashSet::default(),
            depth: Vec::new(),
        },
    };
    collect.visit_program(program);
    let uses = collect.uses;
    let tagged = super::wasm_tags(program);
    // Candidates: function symbol → (function node, removable parameter positions and values).
    struct Candidates<'s, 'q> {
        query: Query<'q>,
        keep: &'s Keep,
        tagged: &'s FxHashSet<u32>,
        uses: &'s Uses,
        exported: FxHashSet<NodeId>,
        found: Vec<(SymbolId, NodeId, usize)>,
    }
    impl<'a> Visit<'a> for Candidates<'_, '_> {
        fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
            walk::walk_function(self, function, flags);
            let (Some(id), true) = (&function.id, function.is_declaration()) else {
                return;
            };
            let symbol = id.symbol_id();
            let node = function.node_id();
            let Some(unit) = self.query.node(node) else {
                return;
            };
            if self.exported.contains(&node)
                || is_island(self.keep, Some(unit), super::forced_island(function, self.tagged, &[function.span.start]))
                || self.uses.other.contains(&symbol)
                || self.uses.arguments.contains(&function.scope_id())
                || function.params.rest.is_some()
                // Defaults evaluate before the body, where the new `const`s would be.
                || function.params.items.iter().any(|parameter| {
                    parameter.initializer.is_some()
                        || !matches!(parameter.pattern, BindingPattern::BindingIdentifier(_))
                })
            {
                return;
            }
            let Some(sites) = self.uses.callees.get(&symbol) else {
                return;
            };
            let complete = self.query.facts.callers_complete(unit);
            if !(complete.value && self.query.usable(complete.trust)) {
                return;
            }
            // Every caller edge must be one of the module's direct calls.
            let graph = self.query.facts.call_graph();
            let direct: FxHashSet<NodeId> = sites.iter().copied().collect();
            if graph
                .callers(unit)
                .any(|edge| edge.site.module != unit.module || !direct.contains(&edge.site.node))
            {
                return;
            }
            self.found.push((symbol, node, sites.len()));
        }
    }
    let mut exported = FxHashSet::default();
    for statement in &program.body {
        match statement {
            Statement::ExportDeclaration(export) => {
                if let Declaration::FunctionDeclaration(function) = &export.declaration {
                    exported.insert(function.node_id());
                }
            }
            Statement::ExportDefaultDeclaration(export) => {
                if let ExportDefaultDeclarationKind::FunctionDeclaration(function) =
                    &export.declaration
                {
                    exported.insert(function.node_id());
                }
            }
            _ => {}
        }
    }
    let mut candidates = Candidates {
        query,
        keep: ctx.keep,
        tagged: &tagged,
        uses: &uses,
        exported,
        found: Vec::new(),
    };
    candidates.visit_program(program);
    if candidates.found.is_empty() {
        return false;
    }
    // Arguments at every call, by function.
    let mut calls: FxHashMap<NodeId, SymbolId> = FxHashMap::default();
    let functions: FxHashMap<SymbolId, NodeId> = candidates
        .found
        .iter()
        .map(|(symbol, node, _)| (*symbol, *node))
        .collect();
    for (symbol, _, _) in &candidates.found {
        for site in &uses.callees[symbol] {
            calls.insert(*site, *symbol);
        }
    }
    struct Arguments<'c, 'a> {
        calls: &'c FxHashMap<NodeId, SymbolId>,
        found: FxHashMap<SymbolId, Vec<Vec<Option<trioxide_analysis::Constant>>>>,
        marker: std::marker::PhantomData<&'a ()>,
    }
    impl<'a> Visit<'a> for Arguments<'_, 'a> {
        fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
            if let Some(symbol) = self.calls.get(&call.node_id()) {
                let spread = call
                    .arguments
                    .iter()
                    .any(|argument| matches!(argument, Argument::SpreadElement(_)));
                let values = if spread {
                    Vec::new()
                } else {
                    call.arguments
                        .iter()
                        .map(|argument| argument.as_expression().and_then(literal_constant))
                        .collect()
                };
                self.found.entry(*symbol).or_default().push(values);
            }
            walk::walk_call_expression(self, call);
        }
    }
    let mut arguments = Arguments {
        calls: &calls,
        found: FxHashMap::default(),
        marker: std::marker::PhantomData,
    };
    arguments.visit_program(program);
    // Parameter positions to drop, by function: every call passes the same literal there.
    let mut drop: FxHashMap<NodeId, Vec<(usize, trioxide_analysis::Constant)>> =
        FxHashMap::default();
    for (symbol, node) in &functions {
        let Some(sites) = arguments.found.get(symbol) else {
            continue;
        };
        let width = sites.iter().map(Vec::len).min().unwrap_or(0);
        for position in 0..width {
            let first = &sites[0][position];
            let Some(value) = first else {
                continue;
            };
            if sites.iter().all(|values| {
                values[position]
                    .as_ref()
                    .is_some_and(|other| same(other, value))
            }) {
                drop.entry(*node)
                    .or_default()
                    .push((position, value.clone()));
            }
        }
    }
    if drop.is_empty() {
        return false;
    }
    // Which call sites drop which positions.
    let mut site_drops: FxHashMap<NodeId, Vec<usize>> = FxHashMap::default();
    for (site, symbol) in &calls {
        if let Some(positions) = drop.get(&functions[symbol]) {
            site_drops.insert(
                *site,
                positions.iter().map(|(position, _)| *position).collect(),
            );
        }
    }
    struct Rewrite<'r, 'a> {
        ast: AstBuilder<'a>,
        drop: &'r FxHashMap<NodeId, Vec<(usize, trioxide_analysis::Constant)>>,
        sites: &'r FxHashMap<NodeId, Vec<usize>>,
        mutated: &'r dyn Fn(&BindingPattern<'a>) -> bool,
        dropped: Vec<Span>,
    }
    impl<'a> VisitMut<'a> for Rewrite<'_, 'a> {
        fn visit_call_expression(&mut self, call: &mut CallExpression<'a>) {
            walk_mut::walk_call_expression(self, call);
            if let Some(positions) = self.sites.get(&call.node_id()) {
                for &position in positions.iter().rev() {
                    if position < call.arguments.len() {
                        call.arguments.remove(position);
                    }
                }
            }
        }

        fn visit_function(&mut self, function: &mut Function<'a>, flags: ScopeFlags) {
            walk_mut::walk_function(self, function, flags);
            let Some(positions) = self.drop.get(&function.node_id()) else {
                return;
            };
            let mut constants = Vec::new();
            for (position, value) in positions.iter().rev() {
                let parameter = function.params.items.remove(*position);
                let kind = if (self.mutated)(&parameter.pattern) {
                    VariableDeclarationKind::Let
                } else {
                    VariableDeclarationKind::Const
                };
                self.dropped.push(parameter.span);
                let init = constant_literal(value, parameter.span, &self.ast);
                let declarator = VariableDeclarator::new(
                    parameter.span,
                    parameter.pattern,
                    parameter.type_annotation,
                    Some(init),
                    false,
                    &self.ast,
                );
                constants.push(Statement::new_variable_declaration(
                    parameter.span,
                    kind,
                    ArenaVec::from_iter_in([declarator], &self.ast),
                    false,
                    &self.ast,
                ));
            }
            if let Some(body) = &mut function.body {
                for statement in constants {
                    body.statements.insert(0, statement);
                }
            }
        }
    }
    let scoping = ctx.scoping();
    let mutated = |pattern: &BindingPattern<'a>| {
        pattern
            .get_binding_identifier()
            .is_none_or(|id| scoping.symbol_is_mutated(id.symbol_id()))
    };
    let mut rewrite = Rewrite {
        ast: AstBuilder::new(ctx.allocator),
        drop: &drop,
        sites: &site_drops,
        mutated: &mutated,
        dropped: Vec::new(),
    };
    rewrite.visit_program(program);
    for span in rewrite.dropped {
        decisions.applied_with(span, "parameter removed: every caller passes this value");
    }
    true
}

/// The value of a literal argument (the forms specialisation copies), if it is one.
fn literal_constant(expression: &Expression<'_>) -> Option<trioxide_analysis::Constant> {
    use trioxide_analysis::Constant;
    let constant = match expression.without_parentheses() {
        Expression::NumericLiteral(literal) => Constant::Number(literal.value),
        Expression::BooleanLiteral(literal) => Constant::Boolean(literal.value),
        Expression::NullLiteral(_) => Constant::Null,
        Expression::StringLiteral(literal) if !literal.lone_surrogates => {
            Constant::String(literal.value.to_string())
        }
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::UnaryNegation => {
            match &unary.argument {
                Expression::NumericLiteral(literal) => Constant::Number(-literal.value),
                _ => return None,
            }
        }
        _ => return None,
    };
    if let Constant::Number(value) = constant
        && value.is_nan()
    {
        return None;
    }
    small_enough(&constant).then_some(constant)
}

/// Whether two literal values are the same value (`0` and `-0` are not).
fn same(a: &trioxide_analysis::Constant, b: &trioxide_analysis::Constant) -> bool {
    use trioxide_analysis::Constant;
    match (a, b) {
        (Constant::Number(a), Constant::Number(b)) => a.to_bits() == b.to_bits(),
        _ => a == b,
    }
}

/// The literal for a constant [`literal_constant`] produced.
fn constant_literal<'a>(
    constant: &trioxide_analysis::Constant,
    span: Span,
    ast: &AstBuilder<'a>,
) -> Expression<'a> {
    use trioxide_analysis::Constant;
    match constant {
        Constant::Number(value) => {
            let magnitude = Expression::new_numeric_literal(
                span,
                value.abs(),
                None,
                oxc_ast::ast::NumberBase::Decimal,
                ast,
            );
            if value.is_sign_negative() {
                Expression::new_unary_expression(span, UnaryOperator::UnaryNegation, magnitude, ast)
            } else {
                magnitude
            }
        }
        Constant::Boolean(value) => Expression::new_boolean_literal(span, *value, ast),
        Constant::Null => Expression::new_null_literal(span, ast),
        Constant::String(value) => Expression::new_string_literal(
            span,
            oxc_ast::ast::Str::from_str_in(value, ast),
            None,
            ast,
        ),
        Constant::BigInt(_) | Constant::Undefined => {
            unreachable!("literal_constant makes no bigints or undefined")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_constants() {
        let allocator = Allocator::default();
        let program =
            oxc_parser::Parser::new(&allocator, "f(-0, 'a', NaN, x)", oxc_span::SourceType::ts())
                .parse()
                .program;
        let Statement::ExpressionStatement(statement) = &program.body[0] else {
            panic!()
        };
        let Expression::CallExpression(call) = &statement.expression else {
            panic!()
        };
        let values: Vec<_> = call
            .arguments
            .iter()
            .map(|argument| literal_constant(argument.as_expression().unwrap()))
            .collect();
        assert!(
            matches!(values[0], Some(trioxide_analysis::Constant::Number(value)) if value.is_sign_negative())
        );
        assert!(values[1].is_some());
        assert!(values[2].is_none());
        assert!(values[3].is_none());
    }
}
