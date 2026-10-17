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
    let Some(info) = world.classes.get(&class) else {
        return Vec::new();
    };
    if info.has_heritage {
        return Vec::new();
    }
    let mut fields = Vec::new();
    for element in &info.class.body.body {
        let ClassElement::PropertyDefinition(property) = element else {
            if let ClassElement::AccessorProperty(property) = element
                && !property.r#static
            {
                return fields;
            }
            continue;
        };
        if property.r#static || property.declare {
            continue;
        }
        let Some(name) = static_key(&property.key, property.computed) else {
            return fields;
        };
        if property.value.as_ref().is_some_and(uses_this) {
            return fields;
        }
        fields.push(name);
    }
    let Some(constructor) = info.class.body.body.iter().find_map(|element| match element {
        ClassElement::MethodDefinition(method) if method.kind == MethodDefinitionKind::Constructor => Some(&method.value),
        _ => None,
    }) else {
        return fields;
    };
    constructor_prefix(constructor, &mut fields);
    fields
}

fn constructor_prefix(constructor: &Function<'_>, fields: &mut Vec<String>) {
    for param in &constructor.params.items {
        if is_parameter_property(param)
            && let BindingPattern::BindingIdentifier(id) = &param.pattern
        {
            fields.push(id.name.to_string());
        }
        if param.initializer.as_deref().is_some_and(uses_this) {
            return;
        }
    }
    let Some(body) = &constructor.body else { return };
    for statement in &body.statements {
        match statement {
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
                        if !fields.contains(&name) {
                            fields.push(name);
                        }
                    }
                    AssignmentTarget::PrivateFieldExpression(member)
                        if matches!(member.object, Expression::ThisExpression(_)) =>
                    {
                        let name = format!("#{}", member.field.name);
                        if !fields.contains(&name) {
                            fields.push(name);
                        }
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
