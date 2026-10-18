//! PLAN §4 pass 9: loop-invariant code motion.
//!
//! Two kinds of expression move out of a loop into a `const` (or, for a `for` loop that
//! declares its counter, a declarator after the counter's: `for (let i = 0, itemsLength =
//! items.length; …)`), named after what they compute:
//!
//! - **Invariants** the analysis lists ([`LoopFacts::invariant`]): pure, deterministic,
//!   unconditionally executed, reading nothing the loop writes. They are safe to evaluate once
//!   the loop's first test has passed, so they may move before it only where that test can't
//!   matter: an invariant of a `for`/`while` test is evaluated by the first test anyway; one in
//!   the body (or a `do…while` test) moves only if it reads no binding the test reads (the test
//!   can only narrow those) and calls no program function (whose termination isn't proven, so
//!   a zero-trip loop mustn't start calling it).
//! - **Array lengths**: `a.length` where `a` is a binding nothing reassigns, every value it
//!   holds is a non-nullish array, the read is usable-pure, and the loop resizes none of the
//!   arrays it may be ([`LoopFacts::resized`], complete only when `resizes_untracked` is
//!   false). The analysis leaves these out of `invariant` when the loop writes any object.
//!
//! A `for…of`/`for…in` loop's head runs before its body, so hoisting before it needs a pure
//! head. Loops in island functions, in code proven never to run and in TypeScript namespaces
//! are left alone.
//!
//! [`LoopFacts::invariant`]: trioxide_analysis::LoopFacts::invariant
//! [`LoopFacts::resized`]: trioxide_analysis::LoopFacts::resized

use oxc_allocator::{ArenaVec, TakeIn};
use oxc_ast::ast::{
    Expression, ForStatementInit, IdentifierReference, Program, Statement, VariableDeclarationKind,
    VariableDeclarator,
};
use oxc_ast_visit::{Visit, VisitMut, walk, walk_mut};
use oxc_semantic::Scoping;
use oxc_span::{ContentEq, GetSpan, GetSpanMut, SPAN, Span};
use oxc_syntax::{
    scope::{ScopeFlags, ScopeId},
    symbol::{SymbolFlags, SymbolId},
};
use oxc_traverse::{BoundIdentifier, Traverse, TraverseCtx};
use rustc_hash::FxHashSet;
use trioxide_analysis::{AllocKind, LoopFacts};
use trioxide_common::{NodeId, SymbolRef};
use trioxide_types::Type;

use super::{
    Decisions, Position,
    arrays::{FunctionNames, Kept, NamesIn, capitalized},
    track_position,
    util::{Query, is_canonical_literal},
};
use crate::{Pass, PassContext};

/// PLAN §4 pass 9 (`"licm"`).
pub struct Licm;

impl Pass for Licm {
    fn name(&self) -> &'static str {
        "licm"
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
        visitor.decisions.flush(ctx.log)
    }
}

struct Visitor<'f> {
    query: Query<'f>,
    kept: Kept,
    names: FunctionNames,
    position: Position,
    decisions: Decisions,
}

/// Where in a loop an expression is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    /// A `for`/`while` test: evaluated by the first test whenever the loop is reached.
    Test,
    /// The body (or a `do…while` test).
    Body,
}

impl<'a> Traverse<'a, ()> for Visitor<'_> {
    track_position!();

    fn enter_statements(
        &mut self,
        statements: &mut ArenaVec<'a, Statement<'a>>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if !self.position.live() {
            return;
        }
        let mut index = 0;
        while index < statements.len() {
            let before = self.hoist(&mut statements[index], ctx);
            let count = before.len();
            for (offset, statement) in before.into_iter().enumerate() {
                statements.insert(index + offset, statement);
            }
            index += count + 1;
        }
    }
}

impl<'a> Visitor<'_> {
    /// Hoists the invariants of the loop `statement` is (through labels); returns the
    /// statements to put before it.
    fn hoist(
        &mut self,
        statement: &mut Statement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) -> Vec<Statement<'a>> {
        let start = statement.span().start;
        let mut target = &mut *statement;
        while let Statement::LabeledStatement(labeled) = target {
            target = &mut labeled.body;
        }
        let (node, span) = match &*target {
            Statement::ForStatement(s) => (s.node_id.get(), s.span),
            Statement::WhileStatement(s) => (s.node_id.get(), s.span),
            Statement::DoWhileStatement(s) => (s.node_id.get(), s.span),
            Statement::ForOfStatement(s) => (s.node_id.get(), s.span),
            Statement::ForInStatement(s) => (s.node_id.get(), s.span),
            _ => return Vec::new(),
        };
        if self.query.dead(node) || self.kept.contains(span) {
            return Vec::new();
        }
        let Some(facts) = self
            .query
            .node(node)
            .and_then(|n| self.query.facts.loop_facts(n))
        else {
            return Vec::new();
        };
        let accepted = self.candidates(target, facts, ctx.scoping());
        if accepted.is_empty() {
            return Vec::new();
        }
        // Where the declarations go: the `for` head when it declares with `let`/`var`, else
        // before the loop.
        let head = match &*target {
            Statement::ForStatement(s) => match &s.init {
                Some(ForStatementInit::VariableDeclaration(declaration))
                    if declaration.kind != VariableDeclarationKind::Const =>
                {
                    Some(declaration.kind)
                }
                _ => None,
            },
            _ => None,
        };
        let (scope, kind) = match (&*target, head) {
            (Statement::ForStatement(s), Some(kind)) => (
                s.scope_id.get().unwrap_or_else(|| ctx.current_scope_id()),
                kind,
            ),
            _ => (ctx.current_block_scope_id(), VariableDeclarationKind::Const),
        };
        // Names in the loop's head only need to be fresh within the loop.
        let mut local = NamesIn::default();
        local.visit_statement(target);
        let names = if head.is_some() {
            Fresh::Local(&mut local)
        } else {
            Fresh::Function(&mut self.names, span)
        };
        let mut replace = Replace {
            ctx,
            accepted: &accepted,
            names,
            scope,
            kind,
            hoisted: Vec::new(),
        };
        match &mut *target {
            Statement::ForStatement(s) => {
                if let Some(test) = &mut s.test {
                    replace.visit_expression(test);
                }
                replace.visit_statement(&mut s.body);
            }
            Statement::WhileStatement(s) => {
                replace.visit_expression(&mut s.test);
                replace.visit_statement(&mut s.body);
            }
            Statement::DoWhileStatement(s) => {
                replace.visit_statement(&mut s.body);
                replace.visit_expression(&mut s.test);
            }
            Statement::ForOfStatement(s) => replace.visit_statement(&mut s.body),
            Statement::ForInStatement(s) => replace.visit_statement(&mut s.body),
            _ => unreachable!("matched above"),
        }
        let Replace { hoisted, ctx, .. } = replace;
        for (span, _) in &hoisted {
            self.decisions.applied(*span);
        }
        let declarators = hoisted.into_iter().map(|(_, (expression, binding))| {
            VariableDeclarator::new(
                SPAN,
                binding.create_binding_pattern(ctx),
                None,
                Some(expression),
                false,
                ctx,
            )
        });
        if head.is_some() {
            let Statement::ForStatement(s) = target else {
                unreachable!("a head is a `for` loop's")
            };
            let Some(ForStatementInit::VariableDeclaration(declaration)) = &mut s.init else {
                unreachable!("checked above")
            };
            declaration.declarations.extend(declarators);
            return Vec::new();
        }
        let mut before = Vec::new();
        // An expression initialiser runs first, as it did.
        if let Statement::ForStatement(s) = target
            && let Some(init) = s.init.take()
        {
            let expression = init.into_expression();
            before.push(Statement::new_expression_statement(
                expression.span(),
                expression,
                ctx,
            ));
        }
        before.push(Statement::new_variable_declaration(
            SPAN,
            VariableDeclarationKind::Const,
            ArenaVec::from_iter_in(declarators, &ctx.ast),
            false,
            ctx,
        ));
        // The loop's leading comments stay above what now comes first.
        *before[0].span_mut() = Span::empty(start);
        before
    }

    /// The expressions of `target` to hoist.
    fn candidates(
        &mut self,
        target: &Statement<'a>,
        facts: &LoopFacts,
        scoping: &Scoping,
    ) -> FxHashSet<NodeId> {
        let invariant: FxHashSet<NodeId> = facts
            .invariant
            .iter()
            .filter(|node| node.module == self.query.module)
            .map(|node| node.node)
            .collect();
        let mut collect = Collect {
            query: self.query,
            invariant: &invariant,
            scoping,
            found: Vec::new(),
            part: Part::Body,
        };
        let mut test_reads = FxHashSet::default();
        let mut pure_head = true;
        match target {
            Statement::ForStatement(s) => {
                if let Some(test) = &s.test {
                    test_reads = reads(test, scoping);
                    collect.part = Part::Test;
                    collect.visit_expression(test);
                    collect.part = Part::Body;
                }
                collect.visit_statement(&s.body);
            }
            Statement::WhileStatement(s) => {
                test_reads = reads(&s.test, scoping);
                collect.part = Part::Test;
                collect.visit_expression(&s.test);
                collect.part = Part::Body;
                collect.visit_statement(&s.body);
            }
            Statement::DoWhileStatement(s) => {
                test_reads = reads(&s.test, scoping);
                collect.visit_statement(&s.body);
                collect.visit_expression(&s.test);
            }
            Statement::ForOfStatement(s) => {
                pure_head = self.query.pure(s.right.node_id());
                collect.visit_statement(&s.body);
            }
            Statement::ForInStatement(s) => {
                pure_head = self.query.pure(s.right.node_id());
                collect.visit_statement(&s.body);
            }
            _ => {}
        }
        let found = collect.found;
        let mut accepted = FxHashSet::default();
        for candidate in found {
            if candidate.trivial {
                continue;
            }
            if !pure_head {
                self.decisions.skipped(
                    candidate.span,
                    "the loop's head may have effects, and runs before anything put before the \
                     loop",
                );
                continue;
            }
            match candidate.length {
                Some((symbol, object)) => {
                    if let Err(reason) =
                        self.invariant_length(candidate.node, symbol, object, facts)
                    {
                        if let Some(reason) = reason {
                            self.decisions.skipped(candidate.span, reason);
                        }
                        continue;
                    }
                }
                None => {
                    if candidate.part == Part::Body {
                        if candidate.calls_program {
                            self.decisions.skipped(
                                candidate.span,
                                "it may call a program function, which a loop that runs zero \
                                 times mustn't start calling",
                            );
                            continue;
                        }
                        if candidate.reads.iter().any(|read| test_reads.contains(read)) {
                            self.decisions.skipped(
                                candidate.span,
                                "it reads a binding the loop's test reads (and may narrow)",
                            );
                            continue;
                        }
                    }
                }
            }
            accepted.insert(candidate.node);
        }
        accepted
    }

    /// Whether `.length` at `node` of the binding `symbol` is the same on every iteration.
    /// `Err(None)` declines quietly (not an array length at all).
    fn invariant_length(
        &self,
        node: NodeId,
        symbol: SymbolId,
        object: NodeId,
        facts: &LoopFacts,
    ) -> Result<(), Option<&'static str>> {
        let fact = self.query.facts.symbol_ty(SymbolRef {
            module: self.query.module,
            symbol,
        });
        let arrays = !matches!(fact.value, Type::Unknown | Type::Never)
            && fact
                .value
                .members()
                .iter()
                .all(|member| matches!(member, Type::Array(_) | Type::Tuple(_)));
        if !arrays {
            return Err(None);
        }
        if !self.query.usable(fact.trust) {
            return Err(Some(
                "the binding's array type is only Declared (trustTypes is off)",
            ));
        }
        if !self.query.pure(node) {
            return Err(Some("the length read may throw"));
        }
        if facts.resizes_untracked {
            return Err(Some(
                "the loop may resize arrays in ways the analysis doesn't follow",
            ));
        }
        let sites = self
            .query
            .node(object)
            .and_then(|n| self.query.facts.points_to(n).value);
        match sites {
            Some(sites) => {
                if sites.is_empty()
                    || !sites.iter().all(|site| {
                        self.query
                            .facts
                            .allocation(*site)
                            .is_some_and(|allocation| allocation.kind == AllocKind::Array)
                    })
                {
                    return Err(Some("the arrays the binding may hold are unknown"));
                }
                if sites.iter().any(|site| facts.resized.contains(site)) {
                    return Err(Some("the loop may resize the array"));
                }
            }
            // An array the analysis doesn't follow (a `Declared` parameter) may be any of
            // them: only a loop that resizes none will do.
            None if !facts.resized.is_empty() => {
                return Err(Some(
                    "the loop resizes arrays, and the binding may hold any of them",
                ));
            }
            None => {}
        }
        Ok(())
    }
}

/// A hoisting candidate.
struct Candidate {
    node: NodeId,
    span: Span,
    part: Part,
    /// For `a.length`: the binding `a` and the node reading it.
    length: Option<(SymbolId, NodeId)>,
    /// The bindings it reads.
    reads: FxHashSet<SymbolId>,
    /// It calls (or runs an accessor of) a program function.
    calls_program: bool,
    /// A literal or something with no binding or property read in it: not worth a name.
    trivial: bool,
}

/// Finds the candidates in a loop, outside nested functions and classes.
struct Collect<'s, 'i, 'f> {
    query: Query<'f>,
    invariant: &'i FxHashSet<NodeId>,
    scoping: &'s Scoping,
    found: Vec<Candidate>,
    part: Part,
}

impl<'a> Visit<'a> for Collect<'_, '_, '_> {
    fn visit_expression(&mut self, expression: &Expression<'a>) {
        let node = expression.node_id();
        if self.invariant.contains(&node) {
            let reads = reads(expression, self.scoping);
            // Arithmetic over locals is cheap and engines hoist it themselves; naming it
            // costs readability for nothing.
            let trivial = is_canonical_literal(expression) || !loads(expression);
            self.found.push(Candidate {
                node,
                span: expression.span(),
                part: self.part,
                length: None,
                reads,
                calls_program: calls_program(expression, self.query),
                trivial,
            });
            return;
        }
        if let Expression::StaticMemberExpression(member) = expression
            && member.property.name == "length"
            && !member.optional
            && let Expression::Identifier(object) = &member.object
            && let Some(symbol) = object
                .reference_id
                .get()
                .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
            && !self.scoping.symbol_is_mutated(symbol)
        {
            self.found.push(Candidate {
                node,
                span: expression.span(),
                part: self.part,
                length: Some((symbol, object.node_id.get())),
                reads: FxHashSet::from_iter([symbol]),
                calls_program: false,
                trivial: false,
            });
            return;
        }
        walk::walk_expression(self, expression);
    }

    fn visit_function(&mut self, _: &oxc_ast::ast::Function<'a>, _: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _: &oxc_ast::ast::ArrowFunctionExpression<'a>) {}

    fn visit_class(&mut self, _: &oxc_ast::ast::Class<'a>) {}
}

/// Whether evaluating `expression` may run a program function: a call, or an accessor at a
/// property read, that the call graph resolves to a unit.
fn calls_program(expression: &Expression<'_>, query: Query<'_>) -> bool {
    struct Find<'f> {
        query: Query<'f>,
        found: bool,
    }
    impl<'a> Visit<'a> for Find<'_> {
        fn visit_expression(&mut self, expression: &Expression<'a>) {
            if let Some(site) = self.query.node(expression.node_id())
                && self.query.facts.call_graph().callees(site).next().is_some()
            {
                self.found = true;
            }
            walk::walk_expression(self, expression);
        }
    }
    let mut find = Find {
        query,
        found: false,
    };
    find.visit_expression(expression);
    find.found
}

/// The bindings `expression` reads.
fn reads(expression: &Expression<'_>, scoping: &Scoping) -> FxHashSet<SymbolId> {
    struct Reads<'s> {
        scoping: &'s Scoping,
        found: FxHashSet<SymbolId>,
    }
    impl<'a> Visit<'a> for Reads<'_> {
        fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
            if let Some(symbol) = ident
                .reference_id
                .get()
                .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
            {
                self.found.insert(symbol);
            }
        }
    }
    let mut reads = Reads {
        scoping,
        found: FxHashSet::default(),
    };
    reads.visit_expression(expression);
    reads.found
}

/// Whether `expression` reads a property or calls something: work worth doing once.
fn loads(expression: &Expression<'_>) -> bool {
    struct Find(bool);
    impl<'a> Visit<'a> for Find {
        fn visit_expression(&mut self, expression: &Expression<'a>) {
            if matches!(
                expression,
                Expression::StaticMemberExpression(_)
                    | Expression::ComputedMemberExpression(_)
                    | Expression::PrivateFieldExpression(_)
                    | Expression::CallExpression(_)
            ) {
                self.0 = true;
                return;
            }
            walk::walk_expression(self, expression);
        }
    }
    let mut find = Find(false);
    find.visit_expression(expression);
    find.0
}

/// The name of the first property read or call in `expression`.
fn first_load(expression: &Expression<'_>) -> Option<String> {
    struct Find(Option<String>);
    impl<'a> Visit<'a> for Find {
        fn visit_expression(&mut self, expression: &Expression<'a>) {
            if self.0.is_some() {
                return;
            }
            match expression {
                Expression::StaticMemberExpression(_) | Expression::CallExpression(_) => {
                    self.0 = Some(name_for(expression));
                }
                _ => walk::walk_expression(self, expression),
            }
        }
    }
    let mut find = Find(None);
    find.visit_expression(expression);
    find.0
}

#[expect(clippy::float_cmp, reason = "an exact literal `1`")]
/// A readable name for the value of `expression`: `itemsLength` for `items.length`,
/// `sqrtScale` for `Math.sqrt(scale)`, `width` for `this.width`.
fn name_for(expression: &Expression<'_>) -> String {
    match expression.without_parentheses() {
        Expression::StaticMemberExpression(member) => match member.object.without_parentheses() {
            Expression::Identifier(object) if object.name != "Math" => {
                format!("{}{}", object.name, capitalized(&member.property.name))
            }
            _ => member.property.name.to_string(),
        },
        Expression::CallExpression(call) => match call.callee.without_parentheses() {
            Expression::StaticMemberExpression(member) => {
                let argument = call
                    .arguments
                    .first()
                    .and_then(|argument| argument.as_expression())
                    .map(|argument| match argument.without_parentheses() {
                        Expression::Identifier(ident) => capitalized(&ident.name),
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                format!("{}{argument}", member.property.name)
            }
            Expression::Identifier(callee) => format!("{}Result", callee.name),
            _ => "invariant".to_string(),
        },
        Expression::BinaryExpression(binary)
            if binary.operator == oxc_syntax::operator::BinaryOperator::Subtraction
                && matches!(&binary.right, Expression::NumericLiteral(one) if one.value == 1.0)
                && matches!(
                    binary.left.without_parentheses(),
                    Expression::StaticMemberExpression(member) if member.property.name == "length"
                ) =>
        {
            "lastIndex".to_string()
        }
        other => first_load(other).map_or_else(
            || "invariant".to_string(),
            |name| format!("hoisted{}", capitalized(&name)),
        ),
    }
}

/// Where hoisted names must be fresh: in the loop (for its head), or in the enclosing
/// function (for a declaration before the loop at `Span`).
enum Fresh<'n> {
    Local(&'n mut NamesIn),
    Function(&'n mut FunctionNames, Span),
}

impl Fresh<'_> {
    fn fresh(&mut self, base: &str) -> String {
        match self {
            Self::Local(names) => names.fresh(base),
            Self::Function(names, at) => names.fresh(base, *at),
        }
    }
}

/// Moves the accepted expressions out, leaving a read of their binding.
struct Replace<'c, 'a, 'n> {
    ctx: &'c mut TraverseCtx<'a, ()>,
    accepted: &'n FxHashSet<NodeId>,
    names: Fresh<'n>,
    scope: ScopeId,
    kind: VariableDeclarationKind,
    /// The first span of each hoisted value, the value and its binding.
    hoisted: Vec<(Span, (Expression<'a>, BoundIdentifier<'a>))>,
}

impl<'a> VisitMut<'a> for Replace<'_, 'a, '_> {
    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        if !self.accepted.contains(&expression.node_id()) {
            walk_mut::walk_expression(self, expression);
            return;
        }
        let span = expression.span();
        let value = expression.take_in(&self.ctx.ast);
        let existing = self
            .hoisted
            .iter()
            .find(|(_, (hoisted, _))| hoisted.content_eq(&value))
            .map(|(_, (_, binding))| binding.clone());
        let binding = if let Some(binding) = existing {
            binding
        } else {
            let name = self.names.fresh(&name_for(&value));
            let flags = match self.kind {
                VariableDeclarationKind::Var => SymbolFlags::FunctionScopedVariable,
                VariableDeclarationKind::Const => {
                    SymbolFlags::BlockScopedVariable | SymbolFlags::ConstVariable
                }
                _ => SymbolFlags::BlockScopedVariable,
            };
            let name = oxc_ast::ast::Ident::from_str_in(&name, self.ctx);
            let binding = self.ctx.generate_binding(name, self.scope, flags);
            self.hoisted.push((span, (value, binding.clone())));
            binding
        };
        *expression = binding.create_spanned_read_expression(span, self.ctx);
    }

    fn visit_function(&mut self, _: &mut oxc_ast::ast::Function<'a>, _: ScopeFlags) {}

    fn visit_arrow_function_expression(
        &mut self,
        _: &mut oxc_ast::ast::ArrowFunctionExpression<'a>,
    ) {
    }

    fn visit_class(&mut self, _: &mut oxc_ast::ast::Class<'a>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let allocator = oxc_allocator::Allocator::default();
        let parse = |source: &'static str| {
            let program = oxc_parser::Parser::new(&allocator, source, oxc_span::SourceType::ts())
                .parse()
                .program;
            let Statement::ExpressionStatement(statement) =
                program.body.into_iter().next().unwrap()
            else {
                unreachable!()
            };
            name_for(&statement.unbox().expression)
        };
        assert_eq!(parse("items.length"), "itemsLength");
        assert_eq!(parse("this.width"), "width");
        assert_eq!(parse("Math.sqrt(scale)"), "sqrtScale");
        assert_eq!(parse("a * b"), "invariant");
        assert_eq!(parse("items.length - 1"), "lastIndex");
        assert_eq!(parse("this.width * 2"), "hoistedWidth");
    }
}
