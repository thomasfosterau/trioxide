//! PLAN §4 pass 12: closure hoisting.
//!
//! A closure created inside a function (or a top-level loop) that captures nothing is created
//! once instead: it becomes a function declaration at the top of the module, and its uses name
//! that. Creating it once is only invisible when nothing can tell one closure object from
//! another, so the closure must be one of:
//!
//! - the initialiser of a `const` (or never reassigned `let`) every reference to which, after
//!   the declaration, is a direct call `f(…)`: never compared, used as a `WeakMap`/`Set` key,
//!   given properties, stored, pushed or passed;
//! - a callback argument of a call at which the facts say nothing runs but that closure
//!   (`xs.map((x) => x * 2)` on a proven array: `site_resolution` is complete and lists only
//!   it), so a modelled builtin that only calls it.
//!
//! and its allocation site must be `escapes = None` (usable). It captures nothing: every name
//! it reads is a module-level binding or a global, and (an arrow) it uses no `this`,
//! `arguments`, `super` or `new.target`. Generators, async functions and named function
//! expressions stay, and so do closures in WASM island functions and in code proven never to
//! run. An arrow becomes a plain function, which is the same when only called with no `this`.

use oxc_allocator::{ArenaBox, ArenaVec, TakeIn};
use oxc_ast::ast::{
    ArrowFunctionExpression, BindingIdentifier, CallExpression, Expression, FormalParameterKind,
    Function, FunctionBody, FunctionType, Ident, IdentifierReference, Program, Statement,
    VariableDeclarationKind,
};
use oxc_ast::{AstKind, builder::AstBuilder};
use oxc_ast_visit::{Visit, VisitMut, walk, walk_mut};
use oxc_semantic::Scoping;
use oxc_span::{GetSpan, Span};
use oxc_syntax::{reference::ReferenceId, scope::ScopeFlags, symbol::SymbolId};
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_analysis::Escape;
use trioxide_common::NodeId;

use super::{
    Decisions,
    speed::{Namer, is_island, rescope},
    util::{Query, has_dynamic_scope},
};
use crate::{Pass, PassContext};

/// PLAN §4 pass 12 (`"closure-hoist"`), in the speed group.
pub struct ClosureHoist;

impl Pass for ClosureHoist {
    fn name(&self) -> &'static str {
        "closure-hoist"
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
        let mut find = Find {
            query,
            scoping: ctx.scoping(),
            keep: ctx.keep,
            tagged: &tagged,
            callees: FxHashMap::default(),
            candidates: Vec::new(),
            functions: Vec::new(),
            loops: 0,
            dead: 0,
            opaque: 0,
            top: 0,
        };
        find.visit_program(program);
        let Find {
            callees,
            candidates,
            ..
        } = find;
        let mut decisions = Decisions::new(self.name());
        let mut namer = Namer::new(ctx.scoping());
        let root = ctx.scoping().root_scope_id();
        let mut plans: FxHashMap<NodeId, Hoist> = FxHashMap::default();
        for candidate in candidates {
            if let Err(reason) = check(&candidate, &callees, query, ctx.scoping()) {
                decisions.skipped(candidate.span, reason);
                continue;
            }
            // The new declaration must not take any name a reference resolves through.
            let name =
                namer.fresh_replacing(&candidate.base, root, ctx.scoping(), candidate.binding);
            decisions.applied(candidate.span);
            plans.insert(
                candidate.closure,
                Hoist {
                    name,
                    binding: candidate.binding,
                    top: candidate.top,
                    reference: candidate.call.map_or(candidate.span, |(_, span)| span),
                },
            );
        }
        if plans.is_empty() {
            return decisions.flush(ctx.log);
        }
        let renames: FxHashMap<SymbolId, String> = plans
            .values()
            .filter_map(|hoist| Some((hoist.binding?, hoist.name.clone())))
            .collect();
        let mut rewrite = Rewrite {
            scoping: ctx.scoping(),
            plans: &plans,
            renames: &renames,
            ast: AstBuilder::new(ctx.allocator),
            hoisted: Vec::new(),
        };
        rewrite.visit_program(program);
        let mut hoisted = rewrite.hoisted;
        hoisted.sort_by_key(|(top, _)| std::cmp::Reverse(*top));
        for (top, declaration) in hoisted {
            let at = top.min(program.body.len());
            program.body.insert(at, declaration);
        }
        *ctx.scoping_mut() = rescope(program);
        decisions.flush(ctx.log)
    }
}

/// A closure that may be hoisted, as found.
struct Candidate {
    closure: NodeId,
    span: Span,
    /// The `const` it initialises.
    binding: Option<SymbolId>,
    /// The call it is an argument of (callback form), and that call's span.
    call: Option<(NodeId, Span)>,
    base: String,
    /// The top-level statement it is in.
    top: usize,
    /// Why it can't be, from syntax.
    rejected: Option<String>,
}

/// Why the facts or the uses don't allow hoisting.
fn check(
    candidate: &Candidate,
    callees: &FxHashMap<ReferenceId, Span>,
    query: Query<'_>,
    scoping: &Scoping,
) -> Result<(), String> {
    if let Some(reason) = &candidate.rejected {
        return Err(reason.clone());
    }
    let site = query.node(candidate.closure).ok_or("no facts")?;
    let escape = query.facts.escapes(site);
    if !query.usable(escape.trust) || escape.value != Escape::None {
        return Err(format!(
            "the closure escapes ({}), so its identity may be seen",
            match escape.value {
                Escape::None => "unknown",
                Escape::Known => "other code holds it",
                Escape::Unknown => "code trioxide doesn't see may hold it",
            }
        ));
    }
    if let Some(symbol) = candidate.binding {
        if scoping.symbol_is_mutated(symbol) {
            return Err("the binding is reassigned".into());
        }
        for reference in scoping.get_resolved_reference_ids(symbol) {
            match callees.get(reference) {
                Some(span) if span.start >= candidate.span.end => {}
                Some(_) => {
                    return Err("the closure is called before its declaration".into());
                }
                None => {
                    return Err(
                        "the closure is used other than by calling it (its identity may be seen)"
                            .into(),
                    );
                }
            }
        }
    }
    if let Some((call, _)) = candidate.call {
        let call = query.node(call).ok_or("no facts")?;
        let resolution = query.facts.site_resolution(call);
        if !(resolution.complete.value && query.usable(resolution.complete.trust))
            || resolution.callees.iter().any(|callee| *callee != site)
        {
            return Err("the call it is passed to may run other code, which could keep it".into());
        }
    }
    Ok(())
}

/// What hoisting one closure does.
struct Hoist {
    name: String,
    binding: Option<SymbolId>,
    top: usize,
    /// The span the new reference gets: the call's, so it isn't inside the function's own
    /// span (which DCE would count as a self-reference).
    reference: Span,
}

/// Finds closures that capture nothing, and direct calls of bindings.
struct Find<'s, 'q> {
    query: Query<'q>,
    scoping: &'s Scoping,
    keep: &'s crate::Keep,
    tagged: &'s FxHashSet<u32>,
    /// References that are direct callees, with their spans.
    callees: FxHashMap<ReferenceId, Span>,
    candidates: Vec<Candidate>,
    /// Enclosing functions: whether each is an island.
    functions: Vec<bool>,
    loops: usize,
    dead: usize,
    opaque: usize,
    top: usize,
}

impl<'a> Find<'_, '_> {
    /// Whether a closure may be hoisted here at all (by position), and if so its problems.
    fn consider(
        &mut self,
        closure: &Expression<'a>,
        binding: Option<(SymbolId, &str)>,
        call: Option<(NodeId, Span, &str)>,
        span: Span,
    ) {
        let (node, problem) = match closure {
            Expression::ArrowFunctionExpression(arrow) => {
                (arrow.node_id(), arrow_problem(arrow, self.scoping))
            }
            Expression::FunctionExpression(function) => {
                (function.node_id(), function_problem(function, self.scoping))
            }
            _ => return,
        };
        if self.dead > 0 || self.opaque > 0 {
            return;
        }
        if self.functions.is_empty() && self.loops == 0 {
            // Created once already.
            return;
        }
        if self.functions.last().copied().unwrap_or(false) {
            return;
        }
        let base = match (binding, call) {
            (Some((_, name)), _) => name.to_string(),
            (None, Some((_, _, method))) => format!("{method}_callback"),
            (None, None) => return,
        };
        self.candidates.push(Candidate {
            closure: node,
            span,
            binding: binding.map(|(symbol, _)| symbol),
            call: call.map(|(call, span, _)| (call, span)),
            base,
            top: self.top,
            rejected: problem,
        });
    }
}

/// Why an arrow can't be hoisted, from its code.
fn arrow_problem(arrow: &ArrowFunctionExpression<'_>, scoping: &Scoping) -> Option<String> {
    if arrow.r#async {
        return Some("the closure is async".into());
    }
    let mut scan = Captures::new(scoping, arrow.scope_id(), true);
    scan.visit_arrow_function_expression(arrow);
    scan.problem
}

/// Why a function expression can't be hoisted, from its code.
fn function_problem(function: &Function<'_>, scoping: &Scoping) -> Option<String> {
    if function.r#async || function.generator {
        return Some("the closure is async or a generator".into());
    }
    if function.id.is_some() {
        return Some("the function expression is named".into());
    }
    let mut scan = Captures::new(scoping, function.scope_id(), false);
    scan.depth = 1;
    if let Some(body) = &function.body {
        scan.visit_formal_parameters(&function.params);
        scan.visit_function_body(body);
    }
    scan.problem
}

/// Looks for what a closure captures.
struct Captures<'s> {
    scoping: &'s Scoping,
    scope: oxc_syntax::scope::ScopeId,
    /// The closure is an arrow, whose `this` and `arguments` are its context's.
    arrow: bool,
    /// Non-arrow functions entered (the closure itself, for a function expression).
    depth: usize,
    problem: Option<String>,
}

impl<'s> Captures<'s> {
    fn new(scoping: &'s Scoping, scope: oxc_syntax::scope::ScopeId, arrow: bool) -> Self {
        Self {
            scoping,
            scope,
            arrow,
            depth: 0,
            problem: None,
        }
    }

    fn stop(&mut self, why: String) {
        if self.problem.is_none() {
            self.problem = Some(why);
        }
    }

    /// `this`, `arguments`, `super` and `new.target` of an arrow (outside nested functions)
    /// belong to its context.
    fn contextual(&self) -> bool {
        self.arrow && self.depth == 0
    }
}

impl<'a> Visit<'a> for Captures<'_> {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        let what = match kind {
            AstKind::ThisExpression(_) => "`this`",
            AstKind::Super(_) => "`super`",
            AstKind::NewTarget(_) => "`new.target`",
            _ => return,
        };
        if self.contextual() || matches!(kind, AstKind::Super(_)) {
            self.stop(format!("the closure uses its context's {what}"));
        }
    }

    fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
        self.depth += 1;
        walk::walk_function(self, function, flags);
        self.depth -= 1;
    }

    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        if ident.name == "arguments" && (self.contextual() || self.depth == 0) {
            self.stop("the closure uses `arguments`".into());
            return;
        }
        let Some(reference) = ident.reference_id.get() else {
            self.stop("the closure has a name the scoping doesn't know".into());
            return;
        };
        let reference = self.scoping.get_reference(reference);
        if !reference.is_value() {
            return;
        }
        let Some(symbol) = reference.symbol_id() else {
            return;
        };
        let scope = self.scoping.symbol_scope_id(symbol);
        let inside = scope == self.scope || self.scoping.scope_is_descendant_of(scope, self.scope);
        if !inside && scope != self.scoping.root_scope_id() {
            self.stop(format!("the closure captures `{}`", ident.name));
        }
    }
}

impl<'a> Visit<'a> for Find<'_, '_> {
    fn visit_program(&mut self, program: &Program<'a>) {
        for (index, statement) in program.body.iter().enumerate() {
            self.top = index;
            self.visit_statement(statement);
        }
    }

    fn visit_statement(&mut self, statement: &Statement<'a>) {
        let dead = self.query.dead(statement.node_id());
        let looping = matches!(
            statement,
            Statement::ForStatement(_)
                | Statement::ForInStatement(_)
                | Statement::ForOfStatement(_)
                | Statement::WhileStatement(_)
                | Statement::DoWhileStatement(_)
        );
        self.dead += usize::from(dead);
        self.loops += usize::from(looping);
        walk::walk_statement(self, statement);
        self.loops -= usize::from(looping);
        self.dead -= usize::from(dead);
    }

    fn visit_ts_namespace_declaration(
        &mut self,
        declaration: &oxc_ast::ast::TSNamespaceDeclaration<'a>,
    ) {
        self.opaque += 1;
        walk::walk_ts_namespace_declaration(self, declaration);
        self.opaque -= 1;
    }

    fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
        let island = is_island(
            self.keep,
            self.query.node(function.node_id()),
            super::forced_island(function, self.tagged, &[function.span.start]),
        ) || self.functions.last().copied().unwrap_or(false);
        let dead = self.query.dead(function.node_id());
        self.dead += usize::from(dead);
        self.functions.push(island);
        let loops = std::mem::take(&mut self.loops);
        walk::walk_function(self, function, flags);
        self.loops = loops;
        self.functions.pop();
        self.dead -= usize::from(dead);
    }

    fn visit_arrow_function_expression(&mut self, arrow: &ArrowFunctionExpression<'a>) {
        let island = is_island(self.keep, self.query.node(arrow.node_id()), false)
            || self.functions.last().copied().unwrap_or(false);
        let dead = self.query.dead(arrow.node_id());
        self.dead += usize::from(dead);
        self.functions.push(island);
        let loops = std::mem::take(&mut self.loops);
        walk::walk_arrow_function_expression(self, arrow);
        self.loops = loops;
        self.functions.pop();
        self.dead -= usize::from(dead);
    }

    fn visit_variable_declaration(&mut self, declaration: &oxc_ast::ast::VariableDeclaration<'a>) {
        if matches!(
            declaration.kind,
            VariableDeclarationKind::Const | VariableDeclarationKind::Let
        ) && !declaration.declare
        {
            for declarator in &declaration.declarations {
                if let (Some(id), Some(init)) =
                    (declarator.id.get_binding_identifier(), &declarator.init)
                {
                    self.consider(
                        init,
                        Some((id.symbol_id(), id.name.as_str())),
                        None,
                        declarator.span,
                    );
                }
            }
        }
        walk::walk_variable_declaration(self, declaration);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if let Expression::Identifier(ident) = &call.callee
            && let Some(reference) = ident.reference_id.get()
            && !call.optional
        {
            self.callees.insert(reference, ident.span);
        }
        if let Some(member) = call.callee.get_member_expr()
            && let Some(method) = member.static_property_name()
        {
            for argument in &call.arguments {
                if let Some(argument) = argument.as_expression()
                    && matches!(
                        argument,
                        Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
                    )
                {
                    let method = method.to_string();
                    self.consider(
                        argument,
                        None,
                        Some((call.node_id(), call.span, &method)),
                        argument.span(),
                    );
                }
            }
        }
        walk::walk_call_expression(self, call);
    }
}

/// Moves hoisted closures to the top level.
struct Rewrite<'s, 'a> {
    scoping: &'s Scoping,
    plans: &'s FxHashMap<NodeId, Hoist>,
    renames: &'s FxHashMap<SymbolId, String>,
    ast: AstBuilder<'a>,
    /// The new declarations, with the top-level index to put each before.
    hoisted: Vec<(usize, Statement<'a>)>,
}

impl<'a> Rewrite<'_, 'a> {
    /// The plan for `expression`, if it is a hoisted closure.
    fn plan(&self, expression: &Expression<'_>) -> Option<&Hoist> {
        let node = match expression {
            Expression::ArrowFunctionExpression(arrow) => arrow.node_id(),
            Expression::FunctionExpression(function) => function.node_id(),
            _ => return None,
        };
        self.plans.get(&node)
    }

    /// The function declaration `name` for the closure `expression`.
    fn declaration(&self, expression: Expression<'a>, name: &str) -> Statement<'a> {
        let ast = &self.ast;
        let span = expression.span();
        let id = Some(BindingIdentifier::new(
            span,
            Ident::from_str_in(name, ast),
            ast,
        ));
        // The function keeps the closure's node id, so facts about it (its callers, what runs
        // at its calls) still answer for it.
        let node = match &expression {
            Expression::ArrowFunctionExpression(arrow) => arrow.node_id(),
            _ => NodeId::DUMMY,
        };
        let declaration = match expression {
            Expression::ArrowFunctionExpression(mut arrow) => {
                let body = match arrow.body.take_in(ast) {
                    oxc_ast::ast::ArrowFunctionBody::FunctionBody(body) => body,
                    body => {
                        let value = body.into_expression();
                        let statement =
                            Statement::new_return_statement(value.span(), Some(value), ast);
                        FunctionBody::boxed(
                            span,
                            ArenaVec::new_in(ast),
                            ArenaVec::from_iter_in([statement], ast),
                            ast,
                        )
                    }
                };
                let mut params = arrow.params.take_in(ast);
                params.kind = FormalParameterKind::FormalParameter;
                Statement::new_function_declaration(
                    span,
                    FunctionType::FunctionDeclaration,
                    id,
                    false,
                    false,
                    false,
                    arrow.type_parameters.take(),
                    None,
                    ArenaBox::new_in(params, ast),
                    arrow.return_type.take(),
                    Some(body),
                    ast,
                )
            }
            Expression::FunctionExpression(mut function) => {
                function.r#type = FunctionType::FunctionDeclaration;
                function.id = id;
                Statement::FunctionDeclaration(function)
            }
            _ => unreachable!("only closures are planned"),
        };
        if let Statement::FunctionDeclaration(function) = &declaration
            && node != NodeId::DUMMY
        {
            function.node_id.set(node);
        }
        declaration
    }

    fn symbol(&self, ident: &IdentifierReference<'_>) -> Option<SymbolId> {
        ident
            .reference_id
            .get()
            .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
    }
}

impl<'a> VisitMut<'a> for Rewrite<'_, 'a> {
    fn visit_statements(&mut self, statements: &mut ArenaVec<'a, Statement<'a>>) {
        walk_mut::walk_statements(self, statements);
        statements.retain_mut(|statement| match statement {
            Statement::VariableDeclaration(declaration) => {
                let before = declaration.declarations.len();
                declaration.declarations.retain(|declarator| {
                    !declarator
                        .id
                        .get_binding_identifier()
                        .is_some_and(|id| self.renames.contains_key(&id.symbol_id()))
                });
                before == declaration.declarations.len() || !declaration.declarations.is_empty()
            }
            _ => true,
        });
    }

    fn visit_variable_declarator(&mut self, declarator: &mut oxc_ast::ast::VariableDeclarator<'a>) {
        if let Some(init) = &mut declarator.init
            && let Some(hoist) = self.plan(init)
        {
            let (top, name) = (hoist.top, hoist.name.clone());
            let closure = init.take_in(&self.ast);
            let mut closure = closure;
            walk_mut::walk_expression(self, &mut closure);
            let declaration = self.declaration(closure, &name);
            self.hoisted.push((top, declaration));
            return;
        }
        walk_mut::walk_variable_declarator(self, declarator);
    }

    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        if let Some(hoist) = self.plan(expression)
            && hoist.binding.is_none()
        {
            let (top, name, span) = (hoist.top, hoist.name.clone(), hoist.reference);
            let mut closure = expression.take_in(&self.ast);
            walk_mut::walk_expression(self, &mut closure);
            let declaration = self.declaration(closure, &name);
            self.hoisted.push((top, declaration));
            *expression =
                Expression::new_identifier(span, Ident::from_str_in(&name, &self.ast), &self.ast);
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
}
