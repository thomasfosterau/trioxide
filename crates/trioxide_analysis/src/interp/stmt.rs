//! Statements, declarations, parameters and control flow.

use oxc_ast::ast::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression, ForStatementInit,
    ForStatementLeft, FormalParameters, Statement, VariableDeclaration, VariableDeclarationKind,
};
use oxc_syntax::node::GetNodeId;
use rustc_hash::FxHashSet;
use trioxide_types::Type;

use super::{
    Cx, EffectKind, Env, Escape, LOOP_WIDEN_AFTER, MAX_LOOP_ITERATIONS, ObjKind,
    expr::{Key, without_undefined},
};
use crate::{
    facts::Trust,
    value::Val,
    world::{MethodRole, static_key},
};

impl<'a> Cx<'_, '_, 'a> {
    /// Interprets statements in order.
    pub fn statements(&mut self, statements: &'a [Statement<'a>]) {
        for statement in statements {
            if !self.env.reachable {
                // Dead code still gets facts (it can't run, so anything holds), but
                // declarations hoisted into it are already bound.
                break;
            }
            self.statement(statement);
        }
    }

    /// Binds the function declarations of a block before its statements run.
    pub fn hoist(&mut self, statements: &'a [Statement<'a>]) {
        for statement in statements {
            let function = match statement {
                Statement::FunctionDeclaration(function) => Some(&**function),
                Statement::ExportDeclaration(export) => match &export.declaration {
                    Declaration::FunctionDeclaration(function) => Some(&**function),
                    _ => None,
                },
                Statement::ExportDefaultDeclaration(export) => match &export.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                        Some(&**function)
                    }
                    _ => None,
                },
                _ => None,
            };
            let Some(function) = function else { continue };
            if function.body.is_none() {
                continue;
            }
            let site = self.alloc(
                function.node_id.get(),
                ObjKind::Function(self.node(function.node_id.get())),
            );
            let value = Val::object(Type::opaque_function(Some(site)), site);
            if let Some((id, symbol)) = function
                .id
                .as_ref()
                .and_then(|id| id.symbol_id.get().map(|symbol| (id, symbol)))
            {
                self.write_symbol(symbol, value, Some(id.node_id.get()));
            } else {
                // `export default function () {}`
                self.record_node(function.node_id.get(), &value);
                let node = self.node(function.node_id.get());
                self.an.join_default(node, &value);
                if !self.an.options.closed_world {
                    self.escape_value(&value, Escape::Unknown);
                }
            }
        }
    }

    /// Interprets one statement.
    pub fn statement(&mut self, statement: &'a Statement<'a>) {
        let labels = std::mem::take(&mut self.pending_labels);
        #[expect(
            clippy::match_same_arms,
            reason = "distinct cases, documented separately"
        )]
        match statement {
            Statement::BlockStatement(block) => {
                self.hoist(&block.body);
                self.labelled_block(&labels, |cx| cx.statements(&block.body));
            }
            Statement::ExpressionStatement(statement) => {
                let _ = self.expr(&statement.expression);
            }
            Statement::EmptyStatement(_) | Statement::DebuggerStatement(_) => {}
            Statement::VariableDeclaration(declaration) => self.variable_declaration(declaration),
            Statement::FunctionDeclaration(_) => {}
            Statement::ClassDeclaration(class) => {
                let _ = self.class_definition(class);
            }
            Statement::IfStatement(statement) => {
                let (yes, no) = self.condition(&statement.test);
                self.env = yes;
                self.labelled_block(&labels, |cx| {
                    cx.statement(&statement.consequent);
                    let after_yes = std::mem::replace(&mut cx.env, no);
                    if let Some(alternate) = &statement.alternate {
                        cx.statement(alternate);
                    }
                    cx.env = after_yes.join(&cx.env);
                });
            }
            Statement::ReturnStatement(statement) => {
                let value = match &statement.argument {
                    Some(argument) => self.expr(argument),
                    None => Val::proven(Type::Undefined),
                };
                let level = if self.an.is_open(self.unit) {
                    Escape::Unknown
                } else {
                    Escape::Known
                };
                self.escape_value(&value, level);
                self.returned = self.returned.join(&value);
                self.env = Env::unreachable();
            }
            Statement::ThrowStatement(statement) => {
                let value = self.expr(&statement.argument);
                self.escape_value(&value, Escape::Unknown);
                self.always(EffectKind::Throws);
                self.env = Env::unreachable();
            }
            Statement::BreakStatement(statement) => {
                let label = statement.label.as_ref().map(|label| label.name.as_str());
                self.breaks.push((label, self.env.clone()));
                self.env = Env::unreachable();
            }
            Statement::ContinueStatement(statement) => {
                let label = statement.label.as_ref().map(|label| label.name.as_str());
                self.continues.push((label, self.env.clone()));
                self.env = Env::unreachable();
            }
            Statement::LabeledStatement(statement) => {
                let mut labels = labels;
                labels.push(statement.label.name.as_str());
                if is_loop(&statement.body) {
                    self.pending_labels = labels;
                    self.statement(&statement.body);
                } else {
                    self.labelled_block(&labels, |cx| cx.statement(&statement.body));
                }
            }
            Statement::WhileStatement(statement) => {
                self.run_loop(
                    &labels,
                    Some(&statement.test),
                    &statement.body,
                    None,
                    false,
                    None,
                );
            }
            Statement::DoWhileStatement(statement) => {
                self.run_loop(
                    &labels,
                    Some(&statement.test),
                    &statement.body,
                    None,
                    true,
                    None,
                );
            }
            Statement::ForStatement(statement) => {
                match &statement.init {
                    Some(ForStatementInit::VariableDeclaration(declaration)) => {
                        self.variable_declaration(declaration);
                    }
                    Some(init) => {
                        if let Some(expression) = init.as_expression() {
                            let _ = self.expr(expression);
                        }
                    }
                    None => {}
                }
                self.run_loop(
                    &labels,
                    statement.test.as_ref(),
                    &statement.body,
                    statement.update.as_ref(),
                    false,
                    None,
                );
            }
            Statement::ForOfStatement(statement) => {
                let iterable = self.expr(&statement.right);
                let element = if statement.r#await {
                    self.escape_value(&iterable, Escape::Unknown);
                    self.always(EffectKind::CallsUnknown);
                    self.always(EffectKind::Throws);
                    Val::unknown()
                } else {
                    self.iterate(&iterable, statement.node_id.get())
                };
                self.run_loop(
                    &labels,
                    None,
                    &statement.body,
                    None,
                    false,
                    Some((&statement.left, &element)),
                );
            }
            Statement::ForInStatement(statement) => {
                let object = self.expr(&statement.right);
                self.always(EffectKind::Reads);
                // Enumerating a proxy's keys runs its traps.
                self.effect(
                    EffectKind::CallsUnknown,
                    object.pts.sites().is_some() && object.trust == Trust::Proven,
                    object.trust >= Trust::Declared,
                );
                self.run_loop(
                    &labels,
                    None,
                    &statement.body,
                    None,
                    false,
                    Some((&statement.left, &Val::proven(Type::String))),
                );
            }
            Statement::SwitchStatement(statement) => self.switch(statement, &labels),
            Statement::TryStatement(statement) => self.try_statement(statement),
            Statement::WithStatement(statement) => {
                let object = self.expr(&statement.object);
                self.escape_value(&object, Escape::Unknown);
                self.always(EffectKind::CallsUnknown);
                self.always(EffectKind::Throws);
                self.statement(&statement.body);
            }
            Statement::ExportDeclaration(export) => self.declaration(&export.declaration),
            Statement::ExportDefaultDeclaration(export) => match &export.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(_) => {}
                ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                    let value = self.class_definition(class);
                    self.record_node(class.node_id.get(), &value);
                    let node = self.node(class.node_id.get());
                    self.an.join_default(node, &value);
                    if !self.an.options.closed_world {
                        self.escape_value(&value, Escape::Unknown);
                    }
                }
                ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => {}
                declaration => {
                    if let Some(expression) = declaration.as_expression() {
                        let value = self.expr(expression);
                        let node = self.node(expression.node_id());
                        self.an.join_default(node, &value);
                        let level = if self.an.options.closed_world {
                            Escape::Known
                        } else {
                            Escape::Unknown
                        };
                        self.escape_value(&value, level);
                    }
                }
            },
            Statement::TSExportAssignment(export) => {
                let value = self.expr(&export.expression);
                self.escape_value(&value, Escape::Unknown);
            }
            Statement::ImportDeclaration(_)
            | Statement::ExportAllDeclaration(_)
            | Statement::ExportNamedDeclaration(_)
            | Statement::ExportFromDeclaration(_)
            | Statement::TSNamespaceExportDeclaration(_) => {}
            declaration => {
                if let Some(declaration) = declaration.as_declaration() {
                    self.declaration(declaration);
                }
            }
        }
    }

    fn declaration(&mut self, declaration: &'a Declaration<'a>) {
        #[expect(
            clippy::match_same_arms,
            reason = "distinct cases, documented separately"
        )]
        match declaration {
            Declaration::VariableDeclaration(declaration) => self.variable_declaration(declaration),
            Declaration::FunctionDeclaration(_) => {}
            Declaration::ClassDeclaration(class) => {
                let _ = self.class_definition(class);
            }
            Declaration::TSEnumDeclaration(declaration) => {
                for member in &declaration.body.members {
                    if let Some(initializer) = &member.initializer {
                        let _ = self.expr(initializer);
                    }
                }
                if let Some(symbol) = declaration.id.symbol_id.get() {
                    // An enum object; members are read through TypeScript's types.
                    self.write_symbol(symbol, Val::unknown(), Some(declaration.id.node_id.get()));
                }
            }
            Declaration::TSNamespaceDeclaration(namespace) => {
                if let oxc_ast::ast::TSNamespaceDeclarationBody::TSModuleBlock(block) =
                    &namespace.body
                {
                    self.hoist(&block.body);
                    self.statements(&block.body);
                }
                // Its members become properties of the namespace object, which trioxide doesn't
                // model: they escape.
                if let Some(scope) = namespace.scope_id.get() {
                    let scoping = self.module.semantic.scoping();
                    let members: Vec<_> = scoping.iter_bindings_in(scope).collect();
                    for symbol in members {
                        let value = self.read_symbol(symbol);
                        self.escape_value(&value, Escape::Unknown);
                    }
                }
                if let Some(symbol) = namespace.id.symbol_id.get() {
                    self.write_symbol(symbol, Val::unknown(), Some(namespace.id.node_id.get()));
                }
            }
            Declaration::TSImportEqualsDeclaration(declaration) => {
                if let Some(symbol) = declaration.id.symbol_id.get() {
                    self.write_symbol(symbol, Val::unknown(), Some(declaration.id.node_id.get()));
                }
            }
            _ => {}
        }
    }

    fn variable_declaration(&mut self, declaration: &'a VariableDeclaration<'a>) {
        if declaration.declare {
            return;
        }
        let using = matches!(
            declaration.kind,
            VariableDeclarationKind::Using | VariableDeclarationKind::AwaitUsing
        );
        for declarator in &declaration.declarations {
            let value = match &declarator.init {
                Some(init) => self.expr(init),
                None if declaration.kind == VariableDeclarationKind::Var => {
                    // `var x;` keeps whatever x holds.
                    continue;
                }
                None => Val::proven(Type::Undefined),
            };
            if using {
                // Disposal calls a method at scope exit.
                self.escape_value(&value, Escape::Unknown);
                self.always(EffectKind::CallsUnknown);
                self.always(EffectKind::Throws);
            }
            self.length_alias(&declarator.id, declarator.init.as_ref());
            self.bind_pattern(&declarator.id, value);
        }
    }

    /// Remembers `const n = array.length` for loop bounds.
    fn length_alias(&mut self, pattern: &'a BindingPattern<'a>, init: Option<&'a Expression<'a>>) {
        let (
            BindingPattern::BindingIdentifier(id),
            Some(Expression::StaticMemberExpression(member)),
        ) = (pattern, init.map(Expression::without_parentheses))
        else {
            return;
        };
        if member.property.name != "length" {
            return;
        }
        let (Some(n), Some(array)) = (id.symbol_id.get(), self.stable_symbol(&member.object))
        else {
            return;
        };
        if self.is_tracked_here(n) {
            // Recorded after the binding is written (which forgets old relations).
            self.pending_length = Some((n, array));
        }
    }

    /// Binds a pattern to a value.
    pub fn bind_pattern(&mut self, pattern: &'a BindingPattern<'a>, value: Val) {
        match pattern {
            BindingPattern::BindingIdentifier(id) => {
                let value = self.or_declared(value, id.node_id.get(), id.span);
                if let Some(symbol) = id.symbol_id.get() {
                    self.write_symbol(symbol, value, Some(id.node_id.get()));
                    if let Some((n, array)) = self.pending_length.take()
                        && n == symbol
                    {
                        self.env.lengths.push((n, array));
                    }
                }
            }
            BindingPattern::ObjectPattern(object) => {
                self.require_object(&value);
                for property in &object.properties {
                    let key = if property.computed {
                        match property.key.as_expression() {
                            Some(key) => {
                                let key = self.expr(key);
                                Key::of_value(&key)
                            }
                            None => Key::Unknown,
                        }
                    } else {
                        static_key(&property.key, false).map_or(Key::Unknown, Key::Name)
                    };
                    let read = self.read_prop(&value, &key, property.node_id.get());
                    self.bind_pattern(&property.value, read);
                }
                if let Some(rest) = &object.rest {
                    let site = self.alloc(rest.node_id.get(), ObjKind::Object);
                    self.mark(site, |obj| obj.dynamic = true);
                    self.escape_value(&value, Escape::Known);
                    self.bind_pattern(&rest.argument, Val::object(Type::empty_object(), site));
                }
            }
            BindingPattern::ArrayPattern(array) => {
                let element = self.iterate(&value, array.node_id.get());
                for item in array.elements.iter().flatten() {
                    self.bind_pattern(item, element.join(&Val::proven(Type::Undefined)));
                }
                if let Some(rest) = &array.rest {
                    let site = self.alloc(rest.node_id.get(), ObjKind::Array);
                    self.store_elem(site, &element);
                    self.bind_pattern(
                        &rest.argument,
                        Val::object(Type::Array(std::sync::Arc::new(Type::Unknown)), site),
                    );
                }
            }
            BindingPattern::AssignmentPattern(assignment) => {
                let value = if value.ty.members().contains(&Type::Undefined)
                    || value.trust == Trust::Unknown
                {
                    let default = self.expr(&assignment.right);
                    without_undefined(&value).join(&default)
                } else {
                    value
                };
                self.bind_pattern(&assignment.left, value);
            }
        }
    }

    /// Binds a unit's parameters to what its callers pass.
    pub fn params(&mut self, params: &'a FormalParameters<'a>) {
        let open = self.an.is_open(self.unit);
        self.an.read(super::Dep::Params(self.unit.id));
        let spread = self.an.state.spread_called.contains(&self.unit.id);
        let recorded = self
            .an
            .state
            .params
            .get(&self.unit.id)
            .cloned()
            .unwrap_or_default();
        let incoming = |index: usize| -> Val {
            let known = recorded.get(index).cloned().unwrap_or_else(Val::never);
            if open || spread {
                known.join(&Val::unknown())
            } else {
                known
            }
        };
        let constructor = self.unit.role == Some(MethodRole::Constructor);
        for (index, param) in params.items.iter().enumerate() {
            let value = incoming(index);
            let value = match &param.initializer {
                Some(initializer)
                    if value.trust == Trust::Unknown
                        || value.ty.members().contains(&Type::Undefined) =>
                {
                    let default = self.expr(initializer);
                    without_undefined(&value).join(&default)
                }
                _ => value,
            };
            self.bind_pattern(&param.pattern, value);
            // `constructor(public x)` assigns `this.x`.
            if constructor
                && (param.accessibility.is_some() || param.readonly || param.r#override)
                && let BindingPattern::BindingIdentifier(id) = &param.pattern
                && let Some(symbol) = id.symbol_id.get()
            {
                let value = self.read_symbol(symbol);
                let this = self.this.clone();
                self.write_prop(
                    &this,
                    &Key::Name(id.name.to_string()),
                    &value,
                    param.node_id.get(),
                    true,
                );
            }
        }
        if let Some(rest) = &params.rest {
            let site = self.alloc(rest.node_id.get(), ObjKind::Array);
            let mut element = Val::never();
            for index in params.items.len()..recorded.len() {
                element = element.join(&incoming(index));
            }
            if open || spread {
                element = element.join(&Val::unknown());
            }
            self.store_elem(site, &element);
            self.mark(site, |obj| obj.length = None);
            let value = Val::object(Type::Array(std::sync::Arc::new(Type::Unknown)), site);
            self.bind_pattern(&rest.rest.argument, value);
        }
    }

    /// Runs `body` as a labelled statement: `break label` inside it lands after it.
    fn labelled_block(&mut self, labels: &[&'a str], body: impl FnOnce(&mut Self)) {
        let mark = self.breaks.len();
        body(self);
        if labels.is_empty() {
            return;
        }
        let mut kept = Vec::new();
        for (label, env) in self.breaks.drain(mark..) {
            if label.is_some_and(|label| labels.contains(&label)) {
                self.env = self.env.join(&env);
            } else {
                kept.push((label, env));
            }
        }
        self.breaks.extend(kept);
    }

    /// A loop, iterated until its head environment is stable.
    fn run_loop(
        &mut self,
        labels: &[&'a str],
        test: Option<&'a Expression<'a>>,
        body: &'a Statement<'a>,
        update: Option<&'a Expression<'a>>,
        do_while: bool,
        binding: Option<(&'a ForStatementLeft<'a>, &Val)>,
    ) {
        let entry = self.env.clone();
        let mut head = entry.clone();
        let mut exit = if binding.is_some() {
            entry.clone()
        } else {
            Env::unreachable()
        };
        let break_mark = self.breaks.len();
        let mut iteration = 0;
        loop {
            self.env = head.clone();
            if let Some(test) = test.filter(|_| !do_while) {
                let (yes, no) = self.condition(test);
                exit = exit.join(&no);
                self.env = yes;
            }
            if let Some((left, element)) = binding {
                self.bind_left(left, element.clone());
            }
            let continue_mark = self.continues.len();
            self.statement(body);
            for (label, env) in self.continues.split_off(continue_mark) {
                if label.is_none_or(|label| labels.contains(&label)) {
                    self.env = self.env.join(&env);
                } else {
                    self.continues.push((label, env));
                }
            }
            if let Some(test) = test.filter(|_| do_while) {
                let (yes, no) = self.condition(test);
                exit = exit.join(&no);
                self.env = yes;
            }
            if let Some(update) = update
                && self.env.reachable
            {
                let _ = self.expr(update);
            }
            if binding.is_some() {
                // A for-in/of loop may end after any iteration.
                exit = exit.join(&self.env);
            }
            let next = if iteration >= LOOP_WIDEN_AFTER {
                head.widen(&self.env)
            } else {
                head.join(&self.env)
            };
            iteration += 1;
            if next == head {
                break;
            }
            head = next;
            if iteration >= MAX_LOOP_ITERATIONS {
                // Give up on the loop: everything it writes is unknown.
                for value in head.vars.values_mut() {
                    *value = Val::unknown();
                }
                head.bounds.clear();
                head.lengths.clear();
            }
            if iteration > MAX_LOOP_ITERATIONS + 1 {
                break;
            }
        }
        let mut kept = Vec::new();
        for (label, env) in self.breaks.split_off(break_mark) {
            if label.is_none_or(|label| labels.contains(&label)) {
                exit = exit.join(&env);
            } else {
                kept.push((label, env));
            }
        }
        self.breaks.extend(kept);
        self.env = exit;
    }

    fn bind_left(&mut self, left: &'a ForStatementLeft<'a>, value: Val) {
        match left {
            ForStatementLeft::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    self.bind_pattern(&declarator.id, value.clone());
                }
            }
            target => {
                if let Some(target) = target.as_assignment_target() {
                    self.assign_target(target, value, target_node(target));
                }
            }
        }
    }

    fn switch(&mut self, statement: &'a oxc_ast::ast::SwitchStatement<'a>, labels: &[&'a str]) {
        let _ = self.expr(&statement.discriminant);
        let entry = self.env.clone();
        let mark = self.breaks.len();
        let mut fall = Env::unreachable();
        let mut has_default = false;
        for case in &statement.cases {
            self.env = entry.clone();
            match &case.test {
                Some(test) => {
                    let _ = self.expr(test);
                }
                None => has_default = true,
            }
            self.env = self.env.join(&fall);
            self.hoist(&case.consequent);
            self.statements(&case.consequent);
            fall = std::mem::replace(&mut self.env, Env::unreachable());
        }
        let mut exit = fall;
        if !has_default {
            exit = exit.join(&entry);
        }
        let mut kept = Vec::new();
        for (label, env) in self.breaks.split_off(mark) {
            if label.is_none_or(|label| labels.contains(&label)) {
                exit = exit.join(&env);
            } else {
                kept.push((label, env));
            }
        }
        self.breaks.extend(kept);
        self.env = exit;
    }

    fn try_statement(&mut self, statement: &'a oxc_ast::ast::TryStatement<'a>) {
        let entry = self.env.clone();
        let catches = usize::from(statement.handler.is_some());
        self.try_writes.push(FxHashSet::default());
        self.catching += catches;
        self.hoist(&statement.block.body);
        self.statements(&statement.block.body);
        self.catching -= catches;
        let writes = self.try_writes.pop().unwrap_or_default();
        if let Some(outer) = self.try_writes.last_mut() {
            outer.extend(writes.iter().copied());
        }
        let after_try = self.env.clone();
        // An exception may leave the try block anywhere: what it wrote may hold anything it
        // assigns.
        let mut thrown = entry.join(&after_try);
        thrown.reachable = entry.reachable;
        for symbol in &writes {
            let all = self
                .an
                .symbol_value(self.module.symbol(*symbol))
                .cloned()
                .unwrap_or_else(Val::unknown);
            if let Some(value) = thrown.vars.get_mut(symbol) {
                *value = value.join(&all);
            } else if self.is_tracked_here(*symbol) {
                thrown.vars.insert(*symbol, all);
            }
            thrown.forget(*symbol);
        }
        let mut after = after_try;
        if let Some(handler) = &statement.handler {
            self.env = thrown.clone();
            if let Some(param) = &handler.param {
                self.bind_pattern(&param.pattern, Val::unknown());
            }
            self.hoist(&handler.body.body);
            self.statements(&handler.body.body);
            after = after.join(&self.env);
        }
        if let Some(finalizer) = &statement.finalizer {
            let completes = after.reachable;
            self.env = after.join(&thrown);
            self.hoist(&finalizer.body);
            self.statements(&finalizer.body);
            if !completes {
                self.env = Env::unreachable();
            }
        } else {
            self.env = after;
        }
    }

    /// Whether `symbol` is a local this unit follows flow-sensitively.
    pub fn is_tracked_here(&self, symbol: oxc_syntax::symbol::SymbolId) -> bool {
        let info = self.module.info(symbol);
        info.tracked && info.unit == Some(self.unit.id)
    }

    /// The symbol `expression` names, if it is an identifier whose binding can't change under
    /// the unit: a tracked local or a const.
    pub fn stable_symbol(
        &self,
        expression: &Expression<'_>,
    ) -> Option<oxc_syntax::symbol::SymbolId> {
        let Expression::Identifier(ident) = expression.without_parentheses() else {
            return None;
        };
        let scoping = self.module.semantic.scoping();
        let symbol = scoping
            .get_reference(ident.reference_id.get()?)
            .symbol_id()?;
        let flags = scoping.symbol_flags(symbol);
        let constant = flags.contains(oxc_syntax::symbol::SymbolFlags::ConstVariable)
            && !self.module.info(symbol).tainted;
        (self.is_tracked_here(symbol) || constant).then_some(symbol)
    }
}

fn is_loop(statement: &Statement<'_>) -> bool {
    matches!(
        statement,
        Statement::ForStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_)
            | Statement::WhileStatement(_)
            | Statement::DoWhileStatement(_)
    )
}

fn target_node(target: &oxc_ast::ast::AssignmentTarget<'_>) -> oxc_syntax::node::NodeId {
    match target {
        oxc_ast::ast::AssignmentTarget::ArrayAssignmentTarget(t) => t.node_id.get(),
        oxc_ast::ast::AssignmentTarget::ObjectAssignmentTarget(t) => t.node_id.get(),
        other => other
            .as_simple_assignment_target()
            .map_or(oxc_syntax::node::NodeId::DUMMY, GetNodeId::node_id),
    }
}
