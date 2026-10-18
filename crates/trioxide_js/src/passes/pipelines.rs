//! PLAN §4 pass 6: array pipelines become loops.
//!
//! A chain of `map` and `filter` calls ending in `map`, `filter`, `forEach`, `reduce`,
//! `some`, `every`, `find`, `findIndex` or `includes` (a single call too) becomes one `for`
//! loop with the callbacks written into its body:
//!
//! ```js
//! const total = values.map((x) => x * 2).filter((y) => y > 3).reduce((sum, y) => sum + y, 0);
//! // becomes
//! let result = 0;
//! for (let i = 0, length = values.length; i < length; i++) {
//!   const x = values[i];
//!   const y = x * 2;
//!   if (!(y > 3)) continue;
//!   result = result + y;
//! }
//! const total = result;
//! ```
//!
//! The result accumulates in a binding of its own, and a declared one is assigned once, after
//! the loop: it stays in its temporal dead zone while the callbacks run (as it did), and a
//! callback that throws leaves a `var` without a partial value.
//!
//! It applies to a chain that initialises a `const`/`let`/`var` binding, is returned, or (for
//! `forEach`) is a statement, when:
//!
//! - **Receiver:** a plain array with intact builtins ([`prove_array`]), and every call's site
//!   is complete and calls only its callback (so the builtin model is what runs).
//! - **Callbacks:** arrows or function expressions written in the call, not async or
//!   generators; function expressions don't use `this`, `arguments` or `new.target` (arrows
//!   keep the enclosing ones, which inlining preserves). Parameters have no defaults or rest
//!   elements; the body is an expression, or statements ending in the only `return` (a
//!   `forEach` body has none), with no `var`, function declarations or labels. A `thisArg`
//!   must be pure, and is dropped.
//! - **Effect order:** fusing runs the callbacks element by element (`f, g, f, g`) instead of
//!   stage by stage (`f, f, g, g`). That is allowed when every callback is free of writes,
//!   throws and unknown calls; or when only the first callback isn't and the others read no
//!   mutable state (they can't see its effects, and its own order is unchanged); or when only
//!   the last isn't, the others read no mutable state, and the receiver's arrays never leave
//!   the function and aren't named in the last callback (it can't change what the earlier
//!   stages read). A short-circuiting terminal (`some`, `every`, `find`, `findIndex`,
//!   `includes`) stops early, so the stages before it must all be free of effects (their
//!   remaining calls, and the exceptions they would throw, are skipped). A `reduce` seed or
//!   `includes` argument moves before the loop, so it must be pure, and read nothing when a
//!   callback has effects. A destructuring parameter needs elements that are never nullish
//!   (destructuring `null` in a parameter throws a different message).
//! - **Holes:** `map`, `filter`, `forEach`, `reduce`, `some` and `every` skip holes, so the
//!   loop starts with `if (!(i in values)) continue;` unless the arrays never have holes and
//!   no callback can make one. `map` keeps holes (its result is then `new Array(length)`
//!   written by index); `find`, `findIndex` and `includes` visit them as `undefined`, which
//!   `values[i]` reads, and a chain that would hand them a hole from a `map` stays.
//! - **Iteration:** the range is fixed at the start (`length` read once) and elements are read
//!   at each step, as the builtins do. Callbacks get `(value, index, array)`; the index after
//!   a `filter` and an intermediate array are not available (the chain stays if a callback
//!   declares them), so intermediate arrays never escape.
//! - `reduce` with no seed throws a `TypeError` on an empty range, as the builtin does.

use oxc_allocator::{ArenaVec, CloneIn, TakeIn};
use oxc_ast::ast::{
    Argument, ArrowFunctionBody, AssignmentOperator, BinaryOperator, BindingPattern,
    CallExpression, Expression, ForStatementInit, IdentifierName, IdentifierReference,
    LogicalOperator, Program, Statement, UnaryOperator, UpdateOperator, VariableDeclarationKind,
    VariableDeclarator,
};
use oxc_ast_visit::{Visit, VisitMut, walk};
use oxc_semantic::Scoping;
use oxc_span::{GetSpan, GetSpanMut, SPAN, Span};
use oxc_syntax::{
    reference::ReferenceFlags,
    scope::{ScopeFlags, ScopeId},
    symbol::{SymbolFlags, SymbolId},
};
use oxc_traverse::{BoundIdentifier, Traverse, TraverseCtx};
use rustc_hash::FxHashSet;
use trioxide_analysis::{Effects, Escape, Trust};
use trioxide_types::Type;

use super::{
    Decisions, Position,
    arrays::{ArrayProof, FunctionNames, Kept, NamesIn, prove_array},
    for_of::{length_of, stable_binding, zero},
    track_position,
    util::{Query, is_global_constant, is_global_name, is_inert, never_nullish, weak},
};
use crate::{Pass, PassContext};

/// PLAN §4 pass 6 (`"pipelines"`).
pub struct Pipelines;

impl Pass for Pipelines {
    fn name(&self) -> &'static str {
        "pipelines"
    }

    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
        let query = Query {
            facts: ctx.facts,
            module: ctx.module,
        };
        let mut visitor = Visitor {
            query,
            kept: Kept::new(program, ctx.keep, query),
            names: FunctionNames::new(program),
            position: Position::default(),
            decisions: Decisions::new(self.name()),
        };
        ctx.traverse(&mut visitor, program, ());
        let changed = visitor.decisions.flush(ctx.log);
        if changed {
            // The next speed pass gets a scoping that matches the rewritten AST.
            *ctx.scoping_mut() = super::speed::rescope(program);
        }
        changed
    }
}

struct Visitor<'f> {
    query: Query<'f>,
    kept: Kept,
    names: FunctionNames,
    position: Position,
    decisions: Decisions,
}

/// The array methods a chain may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Map,
    Filter,
    ForEach,
    Reduce,
    Some,
    Every,
    Find,
    FindIndex,
    Includes,
}

impl Method {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "map" => Self::Map,
            "filter" => Self::Filter,
            "forEach" => Self::ForEach,
            "reduce" => Self::Reduce,
            "some" => Self::Some,
            "every" => Self::Every,
            "find" => Self::Find,
            "findIndex" => Self::FindIndex,
            "includes" => Self::Includes,
            _ => return None,
        })
    }

    /// Whether it visits holes as `undefined` instead of skipping them.
    fn visits_holes(self) -> bool {
        matches!(self, Self::Find | Self::FindIndex | Self::Includes)
    }

    /// How many parameters its callback may declare (`value, index, array`, after the
    /// accumulator for `reduce`).
    fn max_params(self) -> usize {
        if self == Self::Reduce { 4 } else { 3 }
    }

    /// The position of the element among the callback's parameters.
    fn value_param(self) -> usize {
        usize::from(self == Self::Reduce)
    }

    /// A readable name for the chain's result.
    fn result_name(self) -> &'static str {
        match self {
            Self::Map | Self::Filter | Self::Reduce => "result",
            Self::Some | Self::Includes | Self::Find => "found",
            Self::Every => "all",
            Self::FindIndex => "index",
            Self::ForEach => "unused",
        }
    }
}

/// Where the chain is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `const x = chain;` (or `let`, `var`).
    Declaration(VariableDeclarationKind),
    /// `chain;`
    Statement,
    /// `return chain;`
    Return,
}

/// What the checks found about one call of the chain.
struct StageInfo {
    method: Method,
    /// The callback's effects.
    effects: Effects,
}

impl<'a> Traverse<'a, ()> for Visitor<'_> {
    track_position!();

    fn exit_statements(
        &mut self,
        statements: &mut ArenaVec<'a, Statement<'a>>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if !self.position.live() {
            return;
        }
        let mut index = 0;
        while index < statements.len() {
            match self.lower(&mut statements[index], ctx) {
                Some(replacement) => {
                    let count = replacement.len();
                    statements.remove(index);
                    for (offset, statement) in replacement.into_iter().enumerate() {
                        statements.insert(index + offset, statement);
                    }
                    index += count;
                }
                None => index += 1,
            }
        }
    }
}

/// The chain expression of a statement, and its form.
fn chain_of<'s, 'a>(statement: &'s Statement<'a>) -> Option<(&'s Expression<'a>, Form)> {
    match statement {
        Statement::VariableDeclaration(declaration)
            if declaration.declarations.len() == 1
                && !declaration.declare
                && matches!(
                    declaration.declarations[0].id,
                    BindingPattern::BindingIdentifier(_)
                ) =>
        {
            let init = declaration.declarations[0].init.as_ref()?;
            Some((init, Form::Declaration(declaration.kind)))
        }
        Statement::ExpressionStatement(statement) => Some((&statement.expression, Form::Statement)),
        Statement::ReturnStatement(statement) => Some((statement.argument.as_ref()?, Form::Return)),
        _ => None,
    }
}

/// The calls of a chain from the receiver outwards, and the receiver.
fn calls_of<'s, 'a>(
    expression: &'s Expression<'a>,
) -> Option<(Vec<&'s CallExpression<'a>>, &'s Expression<'a>)> {
    let mut calls = Vec::new();
    let mut current = expression.without_parentheses();
    while let Expression::CallExpression(call) = current {
        let Expression::StaticMemberExpression(member) = &call.callee else {
            break;
        };
        let Some(method) = Method::from_name(&member.property.name) else {
            break;
        };
        if call.optional || member.optional || call.type_arguments.is_some() {
            break;
        }
        // Only `map` and `filter` feed another call.
        if !calls.is_empty() && !matches!(method, Method::Map | Method::Filter) {
            break;
        }
        calls.push(&**call);
        current = member.object.without_parentheses();
    }
    if calls.is_empty() {
        return None;
    }
    calls.reverse();
    Some((calls, current))
}

/// The function of a callback argument, as `(unit node, params, async or generator, uses
/// own this/arguments/new.target or its own name)`.
fn callback_parts<'s, 'a>(
    argument: &'s Argument<'a>,
    scoping: &Scoping,
) -> Result<
    (
        oxc_syntax::node::NodeId,
        &'s oxc_ast::ast::FormalParameters<'a>,
    ),
    &'static str,
> {
    match argument {
        Argument::ArrowFunctionExpression(arrow) => {
            if arrow.r#async {
                return Err("the callback is async");
            }
            Ok((arrow.node_id.get(), &arrow.params))
        }
        Argument::FunctionExpression(function) => {
            if function.r#async || function.generator {
                return Err("the callback is async or a generator");
            }
            if let Some(id) = &function.id
                && !scoping.symbol_is_unused(id.symbol_id())
            {
                return Err("the callback refers to itself by name");
            }
            let mut find = OwnContext(false);
            if let Some(body) = &function.body {
                find.visit_function_body(body);
            }
            if find.0 {
                return Err("the callback uses its own `this`, `arguments` or `new.target`");
            }
            Ok((function.node_id.get(), &function.params))
        }
        _ => Err("the callback is not an arrow or function written in the call"),
    }
}

/// Finds `this`, `arguments` and `new.target` that belong to a function (looking through
/// arrows, not into nested functions or classes).
struct OwnContext(bool);

impl<'a> Visit<'a> for OwnContext {
    fn visit_this_expression(&mut self, _: &oxc_ast::ast::ThisExpression) {
        self.0 = true;
    }

    fn visit_new_target(&mut self, _: &oxc_ast::ast::NewTarget) {
        self.0 = true;
    }

    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        if ident.name == "arguments" {
            self.0 = true;
        }
    }

    fn visit_function(&mut self, _: &oxc_ast::ast::Function<'a>, _: ScopeFlags) {}

    fn visit_class(&mut self, _: &oxc_ast::ast::Class<'a>) {}
}

/// The statements of a callback body, checked: `Ok(true)` if it ends in its only `return`
/// (with a value), `Ok(false)` if it has no `return`.
fn body_shape(statements: &[Statement<'_>]) -> Result<bool, &'static str> {
    struct Find {
        returns: usize,
        bad: Option<&'static str>,
    }
    impl<'a> Visit<'a> for Find {
        fn visit_return_statement(&mut self, statement: &oxc_ast::ast::ReturnStatement<'a>) {
            self.returns += 1;
            walk::walk_return_statement(self, statement);
        }

        fn visit_variable_declaration(
            &mut self,
            declaration: &oxc_ast::ast::VariableDeclaration<'a>,
        ) {
            if declaration.kind == VariableDeclarationKind::Var {
                self.bad = Some("the callback declares a `var`");
            }
            walk::walk_variable_declaration(self, declaration);
        }

        fn visit_labeled_statement(&mut self, _: &oxc_ast::ast::LabeledStatement<'a>) {
            self.bad = Some("the callback has a label");
        }

        fn visit_function(&mut self, _: &oxc_ast::ast::Function<'a>, _: ScopeFlags) {}

        fn visit_arrow_function_expression(
            &mut self,
            _: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
        }

        fn visit_class(&mut self, _: &oxc_ast::ast::Class<'a>) {}
    }
    for statement in statements {
        if matches!(
            statement,
            Statement::FunctionDeclaration(_) | Statement::ClassDeclaration(_)
        ) || statement.is_module_declaration()
        {
            return Err("the callback declares a function or class");
        }
    }
    let mut find = Find {
        returns: 0,
        bad: None,
    };
    for statement in statements {
        find.visit_statement(statement);
    }
    if let Some(bad) = find.bad {
        return Err(bad);
    }
    match (find.returns, statements.last()) {
        (0, _) => Ok(false),
        (1, Some(Statement::ReturnStatement(last))) if last.argument.is_some() => Ok(true),
        _ => Err("the callback returns from somewhere other than its last statement"),
    }
}

impl<'a> Visitor<'_> {
    /// The statements replacing `statement`, if it holds a chain that can be lowered.
    fn lower(
        &mut self,
        statement: &mut Statement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) -> Option<Vec<Statement<'a>>> {
        let (expression, form) = chain_of(statement)?;
        let (calls, receiver) = calls_of(expression)?;
        let span = expression.span();
        if self.query.dead(statement.node_id()) || self.kept.contains(span) {
            return None;
        }
        let terminal = Method::from_name(match &calls.last()?.callee {
            Expression::StaticMemberExpression(member) => &member.property.name,
            _ => return None,
        })?;
        match (form, terminal) {
            (Form::Statement, Method::ForEach) => {}
            (Form::Statement, _) | (_, Method::ForEach) => return None,
            _ => {}
        }
        let plan = match self.check(&calls, receiver, ctx.scoping()) {
            Ok(plan) => plan,
            Err(reason) => {
                self.decisions.skipped(span, reason);
                return None;
            }
        };
        self.decisions.applied(span);
        Some(self.rewrite(statement, form, terminal, &plan, ctx))
    }

    /// Checks every precondition; returns what the rewrite needs to know.
    fn check(
        &self,
        calls: &[&CallExpression<'a>],
        receiver: &Expression<'a>,
        scoping: &Scoping,
    ) -> Result<Plan, String> {
        let proof: ArrayProof = prove_array(self.query, receiver)?;
        // Destructuring `null` or `undefined` throws a message that names the parameter, which
        // a declaration's doesn't: patterns need elements that are never nullish.
        let elements_present = match self
            .query
            .ty(receiver.get_inner_expression().node_id())
            .value
        {
            Type::Array(element) => never_nullish(element),
            Type::Tuple(items) => items.iter().all(never_nullish),
            _ => false,
        };
        let mut stages: Vec<StageInfo> = Vec::new();
        let mut filtered = false;
        let mut extras: Vec<&Expression<'a>> = Vec::new();
        for (position, call) in calls.iter().enumerate() {
            let Expression::StaticMemberExpression(member) = &call.callee else {
                unreachable!("calls_of checked")
            };
            let method = Method::from_name(&member.property.name).expect("calls_of checked");
            let site = self
                .query
                .node(call.node_id.get())
                .ok_or("the call has no facts")?;
            let resolution = self.query.facts.site_resolution(site);
            if !self.query.usable(resolution.complete.trust) || !resolution.complete.value {
                return Err(match resolution.complete.trust {
                    Trust::Declared => weak(Trust::Declared, "what the call runs"),
                    _ => format!(
                        "`{}` may run code the analysis doesn't see",
                        member.property.name
                    ),
                });
            }
            if call
                .arguments
                .iter()
                .any(|argument| matches!(argument, Argument::SpreadElement(_)))
            {
                return Err("an argument is spread".to_string());
            }
            if method == Method::Includes {
                if call.arguments.len() != 1 || !resolution.callees.is_empty() {
                    return Err("`includes` has a start index".to_string());
                }
                let search = call.arguments[0].to_expression();
                extras.push(search);
                stages.push(StageInfo {
                    method,
                    effects: Effects::PURE,
                });
                continue;
            }
            let Some(callback) = call.arguments.first() else {
                return Err("the call has no callback".to_string());
            };
            let (unit, params) = callback_parts(callback, scoping)?;
            let unit = self.query.node(unit).ok_or("the callback has no facts")?;
            if resolution.callees.iter().any(|callee| *callee != unit) {
                return Err("the call may run something other than its callback".to_string());
            }
            if params.rest.is_some()
                || params
                    .items
                    .iter()
                    .any(|parameter| parameter.initializer.is_some())
            {
                return Err("a callback parameter has a default or is a rest parameter".to_string());
            }
            let count = params.items.len();
            if count > method.max_params() {
                return Err("the callback declares more parameters than it is given".to_string());
            }
            let value = method.value_param();
            if count > value + 1 && filtered {
                return Err("a callback after a `filter` declares the index parameter".to_string());
            }
            if count > value + 2 && position > 0 {
                return Err(
                    "a callback after the first declares the array parameter (an intermediate \
                     array)"
                        .to_string(),
                );
            }
            if params
                .items
                .get(method.value_param())
                .is_some_and(|parameter| !parameter.pattern.is_binding_identifier())
                && (position > 0 || !elements_present)
            {
                return Err(
                    "a callback destructures an element that may be `null` or `undefined`"
                        .to_string(),
                );
            }
            if method == Method::FindIndex && filtered {
                return Err("`findIndex` after a `filter` needs the filtered index".to_string());
            }
            // The body: an arrow's expression, or statements ending in their only `return`
            // (none for `forEach`).
            let block = match callback {
                Argument::ArrowFunctionExpression(arrow) => arrow.get_function_body(),
                Argument::FunctionExpression(function) => {
                    Some(&**function.body.as_ref().ok_or("the callback has no body")?)
                }
                _ => unreachable!("callback_parts checked"),
            };
            if let Some(body) = block {
                if !body.directives.is_empty() {
                    return Err("the callback has directives".to_string());
                }
                let returns = body_shape(&body.statements)?;
                if returns == (method == Method::ForEach) {
                    return Err(if returns {
                        "the `forEach` callback returns".to_string()
                    } else {
                        "the callback returns nothing".to_string()
                    });
                }
            }
            // `thisArg`, or `reduce`'s seed.
            match (method, call.arguments.get(1)) {
                (_, None) => {}
                (Method::Reduce, Some(seed)) => extras.push(seed.to_expression()),
                (_, Some(this_arg)) => {
                    if !self.query.pure(this_arg.to_expression().node_id()) {
                        return Err("the `thisArg` may have effects".to_string());
                    }
                }
            }
            if call.arguments.len() > 2 {
                return Err("the call has extra arguments".to_string());
            }
            let effects = self.query.facts.effects(unit);
            if !self.query.usable(effects.trust) {
                return Err(weak(effects.trust, "the callback's effects"));
            }
            stages.push(StageInfo {
                method,
                effects: effects.value,
            });
            filtered |= method == Method::Filter;
        }
        // Effect order.
        let effectful: Vec<usize> = stages
            .iter()
            .enumerate()
            .filter(|(_, stage)| !stage.effects.is_side_effect_free())
            .map(|(index, _)| index)
            .collect();
        let strictly_pure = |stage: &StageInfo| {
            let effects = stage.effects;
            !effects.reads && effects.is_side_effect_free()
        };
        let last = stages.len() - 1;
        // Whether the callback of stage `position` can't reach the receiver's arrays: they
        // never leave the function, and it neither names them nor takes the array parameter.
        let local = proof.sites.as_ref().is_some_and(|sites| {
            !sites.is_empty()
                && sites
                    .iter()
                    .all(|site| self.query.facts.escapes(*site).value == Escape::None)
        });
        let cannot_reach = |position: usize| {
            let Some(callback) = calls[position].arguments.first() else {
                return true;
            };
            let named = match receiver.get_inner_expression() {
                Expression::Identifier(ident) => {
                    let mut names = NamesIn::default();
                    names.visit_argument(callback);
                    names.0.contains(ident.name.as_str())
                }
                _ => true,
            };
            let takes_array = callback_parts(callback, scoping).is_ok_and(|(_, params)| {
                params.items.len() > stages[position].method.value_param() + 2
            });
            local && !named && !takes_array
        };
        let first_mutates = match effectful.as_slice() {
            [] => false,
            [_] if stages.len() == 1 => !cannot_reach(0),
            [0] => {
                if !stages[1..].iter().all(strictly_pure) {
                    return Err(
                        "the first callback has effects, which a later one may observe".to_string(),
                    );
                }
                !cannot_reach(0)
            }
            [only] if *only == last => {
                if !stages[..last].iter().all(strictly_pure) {
                    return Err(
                        "the last callback has effects, which an earlier one may observe"
                            .to_string(),
                    );
                }
                if !cannot_reach(last) {
                    return Err(
                        "the last callback has effects, which may change elements earlier stages \
                         read"
                            .to_string(),
                    );
                }
                false
            }
            _ => return Err("more than one callback has effects".to_string()),
        };
        // A short-circuiting terminal stops the loop early: the earlier stages' remaining calls
        // (and any exception they would throw) must not matter.
        if stages.len() > 1
            && matches!(
                stages[last].method,
                Method::Some | Method::Every | Method::Find | Method::FindIndex | Method::Includes
            )
            && !stages[..last]
                .iter()
                .all(|stage| stage.effects.is_side_effect_free())
        {
            return Err(
                "an earlier callback has effects, which the search's early exit would cut short"
                    .to_string(),
            );
        }
        for extra in &extras {
            let node = extra.node_id();
            if !self.query.pure(node) {
                return Err("the seed or search value may have effects".to_string());
            }
            if !effectful.is_empty()
                && !is_inert(extra, scoping)
                && self
                    .query
                    .facts
                    .expr_effects(self.query.node(node).ok_or("no facts")?)
                    .value
                    .reads
            {
                return Err(
                    "the seed or search value reads state a callback with effects may change"
                        .to_string(),
                );
            }
        }
        // Holes.
        // `find`, `findIndex` and `includes` alone visit holes (as `undefined`).
        let guard = (!proof.packed || first_mutates) && !stages[0].method.visits_holes();
        if guard && stages.len() > 1 {
            let reaches = stages
                .iter()
                .position(|stage| stage.method == Method::Filter)
                .is_none_or(|filter| filter == last);
            if reaches && stages[last].method.visits_holes() {
                return Err(format!(
                    "`{}` would see the holes `map` keeps",
                    match stages[last].method {
                        Method::Find => "find",
                        Method::FindIndex => "findIndex",
                        _ => "includes",
                    }
                ));
            }
        }
        let reduce_unseeded =
            stages[last].method == Method::Reduce && calls[last].arguments.len() == 1;
        if reduce_unseeded && !is_global_name(scoping, scoping.root_scope_id(), "TypeError") {
            return Err("`TypeError` is shadowed".to_string());
        }
        let hole_map =
            guard && !filtered_before(&stages, last) && stages[last].method == Method::Map;
        // `===` is SameValueZero unless both sides may be NaN.
        let search_strict = stages[last].method != Method::Includes
            || extras.last().is_some_and(|search| {
                let fact = self.query.ty(search.get_inner_expression().node_id());
                self.query.usable(fact.trust)
                    && !matches!(fact.value, Type::Unknown | Type::Never)
                    && fact.value.members().iter().all(|member| match member {
                        Type::Number(number) => number.non_nan,
                        _ => true,
                    })
            });
        Ok(Plan {
            guard,
            hole_map,
            reduce_unseeded,
            search_strict,
            stages: stages.len(),
        })
    }
}

/// Whether a `filter` comes before stage `index`.
fn filtered_before(stages: &[StageInfo], index: usize) -> bool {
    stages[..index]
        .iter()
        .any(|stage| stage.method == Method::Filter)
}

/// What [`Visitor::check`] decided.
struct Plan {
    /// Skip holes (and elements a callback deleted) with `if (!(i in array)) continue;`.
    guard: bool,
    /// A terminal `map` that keeps holes: `new Array(length)` written by index.
    hole_map: bool,
    /// `reduce` with no seed.
    reduce_unseeded: bool,
    /// The `includes` search value is never NaN, so `===` compares like `SameValueZero`.
    search_strict: bool,
    /// How many calls the chain has.
    stages: usize,
}

/// One callback, taken out of its call.
struct Inlined<'a> {
    params: Vec<BindingPattern<'a>>,
    statements: Vec<Statement<'a>>,
    result: Option<Expression<'a>>,
}

/// The current element of a stage.
enum Value<'a> {
    /// The array read, not bound yet: pure, so it can go unread.
    Element(Expression<'a>),
    /// What a `map` callback returned, not bound yet.
    Expression(Expression<'a>),
    /// Held by a binding nothing reassigns.
    Read(BoundIdentifier<'a>),
}

impl<'a> Visitor<'_> {
    /// Replaces `statement` (checked by [`Self::check`]) with the loop.
    #[expect(clippy::too_many_lines, reason = "one straight-line construction")]
    fn rewrite(
        &mut self,
        statement: &mut Statement<'a>,
        form: Form,
        terminal: Method,
        plan: &Plan,
        ctx: &mut TraverseCtx<'a, ()>,
    ) -> Vec<Statement<'a>> {
        let outer = ctx.current_block_scope_id();
        // Generated references carry the statement's span (DCE tells uses apart by span).
        let at = statement.span();
        let original = statement.take_in(&ctx.ast);
        let (chain, declared) = match original {
            Statement::VariableDeclaration(declaration) => {
                let mut declaration = declaration.unbox();
                let mut declarator = declaration.declarations.pop().expect("one declarator");
                let init = declarator.init.take().expect("chain_of checked");
                (init, Some((declaration.kind, declarator)))
            }
            Statement::ExpressionStatement(statement) => (statement.unbox().expression, None),
            Statement::ReturnStatement(statement) => {
                (statement.unbox().argument.expect("chain_of checked"), None)
            }
            _ => unreachable!("chain_of checked"),
        };
        // Take the chain apart, outermost call first.
        let mut methods = Vec::new();
        let mut callbacks: Vec<Option<Argument<'a>>> = Vec::new();
        let mut extras: Vec<Option<Expression<'a>>> = Vec::new();
        let mut current = strip(chain);
        let receiver = loop {
            let Expression::CallExpression(call) = current else {
                unreachable!("calls_of checked")
            };
            let call = call.unbox();
            let Expression::StaticMemberExpression(member) = call.callee else {
                unreachable!("calls_of checked")
            };
            let member = member.unbox();
            let method = Method::from_name(&member.property.name).expect("calls_of checked");
            let mut arguments = call.arguments.into_iter();
            if method == Method::Includes {
                callbacks.push(None);
                extras.push(arguments.next().map(Argument::into_expression));
            } else {
                callbacks.push(arguments.next());
                extras.push(match method {
                    Method::Reduce => arguments.next().map(Argument::into_expression),
                    _ => None,
                });
            }
            methods.push(method);
            current = strip(member.object);
            if methods.len() == plan.stages {
                break current;
            }
        };
        methods.reverse();
        callbacks.reverse();
        let mut extra = extras.into_iter().next().flatten();
        // Names: those of the whole chain, and of each callback.
        let mut local = NamesIn::default();
        let per_callback: Vec<NamesIn> = callbacks
            .iter()
            .map(|callback| {
                let mut names = NamesIn::default();
                if let Some(callback) = callback {
                    names.visit_argument(callback);
                }
                local.0.extend(names.0.iter().cloned());
                names
            })
            .collect();
        let mut generated = NamesIn::default();
        generated.visit_expression(&receiver);
        if let Some(extra) = &extra {
            generated.visit_expression(extra);
        }
        local.0.extend(generated.0.iter().cloned());
        let mut before: Vec<Statement<'a>> = Vec::new();
        // The receiver: a binding nothing reassigns, or a new one.
        let array = if let Some(binding) = stable_binding(&receiver, ctx) {
            binding
        } else {
            let base = match receiver.get_inner_expression() {
                Expression::StaticMemberExpression(member) => member.property.name.to_string(),
                _ => "items".to_string(),
            };
            let binding = self.declare(&base, at, outer, ctx);
            before.push(const_declaration(&binding, receiver, ctx));
            binding
        };
        generated.0.insert(array.name.to_string());
        // The result binding.
        // Accumulated in a binding of its own: a declared result is assigned once, after the
        // loop, so it stays in its temporal dead zone while the callbacks run, and gets no
        // partial value when one throws.
        let target = (terminal != Method::ForEach)
            .then(|| self.declare(terminal.result_name(), at, outer, ctx));
        if let Some(target) = &target {
            generated.0.insert(target.name.to_string());
        }
        // The `includes` search value moves before the loop.
        let search = if terminal == Method::Includes {
            let search = strip(extra.take().expect("includes has an argument"));
            if is_literal(&search)
                || matches!(&search, Expression::Identifier(ident) if is_global_constant(ident, ctx.scoping()))
            {
                Some(search)
            } else {
                let binding = self.declare("search", at, outer, ctx);
                generated.0.insert(binding.name.to_string());
                before.push(const_declaration(&binding, search, ctx));
                Some(binding.create_spanned_read_expression(at, ctx))
            }
        } else {
            None
        };
        let empty = plan.reduce_unseeded.then(|| {
            let binding = self.declare("empty", at, outer, ctx);
            generated.0.insert(binding.name.to_string());
            binding
        });
        // The result's declaration.
        if let Some(target) = &target {
            let initial = match terminal {
                Method::Map if plan.hole_map => Some(Expression::new_new_expression(
                    SPAN,
                    ctx.create_unbound_ident_expr(SPAN, "Array".into(), ReferenceFlags::Read),
                    None,
                    ArenaVec::from_iter_in([Argument::from(length_of(&array, at, ctx))], &ctx.ast),
                    ctx,
                )),
                Method::Map | Method::Filter => Some(Expression::new_array_expression(
                    SPAN,
                    ArenaVec::new_in(&ctx.ast),
                    ctx,
                )),
                Method::Reduce => extra.take().map(strip),
                Method::Some | Method::Includes => {
                    Some(Expression::new_boolean_literal(SPAN, false, ctx))
                }
                Method::Every => Some(Expression::new_boolean_literal(SPAN, true, ctx)),
                Method::FindIndex => Some(Expression::new_unary_expression(
                    SPAN,
                    UnaryOperator::UnaryNegation,
                    number(1.0, ctx),
                    ctx,
                )),
                Method::Find | Method::ForEach => None,
            };
            let array_result = matches!(terminal, Method::Map | Method::Filter);
            let kind = if array_result {
                VariableDeclarationKind::Const
            } else {
                VariableDeclarationKind::Let
            };
            let declarator = VariableDeclarator::new(
                SPAN,
                target.create_binding_pattern(ctx),
                None,
                initial,
                false,
                ctx,
            );
            before.push(Statement::new_variable_declaration(
                SPAN,
                kind,
                ArenaVec::from_iter_in([declarator], &ctx.ast),
                false,
                ctx,
            ));
        }
        if let Some(empty) = &empty {
            before.push(Statement::new_variable_declaration(
                SPAN,
                VariableDeclarationKind::Let,
                ArenaVec::from_iter_in(
                    [VariableDeclarator::new(
                        SPAN,
                        empty.create_binding_pattern(ctx),
                        None,
                        Some(Expression::new_boolean_literal(SPAN, true, ctx)),
                        false,
                        ctx,
                    )],
                    &ctx.ast,
                ),
                false,
                ctx,
            ));
        }
        // The loop's own scopes.
        let loop_scope = ctx.create_child_scope(outer, ScopeFlags::empty());
        let body_scope = ctx.create_child_scope(loop_scope, ScopeFlags::empty());
        // What each callback refers to outside itself.
        let free: Vec<FxHashSet<String>> = callbacks
            .iter()
            .map(|callback| {
                callback
                    .as_ref()
                    .map(|callback| free_names(callback, ctx.scoping()))
                    .unwrap_or_default()
            })
            .collect();
        // Callback bindings move into the loop body. One that would collide with another
        // callback's, or capture a name the loop or another callback uses, gets a fresh
        // name. Some need no binding of their own: an element parameter after a `filter`
        // whose own has the same name, and a `reduce` accumulator, which becomes the result.
        let mut moved: FxHashSet<String> = FxHashSet::default();
        let mut inlined: Vec<Option<Inlined<'a>>> = Vec::new();
        // The previous stage's element parameter: its symbol, original name and current name.
        let mut previous_element: Option<(SymbolId, String, oxc_ast::ast::Ident<'a>)> = None;
        let mut index_param: Option<BoundIdentifier<'a>> = None;
        for (position, callback) in callbacks.into_iter().enumerate() {
            let method = methods[position];
            let Some(mut callback) = callback else {
                inlined.push(None);
                previous_element = None;
                continue;
            };
            let others: FxHashSet<&str> = free
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != position)
                .flat_map(|(_, names)| names.iter().map(String::as_str))
                .chain(generated.0.iter().map(String::as_str))
                .chain(moved.iter().map(String::as_str))
                .collect();
            let element = param_binding(&callback, method.value_param(), ctx.scoping());
            let alias = match (&previous_element, &element) {
                (Some((_, original, name)), Some((symbol, own)))
                    if position > 0
                        && methods[position - 1] == Method::Filter
                        && original == own =>
                {
                    Some((*symbol, *name))
                }
                _ => None,
            };
            let accumulator = (method == Method::Reduce)
                .then(|| param_binding(&callback, 0, ctx.scoping()))
                .flatten()
                .filter(|(symbol, _)| !captured(*symbol, &callback, ctx.scoping()))
                .zip(target.as_ref())
                .filter(|(_, target)| !per_callback[position].0.contains(target.name.as_str()))
                .map(|((symbol, _), target)| (symbol, target.name));
            let mut renames: Vec<(SymbolId, oxc_ast::ast::Ident<'a>)> = Vec::new();
            let mut own_moved = Vec::new();
            for (symbol, name) in moved_bindings(&callback) {
                if let Some((alias_symbol, alias_name)) = alias
                    && alias_symbol == symbol
                {
                    if alias_name.as_str() != name {
                        renames.push((symbol, alias_name));
                    }
                    continue;
                }
                if let Some((accumulator_symbol, target_name)) = accumulator
                    && accumulator_symbol == symbol
                {
                    renames.push((symbol, target_name));
                    continue;
                }
                if others.contains(name.as_str()) {
                    let fresh = local.fresh(&name);
                    let fresh = oxc_ast::ast::Ident::from_str_in(&fresh, ctx);
                    renames.push((symbol, fresh));
                    own_moved.push(fresh.to_string());
                } else {
                    own_moved.push(name);
                }
            }
            moved.extend(own_moved);
            if !renames.is_empty() {
                let mut rename = Rename {
                    scoping: ctx.scoping(),
                    renames: &renames,
                };
                rename.visit_argument(&mut callback);
                for (symbol, name) in &renames {
                    ctx.scoping_mut().set_symbol_name(*symbol, *name);
                }
            }
            let renamed = |symbol: SymbolId, name: String| {
                renames
                    .iter()
                    .find(|(s, _)| *s == symbol)
                    .map_or(name, |(_, new)| new.to_string())
            };
            previous_element = element.map(|(symbol, original)| {
                let name = renamed(symbol, original.clone());
                (
                    symbol,
                    original,
                    oxc_ast::ast::Ident::from_str_in(&name, ctx),
                )
            });
            if position == 0
                && let Some((symbol, name)) =
                    param_binding(&callback, method.value_param() + 1, ctx.scoping())
            {
                let name = renamed(symbol, name);
                index_param = Some(BoundIdentifier::new(
                    oxc_ast::ast::Ident::from_str_in(&name, ctx),
                    symbol,
                ));
            }
            inlined.push(Some(take_callback(callback, ctx)));
        }
        let index = if let Some(index) = &index_param {
            index.clone()
        } else {
            let name =
                local.fresh_index(|name| ctx.scoping().find_binding(outer, name.into()).is_some());
            super::for_of::generate_named(&name, loop_scope, ctx)
        };
        let length = super::for_of::generate(&mut local, "length", loop_scope, ctx);
        // The body.
        let mut body: Vec<Statement<'a>> = Vec::new();
        if plan.guard {
            let test = Expression::new_binary_expression(
                SPAN,
                index.create_spanned_read_expression(at, ctx),
                BinaryOperator::In,
                array.create_spanned_read_expression(at, ctx),
                ctx,
            );
            body.push(Statement::new_if_statement(
                SPAN,
                not(test, ctx),
                Statement::new_continue_statement(SPAN, None, ctx),
                None,
                ctx,
            ));
        }
        let element = Expression::new_computed_member_expression(
            SPAN,
            array.create_spanned_read_expression(at, ctx),
            index.create_spanned_read_expression(at, ctx),
            false,
            ctx,
        );
        let mut value = Some(Value::Element(element));
        let count = methods.len();
        for (position, (method, callback)) in methods.into_iter().zip(inlined).enumerate() {
            let last = position + 1 == count;
            let current = value.take().expect("each stage gets an element");
            let Some(callback) = callback else {
                // `includes`, always last.
                let read = Self::bind_fresh(current, &mut local, &mut body, body_scope, ctx);
                let search = search.as_ref().expect("includes has a search value");
                let test = same_value_zero(at, &read, search, plan.search_strict, ctx);
                let target = target.as_ref().expect("includes has a result");
                let yes = Expression::new_boolean_literal(SPAN, true, ctx);
                body.push(found(at, test, target, yes, body_scope, ctx));
                break;
            };
            let Inlined {
                params,
                mut statements,
                result,
            } = callback;
            let mut params = params.into_iter();
            let accumulator = if method == Method::Reduce {
                params.next()
            } else {
                None
            };
            // The element.
            let read = match params.next() {
                Some(BindingPattern::BindingIdentifier(id))
                    if !mutated(id.symbol_id(), ctx.scoping()) =>
                {
                    let binding = BoundIdentifier::new(id.name, id.symbol_id());
                    match current {
                        Value::Element(expression) | Value::Expression(expression) => {
                            body.push(declare_pattern(
                                BindingPattern::BindingIdentifier(id),
                                expression,
                                ctx,
                            ));
                        }
                        // The same element under the same name needs no new binding.
                        Value::Read(read) if read.name == binding.name => {}
                        Value::Read(read) => {
                            let init = read.create_spanned_read_expression(at, ctx);
                            body.push(declare_pattern(
                                BindingPattern::BindingIdentifier(id),
                                init,
                                ctx,
                            ));
                        }
                    }
                    binding
                }
                Some(pattern) => {
                    let needed = matches!(method, Method::Filter | Method::Find)
                        || (method == Method::Reduce && empty.is_some());
                    match current {
                        // Nothing else reads the element: destructure it directly.
                        Value::Element(expression) | Value::Expression(expression) if !needed => {
                            body.push(declare_pattern(pattern, expression, ctx));
                            index.clone()
                        }
                        current => {
                            let read =
                                Self::bind_fresh(current, &mut local, &mut body, body_scope, ctx);
                            let init = read.create_spanned_read_expression(at, ctx);
                            body.push(declare_pattern(pattern, init, ctx));
                            read
                        }
                    }
                }
                None => {
                    let needed = matches!(method, Method::Filter | Method::Find)
                        || (method == Method::Reduce && empty.is_some());
                    match current {
                        // Nothing reads the element: skip reading it.
                        Value::Element(_) if !needed => index.clone(),
                        current => {
                            Self::bind_fresh(current, &mut local, &mut body, body_scope, ctx)
                        }
                    }
                }
            };
            if let Some(empty) = &empty
                && method == Method::Reduce
            {
                // The first element seeds the accumulator, and the callback doesn't run.
                let target = target.as_ref().expect("reduce has a result");
                let seeded = block(
                    [
                        assign(
                            target.create_spanned_write_target(at, ctx),
                            read.create_spanned_read_expression(at, ctx),
                            ctx,
                        ),
                        assign(
                            empty.create_spanned_write_target(at, ctx),
                            Expression::new_boolean_literal(SPAN, false, ctx),
                            ctx,
                        ),
                        Statement::new_continue_statement(SPAN, None, ctx),
                    ],
                    body_scope,
                    ctx,
                );
                body.push(Statement::new_if_statement(
                    SPAN,
                    empty.create_spanned_read_expression(at, ctx),
                    seeded,
                    None,
                    ctx,
                ));
            }
            if let Some(accumulator) = accumulator {
                let target = target.as_ref().expect("reduce has a result");
                let is_target = matches!(
                    &accumulator,
                    BindingPattern::BindingIdentifier(id) if id.name == target.name
                );
                if !is_target {
                    let init = target.create_spanned_read_expression(at, ctx);
                    body.push(declare_pattern(accumulator, init, ctx));
                }
            }
            if let Some(pattern) = params.next() {
                let is_index = matches!(
                    (&pattern, &index_param),
                    (BindingPattern::BindingIdentifier(id), Some(index))
                        if id.symbol_id() == index.symbol_id
                );
                if !is_index {
                    let init = index.create_spanned_read_expression(at, ctx);
                    body.push(declare_pattern(pattern, init, ctx));
                }
            }
            if let Some(pattern) = params.next() {
                let init = array.create_spanned_read_expression(at, ctx);
                body.push(declare_pattern(pattern, init, ctx));
            }
            body.append(&mut statements);
            let target = target.as_ref();
            match method {
                Method::Map => {
                    let result = strip(result.expect("map returns"));
                    if last {
                        let target = target.expect("map has a result");
                        body.push(if plan.hole_map {
                            let element = Expression::new_computed_member_expression(
                                SPAN,
                                target.create_spanned_read_expression(at, ctx),
                                index.create_spanned_read_expression(at, ctx),
                                false,
                                ctx,
                            );
                            let element = oxc_ast::ast::AssignmentTarget::from(
                                element.into_member_expression(),
                            );
                            assign(element, result, ctx)
                        } else {
                            push(at, target, result, ctx)
                        });
                    } else {
                        value = Some(Value::Expression(result));
                    }
                }
                Method::Filter => {
                    let result = strip(result.expect("filter returns"));
                    body.push(Statement::new_if_statement(
                        SPAN,
                        not(result, ctx),
                        Statement::new_continue_statement(SPAN, None, ctx),
                        None,
                        ctx,
                    ));
                    if last {
                        let target = target.expect("filter has a result");
                        let element = read.create_spanned_read_expression(at, ctx);
                        body.push(push(at, target, element, ctx));
                    }
                    value = Some(Value::Read(read));
                }
                Method::ForEach => {
                    if let Some(result) = result {
                        body.push(Statement::new_expression_statement(
                            result.span(),
                            result,
                            ctx,
                        ));
                    }
                }
                Method::Reduce => {
                    let result = strip(result.expect("reduce returns"));
                    let target = target.expect("reduce has a result");
                    body.push(assign(
                        target.create_spanned_write_target(at, ctx),
                        result,
                        ctx,
                    ));
                }
                Method::Some | Method::Every | Method::Find | Method::FindIndex => {
                    let result = strip(result.expect("the callback returns"));
                    let target = target.expect("the chain has a result");
                    let (test, found_value) = match method {
                        Method::Some => (result, Expression::new_boolean_literal(SPAN, true, ctx)),
                        Method::Every => (
                            not(result, ctx),
                            Expression::new_boolean_literal(SPAN, false, ctx),
                        ),
                        Method::Find => (result, read.create_spanned_read_expression(at, ctx)),
                        _ => (result, index.create_spanned_read_expression(at, ctx)),
                    };
                    body.push(found(at, test, target, found_value, body_scope, ctx));
                }
                Method::Includes => unreachable!("has no callback"),
            }
        }
        // The loop.
        let init = ForStatementInit::new_variable_declaration(
            SPAN,
            VariableDeclarationKind::Let,
            ArenaVec::from_iter_in(
                [
                    VariableDeclarator::new(
                        SPAN,
                        index.create_binding_pattern(ctx),
                        None,
                        Some(zero(ctx)),
                        false,
                        ctx,
                    ),
                    VariableDeclarator::new(
                        SPAN,
                        length.create_binding_pattern(ctx),
                        None,
                        Some(length_of(&array, at, ctx)),
                        false,
                        ctx,
                    ),
                ],
                &ctx.ast,
            ),
            false,
            ctx,
        );
        let test = Expression::new_binary_expression(
            SPAN,
            index.create_spanned_read_expression(at, ctx),
            BinaryOperator::LessThan,
            length.create_spanned_read_expression(at, ctx),
            ctx,
        );
        let update = Expression::new_update_expression(
            SPAN,
            UpdateOperator::Increment,
            false,
            index.create_spanned_read_write_simple_target(at, ctx),
            ctx,
        );
        let body = Statement::new_block_statement_with_scope_id(
            SPAN,
            ArenaVec::from_iter_in(body, &ctx.ast),
            body_scope,
            ctx,
        );
        before.push(Statement::new_for_statement_with_scope_id(
            SPAN,
            Some(init),
            Some(test),
            Some(update),
            body,
            loop_scope,
            ctx,
        ));
        if let Some(empty) = &empty {
            let error = Expression::new_new_expression(
                SPAN,
                ctx.create_unbound_ident_expr(SPAN, "TypeError".into(), ReferenceFlags::Read),
                None,
                ArenaVec::from_iter_in(
                    [Argument::from(Expression::new_string_literal(
                        SPAN,
                        oxc_ast::ast::Str::from_str_in(
                            "Reduce of empty array with no initial value",
                            ctx,
                        ),
                        None,
                        ctx,
                    ))],
                    &ctx.ast,
                ),
                ctx,
            );
            before.push(Statement::new_if_statement(
                SPAN,
                empty.create_spanned_read_expression(at, ctx),
                Statement::new_throw_statement(SPAN, error, ctx),
                None,
                ctx,
            ));
        }
        if let Some((kind, mut declarator)) = declared {
            let target = target.as_ref().expect("a declared chain has a result");
            declarator.init = Some(target.create_spanned_read_expression(at, ctx));
            before.push(Statement::new_variable_declaration(
                SPAN,
                kind,
                ArenaVec::from_iter_in([declarator], &ctx.ast),
                false,
                ctx,
            ));
        }
        if form == Form::Return {
            let target = target.as_ref().expect("a returned chain has a result");
            let value = target.create_spanned_read_expression(at, ctx);
            before.push(Statement::new_return_statement(SPAN, Some(value), ctx));
        }
        // The statement's leading comments stay above what now comes first.
        *before[0].span_mut() = Span::empty(at.start);
        before
    }

    /// A new binding in the enclosing statement list, with a name nothing in the enclosing
    /// function uses.
    fn declare(
        &mut self,
        base: &str,
        at: Span,
        scope: ScopeId,
        ctx: &mut TraverseCtx<'a, ()>,
    ) -> BoundIdentifier<'a> {
        let name = self.names.fresh(base, at);
        let name = oxc_ast::ast::Ident::from_str_in(&name, ctx);
        ctx.generate_binding(name, scope, SymbolFlags::BlockScopedVariable)
    }

    /// `value` held by a binding: itself if it is one, else a new `const item = …`.
    fn bind_fresh(
        value: Value<'a>,
        local: &mut NamesIn,
        body: &mut Vec<Statement<'a>>,
        scope: ScopeId,
        ctx: &mut TraverseCtx<'a, ()>,
    ) -> BoundIdentifier<'a> {
        match value {
            Value::Read(read) => read,
            Value::Element(expression) | Value::Expression(expression) => {
                let binding = super::for_of::generate(local, "item", scope, ctx);
                body.push(const_declaration(&binding, expression, ctx));
                binding
            }
        }
    }
}

/// The bindings a callback declares at its top level (parameters and the body's own
/// declarations), which inlining moves into the loop body.
fn moved_bindings(callback: &Argument<'_>) -> Vec<(SymbolId, String)> {
    let (params, statements): (&oxc_ast::ast::FormalParameters<'_>, &[Statement<'_>]) =
        match callback {
            Argument::ArrowFunctionExpression(arrow) => (
                &arrow.params,
                arrow
                    .get_function_body()
                    .map_or(&[][..], |body| body.statements.as_slice()),
            ),
            Argument::FunctionExpression(function) => (
                &function.params,
                function
                    .body
                    .as_ref()
                    .map_or(&[][..], |body| body.statements.as_slice()),
            ),
            _ => return Vec::new(),
        };
    let mut found = Vec::new();
    let mut add = |pattern: &BindingPattern<'_>| {
        pattern.all_binding_identifiers(&mut |ident| {
            found.push((ident.symbol_id(), ident.name.to_string()));
            true
        });
    };
    for parameter in &params.items {
        add(&parameter.pattern);
    }
    for statement in statements {
        if let Statement::VariableDeclaration(declaration) = statement {
            for declarator in &declaration.declarations {
                add(&declarator.id);
            }
        }
    }
    found
}

/// The parameter at `position` of a callback, if it is a plain identifier nothing assigns.
fn param_binding(
    callback: &Argument<'_>,
    position: usize,
    scoping: &Scoping,
) -> Option<(SymbolId, String)> {
    let params = match callback {
        Argument::ArrowFunctionExpression(arrow) => &arrow.params,
        Argument::FunctionExpression(function) => &function.params,
        _ => return None,
    };
    match &params.items.get(position)?.pattern {
        BindingPattern::BindingIdentifier(id) if !mutated(id.symbol_id(), scoping) => {
            Some((id.symbol_id(), id.name.to_string()))
        }
        _ => None,
    }
}

/// Whether a function nested in `callback` refers to `symbol`.
fn captured(symbol: SymbolId, callback: &Argument<'_>, scoping: &Scoping) -> bool {
    struct Find<'s> {
        scoping: &'s Scoping,
        symbol: SymbolId,
        depth: usize,
        found: bool,
    }
    impl<'a> Visit<'a> for Find<'_> {
        fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
            if self.depth > 1
                && ident.reference_id.get().is_some_and(|reference| {
                    self.scoping.get_reference(reference).symbol_id() == Some(self.symbol)
                })
            {
                self.found = true;
            }
        }

        fn visit_function(&mut self, function: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
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
    }
    let mut find = Find {
        scoping,
        symbol,
        depth: 0,
        found: false,
    };
    find.visit_argument(callback);
    find.found
}

/// The names a callback refers to that it doesn't declare (globals, and bindings outside it).
fn free_names(callback: &Argument<'_>, scoping: &Scoping) -> FxHashSet<String> {
    struct Find<'s> {
        scoping: &'s Scoping,
        within: Span,
        found: FxHashSet<String>,
    }
    impl<'a> Visit<'a> for Find<'_> {
        fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
            let inside = ident
                .reference_id
                .get()
                .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
                .is_some_and(|symbol| {
                    let span = self.scoping.symbol_span(symbol);
                    self.within.start <= span.start && span.end <= self.within.end
                });
            if !inside {
                self.found.insert(ident.name.to_string());
            }
        }
    }
    let mut find = Find {
        scoping,
        within: callback.span(),
        found: FxHashSet::default(),
    };
    find.visit_argument(callback);
    find.found
}

/// Renames bindings (and their references) in a callback.
struct Rename<'s, 'r, 'a> {
    scoping: &'s Scoping,
    renames: &'r [(SymbolId, oxc_ast::ast::Ident<'a>)],
}

impl<'a> VisitMut<'a> for Rename<'_, '_, 'a> {
    fn visit_binding_identifier(&mut self, ident: &mut oxc_ast::ast::BindingIdentifier<'a>) {
        if let Some(symbol) = ident.symbol_id.get()
            && let Some((_, name)) = self.renames.iter().find(|(s, _)| *s == symbol)
        {
            ident.name = *name;
        }
    }

    fn visit_identifier_reference(&mut self, ident: &mut IdentifierReference<'a>) {
        if let Some(reference) = ident.reference_id.get()
            && let Some(symbol) = self.scoping.get_reference(reference).symbol_id()
            && let Some((_, name)) = self.renames.iter().find(|(s, _)| *s == symbol)
        {
            ident.name = *name;
        }
    }
}

/// Takes a checked callback apart.
fn take_callback<'a>(callback: Argument<'a>, ctx: &mut TraverseCtx<'a, ()>) -> Inlined<'a> {
    let (params, body) = match callback {
        Argument::ArrowFunctionExpression(arrow) => {
            let arrow = arrow.unbox();
            match arrow.body {
                ArrowFunctionBody::FunctionBody(body) => (arrow.params, Ok(body)),
                other => (arrow.params, Err(other.into_expression())),
            }
        }
        Argument::FunctionExpression(function) => {
            let mut function = function.unbox();
            let body = function.body.take().expect("checked");
            (function.params, Ok(body))
        }
        _ => unreachable!("checked"),
    };
    let _ = ctx;
    let params = params
        .unbox()
        .items
        .into_iter()
        .map(|parameter| parameter.pattern)
        .collect();
    let body = match body {
        Ok(body) => body,
        Err(expression) => {
            return Inlined {
                params,
                statements: Vec::new(),
                result: Some(expression),
            };
        }
    };
    let mut statements: Vec<Statement<'a>> = body.unbox().statements.into_iter().collect();
    let result = if matches!(statements.last(), Some(Statement::ReturnStatement(_))) {
        match statements.pop() {
            Some(Statement::ReturnStatement(statement)) => statement.unbox().argument,
            _ => unreachable!("checked"),
        }
    } else {
        None
    };
    Inlined {
        params,
        statements,
        result,
    }
}

/// Whether `symbol` is ever assigned (a `const` never is).
fn mutated(symbol: SymbolId, scoping: &Scoping) -> bool {
    scoping
        .get_resolved_references(symbol)
        .any(oxc_semantic::Reference::is_write)
}

/// `expression` without the parentheses around it, unless they hold a sequence.
fn strip(expression: Expression<'_>) -> Expression<'_> {
    match expression {
        Expression::ParenthesizedExpression(paren)
            if !matches!(paren.expression, Expression::SequenceExpression(_)) =>
        {
            strip(paren.unbox().expression)
        }
        other => other,
    }
}

/// Whether `expression` is a literal that can be copied.
fn is_literal(expression: &Expression<'_>) -> bool {
    matches!(
        expression,
        Expression::NumericLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
    )
}

/// `!expression`, parenthesised unless it binds tighter than `!`.
fn not<'a>(expression: Expression<'a>, ctx: &TraverseCtx<'a, ()>) -> Expression<'a> {
    let operand = match expression {
        Expression::Identifier(_)
        | Expression::CallExpression(_)
        | Expression::StaticMemberExpression(_)
        | Expression::ComputedMemberExpression(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::ThisExpression(_)
        | Expression::ParenthesizedExpression(_)
        | Expression::UnaryExpression(_) => expression,
        other => Expression::new_parenthesized_expression(SPAN, other, ctx),
    };
    Expression::new_unary_expression(SPAN, UnaryOperator::LogicalNot, operand, ctx)
}

/// `value === search`, or `SameValueZero` spelled out when both may be NaN.
fn same_value_zero<'a>(
    at: Span,
    value: &BoundIdentifier<'a>,
    search: &Expression<'a>,
    strict: bool,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Expression<'a> {
    let equal = Expression::new_binary_expression(
        SPAN,
        value.create_spanned_read_expression(at, ctx),
        BinaryOperator::StrictEquality,
        search.clone_in(oxc_allocator::GetAllocator::allocator(&*ctx)),
        ctx,
    );
    if strict {
        return equal;
    }
    let value_nan = Expression::new_binary_expression(
        SPAN,
        value.create_spanned_read_expression(at, ctx),
        BinaryOperator::StrictInequality,
        value.create_spanned_read_expression(at, ctx),
        ctx,
    );
    let search_nan = Expression::new_binary_expression(
        SPAN,
        search.clone_in(oxc_allocator::GetAllocator::allocator(&*ctx)),
        BinaryOperator::StrictInequality,
        search.clone_in(oxc_allocator::GetAllocator::allocator(&*ctx)),
        ctx,
    );
    let both =
        Expression::new_logical_expression(SPAN, value_nan, LogicalOperator::And, search_nan, ctx);
    Expression::new_logical_expression(
        SPAN,
        equal,
        LogicalOperator::Or,
        Expression::new_parenthesized_expression(SPAN, both, ctx),
        ctx,
    )
}

/// `if (test) { target = value; break; }`
fn found<'a>(
    at: Span,
    test: Expression<'a>,
    target: &BoundIdentifier<'a>,
    value: Expression<'a>,
    scope: ScopeId,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Statement<'a> {
    let set = assign(target.create_spanned_write_target(at, ctx), value, ctx);
    let consequent = block(
        [set, Statement::new_break_statement(SPAN, None, ctx)],
        scope,
        ctx,
    );
    Statement::new_if_statement(SPAN, test, consequent, None, ctx)
}

/// A block of `statements` in a new scope under `parent`.
fn block<'a, const N: usize>(
    statements: [Statement<'a>; N],
    parent: ScopeId,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Statement<'a> {
    let scope = ctx.create_child_scope(parent, ScopeFlags::empty());
    Statement::new_block_statement_with_scope_id(
        SPAN,
        ArenaVec::from_iter_in(statements, &ctx.ast),
        scope,
        ctx,
    )
}

/// `target = value;`
fn assign<'a>(
    target: oxc_ast::ast::AssignmentTarget<'a>,
    value: Expression<'a>,
    ctx: &TraverseCtx<'a, ()>,
) -> Statement<'a> {
    let assignment =
        Expression::new_assignment_expression(SPAN, AssignmentOperator::Assign, target, value, ctx);
    Statement::new_expression_statement(SPAN, assignment, ctx)
}

/// `target.push(value);`
fn push<'a>(
    at: Span,
    target: &BoundIdentifier<'a>,
    value: Expression<'a>,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Statement<'a> {
    let callee = Expression::new_static_member_expression(
        SPAN,
        target.create_spanned_read_expression(at, ctx),
        IdentifierName::new(SPAN, "push", ctx),
        false,
        ctx,
    );
    let call = Expression::new_call_expression(
        SPAN,
        callee,
        None,
        ArenaVec::from_iter_in([Argument::from(value)], &ctx.ast),
        false,
        ctx,
    );
    Statement::new_expression_statement(SPAN, call, ctx)
}

/// `const binding = init;`
fn const_declaration<'a>(
    binding: &BoundIdentifier<'a>,
    init: Expression<'a>,
    ctx: &mut TraverseCtx<'a, ()>,
) -> Statement<'a> {
    let pattern = binding.create_binding_pattern(ctx);
    declare_with(VariableDeclarationKind::Const, pattern, init, ctx)
}

/// `const pattern = init;`, or `let` if the pattern binds something that is reassigned.
fn declare_pattern<'a>(
    pattern: BindingPattern<'a>,
    init: Expression<'a>,
    ctx: &TraverseCtx<'a, ()>,
) -> Statement<'a> {
    let mut reassigned = false;
    pattern.all_binding_identifiers(&mut |ident| {
        reassigned |= mutated(ident.symbol_id(), ctx.scoping());
        true
    });
    let kind = if reassigned {
        VariableDeclarationKind::Let
    } else {
        VariableDeclarationKind::Const
    };
    declare_with(kind, pattern, init, ctx)
}

fn declare_with<'a>(
    kind: VariableDeclarationKind,
    pattern: BindingPattern<'a>,
    init: Expression<'a>,
    ctx: &TraverseCtx<'a, ()>,
) -> Statement<'a> {
    Statement::new_variable_declaration(
        SPAN,
        kind,
        ArenaVec::from_iter_in(
            [VariableDeclarator::new(
                SPAN,
                pattern,
                None,
                Some(init),
                false,
                ctx,
            )],
            &ctx.ast,
        ),
        false,
        ctx,
    )
}

/// A numeric literal.
fn number<'a>(value: f64, ctx: &TraverseCtx<'a, ()>) -> Expression<'a> {
    Expression::new_numeric_literal(SPAN, value, None, oxc_ast::ast::NumberBase::Decimal, ctx)
}
