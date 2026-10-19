//! PLAN §4 pass 11: compile-time partial evaluation.
//!
//! A call in module top-level code (`const TABLE = buildTable(8)`) to a function declaration of
//! the module, with constant arguments, is evaluated at compile time by a small interpreter and
//! replaced with the literal it returns, when that is small ([`MAX_LEAVES`]).
//!
//! **Facts.** The call must be all that runs there (`site_resolution` complete, listing only the
//! callee), usable-free of unknown calls (`expr_effects`: so the builtins are the real ones;
//! it may write its own objects and throw, which the interpreter sees), and not in code proven
//! never to run.
//!
//! **The interpreter** is deterministic by construction: it knows numbers, strings, booleans,
//! `null`, `undefined`, arrays and plain objects the evaluation creates, the module's function
//! declarations, `const`s with literal values declared before the call ([`Facts::symbol_constant`],
//! imports included), and the exactly specified parts of `Math`, `Number` and `String`
//! (rounding, `sqrt`, `min`/`max`, `imul`, `fround`, `clz32`, constants; not the
//! implementation-approximated functions, `random`, or anything reading a clock, I/O or
//! global state). Anything else (another binding, a closure, `this`, `try`, a getter, a
//! conversion it doesn't model) stops it, and the code stays as it was; so do a `throw`, an
//! exception the engine would raise, and running out of fuel ([`FUEL`] steps).
//!
//! **Results.** Numbers print exactly (`-0`, `NaN`, `Infinity` and every finite double), strings
//! with lone surrogates as `\u` escapes; strings with invisible characters are left alone. An
//! array or object result must be a tree of objects the call created (an object reachable twice
//! would become two), with no hole, accessor or `__proto__` key; it is made fresh at the same
//! point, so nothing could have seen its identity before.
//!
//! **The function goes** when no reference to it is left but in its own body: a top-level,
//! unexported function declaration that isn't an island and that no code trioxide doesn't
//! see can call (`callers_complete`), as inlining does.
//!
//! [`Facts::symbol_constant`]: trioxide_analysis::Facts::symbol_constant

use std::{fmt::Write as _, rc::Rc};

use oxc_allocator::ArenaVec;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, AssignmentOperator, AssignmentTarget, BinaryOperator,
    BindingPattern, CallExpression, Expression, ForStatementInit, ForStatementLeft, Function,
    Ident, IdentifierReference, LogicalOperator, ObjectPropertyKind, Program, PropertyKey,
    PropertyKind, SimpleAssignmentTarget, Statement, Str, UnaryOperator, UpdateOperator,
    VariableDeclarationKind,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::Scoping;
use oxc_span::Span;
use oxc_syntax::{number::ToJsString, scope::ScopeFlags, symbol::SymbolId};
use oxc_traverse::{Traverse, TraverseCtx};
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_analysis::Constant;
use trioxide_common::{NodeId, NodeRef, SymbolRef};

use super::{
    Decisions, Position,
    speed::{is_island, rescope},
    track_position,
    util::{Query, Slot, constant_expr, has_dynamic_scope, live_references, weak},
};
use crate::{Pass, PassContext};

/// The most interpreter steps one call may take.
pub const FUEL: u64 = 200_000;
/// The most primitive values (array elements and object values, nested) a result may have.
pub const MAX_LEAVES: usize = 128;
/// The longest string (in UTF-16 code units) a result may contain.
pub const MAX_STRING: usize = 256;

/// PLAN §4 pass 11 (`"partial-eval"`), the first speed pass of the foundation group.
pub struct PartialEval;

impl Pass for PartialEval {
    fn name(&self) -> &'static str {
        "partial-eval"
    }

    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
        let query = Query {
            facts: ctx.facts,
            module: ctx.module,
        };
        if has_dynamic_scope(program, ctx.scoping()) {
            return false;
        }
        let tagged = super::wasm_tags(program);
        let mut decisions = Decisions::new(self.name());
        let results = {
            let mut functions = FxHashMap::default();
            let mut islands = FxHashSet::default();
            for statement in &program.body {
                let (function, start) = match statement {
                    Statement::FunctionDeclaration(function) => (&**function, function.span.start),
                    Statement::ExportDeclaration(export) => match &export.declaration {
                        oxc_ast::ast::Declaration::FunctionDeclaration(function) => {
                            (&**function, export.span.start)
                        }
                        _ => continue,
                    },
                    _ => continue,
                };
                let Some(id) = &function.id else { continue };
                functions.insert(id.symbol_id(), function);
                if is_island(
                    ctx.keep,
                    query.node(function.node_id()),
                    super::forced_island(function, &tagged, &[function.span.start, start]),
                ) {
                    islands.insert(id.symbol_id());
                }
            }
            let mut find = Find {
                query,
                scoping: ctx.scoping(),
                functions,
                islands,
                results: FxHashMap::default(),
                callees: FxHashMap::default(),
                skipped: Vec::new(),
                depth: 0,
                dead: 0,
            };
            find.visit_program(program);
            for (span, reason) in find.skipped {
                decisions.skipped(span, reason);
            }
            (find.results, find.callees)
        };
        let (results, callees) = results;
        if results.is_empty() {
            return decisions.flush(ctx.log);
        }
        let mut replace = Replace {
            query,
            position: Position::default(),
            decisions,
            results,
            callees,
            replaced: FxHashSet::default(),
        };
        ctx.traverse(&mut replace, program, ());
        let Replace {
            mut decisions,
            replaced,
            ..
        } = replace;
        if remove_unreferenced(program, ctx, &replaced, &mut decisions) {
            *ctx.scoping_mut() = rescope(program);
        }
        decisions.flush(ctx.log)
    }
}

/// Removes the module's top-level function declarations among `functions` (whose calls the
/// pass replaced) that nothing references any more but their own bodies. Returns whether it
/// removed any.
fn remove_unreferenced<'a>(
    program: &mut Program<'a>,
    ctx: &PassContext<'a, '_>,
    functions: &FxHashSet<SymbolId>,
    decisions: &mut Decisions,
) -> bool {
    if functions.is_empty() {
        return false;
    }
    let references = live_references(program, ctx.scoping());
    let mut keep = Vec::with_capacity(program.body.len());
    for statement in &program.body {
        let Statement::FunctionDeclaration(function) = statement else {
            keep.push(true);
            continue;
        };
        let Some(id) = function
            .id
            .as_ref()
            .filter(|id| functions.contains(&id.symbol_id()))
        else {
            keep.push(true);
            continue;
        };
        let elsewhere = references.get(&id.symbol_id()).is_some_and(|spans| {
            spans
                .iter()
                .any(|span| !function.span.contains_inclusive(*span))
        });
        let node = NodeRef {
            module: ctx.module,
            node: function.node_id(),
        };
        let unit = ctx.facts.callers_complete(node);
        let gone = !elsewhere
            && !is_island(ctx.keep, Some(node), false)
            && unit.value
            && ctx.facts.is_usable(unit.trust);
        if gone {
            decisions.applied_with(function.span, "removed: its calls were evaluated");
        } else if !elsewhere {
            decisions.skipped(
                function.span,
                "its calls were evaluated, but code trioxide doesn't see may call it",
            );
        }
        keep.push(!gone);
    }
    if keep.iter().all(|kept| *kept) {
        return false;
    }
    let mut comments = super::Comments::default();
    comments.remove(&mut program.body, &keep);
    comments.apply(program);
    true
}

/// A literal a call evaluated to, ready to print.
#[derive(Debug, Clone)]
enum Literal {
    Constant(Constant),
    /// A string with lone surrogates, as UTF-16.
    Units(Vec<u16>),
    Array(Vec<Literal>),
    Object(Vec<(String, Literal)>),
}

/// Finds top-level calls of the module's functions and evaluates them.
struct Find<'s, 'q, 'p, 'a> {
    query: Query<'q>,
    scoping: &'s Scoping,
    /// The module's top-level function declarations.
    functions: FxHashMap<SymbolId, &'p Function<'a>>,
    islands: FxHashSet<SymbolId>,
    results: FxHashMap<NodeId, Literal>,
    /// The function each evaluated call calls.
    callees: FxHashMap<NodeId, SymbolId>,
    skipped: Vec<(Span, String)>,
    /// Functions, classes and arrows entered.
    depth: usize,
    dead: usize,
}

impl<'a> Visit<'a> for Find<'_, '_, '_, 'a> {
    fn visit_statement(&mut self, statement: &Statement<'a>) {
        let dead = self.query.dead(statement.node_id());
        self.dead += usize::from(dead);
        walk::walk_statement(self, statement);
        self.dead -= usize::from(dead);
    }

    fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
        self.depth += 1;
        walk::walk_function(self, function, flags);
        self.depth -= 1;
    }

    fn visit_arrow_function_expression(
        &mut self,
        arrow: &oxc_ast::ast::ArrowFunctionExpression<'a>,
    ) {
        self.depth += 1;
        walk::walk_arrow_function_expression(self, arrow);
        self.depth -= 1;
    }

    fn visit_class(&mut self, class: &oxc_ast::ast::Class<'a>) {
        self.depth += 1;
        walk::walk_class(self, class);
        self.depth -= 1;
    }

    fn visit_ts_namespace_declaration(&mut self, _: &oxc_ast::ast::TSNamespaceDeclaration<'a>) {}

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if self.depth == 0 && self.dead == 0 && !call.optional {
            match self.evaluate(call) {
                Ok(literal) => {
                    self.results.insert(call.node_id(), literal);
                    if let Expression::Identifier(callee) = &call.callee
                        && let Some(symbol) = self.resolve(callee)
                    {
                        self.callees.insert(call.node_id(), symbol);
                    }
                    return;
                }
                Err(Some(reason)) => self.skipped.push((call.span, reason)),
                Err(None) => {}
            }
        }
        walk::walk_call_expression(self, call);
    }
}

impl<'a> Find<'_, '_, '_, 'a> {
    /// Evaluates a top-level call. `Err(None)`: not a candidate (not a call of a module
    /// function with constant arguments).
    fn evaluate(&self, call: &CallExpression<'a>) -> Result<Literal, Option<String>> {
        let Expression::Identifier(callee) = &call.callee else {
            return Err(None);
        };
        let symbol = self.resolve(callee).ok_or(None)?;
        if !self.functions.contains_key(&symbol) {
            return Err(None);
        }
        let mut interpreter = Interpreter {
            find: self,
            fuel: FUEL,
            heap: Vec::new(),
            frames: vec![FxHashMap::default()],
            depth: 0,
            before: call.span.start,
        };
        // Arguments must be constants (no calls, no variables).
        let mut arguments = Vec::new();
        for argument in &call.arguments {
            let Some(argument) = argument.as_expression() else {
                return Err(None);
            };
            if !constant_argument(argument) {
                return Err(None);
            }
            arguments.push(interpreter.expression(argument).map_err(|_| None)?);
        }
        if self.islands.contains(&symbol) {
            return Err(Some(
                "calls a WASM island function, which stays a call".into(),
            ));
        }
        let site = self.query.node(call.node_id()).ok_or(None)?;
        let facts = self.query.facts;
        let resolution = facts.site_resolution(site);
        if !self.query.usable(resolution.complete.trust) {
            return Err(Some(weak(
                resolution.complete.trust,
                "whether the callee is all that runs at the call",
            )));
        }
        let unit = self.query.node(self.functions[&symbol].node_id());
        if !resolution.complete.value
            || resolution
                .callees
                .iter()
                .any(|callee| Some(*callee) != unit)
        {
            return Err(Some(
                "other code than the callee may run at the call".into(),
            ));
        }
        let effects = facts.expr_effects(site);
        if !self.query.usable(effects.trust) {
            return Err(Some(weak(effects.trust, "the call's effects")));
        }
        // Writes are fine: the interpreter only writes the call's own locals and objects (and
        // stops at anything else); unknown calls mean the builtins may not be the real ones.
        if effects.value.calls_unknown {
            return Err(Some("the call may run code trioxide doesn't see".into()));
        }
        let value = interpreter
            .call(symbol, arguments)
            .map_err(|stop| Some(stop.reason()))?;
        let mut seen = FxHashSet::default();
        let mut leaves = 0;
        interpreter
            .literal(&value, &mut seen, &mut leaves)
            .map_err(Some)
    }

    fn resolve(&self, ident: &IdentifierReference<'_>) -> Option<SymbolId> {
        ident
            .reference_id
            .get()
            .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
    }
}

/// Whether an argument is a constant: a literal, a name or an array or object of them.
fn constant_argument(expression: &Expression<'_>) -> bool {
    match expression.without_parentheses() {
        Expression::NumericLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::Identifier(_) => true,
        Expression::UnaryExpression(unary) => {
            matches!(
                unary.operator,
                UnaryOperator::UnaryNegation | UnaryOperator::UnaryPlus
            ) && constant_argument(&unary.argument)
        }
        Expression::ArrayExpression(array) => array
            .elements
            .iter()
            .all(|element| element.as_expression().is_some_and(constant_argument)),
        Expression::BinaryExpression(binary) => {
            constant_argument(&binary.left) && constant_argument(&binary.right)
        }
        _ => false,
    }
}

/// A value the interpreter computes with.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(Rc<Vec<u16>>),
    /// An array or object on the interpreter's heap.
    Object(usize),
    /// A function declaration of the module.
    Function(SymbolId),
    /// An array hole (never a value an expression has).
    Hole,
}

/// An object the evaluation created.
#[derive(Debug, Clone)]
enum Object {
    Array(Vec<Value>),
    Plain(Vec<(String, Value)>),
}

/// Why evaluation stopped.
#[derive(Debug)]
enum Stop {
    /// Something the interpreter doesn't model.
    Unsupported(&'static str),
    /// The code throws (or would, in the engine).
    Throws,
    OutOfFuel,
}

impl Stop {
    fn reason(&self) -> String {
        match self {
            Self::Unsupported(what) => format!("the interpreter doesn't model {what}"),
            Self::Throws => "evaluating the call throws".into(),
            Self::OutOfFuel => format!("evaluating the call takes more than {FUEL} steps"),
        }
    }
}

type Eval<T> = Result<T, Stop>;

/// How a statement finished.
enum Completion {
    Normal,
    Return(Value),
    Break,
    Continue,
}

struct Interpreter<'f, 's, 'q, 'p, 'a> {
    find: &'f Find<'s, 'q, 'p, 'a>,
    fuel: u64,
    heap: Vec<Object>,
    /// Local bindings of each active call.
    frames: Vec<FxHashMap<SymbolId, Value>>,
    depth: usize,
    /// Module bindings must be declared before this offset (the call).
    before: u32,
}

fn unsupported<T>(what: &'static str) -> Eval<T> {
    Err(Stop::Unsupported(what))
}

fn units(text: &str) -> Rc<Vec<u16>> {
    Rc::new(text.encode_utf16().collect())
}

#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_cmp,
    reason = "lengths and indices stay below 2^32, and JavaScript compares doubles exactly"
)]
impl<'a> Interpreter<'_, '_, '_, '_, 'a> {
    fn step(&mut self, cost: u64) -> Eval<()> {
        self.fuel = self.fuel.checked_sub(cost).ok_or(Stop::OutOfFuel)?;
        Ok(())
    }

    fn frame(&mut self) -> &mut FxHashMap<SymbolId, Value> {
        self.frames.last_mut().expect("a frame")
    }

    fn call(&mut self, symbol: SymbolId, arguments: Vec<Value>) -> Eval<Value> {
        self.step(1)?;
        if self.find.islands.contains(&symbol) {
            return unsupported("calls to WASM island functions");
        }
        let function = *self
            .find
            .functions
            .get(&symbol)
            .ok_or(Stop::Unsupported("this callee"))?;
        if function.generator || function.r#async {
            return unsupported("generators or async functions");
        }
        let body = function
            .body
            .as_ref()
            .ok_or(Stop::Unsupported("bodiless functions"))?;
        if !body.directives.is_empty() {
            return unsupported("directives");
        }
        if self.depth > 64 {
            return unsupported("deep recursion");
        }
        self.depth += 1;
        self.frames.push(FxHashMap::default());
        let result = self.run_body(function, body, arguments);
        self.frames.pop();
        self.depth -= 1;
        result
    }

    fn run_body(
        &mut self,
        function: &Function<'a>,
        body: &oxc_ast::ast::FunctionBody<'a>,
        arguments: Vec<Value>,
    ) -> Eval<Value> {
        let mut arguments = arguments.into_iter();
        for parameter in &function.params.items {
            let BindingPattern::BindingIdentifier(id) = &parameter.pattern else {
                return unsupported("destructured parameters");
            };
            let mut value = arguments.next().unwrap_or(Value::Undefined);
            if value == Value::Undefined
                && let Some(default) = &parameter.initializer
            {
                value = self.expression(default)?;
            }
            self.frame().insert(id.symbol_id(), value);
        }
        if let Some(rest) = &function.params.rest {
            let BindingPattern::BindingIdentifier(id) = &rest.rest.argument else {
                return unsupported("destructured parameters");
            };
            let elements: Vec<Value> = arguments.collect();
            let array = self.allocate(Object::Array(elements))?;
            self.frame().insert(id.symbol_id(), array);
        }
        match self.statements(&body.statements)? {
            Completion::Return(value) => Ok(value),
            Completion::Normal => Ok(Value::Undefined),
            Completion::Break | Completion::Continue => unsupported("`break` outside a loop"),
        }
    }

    /// Drops the bindings `statements` declare from the current call's locals.
    fn forget(&mut self, statements: &[Statement<'_>]) {
        for statement in statements {
            if let Statement::VariableDeclaration(declaration) = statement {
                for declarator in &declaration.declarations {
                    if let BindingPattern::BindingIdentifier(id) = &declarator.id {
                        self.frame().remove(&id.symbol_id());
                    }
                }
            }
        }
    }

    fn allocate(&mut self, object: Object) -> Eval<Value> {
        self.step(1)?;
        self.heap.push(object);
        Ok(Value::Object(self.heap.len() - 1))
    }

    fn statements(&mut self, statements: &[Statement<'a>]) -> Eval<Completion> {
        for statement in statements {
            match self.statement(statement)? {
                Completion::Normal => {}
                other => return Ok(other),
            }
        }
        Ok(Completion::Normal)
    }

    fn statement(&mut self, statement: &Statement<'a>) -> Eval<Completion> {
        self.step(1)?;
        match statement {
            Statement::EmptyStatement(_) => Ok(Completion::Normal),
            Statement::ExpressionStatement(expression) => {
                self.expression(&expression.expression)?;
                Ok(Completion::Normal)
            }
            Statement::BlockStatement(block) => {
                let completion = self.statements(&block.body);
                // Its bindings end with it (and start afresh each loop iteration), so a read
                // before the declaration next time round finds nothing, as the engine's TDZ.
                self.forget(&block.body);
                completion
            }
            Statement::VariableDeclaration(declaration) => {
                self.declaration(declaration)?;
                Ok(Completion::Normal)
            }
            Statement::ReturnStatement(ret) => {
                let value = match &ret.argument {
                    Some(argument) => self.expression(argument)?,
                    None => Value::Undefined,
                };
                Ok(Completion::Return(value))
            }
            Statement::IfStatement(branch) => {
                let test = self.expression(&branch.test)?;
                if Self::truthy(&test) {
                    self.statement(&branch.consequent)
                } else if let Some(alternate) = &branch.alternate {
                    self.statement(alternate)
                } else {
                    Ok(Completion::Normal)
                }
            }
            Statement::ForStatement(for_statement) => {
                match &for_statement.init {
                    Some(ForStatementInit::VariableDeclaration(declaration)) => {
                        // A binding from an earlier run of this loop is gone (its TDZ is back).
                        for declarator in &declaration.declarations {
                            if let BindingPattern::BindingIdentifier(id) = &declarator.id {
                                self.frame().remove(&id.symbol_id());
                            }
                        }
                        self.declaration(declaration)?;
                    }
                    Some(init) => {
                        if let Some(expression) = init.as_expression() {
                            self.expression(expression)?;
                        }
                    }
                    None => {}
                }
                loop {
                    if let Some(test) = &for_statement.test {
                        let test = self.expression(test)?;
                        if !Self::truthy(&test) {
                            break;
                        }
                    }
                    match self.statement(&for_statement.body)? {
                        Completion::Break => break,
                        Completion::Return(value) => return Ok(Completion::Return(value)),
                        Completion::Normal | Completion::Continue => {}
                    }
                    if let Some(update) = &for_statement.update {
                        self.expression(update)?;
                    }
                }
                Ok(Completion::Normal)
            }
            Statement::WhileStatement(while_statement) => {
                loop {
                    let test = self.expression(&while_statement.test)?;
                    if !Self::truthy(&test) {
                        break;
                    }
                    match self.statement(&while_statement.body)? {
                        Completion::Break => break,
                        Completion::Return(value) => return Ok(Completion::Return(value)),
                        Completion::Normal | Completion::Continue => {}
                    }
                }
                Ok(Completion::Normal)
            }
            Statement::DoWhileStatement(do_while) => {
                loop {
                    match self.statement(&do_while.body)? {
                        Completion::Break => break,
                        Completion::Return(value) => return Ok(Completion::Return(value)),
                        Completion::Normal | Completion::Continue => {}
                    }
                    let test = self.expression(&do_while.test)?;
                    if !Self::truthy(&test) {
                        break;
                    }
                }
                Ok(Completion::Normal)
            }
            Statement::ForOfStatement(for_of) => {
                if for_of.r#await {
                    return unsupported("`for await`");
                }
                let ForStatementLeft::VariableDeclaration(declaration) = &for_of.left else {
                    return unsupported("`for…of` without a declaration");
                };
                let [declarator] = declaration.declarations.as_slice() else {
                    return unsupported("`for…of` with several bindings");
                };
                let BindingPattern::BindingIdentifier(id) = &declarator.id else {
                    return unsupported("destructuring");
                };
                if declaration.kind == VariableDeclarationKind::Var {
                    return unsupported("`var`");
                }
                let Value::Object(array) = self.expression(&for_of.right)? else {
                    return unsupported("iterating a non-array");
                };
                let mut index = 0;
                loop {
                    let element = match &self.heap[array] {
                        Object::Array(elements) => match elements.get(index) {
                            Some(Value::Hole) => Value::Undefined,
                            Some(value) => value.clone(),
                            None => break,
                        },
                        Object::Plain(_) => return unsupported("iterating a plain object"),
                    };
                    index += 1;
                    self.frame().insert(id.symbol_id(), element);
                    match self.statement(&for_of.body)? {
                        Completion::Break => break,
                        Completion::Return(value) => return Ok(Completion::Return(value)),
                        Completion::Normal | Completion::Continue => {}
                    }
                }
                Ok(Completion::Normal)
            }
            Statement::BreakStatement(statement) if statement.label.is_none() => {
                Ok(Completion::Break)
            }
            Statement::ContinueStatement(statement) if statement.label.is_none() => {
                Ok(Completion::Continue)
            }
            Statement::ThrowStatement(_) => Err(Stop::Throws),
            Statement::FunctionDeclaration(_) | Statement::ClassDeclaration(_) => {
                unsupported("nested functions or classes")
            }
            _ if statement.is_typescript_syntax() => Ok(Completion::Normal),
            _ => unsupported("this statement"),
        }
    }

    fn declaration(&mut self, declaration: &oxc_ast::ast::VariableDeclaration<'a>) -> Eval<()> {
        if !matches!(
            declaration.kind,
            VariableDeclarationKind::Let | VariableDeclarationKind::Const
        ) {
            return unsupported("`var` or `using`");
        }
        for declarator in &declaration.declarations {
            let BindingPattern::BindingIdentifier(id) = &declarator.id else {
                return unsupported("destructuring");
            };
            let value = match &declarator.init {
                Some(init) => self.expression(init)?,
                None => Value::Undefined,
            };
            self.frame().insert(id.symbol_id(), value);
        }
        Ok(())
    }

    fn truthy(value: &Value) -> bool {
        match value {
            Value::Undefined | Value::Null | Value::Hole => false,
            Value::Boolean(value) => *value,
            Value::Number(value) => !(*value == 0.0 || value.is_nan()),
            Value::String(value) => !value.is_empty(),
            Value::Object(_) | Value::Function(_) => true,
        }
    }

    /// The value of a name.
    fn read(&mut self, ident: &IdentifierReference<'a>) -> Eval<Value> {
        let Some(symbol) = self.find.resolve(ident) else {
            let global = |name: &str| {
                self.find
                    .scoping
                    .find_binding(self.find.scoping.root_scope_id(), name.into())
                    .is_none()
            };
            return match ident.name.as_str() {
                "undefined" if global("undefined") => Ok(Value::Undefined),
                "NaN" if global("NaN") => Ok(Value::Number(f64::NAN)),
                "Infinity" if global("Infinity") => Ok(Value::Number(f64::INFINITY)),
                _ => unsupported("globals"),
            };
        };
        if let Some(value) = self.frame().get(&symbol) {
            return Ok(value.clone());
        }
        if self.find.functions.contains_key(&symbol) && !self.find.scoping.symbol_is_mutated(symbol)
        {
            return Ok(Value::Function(symbol));
        }
        // A module `const` (or an import of one) with a literal value, declared before the call.
        let reference = SymbolRef {
            module: self.find.query.module,
            symbol,
        };
        let fact = self.find.query.facts.symbol_constant(reference);
        let local = self.find.query.facts.resolve_import(reference).is_none();
        let scoping = self.find.scoping;
        if local
            && (scoping.symbol_scope_id(symbol) != scoping.root_scope_id()
                || scoping.symbol_span(symbol).end > self.before)
        {
            return unsupported("bindings other than earlier module constants");
        }
        match fact.value {
            Some(constant) if self.find.query.usable(fact.trust) => Ok(match constant {
                Constant::Number(value) => Value::Number(*value),
                Constant::String(value) => Value::String(units(value)),
                Constant::Boolean(value) => Value::Boolean(*value),
                Constant::Null => Value::Null,
                Constant::Undefined => Value::Undefined,
                Constant::BigInt(_) => return unsupported("bigints"),
            }),
            _ => unsupported("bindings other than earlier module constants"),
        }
    }

    /// Assigns a local of the current call.
    fn write(&mut self, ident: &IdentifierReference<'a>, value: Value) -> Eval<()> {
        let symbol = self
            .find
            .resolve(ident)
            .ok_or(Stop::Unsupported("writing globals"))?;
        let constant = self.find.scoping.symbol_flags(symbol).is_const_variable();
        let frame = self.frame();
        if !frame.contains_key(&symbol) {
            return unsupported("writing bindings outside the call");
        }
        if constant {
            return Err(Stop::Throws);
        }
        frame.insert(symbol, value);
        Ok(())
    }

    fn expression(&mut self, expression: &Expression<'a>) -> Eval<Value> {
        self.step(1)?;
        match expression {
            Expression::NumericLiteral(literal) => Ok(Value::Number(literal.value)),
            Expression::StringLiteral(literal) => {
                if literal.lone_surrogates {
                    return unsupported("strings with lone surrogates");
                }
                Ok(Value::String(units(&literal.value)))
            }
            Expression::BooleanLiteral(literal) => Ok(Value::Boolean(literal.value)),
            Expression::NullLiteral(_) => Ok(Value::Null),
            Expression::TemplateLiteral(template) => {
                let mut text: Vec<u16> = Vec::new();
                for (index, quasi) in template.quasis.iter().enumerate() {
                    let cooked = quasi
                        .value
                        .cooked
                        .as_ref()
                        .ok_or(Stop::Unsupported("invalid escapes"))?;
                    if quasi.lone_surrogates {
                        return unsupported("strings with lone surrogates");
                    }
                    text.extend(cooked.encode_utf16());
                    if let Some(expression) = template.expressions.get(index) {
                        let value = self.expression(expression)?;
                        text.extend(Self::to_string(&value)?.iter());
                    }
                }
                self.string(text)
            }
            Expression::Identifier(ident) => self.read(ident),
            Expression::ParenthesizedExpression(paren) => self.expression(&paren.expression),
            Expression::TSAsExpression(e) => self.expression(&e.expression),
            Expression::TSSatisfiesExpression(e) => self.expression(&e.expression),
            Expression::TSNonNullExpression(e) => self.expression(&e.expression),
            Expression::TSTypeAssertion(e) => self.expression(&e.expression),
            Expression::ArrayExpression(array) => {
                let mut elements = Vec::with_capacity(array.elements.len());
                for element in &array.elements {
                    match element {
                        ArrayExpressionElement::SpreadElement(_) => return unsupported("spread"),
                        ArrayExpressionElement::Elision(_) => elements.push(Value::Hole),
                        _ => {
                            let element = element.as_expression().expect("an expression");
                            elements.push(self.expression(element)?);
                        }
                    }
                }
                self.allocate(Object::Array(elements))
            }
            Expression::ObjectExpression(object) => {
                let mut properties: Vec<(String, Value)> = Vec::new();
                for property in &object.properties {
                    let ObjectPropertyKind::ObjectProperty(property) = property else {
                        return unsupported("object spread");
                    };
                    if property.kind != PropertyKind::Init || property.method || property.computed {
                        return unsupported("accessors, methods or computed keys");
                    }
                    let key = match &property.key {
                        PropertyKey::StaticIdentifier(ident) => ident.name.to_string(),
                        PropertyKey::StringLiteral(literal) if !literal.lone_surrogates => {
                            literal.value.to_string()
                        }
                        _ => return unsupported("this property key"),
                    };
                    if !plain_key(&key) {
                        return unsupported("`__proto__` or integer-like keys");
                    }
                    let value = self.expression(&property.value)?;
                    if let Some(entry) = properties.iter_mut().find(|(name, _)| *name == key) {
                        entry.1 = value;
                    } else {
                        properties.push((key, value));
                    }
                }
                self.allocate(Object::Plain(properties))
            }
            Expression::UnaryExpression(unary) => self.unary(unary.operator, &unary.argument),
            Expression::BinaryExpression(binary) => {
                let left = self.expression(&binary.left)?;
                let right = self.expression(&binary.right)?;
                self.binary(binary.operator, &left, &right)
            }
            Expression::LogicalExpression(logical) => {
                let left = self.expression(&logical.left)?;
                let take_left = match logical.operator {
                    LogicalOperator::And => !Self::truthy(&left),
                    LogicalOperator::Or => Self::truthy(&left),
                    LogicalOperator::Coalesce => !matches!(left, Value::Null | Value::Undefined),
                };
                if take_left {
                    Ok(left)
                } else {
                    self.expression(&logical.right)
                }
            }
            Expression::ConditionalExpression(conditional) => {
                let test = self.expression(&conditional.test)?;
                if Self::truthy(&test) {
                    self.expression(&conditional.consequent)
                } else {
                    self.expression(&conditional.alternate)
                }
            }
            Expression::SequenceExpression(sequence) => {
                let mut last = Value::Undefined;
                for expression in &sequence.expressions {
                    last = self.expression(expression)?;
                }
                Ok(last)
            }
            Expression::AssignmentExpression(assignment) => self.assignment(assignment),
            Expression::UpdateExpression(update) => self.update(update),
            Expression::StaticMemberExpression(member) => {
                if member.optional {
                    return unsupported("optional chains");
                }
                if let Some(value) = self.namespace_member(&member.object, &member.property.name)? {
                    return Ok(value);
                }
                let object = self.expression(&member.object)?;
                self.get(
                    &object,
                    &PropertyName::Name(member.property.name.to_string()),
                )
            }
            Expression::ComputedMemberExpression(member) => {
                if member.optional {
                    return unsupported("optional chains");
                }
                let object = self.expression(&member.object)?;
                let key = self.expression(&member.expression)?;
                let key = Self::key(&key)?;
                self.get(&object, &key)
            }
            Expression::CallExpression(call) => self.call_expression(call),
            Expression::NewExpression(new) => {
                let Expression::Identifier(callee) = &new.callee else {
                    return unsupported("`new` of this");
                };
                if callee.name != "Array" || self.find.resolve(callee).is_some() {
                    return unsupported("`new` of this");
                }
                self.array_constructor(&new.arguments)
            }
            _ => unsupported("this expression"),
        }
    }

    fn string(&mut self, text: Vec<u16>) -> Eval<Value> {
        self.step(text.len() as u64 / 64)?;
        Ok(Value::String(Rc::new(text)))
    }

    /// `Math.PI`, `Number.MAX_SAFE_INTEGER` and the like.
    fn namespace_member(&mut self, object: &Expression<'a>, name: &str) -> Eval<Option<Value>> {
        let Expression::Identifier(namespace) = object else {
            return Ok(None);
        };
        if self.find.resolve(namespace).is_some() {
            return Ok(None);
        }
        let value = match (namespace.name.as_str(), name) {
            ("Math", "PI") => std::f64::consts::PI,
            ("Math", "E") => std::f64::consts::E,
            ("Math", "LN2") => std::f64::consts::LN_2,
            ("Math", "LN10") => std::f64::consts::LN_10,
            ("Math", "LOG2E") => std::f64::consts::LOG2_E,
            ("Math", "LOG10E") => std::f64::consts::LOG10_E,
            ("Math", "SQRT2") => std::f64::consts::SQRT_2,
            ("Math", "SQRT1_2") => std::f64::consts::FRAC_1_SQRT_2,
            ("Number", "MAX_SAFE_INTEGER") => 9_007_199_254_740_991.0,
            ("Number", "MIN_SAFE_INTEGER") => -9_007_199_254_740_991.0,
            ("Number", "EPSILON") => f64::EPSILON,
            ("Number", "MAX_VALUE") => f64::MAX,
            ("Number", "MIN_VALUE") => 5e-324,
            ("Number", "POSITIVE_INFINITY") => f64::INFINITY,
            ("Number", "NEGATIVE_INFINITY") => f64::NEG_INFINITY,
            ("Number", "NaN") => f64::NAN,
            ("Math" | "Number", _) => return unsupported("this `Math`/`Number` member"),
            _ => return Ok(None),
        };
        Ok(Some(Value::Number(value)))
    }

    fn array_constructor(&mut self, arguments: &ArenaVec<'a, Argument<'a>>) -> Eval<Value> {
        let values = self.arguments(arguments)?;
        match values.as_slice() {
            [Value::Number(length)] => {
                let length = array_length(*length).ok_or(Stop::Throws)?;
                if length > 100_000 {
                    return unsupported("large arrays");
                }
                self.step(length as u64)?;
                self.allocate(Object::Array(vec![Value::Hole; length]))
            }
            [] => self.allocate(Object::Array(Vec::new())),
            _ => unsupported("`new Array` with these arguments"),
        }
    }

    fn arguments(&mut self, arguments: &ArenaVec<'a, Argument<'a>>) -> Eval<Vec<Value>> {
        arguments
            .iter()
            .map(|argument| match argument.as_expression() {
                Some(expression) => self.expression(expression),
                None => unsupported("spread arguments"),
            })
            .collect()
    }

    fn call_expression(&mut self, call: &CallExpression<'a>) -> Eval<Value> {
        if call.optional {
            return unsupported("optional calls");
        }
        match &call.callee {
            Expression::Identifier(callee) => {
                if callee.name == "Array" && self.find.resolve(callee).is_none() {
                    return self.array_constructor(&call.arguments);
                }
                let Value::Function(symbol) = self.read(callee)? else {
                    return unsupported("calling this value");
                };
                let arguments = self.arguments(&call.arguments)?;
                self.call(symbol, arguments)
            }
            Expression::StaticMemberExpression(member) if !member.optional => {
                let name = member.property.name.as_str();
                if let Expression::Identifier(namespace) = &member.object
                    && self.find.resolve(namespace).is_none()
                    && matches!(
                        namespace.name.as_str(),
                        "Math" | "Number" | "String" | "Array"
                    )
                {
                    let arguments = self.arguments(&call.arguments)?;
                    return self.builtin(namespace.name.as_str(), name, &arguments);
                }
                let receiver = self.expression(&member.object)?;
                let arguments = self.arguments(&call.arguments)?;
                self.method(&receiver, name, arguments)
            }
            _ => unsupported("this callee"),
        }
    }

    /// `Math.*`, `Number.*`, `String.fromCharCode` and `Array.isArray`.
    fn builtin(&mut self, namespace: &str, name: &str, arguments: &[Value]) -> Eval<Value> {
        let numbers = || -> Eval<Vec<f64>> {
            arguments
                .iter()
                .map(|value| match value {
                    Value::Number(value) => Ok(*value),
                    _ => unsupported("non-number arguments to `Math`"),
                })
                .collect()
        };
        let one = |numbers: &[f64]| numbers.first().copied().unwrap_or(f64::NAN);
        let result = match (namespace, name) {
            ("Math", "floor") => one(&numbers()?).floor(),
            ("Math", "ceil") => one(&numbers()?).ceil(),
            ("Math", "trunc") => one(&numbers()?).trunc(),
            ("Math", "abs") => one(&numbers()?).abs(),
            ("Math", "sqrt") => one(&numbers()?).sqrt(),
            ("Math", "round") => js_round(one(&numbers()?)),
            ("Math", "sign") => {
                let x = one(&numbers()?);
                if x.is_nan() || x == 0.0 {
                    x
                } else {
                    x.signum()
                }
            }
            ("Math", "fround") => f64::from(one(&numbers()?) as f32),
            ("Math", "max") => numbers()?.into_iter().fold(f64::NEG_INFINITY, js_max),
            ("Math", "min") => numbers()?.into_iter().fold(f64::INFINITY, js_min),
            ("Math", "imul") => {
                let numbers = numbers()?;
                let (a, b) = (
                    to_int32(one(&numbers)),
                    to_int32(numbers.get(1).copied().unwrap_or(f64::NAN)),
                );
                f64::from(a.wrapping_mul(b))
            }
            ("Math", "clz32") => f64::from(to_uint32(one(&numbers()?)).leading_zeros()),
            ("Math", "pow") => {
                let numbers = numbers()?;
                exact_power(one(&numbers), numbers.get(1).copied().unwrap_or(f64::NAN))?
            }
            ("Number", "isInteger") => {
                return Ok(Value::Boolean(
                    matches!(arguments.first(), Some(Value::Number(x)) if x.is_finite() && x.trunc() == *x),
                ));
            }
            ("Number", "isFinite") => {
                return Ok(Value::Boolean(
                    matches!(arguments.first(), Some(Value::Number(x)) if x.is_finite()),
                ));
            }
            ("Number", "isNaN") => {
                return Ok(Value::Boolean(
                    matches!(arguments.first(), Some(Value::Number(x)) if x.is_nan()),
                ));
            }
            ("String", "fromCharCode") => {
                let text = numbers()?
                    .into_iter()
                    .map(|code| (to_uint32(code) & 0xFFFF) as u16)
                    .collect();
                return self.string(text);
            }
            ("Array", "isArray") => {
                let is_array = matches!(arguments.first(), Some(Value::Object(id)) if matches!(self.heap[*id], Object::Array(_)));
                return Ok(Value::Boolean(is_array));
            }
            _ => return unsupported("this builtin (only exactly specified ones are)"),
        };
        Ok(Value::Number(result))
    }

    /// Methods of arrays and strings the evaluation has.
    fn method(&mut self, receiver: &Value, name: &str, arguments: Vec<Value>) -> Eval<Value> {
        match receiver {
            Value::Object(id) => {
                let id = *id;
                let Object::Array(elements) = &mut self.heap[id] else {
                    return unsupported("methods of plain objects");
                };
                match name {
                    "push" => {
                        let count = arguments.len() as u64;
                        elements.extend(arguments);
                        let length = elements.len();
                        self.step(count)?;
                        Ok(Value::Number(length as f64))
                    }
                    "pop" => Ok(match elements.pop() {
                        Some(Value::Hole) | None => Value::Undefined,
                        Some(value) => value,
                    }),
                    "fill" if arguments.len() <= 1 => {
                        let value = arguments.into_iter().next().unwrap_or(Value::Undefined);
                        let count = elements.len() as u64;
                        for element in elements.iter_mut() {
                            *element = value.clone();
                        }
                        self.step(count)?;
                        Ok(Value::Object(id))
                    }
                    "includes" | "indexOf" => {
                        let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                        if matches!(target, Value::Object(_)) {
                            return unsupported("searching for objects");
                        }
                        // `indexOf` skips holes; `includes` reads them as `undefined`.
                        let found = elements.iter().position(|element| {
                            if name == "includes" {
                                same_value_zero(element, &target)
                            } else {
                                *element != Value::Hole && strict_equals(element, &target)
                            }
                        });
                        let count = elements.len() as u64;
                        self.step(count)?;
                        Ok(if name == "includes" {
                            Value::Boolean(found.is_some())
                        } else {
                            Value::Number(found.map_or(-1.0, |index| index as f64))
                        })
                    }
                    _ => unsupported("this array method"),
                }
            }
            Value::String(text) => match name {
                "charCodeAt" => {
                    let index = match arguments.first() {
                        None | Some(Value::Undefined) => 0.0,
                        // ToIntegerOrInfinity: NaN is 0.
                        Some(Value::Number(index)) if index.is_nan() => 0.0,
                        Some(Value::Number(index)) => index.trunc(),
                        _ => return unsupported("`charCodeAt` with this argument"),
                    };
                    Ok(Value::Number(
                        if index >= 0.0 && (index as usize) < text.len() && !index.is_nan() {
                            f64::from(text[index as usize])
                        } else {
                            f64::NAN
                        },
                    ))
                }
                _ => unsupported("this string method"),
            },
            Value::Undefined | Value::Null => Err(Stop::Throws),
            _ => unsupported("methods of this value"),
        }
    }

    /// A property key from a value.
    fn key(key: &Value) -> Eval<PropertyName> {
        match key {
            Value::Number(index) if array_index(*index).is_some() => {
                Ok(PropertyName::Index(array_index(*index).expect("checked")))
            }
            Value::String(text) => {
                let text = String::from_utf16(text)
                    .map_err(|_| Stop::Unsupported("lone surrogates in keys"))?;
                if text.parse::<u32>().is_ok() || !plain_key(&text) {
                    return unsupported("this key");
                }
                Ok(PropertyName::Name(text))
            }
            _ => unsupported("this key"),
        }
    }

    fn get(&mut self, object: &Value, key: &PropertyName) -> Eval<Value> {
        if matches!(key, PropertyName::Name(name) if name == "__proto__") {
            return unsupported("`__proto__`");
        }
        match (object, key) {
            (Value::Object(id), _) => match (&self.heap[*id], key) {
                (Object::Array(elements), PropertyName::Index(index)) => {
                    Ok(match elements.get(*index) {
                        Some(Value::Hole) | None => Value::Undefined,
                        Some(value) => value.clone(),
                    })
                }
                (Object::Array(elements), PropertyName::Name(name)) if name == "length" => {
                    Ok(Value::Number(elements.len() as f64))
                }
                (Object::Plain(properties), PropertyName::Name(name)) => properties
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.clone())
                    .ok_or(Stop::Unsupported("reading a key an object doesn't have")),
                _ => unsupported("this property"),
            },
            (Value::String(text), PropertyName::Name(name)) if name == "length" => {
                Ok(Value::Number(text.len() as f64))
            }
            (Value::String(text), PropertyName::Index(index)) => match text.get(*index) {
                Some(&unit) => self.string(vec![unit]),
                None => Ok(Value::Undefined),
            },
            (Value::Undefined | Value::Null, _) => Err(Stop::Throws),
            _ => unsupported("this property"),
        }
    }

    fn set(&mut self, object: &Value, key: &PropertyName, value: Value) -> Eval<()> {
        if matches!(key, PropertyName::Name(name) if name == "__proto__") {
            return unsupported("`__proto__`");
        }
        let Value::Object(id) = object else {
            return if matches!(object, Value::Undefined | Value::Null) {
                Err(Stop::Throws)
            } else {
                unsupported("writing properties of primitives")
            };
        };
        match (&mut self.heap[*id], key) {
            (Object::Array(elements), PropertyName::Index(index)) => {
                if *index > 100_000 {
                    return unsupported("large arrays");
                }
                if *index >= elements.len() {
                    elements.resize(index + 1, Value::Hole);
                }
                elements[*index] = value;
                Ok(())
            }
            (Object::Plain(properties), PropertyName::Name(name)) => {
                match properties.iter_mut().find(|(key, _)| key == name) {
                    Some(entry) => entry.1 = value,
                    None => properties.push((name.clone(), value)),
                }
                Ok(())
            }
            _ => unsupported("writing this property"),
        }
    }

    /// The value and reference of a member target (`a[i]`, `o.k`).
    fn member_target(
        &mut self,
        target: &SimpleAssignmentTarget<'a>,
    ) -> Eval<Option<(Value, PropertyName)>> {
        match target {
            SimpleAssignmentTarget::StaticMemberExpression(member) => {
                let object = self.expression(&member.object)?;
                Ok(Some((
                    object,
                    PropertyName::Name(member.property.name.to_string()),
                )))
            }
            SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                let object = self.expression(&member.object)?;
                let key = self.expression(&member.expression)?;
                let key = Self::key(&key)?;
                Ok(Some((object, key)))
            }
            SimpleAssignmentTarget::AssignmentTargetIdentifier(_) => Ok(None),
            _ => unsupported("this assignment target"),
        }
    }

    fn assignment(&mut self, assignment: &oxc_ast::ast::AssignmentExpression<'a>) -> Eval<Value> {
        let Some(target) = assignment.left.as_simple_assignment_target() else {
            return unsupported("destructuring assignment");
        };
        let operator = match assignment.operator {
            AssignmentOperator::Assign => None,
            operator => match operator.to_binary_operator() {
                Some(operator) => Some(Ok(operator)),
                None => Some(Err(operator)),
            },
        };
        if let AssignmentTarget::AssignmentTargetIdentifier(ident) = &assignment.left {
            let value = match operator {
                None => self.expression(&assignment.right)?,
                Some(Ok(operator)) => {
                    let old = self.read(ident)?;
                    let right = self.expression(&assignment.right)?;
                    self.binary(operator, &old, &right)?
                }
                Some(Err(logical)) => {
                    let old = self.read(ident)?;
                    let keep = match logical {
                        AssignmentOperator::LogicalAnd => !Self::truthy(&old),
                        AssignmentOperator::LogicalOr => Self::truthy(&old),
                        _ => !matches!(old, Value::Null | Value::Undefined),
                    };
                    if keep {
                        return Ok(old);
                    }
                    self.expression(&assignment.right)?
                }
            };
            self.write(ident, value.clone())?;
            return Ok(value);
        }
        let Some((object, key)) = self.member_target(target)? else {
            return unsupported("this assignment target");
        };
        let value = match operator {
            None => self.expression(&assignment.right)?,
            Some(Ok(operator)) => {
                let old = self.get(&object, &key)?;
                let right = self.expression(&assignment.right)?;
                self.binary(operator, &old, &right)?
            }
            Some(Err(_)) => return unsupported("logical assignment to properties"),
        };
        self.set(&object, &key, value.clone())?;
        Ok(value)
    }

    fn update(&mut self, update: &oxc_ast::ast::UpdateExpression<'a>) -> Eval<Value> {
        let delta = if update.operator == UpdateOperator::Increment {
            1.0
        } else {
            -1.0
        };
        if let SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) = &update.argument {
            let Value::Number(old) = self.read(ident)? else {
                return unsupported("`++`/`--` of non-numbers");
            };
            self.write(ident, Value::Number(old + delta))?;
            return Ok(Value::Number(if update.prefix { old + delta } else { old }));
        }
        let Some((object, key)) = self.member_target(&update.argument)? else {
            return unsupported("this update target");
        };
        let Value::Number(old) = self.get(&object, &key)? else {
            return unsupported("`++`/`--` of non-numbers");
        };
        self.set(&object, &key, Value::Number(old + delta))?;
        Ok(Value::Number(if update.prefix { old + delta } else { old }))
    }

    fn unary(&mut self, operator: UnaryOperator, argument: &Expression<'a>) -> Eval<Value> {
        let value = self.expression(argument)?;
        Ok(match operator {
            UnaryOperator::LogicalNot => Value::Boolean(!Self::truthy(&value)),
            UnaryOperator::Void => Value::Undefined,
            UnaryOperator::Typeof => {
                let name = match value {
                    Value::Undefined | Value::Hole => "undefined",
                    Value::Null | Value::Object(_) => "object",
                    Value::Boolean(_) => "boolean",
                    Value::Number(_) => "number",
                    Value::String(_) => "string",
                    Value::Function(_) => "function",
                };
                Value::String(units(name))
            }
            UnaryOperator::UnaryNegation => Value::Number(-Self::number(&value)?),
            UnaryOperator::UnaryPlus => Value::Number(Self::number(&value)?),
            UnaryOperator::BitwiseNot => Value::Number(f64::from(!to_int32(Self::number(&value)?))),
            UnaryOperator::Delete => return unsupported("`delete`"),
        })
    }

    /// `ToNumber` for the values where it can't run user code or depend on string parsing.
    fn number(value: &Value) -> Eval<f64> {
        match value {
            Value::Number(value) => Ok(*value),
            Value::Boolean(value) => Ok(f64::from(u8::from(*value))),
            Value::Null => Ok(0.0),
            Value::Undefined => Ok(f64::NAN),
            _ => unsupported("converting this value to a number"),
        }
    }

    /// `ToString` for primitives.
    fn to_string(value: &Value) -> Eval<Rc<Vec<u16>>> {
        Ok(match value {
            Value::String(text) => Rc::clone(text),
            Value::Number(value) => units(&value.to_js_string()),
            Value::Boolean(value) => units(if *value { "true" } else { "false" }),
            Value::Null => units("null"),
            Value::Undefined => units("undefined"),
            _ => return unsupported("converting objects to strings"),
        })
    }

    fn binary(&mut self, operator: BinaryOperator, left: &Value, right: &Value) -> Eval<Value> {
        use BinaryOperator as B;
        if operator == B::Addition
            && (matches!(left, Value::String(_)) || matches!(right, Value::String(_)))
        {
            let mut text = (*Self::to_string(left)?).clone();
            text.extend(Self::to_string(right)?.iter());
            return self.string(text);
        }
        Ok(match operator {
            B::StrictEquality => Value::Boolean(strict_equals(left, right)),
            B::StrictInequality => Value::Boolean(!strict_equals(left, right)),
            B::Equality | B::Inequality => {
                let equal = match (left, right) {
                    (Value::Null | Value::Undefined, Value::Null | Value::Undefined) => true,
                    (Value::Null | Value::Undefined, _) | (_, Value::Null | Value::Undefined) => {
                        false
                    }
                    _ if std::mem::discriminant(left) == std::mem::discriminant(right) => {
                        strict_equals(left, right)
                    }
                    _ => return unsupported("`==` across types"),
                };
                Value::Boolean(equal == (operator == B::Equality))
            }
            B::LessThan | B::LessEqualThan | B::GreaterThan | B::GreaterEqualThan => {
                let order = if let (Value::String(a), Value::String(b)) = (left, right) {
                    Some(a.cmp(b))
                } else {
                    let (a, b) = (Self::number(left)?, Self::number(right)?);
                    a.partial_cmp(&b)
                };
                let Some(order) = order else {
                    return Ok(Value::Boolean(false));
                };
                Value::Boolean(match operator {
                    B::LessThan => order.is_lt(),
                    B::LessEqualThan => order.is_le(),
                    B::GreaterThan => order.is_gt(),
                    _ => order.is_ge(),
                })
            }
            B::In | B::Instanceof => return unsupported("`in` or `instanceof`"),
            _ => {
                let (a, b) = (Self::number(left)?, Self::number(right)?);
                Value::Number(match operator {
                    B::Addition => a + b,
                    B::Subtraction => a - b,
                    B::Multiplication => a * b,
                    B::Division => a / b,
                    B::Remainder => a % b,
                    B::Exponential => exact_power(a, b)?,
                    B::BitwiseAnd => f64::from(to_int32(a) & to_int32(b)),
                    B::BitwiseOR => f64::from(to_int32(a) | to_int32(b)),
                    B::BitwiseXOR => f64::from(to_int32(a) ^ to_int32(b)),
                    B::ShiftLeft => f64::from(to_int32(a).wrapping_shl(to_uint32(b) & 31)),
                    B::ShiftRight => f64::from(to_int32(a) >> (to_uint32(b) & 31)),
                    B::ShiftRightZeroFill => f64::from(to_uint32(a) >> (to_uint32(b) & 31)),
                    _ => return unsupported("this operator"),
                })
            }
        })
    }

    /// The literal for a result: a tree of fresh objects, small enough.
    fn literal(
        &self,
        value: &Value,
        seen: &mut FxHashSet<usize>,
        leaves: &mut usize,
    ) -> Result<Literal, String> {
        *leaves += 1;
        if *leaves > MAX_LEAVES {
            return Err(format!(
                "the result is too big to write out (over {MAX_LEAVES} values)"
            ));
        }
        Ok(match value {
            Value::Undefined => Literal::Constant(Constant::Undefined),
            Value::Null => Literal::Constant(Constant::Null),
            Value::Boolean(value) => Literal::Constant(Constant::Boolean(*value)),
            Value::Number(value) => Literal::Constant(Constant::Number(*value)),
            Value::String(text) => {
                if text.len() > MAX_STRING {
                    return Err(format!(
                        "the result has a string over {MAX_STRING} characters"
                    ));
                }
                match String::from_utf16(text) {
                    Ok(text) if text.chars().any(invisible) => {
                        return Err("the result has a string with invisible characters".into());
                    }
                    Ok(text) => Literal::Constant(Constant::String(text)),
                    Err(_) => Literal::Units((**text).clone()),
                }
            }
            Value::Function(_) => return Err("the result is a function".into()),
            Value::Hole => return Err("the result has holes".into()),
            Value::Object(id) => {
                if !seen.insert(*id) {
                    return Err(
                        "an object in the result is reachable twice (copies would lose that)"
                            .into(),
                    );
                }
                match &self.heap[*id] {
                    Object::Array(elements) => Literal::Array(
                        elements
                            .iter()
                            .map(|element| self.literal(element, seen, leaves))
                            .collect::<Result<_, _>>()?,
                    ),
                    Object::Plain(properties) => Literal::Object(
                        properties
                            .iter()
                            .map(|(key, value)| {
                                Ok((key.clone(), self.literal(value, seen, leaves)?))
                            })
                            .collect::<Result<_, String>>()?,
                    ),
                }
            }
        })
    }
}

/// A property name the interpreter models.
enum PropertyName {
    Index(usize),
    Name(String),
}

/// Whether an object key keeps insertion order and sets no prototype.
fn plain_key(key: &str) -> bool {
    key != "__proto__" && !key.is_empty() && !key.starts_with(|c: char| c.is_ascii_digit())
}

/// Characters a reader can't see, which would print raw.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFD}'
    ) || (c.is_control() && !matches!(c, '\n' | '\t' | '\r'))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked"
)]
fn array_index(value: f64) -> Option<usize> {
    (value.fract() == 0.0 && (0.0..4_294_967_295.0).contains(&value)).then_some(value as usize)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked"
)]
fn array_length(value: f64) -> Option<usize> {
    (value.fract() == 0.0 && (0.0..4_294_967_296.0).contains(&value)).then_some(value as usize)
}

/// ECMAScript `ToInt32`.
#[expect(clippy::cast_possible_wrap, reason = "modular conversion")]
fn to_int32(value: f64) -> i32 {
    to_uint32(value) as i32
}

/// ECMAScript `ToUint32`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "modular conversion"
)]
fn to_uint32(value: f64) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    let truncated = value.trunc();
    let modulo = truncated.rem_euclid(4_294_967_296.0);
    modulo as u32
}

/// `Math.round`: halves round up, `-0` and the sign of small negatives are kept.
fn js_round(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let floor = x.floor();
    let rounded = if x - floor >= 0.5 { floor + 1.0 } else { floor };
    if rounded == 0.0 && x < 0.0 {
        -0.0
    } else {
        rounded
    }
}

/// `Math.max` of two: NaN wins, `+0` beats `-0`.
#[expect(clippy::float_cmp, reason = "exact comparison, as JavaScript's")]
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == b && a == 0.0 {
        if a.is_sign_negative() { b } else { a }
    } else if a > b {
        a
    } else {
        b
    }
}

/// `Math.min` of two: NaN wins, `-0` beats `+0`.
#[expect(clippy::float_cmp, reason = "exact comparison, as JavaScript's")]
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == b && a == 0.0 {
        if a.is_sign_negative() { a } else { b }
    } else if a < b {
        a
    } else {
        b
    }
}

/// `base ** exponent` where it is exact without the engine's `pow`: integer bases and small
/// non-negative integer exponents whose result stays an exact integer.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked range"
)]
fn exact_power(base: f64, exponent: f64) -> Eval<f64> {
    if exponent.fract() != 0.0
        || !(0.0..=64.0).contains(&exponent)
        || base.fract() != 0.0
        || !base.is_finite()
    {
        return unsupported("`**` other than small integer powers");
    }
    let mut result = 1.0_f64;
    for _ in 0..(exponent as u32) {
        result *= base;
        if result.abs() > 9_007_199_254_740_992.0 {
            return unsupported("`**` beyond exact integers");
        }
    }
    if result == 0.0 && base.is_sign_negative() && (exponent as u32) % 2 == 1 {
        result = -0.0;
    }
    Ok(result)
}

#[expect(clippy::float_cmp, reason = "`===` compares doubles exactly")]
fn strict_equals(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Hole | Value::Undefined, Value::Hole | Value::Undefined) => true,
        _ => a == b,
    }
}

#[expect(clippy::float_cmp, reason = "SameValueZero compares doubles exactly")]
fn same_value_zero(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a == b || (a.is_nan() && b.is_nan()),
        _ => strict_equals(a, b),
    }
}

/// Puts the evaluated literals in place of their calls.
struct Replace<'f> {
    query: Query<'f>,
    position: Position,
    decisions: Decisions,
    results: FxHashMap<NodeId, Literal>,
    callees: FxHashMap<NodeId, SymbolId>,
    /// The functions some of whose calls were replaced.
    replaced: FxHashSet<SymbolId>,
}

impl<'a> Traverse<'a, ()> for Replace<'_> {
    track_position!();

    fn enter_expression(&mut self, expression: &mut Expression<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        let Expression::CallExpression(call) = expression else {
            return;
        };
        let node = call.node_id();
        let Some(literal) = self.results.remove(&node) else {
            return;
        };
        let span = call.span;
        let slot = Slot::of(ctx);
        let replacement = build(&literal, span, ctx);
        if !slot.admits(&replacement, true) {
            return;
        }
        *expression = slot.wrap(replacement, ctx);
        self.decisions.applied(span);
        if let Some(&callee) = self.callees.get(&node) {
            self.replaced.insert(callee);
        }
    }
}

/// The expression for a literal.
fn build<'a>(literal: &Literal, span: Span, ctx: &mut TraverseCtx<'a, ()>) -> Expression<'a> {
    match literal {
        Literal::Constant(constant) => constant_expr(constant, span, ctx),
        Literal::Units(units) => {
            // oxc's encoding of lone surrogates: U+FFFD then the code unit in hex.
            let mut text = String::new();
            for unit in char::decode_utf16(units.iter().copied()) {
                match unit {
                    Ok('\u{FFFD}') => text.push_str("\u{FFFD}fffd"),
                    Ok(c) => text.push(c),
                    Err(error) => {
                        text.push('\u{FFFD}');
                        let _ = write!(text, "{:04x}", error.unpaired_surrogate());
                    }
                }
            }
            let mut literal =
                oxc_ast::ast::StringLiteral::new(span, Str::from_str_in(&text, ctx), None, ctx);
            literal.lone_surrogates = true;
            Expression::StringLiteral(ctx.alloc(literal))
        }
        Literal::Array(elements) => {
            let elements = elements
                .iter()
                .map(|element| ArrayExpressionElement::from(build(element, span, ctx)))
                .collect::<Vec<_>>();
            Expression::new_array_expression(span, ArenaVec::from_iter_in(elements, ctx), ctx)
        }
        Literal::Object(properties) => {
            let properties = properties
                .iter()
                .map(|(key, value)| {
                    let key = if oxc_syntax::identifier::is_identifier_name(key) {
                        PropertyKey::StaticIdentifier(ctx.alloc(oxc_ast::ast::IdentifierName::new(
                            span,
                            Ident::from_str_in(key, ctx),
                            ctx,
                        )))
                    } else {
                        PropertyKey::StringLiteral(ctx.alloc(oxc_ast::ast::StringLiteral::new(
                            span,
                            Str::from_str_in(key, ctx),
                            None,
                            ctx,
                        )))
                    };
                    let value = build(value, span, ctx);
                    ObjectPropertyKind::ObjectProperty(ctx.alloc(
                        oxc_ast::ast::ObjectProperty::new(
                            span,
                            PropertyKind::Init,
                            key,
                            value,
                            false,
                            false,
                            false,
                            ctx,
                        ),
                    ))
                })
                .collect::<Vec<_>>();
            Expression::new_object_expression(span, ArenaVec::from_iter_in(properties, ctx), ctx)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_matches_javascript() {
        assert!(js_round(-0.5).is_sign_negative());
        assert_eq!(js_round(-0.5), 0.0);
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(0.499_999_999_999_999_94), 0.0);
        assert!(js_round(-0.2).is_sign_negative());
    }

    #[test]
    fn int32_conversions() {
        assert_eq!(to_int32(4_294_967_295.0), -1);
        assert_eq!(to_int32(-1.5), -1);
        assert_eq!(to_uint32(-1.0), 4_294_967_295);
        assert_eq!(to_int32(f64::NAN), 0);
    }

    #[test]
    fn powers_are_exact_or_refused() {
        assert_eq!(exact_power(2.0, 10.0).unwrap(), 1024.0);
        assert!(exact_power(2.0, 0.5).is_err());
        assert!(exact_power(10.0, 20.0).is_err());
    }

    #[test]
    fn min_and_max_order_zeros() {
        assert!(js_max(-0.0, 0.0).is_sign_positive());
        assert!(js_min(0.0, -0.0).is_sign_negative());
        assert!(js_max(1.0, f64::NAN).is_nan());
    }
}
