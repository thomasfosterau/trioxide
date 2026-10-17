//! Statements and assignments.

use oxc_syntax::operator::AssignmentOperator;

#[allow(
    clippy::wildcard_imports,
    reason = "one impl split over modules (`expect` is unfulfilled in test builds)"
)]
use super::*;

impl<'a> FnCx<'_, '_, 'a> {
    pub(super) fn block(&mut self, statements: &'a [Statement<'a>]) -> Vec<Stmt> {
        let mut out = Vec::new();
        for statement in statements {
            self.stmt(statement, &mut out);
        }
        out
    }

    fn nested(&mut self, statement: &'a Statement<'a>) -> Vec<Stmt> {
        self.block(std::slice::from_ref(statement))
    }

    fn stmt(&mut self, statement: &'a Statement<'a>, out: &mut Vec<Stmt>) {
        // Dead code has no facts to lower it with, and never runs.
        let reachable = self.facts().reachable(self.node(statement.node_id()));
        if !reachable.value && self.facts().is_usable(reachable.trust) {
            if !matches!(out.last(), Some(Stmt::Unreachable)) {
                out.push(Stmt::Unreachable);
            }
            return;
        }
        match statement {
            Statement::VariableDeclaration(declaration) => self.var_decl(declaration, out),
            Statement::ExpressionStatement(statement) => self.expr_stmt(&statement.expression, out),
            Statement::IfStatement(statement) => {
                let test = self.condition(&statement.test);
                let consequent = self.nested(&statement.consequent);
                let alternate = statement
                    .alternate
                    .as_ref()
                    .map(|alternate| self.nested(alternate))
                    .unwrap_or_default();
                if let Some(test) = test {
                    out.push(Stmt::If(test, consequent, alternate));
                }
            }
            Statement::BlockStatement(block) => {
                let body = self.block(&block.body);
                out.extend(body);
            }
            Statement::ForStatement(statement) => {
                let bound = self.loop_bound(statement);
                if let Some(init) = &statement.init {
                    match init {
                        ForStatementInit::VariableDeclaration(declaration) => {
                            self.var_decl(declaration, out);
                        }
                        other => {
                            if let Some(expression) = other.as_expression() {
                                self.expr_stmt(expression, out);
                            }
                        }
                    }
                }
                self.bounds.extend(bound);
                self.loop_stmt(
                    None,
                    statement.test.as_ref(),
                    statement.update.as_ref(),
                    &statement.body,
                    false,
                    out,
                );
                if bound.is_some() {
                    self.bounds.pop();
                }
            }
            Statement::WhileStatement(statement) => {
                self.loop_stmt(
                    None,
                    Some(&statement.test),
                    None,
                    &statement.body,
                    false,
                    out,
                );
            }
            Statement::DoWhileStatement(statement) => {
                self.loop_stmt(
                    None,
                    Some(&statement.test),
                    None,
                    &statement.body,
                    true,
                    out,
                );
            }
            Statement::ForOfStatement(statement) => self.for_of(None, statement, out),
            Statement::ReturnStatement(statement) => {
                self.return_stmt(statement.argument.as_ref(), statement.span, out);
            }
            Statement::BreakStatement(statement) => {
                self.jump(true, statement.label.as_ref(), statement.span, out);
            }
            Statement::ContinueStatement(statement) => {
                self.jump(false, statement.label.as_ref(), statement.span, out);
            }
            Statement::LabeledStatement(statement) => {
                let label = statement.label.name.to_string();
                match &statement.body {
                    Statement::ForStatement(inner) => {
                        if let Some(init) = &inner.init {
                            match init {
                                ForStatementInit::VariableDeclaration(declaration) => {
                                    self.var_decl(declaration, out);
                                }
                                other => {
                                    if let Some(expression) = other.as_expression() {
                                        self.expr_stmt(expression, out);
                                    }
                                }
                            }
                        }
                        self.loop_stmt(
                            Some(label),
                            inner.test.as_ref(),
                            inner.update.as_ref(),
                            &inner.body,
                            false,
                            out,
                        );
                    }
                    Statement::WhileStatement(inner) => {
                        self.loop_stmt(
                            Some(label),
                            Some(&inner.test),
                            None,
                            &inner.body,
                            false,
                            out,
                        );
                    }
                    Statement::DoWhileStatement(inner) => {
                        self.loop_stmt(
                            Some(label),
                            Some(&inner.test),
                            None,
                            &inner.body,
                            true,
                            out,
                        );
                    }
                    Statement::ForOfStatement(inner) => self.for_of(Some(label), inner, out),
                    body => {
                        let id = self.push_frame(Some(label), FrameKind::Block);
                        let inner = self.nested(body);
                        let frame = self.frames.pop().expect("pushed");
                        out.push(Stmt::Block(frame.used.then_some(id), inner));
                    }
                }
            }
            Statement::EmptyStatement(_)
            | Statement::TSTypeAliasDeclaration(_)
            | Statement::TSInterfaceDeclaration(_) => {}
            Statement::ThrowStatement(s) => {
                self.fail::<()>(
                    s.span,
                    "throws an exception (exceptions can't cross the boundary)",
                );
            }
            Statement::TryStatement(s) => {
                self.fail::<()>(s.span, "catches exceptions");
            }
            Statement::SwitchStatement(s) => {
                self.fail::<()>(s.span, "uses `switch`");
            }
            Statement::ForInStatement(s) => {
                self.fail::<()>(s.span, "uses `for…in` (dynamic property keys)");
            }
            Statement::FunctionDeclaration(f) => {
                self.fail::<()>(
                    f.span,
                    "declares a nested function (closures aren't supported)",
                );
            }
            Statement::ClassDeclaration(c) => {
                self.fail::<()>(c.span, "declares a nested class");
            }
            other => {
                self.fail::<()>(other.span(), "uses a statement the backend doesn't support");
            }
        }
    }

    /// `(i, a)` for `for (let i = k; i < a.length; i++)` where `k` is a non-negative integer,
    /// only the update writes `i`, and `a` is never reassigned.
    fn loop_bound(
        &self,
        statement: &'a oxc_ast::ast::ForStatement<'a>,
    ) -> Option<(SymbolId, SymbolId)> {
        let Some(ForStatementInit::VariableDeclaration(declaration)) = &statement.init else {
            return None;
        };
        let [declarator] = declaration.declarations.as_slice() else {
            return None;
        };
        let BindingPattern::BindingIdentifier(counter) = &declarator.id else {
            return None;
        };
        let Some(Expression::NumericLiteral(start)) = declarator
            .init
            .as_ref()
            .map(Expression::get_inner_expression)
        else {
            return None;
        };
        if start.value < 0.0 || start.value.fract() != 0.0 {
            return None;
        }
        let counter_symbol = counter.symbol_id.get()?;
        let Some(Expression::BinaryExpression(test)) = statement
            .test
            .as_ref()
            .map(Expression::get_inner_expression)
        else {
            return None;
        };
        if test.operator != BinaryOperator::LessThan {
            return None;
        }
        let Expression::Identifier(left) = test.left.get_inner_expression() else {
            return None;
        };
        if self.module.reference_symbol(left.reference_id.get()) != Some(counter_symbol) {
            return None;
        }
        let Expression::StaticMemberExpression(length) = test.right.get_inner_expression() else {
            return None;
        };
        if length.property.name != "length" {
            return None;
        }
        let Expression::Identifier(array) = length.object.get_inner_expression() else {
            return None;
        };
        let array_symbol = self.module.reference_symbol(array.reference_id.get())?;
        let scoping = self.module.semantic.scoping();
        if scoping.symbol_is_mutated(array_symbol) {
            return None;
        }
        let update = statement.update.as_ref()?;
        let increments = match update.get_inner_expression() {
            Expression::UpdateExpression(u) => u.operator == UpdateOperator::Increment,
            Expression::AssignmentExpression(a) => {
                a.operator == AssignmentOperator::Addition
                    && matches!(a.right.get_inner_expression(), Expression::NumericLiteral(n) if n.value.to_bits() == 1.0_f64.to_bits())
            }
            _ => false,
        };
        if !increments {
            return None;
        }
        let update_span = update.span();
        let nodes = self.module.semantic.nodes();
        let only_update_writes = scoping
            .get_resolved_references(counter_symbol)
            .all(|reference| {
                !reference.is_write() || {
                    let span = nodes.kind(reference.node_id()).span();
                    update_span.start <= span.start && span.end <= update_span.end
                }
            });
        only_update_writes.then_some((counter_symbol, array_symbol))
    }

    /// Notes what a constant array local's length is, for [`FnCx::proven_in_bounds`].
    fn note_length(&mut self, symbol: SymbolId, init: &'a Expression<'a>) {
        if self.module.semantic.scoping().symbol_is_mutated(symbol) {
            return;
        }
        let length = match init.get_inner_expression() {
            Expression::NewExpression(new) => new.arguments.first().and_then(|a| a.as_expression()),
            Expression::CallExpression(call) => match call.callee.get_inner_expression() {
                Expression::StaticMemberExpression(fill) if fill.property.name == "fill" => {
                    match fill.object.get_inner_expression() {
                        Expression::NewExpression(new) => {
                            new.arguments.first().and_then(|a| a.as_expression())
                        }
                        _ => None,
                    }
                }
                _ => None,
            },
            _ => None,
        };
        match length.map(Expression::get_inner_expression) {
            Some(Expression::NumericLiteral(n)) => {
                self.fixed_len.insert(symbol, n.value);
            }
            Some(Expression::StaticMemberExpression(member))
                if member.property.name == "length" =>
            {
                if let Expression::Identifier(id) = member.object.get_inner_expression()
                    && let Some(array) = self.module.reference_symbol(id.reference_id.get())
                    && !self.module.semantic.scoping().symbol_is_mutated(array)
                {
                    self.same_len.insert(symbol, array);
                }
            }
            _ => {}
        }
    }

    /// Whether `object[index]` is in bounds by the island-local rules: a loop bound, or a
    /// constant index into an array of constant length.
    pub(super) fn proven_in_bounds(&self, object: &Expression<'_>, index: &Expression<'_>) -> bool {
        let Expression::Identifier(array) = object.get_inner_expression() else {
            return false;
        };
        let Some(array) = self.module.reference_symbol(array.reference_id.get()) else {
            return false;
        };
        match index.get_inner_expression() {
            Expression::Identifier(counter) => {
                let Some(counter) = self.module.reference_symbol(counter.reference_id.get()) else {
                    return false;
                };
                self.bounds.iter().any(|&(c, a)| {
                    c == counter && (a == array || self.same_len.get(&array) == Some(&a))
                })
            }
            Expression::NumericLiteral(n) => {
                n.value >= 0.0
                    && n.value.fract() == 0.0
                    && self.fixed_len.get(&array).is_some_and(|&len| n.value < len)
            }
            _ => false,
        }
    }

    fn push_frame(&mut self, label: Option<String>, kind: FrameKind) -> LoopId {
        let id = LoopId(self.next_loop);
        self.next_loop += 1;
        self.frames.push(Frame {
            id,
            label,
            kind,
            has_continue: false,
            labelled: false,
            used: false,
        });
        id
    }

    fn loop_stmt(
        &mut self,
        label: Option<String>,
        test: Option<&'a Expression<'a>>,
        update: Option<&'a Expression<'a>>,
        body: &'a Statement<'a>,
        test_after: bool,
        out: &mut Vec<Stmt>,
    ) {
        let id = self.push_frame(label, FrameKind::Loop);
        let test = match test {
            Some(test) => match test.get_inner_expression() {
                Expression::BooleanLiteral(b) if b.value && !test_after => None,
                _ => {
                    let Some(test) = self.condition(test) else {
                        self.frames.pop();
                        return;
                    };
                    Some(test)
                }
            },
            None => None,
        };
        let body = self.nested(body);
        let mut update_stmts = Vec::new();
        if let Some(update) = update {
            self.expr_stmt(update, &mut update_stmts);
        }
        let frame = self.frames.pop().expect("pushed");
        out.push(Stmt::Loop(Loop {
            id,
            test,
            test_after,
            body,
            update: update_stmts,
            labelled: frame.labelled,
            has_continue: frame.has_continue,
        }));
    }

    fn for_of(
        &mut self,
        label: Option<String>,
        statement: &'a ForOfStatement<'a>,
        out: &mut Vec<Stmt>,
    ) {
        if statement.r#await {
            self.fail::<()>(statement.span, "uses `for await`");
            return;
        }
        let ForStatementLeft::VariableDeclaration(declaration) = &statement.left else {
            self.fail::<()>(
                statement.left.span(),
                "assigns a `for…of` element to an existing variable",
            );
            return;
        };
        let [declarator] = declaration.declarations.as_slice() else {
            self.fail::<()>(declaration.span, "declares several `for…of` variables");
            return;
        };
        let BindingPattern::BindingIdentifier(id) = &declarator.id else {
            self.fail::<()>(declarator.span, "destructures `for…of` elements");
            return;
        };
        if matches!(declaration.kind, VariableDeclarationKind::Var) {
            self.fail::<()>(declaration.span, "uses `var`");
            return;
        }
        let Some(array) = self.expr(&statement.right) else {
            return;
        };
        let Kind::Array { elem, .. } = array.kind else {
            let message = format!("iterates over {}, not an array", array.kind.describe());
            self.fail::<()>(statement.right.span(), message);
            return;
        };
        let array_expr = match array.expr {
            // JS iterates the array it started with; a reassigned variable would switch.
            Expr::Local(local) if self.locals[local.0].mutable => {
                self.fail::<()>(
                    statement.right.span(),
                    "iterates over a variable it reassigns",
                );
                return;
            }
            Expr::Local(_) => array.expr,
            other => {
                let local = self.temp("items", array.ty);
                out.push(Stmt::Let(local, other));
                Expr::Local(local)
            }
        };
        // A `number[]` may have holes, which read as undefined.
        let kind = Kind::Num {
            undef: elem == Elem::Number,
        };
        let symbol = id.symbol_id.get().map(|s| self.module.symbol(s));
        if let Some(symbol) = symbol {
            self.lw.env.join_local(symbol, &kind);
        }
        let counter = self.temp("index", Ty::Index);
        let local = self.new_local(&id.name, symbol, None, &kind, false);
        if let Some(symbol) = id.symbol_id.get() {
            self.by_symbol.insert(symbol, local);
        }
        let loop_id = self.push_frame(label, FrameKind::Loop);
        let body = self.nested(&statement.body);
        self.frames.pop();
        out.push(Stmt::ForOf {
            id: loop_id,
            counter,
            array: array_expr,
            local,
            elem,
            body,
        });
    }

    fn jump(
        &mut self,
        is_break: bool,
        label: Option<&LabelIdentifier<'a>>,
        span: Span,
        out: &mut Vec<Stmt>,
    ) {
        let innermost_loop = self.frames.iter().rposition(|f| f.kind == FrameKind::Loop);
        let target = match label {
            Some(label) => self
                .frames
                .iter()
                .rposition(|f| f.label.as_deref() == Some(label.name.as_str())),
            None => innermost_loop,
        };
        let Some(target) = target else {
            self.fail::<()>(span, "jumps out of the function's loops");
            return;
        };
        let frame = &mut self.frames[target];
        frame.used = true;
        if Some(target) != innermost_loop || frame.kind == FrameKind::Block {
            frame.labelled = true;
        }
        if !is_break {
            frame.has_continue = true;
        }
        let id = frame.id;
        out.push(if is_break {
            Stmt::Break(id)
        } else {
            Stmt::Continue(id)
        });
    }

    fn return_stmt(
        &mut self,
        argument: Option<&'a Expression<'a>>,
        span: Span,
        out: &mut Vec<Stmt>,
    ) {
        let Some(argument) = argument else {
            self.lw.env.join_ret(self.id, &Kind::Unit, None);
            out.push(Stmt::Return(None));
            return;
        };
        if let Expression::ObjectExpression(object) = argument.get_inner_expression() {
            let site = self.node(object.node_id());
            self.lw.env.literals.entry(site).or_default().returned_by = Some(self.id);
        }
        let Some(value) = self.expr(argument) else {
            return;
        };
        if value.kind == Kind::Unit {
            self.lw.env.join_ret(self.id, &Kind::Unit, None);
            out.push(Stmt::Expr(value.expr));
            out.push(Stmt::Return(None));
            return;
        }
        let fact = match value.kind {
            Kind::Num { .. } => self.expr_fact(argument),
            _ => None,
        };
        let kind = match &value.kind {
            // A local holding only fresh arrays returns a fresh array; check its origins.
            Kind::Array {
                elem,
                fresh,
                params,
            } => Kind::Array {
                elem: *elem,
                fresh: *fresh,
                params: *params,
            },
            other => other.clone(),
        };
        self.lw.env.join_ret(self.id, &kind, fact);
        let ret = self.ret;
        if let Some(expr) = self.coerce(value, ret, true, span) {
            out.push(Stmt::Return(Some(expr)));
        }
    }

    fn var_decl(&mut self, declaration: &'a VariableDeclaration<'a>, out: &mut Vec<Stmt>) {
        if !matches!(
            declaration.kind,
            VariableDeclarationKind::Let | VariableDeclarationKind::Const
        ) {
            self.fail::<()>(declaration.span, "uses `var` or `using`");
            return;
        }
        for declarator in &declaration.declarations {
            let BindingPattern::BindingIdentifier(id) = &declarator.id else {
                self.fail::<()>(declarator.span, "destructures a value");
                continue;
            };
            let Some(init) = &declarator.init else {
                self.fail::<()>(
                    declarator.span,
                    format!("declares `{}` without a value", id.name),
                );
                continue;
            };
            let symbol = id.symbol_id.get().map(|s| self.module.symbol(s));
            let Some(value) = self.expr(init) else {
                // Declare it anyway, so its uses don't add reasons of their own.
                let local = self.new_local(&id.name, symbol, None, &Kind::Error, true);
                self.info[local.0].poisoned = true;
                if let Some(symbol) = id.symbol_id.get() {
                    self.by_symbol.insert(symbol, local);
                }
                continue;
            };
            if !self.check_stored(&value, &id.name, init.span()) {
                continue;
            }
            let stored = stored_kind(&value.kind);
            if let Some(symbol) = symbol {
                self.lw.env.join_local(symbol, &stored);
            }
            let mutable = id
                .symbol_id
                .get()
                .is_some_and(|s| self.module.semantic.scoping().symbol_is_mutated(s));
            let kind = symbol.map_or(stored.clone(), |s| {
                self.lw.env.locals.get(&s).cloned().unwrap_or(stored)
            });
            if self.final_round() && kind == Kind::Error {
                self.fail::<()>(
                    id.span,
                    format!("`{}` holds values of different types", id.name),
                );
            }
            let local = self.new_local(&id.name, symbol, None, &kind, mutable);
            if let Some(symbol) = id.symbol_id.get() {
                self.by_symbol.insert(symbol, local);
                self.note_length(symbol, init);
            }
            let ty = self.locals[local.0].ty;
            if let Some(expr) = self.coerce(value, ty, true, init.span()) {
                out.push(Stmt::Let(local, expr));
            }
        }
    }

    /// Whether `value` may be stored in a variable; records a reason if not.
    fn check_stored(&mut self, value: &V, name: &str, span: Span) -> bool {
        match &value.kind {
            Kind::Unit => self
                .fail::<()>(span, format!("stores undefined in `{name}`"))
                .is_some(),
            Kind::ParamStruct { .. } => self
                .fail::<()>(
                    span,
                    format!("copies an object parameter into `{name}` (it must be used directly)"),
                )
                .is_some(),
            Kind::Array { .. } => match value.expr {
                Expr::Local(local) if self.info[local.0].param.is_none() => self
                    .fail::<()>(
                        span,
                        format!("copies an array from one variable to `{name}`"),
                    )
                    .is_some(),
                Expr::Local(local) => {
                    let param = self.info[local.0].param.expect("checked");
                    if self.final_round() && self.lw.env.mutated.contains(&(self.id, param)) {
                        return self
                            .fail::<()>(
                                span,
                                format!(
                                    "copies an array parameter it writes to into `{name}` (the \
                                     copy would alias it)"
                                ),
                            )
                            .is_some();
                    }
                    true
                }
                _ => true,
            },
            _ => true,
        }
    }

    fn expr_stmt(&mut self, expression: &'a Expression<'a>, out: &mut Vec<Stmt>) {
        match expression.get_inner_expression() {
            Expression::AssignmentExpression(assignment) => self.assign(assignment, out),
            Expression::UpdateExpression(update) => self.update(update, out),
            Expression::SequenceExpression(sequence) => {
                for expression in &sequence.expressions {
                    self.expr_stmt(expression, out);
                }
            }
            other => {
                if let Some(value) = self.expr(other) {
                    out.push(Stmt::Expr(value.expr));
                }
            }
        }
    }

    /// Looks up a local the function may assign.
    pub(super) fn assignable_local(&mut self, id: &IdentifierReference<'a>) -> Option<LocalId> {
        let symbol = self.module.reference_symbol(id.reference_id.get());
        match symbol.and_then(|s| self.by_symbol.get(&s).copied()) {
            Some(local) => Some(local),
            None => self.fail(
                id.span,
                format!(
                    "assigns `{}`, which is not a local variable of the function",
                    id.name
                ),
            ),
        }
    }

    fn assign(&mut self, assignment: &'a AssignmentExpression<'a>, out: &mut Vec<Stmt>) {
        let op = assignment.operator;
        if op.to_logical_operator().is_some() {
            self.fail::<()>(assignment.span, "uses `&&=`, `||=` or `??=`");
            return;
        }
        let binary = op.to_binary_operator();
        let node = assignment.node_id();
        match &assignment.left {
            AssignmentTarget::AssignmentTargetIdentifier(id) => {
                let Some(local) = self.assignable_local(id) else {
                    return;
                };
                let value = match binary {
                    None => self.expr(&assignment.right),
                    Some(op) => {
                        let current = self.local_value(local);
                        let fact = self.assign_fact(local, node);
                        self.binary_values(op, current, &assignment.right, fact, assignment.span)
                    }
                };
                let Some(value) = value else { return };
                self.assign_local(local, value, &id.name, assignment.right.span(), out);
            }
            AssignmentTarget::StaticMemberExpression(member) => {
                self.field_write(
                    member,
                    binary,
                    &assignment.right,
                    node,
                    assignment.span,
                    out,
                );
            }
            AssignmentTarget::ComputedMemberExpression(member) => {
                self.element_write(
                    member,
                    binary,
                    Some(&assignment.right),
                    None,
                    assignment.span,
                    out,
                );
            }
            AssignmentTarget::ArrayAssignmentTarget(_)
            | AssignmentTarget::ObjectAssignmentTarget(_) => {
                self.fail::<()>(assignment.span, "uses a destructuring assignment");
            }
            _ => {
                self.fail::<()>(
                    assignment.span,
                    "assigns to something other than a variable, array element or field",
                );
            }
        }
    }

    /// The fact for the new value of `local` after an assignment at `node`.
    fn assign_fact(&self, local: LocalId, node: NodeId) -> Option<NumberType> {
        match self.locals[local.0].ty {
            // The local only holds values its symbol fact allows, the new one included.
            Ty::I32 | Ty::U32 => self.info[local.0].symbol.and_then(|s| self.symbol_fact(s)),
            _ => self.num_fact(node),
        }
    }

    pub(super) fn assign_local(
        &mut self,
        local: LocalId,
        value: V,
        name: &str,
        span: Span,
        out: &mut Vec<Stmt>,
    ) {
        if !self.check_stored(&value, name, span) {
            return;
        }
        let stored = stored_kind(&value.kind);
        match (self.info[local.0].param, self.info[local.0].symbol) {
            (Some(param), _) => {
                // Parameters keep their own kind; what is assigned must fit it.
                let current = self.local_kind(local);
                let fits = match (&current, &stored) {
                    (Kind::Array { elem: a, .. }, Kind::Array { elem: b, .. }) => a == b,
                    (current, stored) => current.join(stored) != Kind::Error,
                };
                if !fits {
                    return self
                        .fail(
                            span,
                            format!("assigns {} to the parameter `{name}`", stored.describe()),
                        )
                        .unwrap_or(());
                }
                if !matches!(stored, Kind::Array { .. }) {
                    self.lw.env.join_param(self.id, param, &stored);
                }
            }
            (None, Some(symbol)) => self.lw.env.join_local(symbol, &stored),
            (None, None) => {}
        }
        let ty = self.locals[local.0].ty;
        if let Some(expr) = self.coerce(value, ty, true, span) {
            out.push(Stmt::Assign(local, expr));
        }
    }

    fn update(&mut self, update: &'a UpdateExpression<'a>, out: &mut Vec<Stmt>) {
        let op = match update.operator {
            UpdateOperator::Increment => BinaryOperator::Addition,
            UpdateOperator::Decrement => BinaryOperator::Subtraction,
        };
        let one = V::new(Expr::Lit(Lit::I32(1)), Ty::I32, Kind::NUM);
        match &update.argument {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                let Some(local) = self.assignable_local(id) else {
                    return;
                };
                let current = self.local_value(local);
                if !matches!(current.kind, Kind::Num { .. }) {
                    self.fail::<()>(
                        update.span,
                        format!("increments {}", current.kind.describe()),
                    );
                    return;
                }
                let fact = match self.locals[local.0].ty {
                    Ty::I32 | Ty::U32 => self.assign_fact(local, update.node_id()),
                    _ => None,
                };
                let Some(value) = self.arith(op, current, one, fact, update.span) else {
                    return;
                };
                self.assign_local(local, value, &id.name, update.span, out);
            }
            SimpleAssignmentTarget::StaticMemberExpression(member) => {
                let Some(current) = self.member_read(member, true) else {
                    return;
                };
                let Some(value) = self.arith(op, current, one, None, update.span) else {
                    return;
                };
                self.field_store(member, value, update.span, out);
            }
            SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                self.element_write(member, Some(op), None, Some(one), update.span, out);
            }
            _ => {
                self.fail::<()>(
                    update.span,
                    "increments something other than a variable, array element or field",
                );
            }
        }
    }

    fn field_write(
        &mut self,
        member: &'a StaticMemberExpression<'a>,
        op: Option<BinaryOperator>,
        right: &'a Expression<'a>,
        node: NodeId,
        span: Span,
        out: &mut Vec<Stmt>,
    ) {
        let value = match op {
            None => self.expr(right),
            Some(op) => {
                let Some(current) = self.member_read(member, true) else {
                    return;
                };
                let fact = self.num_fact(node);
                self.binary_values(op, current, right, fact, span)
            }
        };
        let Some(value) = value else { return };
        self.field_store(member, value, span, out);
    }

    /// `p.f = value` on an object parameter.
    fn field_store(
        &mut self,
        member: &'a StaticMemberExpression<'a>,
        value: V,
        span: Span,
        out: &mut Vec<Stmt>,
    ) {
        let field = member.property.name.to_string();
        let base = match member.object.get_inner_expression() {
            Expression::Identifier(id) => {
                let symbol = self.module.reference_symbol(id.reference_id.get());
                symbol.and_then(|s| self.by_symbol.get(&s).copied())
            }
            _ => None,
        };
        let Some(base) = base else {
            self.fail::<()>(
                span,
                format!("writes `.{field}` of something other than an object parameter"),
            );
            return;
        };
        let (Some(param), Kind::ParamStruct { group }) =
            (self.info[base.0].param, self.local_kind(base))
        else {
            self.fail::<()>(
                span,
                format!(
                    "writes `.{field}` of an object created in the island (objects are immutable \
                     there) or of something that isn't an object"
                ),
            );
            return;
        };
        match &value.kind {
            Kind::Num { undef: false } | Kind::Bool => {}
            other => {
                let message = format!("stores {} in `.{field}`", other.describe());
                self.fail::<()>(span, message);
                return;
            }
        }
        if self.final_round() && self.lw.env.groups[group].field_mut(&field).read.is_none() {
            self.fail::<()>(
                span,
                format!(
                    "writes `.{field}` but never reads it, so nothing proves the object has it \
                     as a number or boolean"
                ),
            );
            return;
        }
        {
            let entry = self.lw.env.groups[group].field_mut(&field);
            let written = entry
                .written
                .clone()
                .unwrap_or(Kind::Bottom)
                .join(&value.kind);
            if entry.written.as_ref() != Some(&written) {
                entry.written = Some(written);
                self.lw.env.changed = true;
            }
        }
        if self.lw.env.writes.insert((self.id, param)) {
            self.lw.env.changed = true;
        }
        let ty = self.group_field_ty(group, &field);
        if let Some(value) = self.coerce(value, ty, false, span) {
            // Rust evaluates `value` before `base.field`.
            if self.locals[base.0].mutable && value.runs_statements() {
                self.fail::<()>(
                    span,
                    format!(
                        "writes `.{field}` of an object the stored value's evaluation may replace"
                    ),
                );
                return;
            }
            out.push(Stmt::FieldSet {
                base: Expr::Local(base),
                field,
                value,
            });
        }
    }

    pub(super) fn group_field_ty(&self, group: usize, field: &str) -> Ty {
        if self.final_round() {
            let StructRef(index) = self.struct_ref(StructSlot::Group(group));
            if let Some((_, ty)) = self.lw.structs[index]
                .fields
                .iter()
                .find(|(name, _)| name == field)
            {
                return *ty;
            }
        }
        self.lw.env.groups[group]
            .fields
            .iter()
            .find(|f| f.name == field)
            .map_or(Ty::F64, Group::field_ty)
    }

    /// `a[i] = v`, `a[i] op= v` or `a[i]++` (`increment` is the `1`).
    fn element_write(
        &mut self,
        member: &'a ComputedMemberExpression<'a>,
        op: Option<BinaryOperator>,
        right: Option<&'a Expression<'a>>,
        increment: Option<V>,
        span: Span,
        out: &mut Vec<Stmt>,
    ) {
        let Some(array) = self.expr(&member.object) else {
            return;
        };
        let Kind::Array {
            elem,
            fresh,
            params,
        } = array.kind.clone()
        else {
            let message = format!("writes a computed property of {}", array.kind.describe());
            self.fail::<()>(span, message);
            return;
        };
        // Who is written to.
        match array.expr {
            Expr::Local(local) if self.info[local.0].param.is_some() => {
                let param = self.info[local.0].param.expect("checked");
                if self.lw.env.writes.insert((self.id, param)) {
                    self.lw.env.changed = true;
                }
            }
            _ if params == 0 && fresh => {}
            _ => {
                self.fail::<()>(
                    span,
                    "writes to an array through a variable that may hold one of its parameters",
                );
                return;
            }
        }
        let Some(index) = self.index(&member.expression, member.node_id()) else {
            return;
        };
        let known = index.in_bounds || self.proven_in_bounds(&member.object, &member.expression);
        if elem == Elem::Number && !known {
            self.fail::<()>(
                span,
                "may write past the end of a `number[]` (which would grow it); the analysis can't \
                 prove the index in bounds",
            );
            return;
        }
        let array_expr = array.expr.clone();
        let mut prelude = Vec::new();
        // JS evaluates the array and the index before the value; a value with effects is
        // evaluated into a temporary before the store, so the index goes first.
        let hoist = right.is_some_and(|right| {
            js_assigns(right) || index.value.has_effects() && js_effects(right)
        });
        let index = if op.is_some() && !is_cheap(&index.value)
            || hoist && !matches!(index.value, Expr::Lit(_) | Expr::Const(_))
        {
            let local = self.temp("index", index.ty);
            prelude.push(Stmt::Let(local, index.value));
            Index {
                value: Expr::Local(local),
                ..index
            }
        } else {
            index
        };
        let value = match (op, right, increment) {
            (None, Some(right), _) => self.expr(right),
            (Some(op), right, increment) => {
                let current = V::new(
                    Expr::Get {
                        array: Box::new(array_expr.clone()),
                        index: Box::new(index.clone()),
                    },
                    Ty::F64,
                    Kind::Num {
                        undef: !known || (elem == Elem::Number && !fresh),
                    },
                );
                match (right, increment) {
                    (Some(right), _) => self.binary_values(op, current, right, None, span),
                    (None, Some(one)) => self.arith(op, current, one, None, span),
                    _ => None,
                }
            }
            _ => None,
        };
        let Some(value) = value else { return };
        match (&value.kind, elem) {
            (Kind::Num { undef: false }, _) | (Kind::Num { undef: true }, Elem::Typed(_)) => {}
            (Kind::Num { undef: true }, Elem::Number) => {
                self.fail::<()>(span, "may store undefined in a `number[]` (a value read from a hole or past the end, which the analysis can't rule out)");
                return;
            }
            (other, _) => {
                let message = format!("stores {} in an array", other.describe());
                self.fail::<()>(span, message);
                return;
            }
        }
        let Some(value) = self.coerce(value, Ty::F64, false, span) else {
            return;
        };
        if value.has_effects() {
            // The value is evaluated before the array expression below.
            let reassigned = match array_expr {
                Expr::Local(local) => self.locals[local.0].mutable && value.runs_statements(),
                ref other => other.has_effects(),
            };
            if reassigned {
                self.fail::<()>(
                    span,
                    "stores into an array that the stored value's own evaluation may replace",
                );
                return;
            }
        }
        // Evaluate the value before borrowing the array mutably.
        let value = if is_simple(&value) || prelude.is_empty() && !mentions_call(&value) {
            value
        } else {
            let local = self.temp("value", Ty::F64);
            prelude.push(Stmt::Let(local, value));
            Expr::Local(local)
        };
        out.extend(prelude);
        out.push(Stmt::ArraySet {
            array: array_expr,
            elem,
            index,
            value,
        });
    }
}

/// Whether evaluating a JS expression may have side effects (conservatively: anything but
/// literals, variables, member reads, operators and calls on them).
pub(super) fn js_effects(expr: &Expression<'_>) -> bool {
    js_scan(expr, true)
}

/// Whether evaluating a JS expression may assign one of the function's variables (calls
/// can't: the island has no closures).
pub(super) fn js_assigns(expr: &Expression<'_>) -> bool {
    js_scan(expr, false)
}

fn js_scan(expr: &Expression<'_>, calls: bool) -> bool {
    let all = |args: &oxc_allocator::Vec<'_, Argument<'_>>| {
        args.iter()
            .any(|arg| arg.as_expression().is_none_or(|e| js_scan(e, calls)))
    };
    match expr.get_inner_expression() {
        Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::Identifier(_)
        | Expression::ThisExpression(_) => false,
        Expression::BinaryExpression(e) => js_scan(&e.left, calls) || js_scan(&e.right, calls),
        Expression::LogicalExpression(e) => js_scan(&e.left, calls) || js_scan(&e.right, calls),
        Expression::ConditionalExpression(e) => {
            js_scan(&e.test, calls) || js_scan(&e.consequent, calls) || js_scan(&e.alternate, calls)
        }
        Expression::UnaryExpression(e) => {
            e.operator == UnaryOperator::Delete || js_scan(&e.argument, calls)
        }
        Expression::StaticMemberExpression(e) => js_scan(&e.object, calls),
        Expression::ComputedMemberExpression(e) => {
            js_scan(&e.object, calls) || js_scan(&e.expression, calls)
        }
        Expression::CallExpression(e) => calls || js_scan(&e.callee, calls) || all(&e.arguments),
        Expression::NewExpression(e) => calls || js_scan(&e.callee, calls) || all(&e.arguments),
        Expression::SequenceExpression(e) => e.expressions.iter().any(|e| js_scan(e, calls)),
        _ => true,
    }
}

/// The kind a variable holding `kind` has: arrays forget nothing, numbers keep `undef`.
pub(super) fn stored_kind(kind: &Kind) -> Kind {
    kind.clone()
}

/// A local, constant or literal: free to evaluate twice.
pub(super) fn is_simple(expr: &Expr) -> bool {
    matches!(expr, Expr::Local(_) | Expr::Lit(_) | Expr::Const(_))
        || matches!(expr, Expr::Convert { arg, .. } if is_simple(arg))
}

/// Whether evaluating `expr` may call a function or run statements (so it must run before
/// the array it is stored into is borrowed mutably, and can't be evaluated twice).
pub(super) fn mentions_call(expr: &Expr) -> bool {
    expr.has_effects()
}

/// Locals, constants and arithmetic on them: cheap enough to evaluate twice.
pub(super) fn is_cheap(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(_) | Expr::Local(_) | Expr::Const(_) => true,
        Expr::Convert { arg, .. } | Expr::Unary(_, arg) => is_cheap(arg),
        Expr::Binary(_, a, b) => is_cheap(a) && is_cheap(b),
        _ => false,
    }
}

/// Whether evaluating `expr` twice gives the same value with no effects.
pub(super) fn is_pure(expr: &Expr) -> bool {
    !mentions_call(expr)
}

/// Whether control can reach the end of `stmts`, by Rust's rules (so a function the backend
/// says always returns also type-checks).
pub(super) fn completes(stmts: &[Stmt]) -> bool {
    !stmts.iter().any(diverges)
}

fn diverges(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(_) | Stmt::Break(_) | Stmt::Continue(_) | Stmt::Unreachable => true,
        Stmt::If(_, a, b) => !completes(a) && !completes(b),
        Stmt::Loop(Loop {
            id,
            test: None,
            test_after: false,
            body,
            update,
            ..
        }) => !breaks_to(*id, body) && !breaks_to(*id, update),
        Stmt::Block(label, body) => !completes(body) && label.is_none_or(|id| !breaks_to(id, body)),
        _ => false,
    }
}

/// Whether `stmts` contain a `break` of loop `id`.
fn breaks_to(id: LoopId, stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Break(target) => *target == id,
        Stmt::If(_, a, b) => breaks_to(id, a) || breaks_to(id, b),
        Stmt::Loop(Loop { body, update, .. }) => breaks_to(id, body) || breaks_to(id, update),
        Stmt::ForOf { body, .. } | Stmt::Block(_, body) => breaks_to(id, body),
        _ => false,
    })
}
