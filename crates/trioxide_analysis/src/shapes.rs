//! Hidden-class shape facts for classes: which fields every instance has, in what order, and
//! whether instances change shape after construction (PLAN §4 pass 10 consumes these).

use oxc_ast::ast::{
    AssignmentTarget, BindingPattern, ClassElement, Expression, FormalParameter, Function,
    MethodDefinitionKind, Statement,
};
use oxc_ast_visit::{Visit, walk};
use oxc_syntax::scope::ScopeFlags;
use trioxide_common::NodeRef;

use crate::world::{World, static_key};

/// The instance fields of `class` that are defined before any code can observe the instance:
/// class fields and constructor assignments up to the first use of `this` that isn't a plain
/// `this.x = …` (with no `this` on the right).
///
/// A derived class gets none: its parent constructor may call overridden methods first.
pub fn definite_fields(world: &World<'_>, class: NodeRef) -> Vec<String> {
    let (fields, _) = fields_and_constructor_assignments(world, class);
    fields
}

/// The fields the constructor's prefix assigns (as in [`definite_fields`]): a field declared
/// without an initializer and assigned there is never seen holding `undefined`.
pub fn constructor_assigned(world: &World<'_>, class: NodeRef) -> Vec<String> {
    let (_, assigned) = fields_and_constructor_assignments(world, class);
    assigned
}

fn fields_and_constructor_assignments(
    world: &World<'_>,
    class: NodeRef,
) -> (Vec<String>, Vec<String>) {
    let Some(info) = world.classes.get(&class) else {
        return (Vec::new(), Vec::new());
    };
    let mut fields = Vec::new();
    if info.has_heritage {
        // The parent constructor runs first: only safe if it (and its parents) can't let
        // `this` out, so nothing can observe the instance before the subclass sets it up.
        let Some(parent) = static_superclass(world, class) else {
            return (Vec::new(), Vec::new());
        };
        if !is_quiet(world, parent, 0) {
            return (Vec::new(), Vec::new());
        }
        fields = definite_fields(world, parent);
    }
    // Parameter properties come first: TypeScript emits them before the other fields under
    // both field semantics.
    let constructor_params = info
        .class
        .body
        .body
        .iter()
        .find_map(|element| match element {
            ClassElement::MethodDefinition(method)
                if method.kind == MethodDefinitionKind::Constructor =>
            {
                Some(&method.value.params)
            }
            _ => None,
        });
    for param in constructor_params
        .into_iter()
        .flat_map(|params| &params.items)
    {
        if param.initializer.as_deref().is_some_and(uses_this) {
            break;
        }
        if is_parameter_property(param)
            && let BindingPattern::BindingIdentifier(id) = &param.pattern
            && !fields.contains(&id.name.to_string())
        {
            fields.push(id.name.to_string());
        }
    }
    for element in &info.class.body.body {
        let ClassElement::PropertyDefinition(property) = element else {
            if let ClassElement::AccessorProperty(property) = element
                && !property.r#static
            {
                return (fields, Vec::new());
            }
            continue;
        };
        if property.r#static || property.declare {
            continue;
        }
        let Some(name) = static_key(&property.key, property.computed) else {
            return (fields, Vec::new());
        };
        if name == "__proto__" {
            // Without `useDefineForClassFields` this sets the prototype.
            return (fields, Vec::new());
        }
        if property.value.as_ref().is_some_and(uses_this) {
            return (fields, Vec::new());
        }
        fields.push(name);
    }
    let Some(constructor) = info
        .class
        .body
        .body
        .iter()
        .find_map(|element| match element {
            ClassElement::MethodDefinition(method)
                if method.kind == MethodDefinitionKind::Constructor =>
            {
                Some(&method.value)
            }
            _ => None,
        })
    else {
        return (fields, Vec::new());
    };
    let mut assigned = Vec::new();
    constructor_prefix(constructor, &mut fields, &mut assigned);
    (fields, assigned)
}

fn constructor_prefix(
    constructor: &Function<'_>,
    fields: &mut Vec<String>,
    assigned: &mut Vec<String>,
) {
    for param in &constructor.params.items {
        if is_parameter_property(param)
            && let BindingPattern::BindingIdentifier(id) = &param.pattern
            && !fields.contains(&id.name.to_string())
        {
            fields.push(id.name.to_string());
        }
        if param.initializer.as_deref().is_some_and(uses_this) {
            return;
        }
    }
    let Some(body) = &constructor.body else {
        return;
    };
    for statement in &body.statements {
        match statement {
            Statement::ExpressionStatement(statement) if matches!(&statement.expression, Expression::CallExpression(call) if matches!(call.callee, Expression::Super(_)) && !call.arguments.iter().any(|argument| argument.as_expression().is_some_and(uses_this))) =>
                {}
            Statement::ExpressionStatement(statement) => {
                let Expression::AssignmentExpression(assign) = &statement.expression else {
                    if uses_this(&statement.expression) {
                        return;
                    }
                    continue;
                };
                if uses_this(&assign.right) {
                    return;
                }
                match &assign.left {
                    AssignmentTarget::StaticMemberExpression(member)
                        if matches!(member.object, Expression::ThisExpression(_)) =>
                    {
                        let name = member.property.name.to_string();
                        if name == "__proto__" {
                            // Sets the prototype; no own property.
                            return;
                        }
                        if !fields.contains(&name) {
                            fields.push(name.clone());
                        }
                        assigned.push(name);
                    }
                    AssignmentTarget::PrivateFieldExpression(member)
                        if matches!(member.object, Expression::ThisExpression(_)) =>
                    {
                        let name = format!("#{}", member.field.name);
                        if !fields.contains(&name) {
                            fields.push(name.clone());
                        }
                        assigned.push(name);
                    }
                    target => {
                        let mut finder = ThisFinder::default();
                        finder.visit_assignment_target(target);
                        if finder.found {
                            return;
                        }
                    }
                }
            }
            Statement::VariableDeclaration(declaration) => {
                if declaration
                    .declarations
                    .iter()
                    .any(|declarator| declarator.init.as_ref().is_some_and(uses_this))
                {
                    return;
                }
            }
            _ => return,
        }
    }
}

/// The class `class` extends, when its `extends` clause names a class declared in the same
/// module by a binding nothing reassigns.
pub fn static_superclass(world: &World<'_>, class: NodeRef) -> Option<NodeRef> {
    let info = world.classes.get(&class)?;
    let Expression::Identifier(ident) = info
        .class
        .heritage
        .as_ref()?
        .expression
        .without_parentheses()
    else {
        return None;
    };
    let module = world.module(class.module);
    let scoping = module.semantic.scoping();
    let symbol = scoping
        .get_reference(ident.reference_id.get()?)
        .symbol_id()?;
    if scoping.symbol_is_mutated(symbol) || module.info(symbol).tainted {
        return None;
    }
    let declaration = scoping.symbol_declaration(symbol);
    match module.kind(declaration) {
        oxc_ast::AstKind::Class(_) => Some(module.node(declaration)),
        _ => None,
    }
}

/// Whether constructing `class` runs no code that could see `this` before the constructor
/// returns: every field initialiser and constructor statement is a plain assignment or
/// declaration without `this` (and the same holds for its parents).
fn is_quiet(world: &World<'_>, class: NodeRef, depth: usize) -> bool {
    let Some(info) = world.classes.get(&class) else {
        return false;
    };
    if depth > 16 {
        return false;
    }
    if info.has_heritage
        && !static_superclass(world, class).is_some_and(|parent| is_quiet(world, parent, depth + 1))
    {
        return false;
    }
    for element in &info.class.body.body {
        match element {
            ClassElement::PropertyDefinition(property) if !property.r#static => {
                if property.computed || property.value.as_ref().is_some_and(uses_this) {
                    return false;
                }
            }
            ClassElement::AccessorProperty(property) if !property.r#static => return false,
            ClassElement::MethodDefinition(method)
                if method.kind == MethodDefinitionKind::Constructor =>
            {
                let Some(body) = &method.value.body else {
                    continue;
                };
                for param in &method.value.params.items {
                    if param.initializer.as_deref().is_some_and(uses_this) {
                        return false;
                    }
                }
                for statement in &body.statements {
                    let quiet = match statement {
                        Statement::ExpressionStatement(statement) => match &statement.expression {
                            Expression::AssignmentExpression(assign) => {
                                !uses_this(&assign.right)
                                    && matches!(&assign.left, AssignmentTarget::StaticMemberExpression(member) if matches!(member.object, Expression::ThisExpression(_)))
                            }
                            Expression::CallExpression(call)
                                if matches!(call.callee, Expression::Super(_)) =>
                            {
                                !call
                                    .arguments
                                    .iter()
                                    .any(|argument| argument.as_expression().is_some_and(uses_this))
                            }
                            expression => !uses_this(expression),
                        },
                        Statement::VariableDeclaration(declaration) => !declaration
                            .declarations
                            .iter()
                            .any(|declarator| declarator.init.as_ref().is_some_and(uses_this)),
                        _ => false,
                    };
                    if !quiet {
                        return false;
                    }
                }
            }
            _ => {}
        }
    }
    true
}

/// `constructor(public x)` and friends.
pub fn is_parameter_property(param: &FormalParameter<'_>) -> bool {
    param.accessibility.is_some() || param.readonly || param.r#override
}

/// Whether the expression refers to `this` or `super` (in itself or in arrow functions, which
/// share them).
fn uses_this(expression: &Expression<'_>) -> bool {
    let mut finder = ThisFinder::default();
    finder.visit_expression(expression);
    finder.found
}

#[derive(Default)]
struct ThisFinder {
    found: bool,
}

impl<'a> Visit<'a> for ThisFinder {
    fn visit_this_expression(&mut self, _: &oxc_ast::ast::ThisExpression) {
        self.found = true;
    }

    fn visit_super(&mut self, _: &oxc_ast::ast::Super) {
        self.found = true;
    }

    fn visit_identifier_reference(&mut self, ident: &oxc_ast::ast::IdentifierReference<'a>) {
        // `eval` and `arguments` can reach `this` too.
        if ident.name == "eval" {
            self.found = true;
        }
    }

    fn visit_function(&mut self, _: &Function<'a>, _: ScopeFlags) {
        // A nested function has its own `this`.
    }

    fn visit_class(&mut self, class: &oxc_ast::ast::Class<'a>) {
        // Only the heritage and computed keys are evaluated now.
        if let Some(heritage) = &class.heritage {
            walk::walk_expression(self, &heritage.expression);
        }
    }
}
