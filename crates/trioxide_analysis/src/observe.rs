//! Which classes' instances may have their own keys enumerated or tested (PLAN §4 pass 10):
//! code that can tell whether, and in which order, an instance got its properties.

use oxc_ast::ast::{
    BindingPattern, CallExpression, Expression, ForInStatement, ObjectExpression,
    ObjectPropertyKind, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_syntax::operator::BinaryOperator;
use rustc_hash::FxHashSet;
use trioxide_common::NodeRef;

use crate::{
    interp::{Escape, ObjKind, Results},
    world::World,
};

/// The classes no code can observe the own keys of: every class whose instances (and
/// prototype) the analysis follows, that no program class extends, and whose instances no key
/// observer may see. Key observers are `for…in`, `in`, object spread and rest, and every call
/// of an `Object.*`, `JSON.*` or `Reflect.*` function, of `.call`/`.apply`, `structuredClone`,
/// `hasOwnProperty` or `propertyIsEnumerable`, on any argument or receiver.
///
/// An observed value whose points-to set is unknown can't be one of these instances: an
/// instance joined with an untracked object escapes to unknown code, which makes its class
/// open.
pub fn unobserved_classes(world: &World<'_>, results: &Results) -> FxHashSet<NodeRef> {
    let patched = crate::interp::patched_builtins(world);
    if ["Object", "JSON"]
        .iter()
        .any(|name| patched.contains(*name))
    {
        // Some use of `Object` or `JSON` other than a direct call: its calls aren't visible.
        return FxHashSet::default();
    }
    let mut observed: FxHashSet<NodeRef> = FxHashSet::default();
    for module in &world.modules {
        if module.broken {
            return FxHashSet::default();
        }
        let mut find = Observers {
            module: module.id,
            operands: Vec::new(),
            opaque: false,
        };
        find.visit_program(module.program);
        if find.opaque {
            return FxHashSet::default();
        }
        for operand in find.operands {
            // An operand with no recorded value may be one the interpreter evaluates without
            // recording (or code that never runs): either way, nothing is known.
            let Some(value) = results.output.nodes.get(&operand) else {
                return FxHashSet::default();
            };
            if let Some(sites) = value.pts.sites() {
                observed.extend(sites.iter().copied());
            }
        }
    }
    let state = &results.state;
    let mut classes = FxHashSet::default();
    'classes: for &class in world.classes.keys() {
        if crate::interp::class_open(world, state, class) {
            continue;
        }
        for (subclass, parent) in &state.supers {
            if *subclass != class
                && parent
                    .pts
                    .sites()
                    .is_none_or(|sites| sites.contains(&class))
            {
                continue 'classes;
            }
        }
        for (site, obj) in &state.heap {
            let spoiled = match obj.kind {
                ObjKind::Instance(of) if of == class => {
                    obj.escape == Escape::Unknown || obj.poisoned || observed.contains(site)
                }
                ObjKind::Prototype(of) if of == class => {
                    obj.escape == Escape::Unknown
                        || obj.poisoned
                        || obj.dynamic
                        || !obj.added.is_empty()
                }
                _ => false,
            };
            if spoiled {
                continue 'classes;
            }
        }
        classes.insert(class);
    }
    classes
}

/// Collects the operands of key observers in one module.
struct Observers {
    module: trioxide_common::ModuleId,
    operands: Vec<NodeRef>,
    /// An observer whose operand can't be named (an object rest pattern outside a variable
    /// declaration).
    opaque: bool,
}

impl Observers {
    fn operand(&mut self, expression: &Expression<'_>) {
        self.operands.push(NodeRef {
            module: self.module,
            node: expression.get_inner_expression().node_id(),
        });
    }
}

/// Whether `pattern` has an object rest element at its top level or below.
fn has_object_rest(pattern: &BindingPattern<'_>) -> bool {
    match pattern {
        BindingPattern::ObjectPattern(object) => {
            object.rest.is_some()
                || object
                    .properties
                    .iter()
                    .any(|property| has_object_rest(&property.value))
        }
        BindingPattern::ArrayPattern(array) => {
            array.elements.iter().flatten().any(has_object_rest)
                || array
                    .rest
                    .as_ref()
                    .is_some_and(|rest| has_object_rest(&rest.argument))
        }
        BindingPattern::AssignmentPattern(assignment) => has_object_rest(&assignment.left),
        BindingPattern::BindingIdentifier(_) => false,
    }
}

impl<'a> Visit<'a> for Observers {
    fn visit_for_in_statement(&mut self, statement: &ForInStatement<'a>) {
        self.operand(&statement.right);
        walk::walk_for_in_statement(self, statement);
    }

    fn visit_binary_expression(&mut self, binary: &oxc_ast::ast::BinaryExpression<'a>) {
        if binary.operator == BinaryOperator::In {
            self.operand(&binary.right);
        }
        walk::walk_binary_expression(self, binary);
    }

    fn visit_object_expression(&mut self, object: &ObjectExpression<'a>) {
        for property in &object.properties {
            if let ObjectPropertyKind::SpreadProperty(spread) = property {
                self.operand(&spread.argument);
            }
        }
        walk::walk_object_expression(self, object);
    }

    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if let BindingPattern::ObjectPattern(object) = &declarator.id
            && object.rest.is_some()
            && let Some(init) = &declarator.init
            && !object
                .properties
                .iter()
                .any(|property| has_object_rest(&property.value))
        {
            self.operand(init);
            // The pattern itself is accounted for.
            if let Some(init) = &declarator.init {
                self.visit_expression(init);
            }
            for property in &object.properties {
                self.visit_binding_pattern(&property.value);
            }
            return;
        }
        walk::walk_variable_declarator(self, declarator);
    }

    fn visit_object_pattern(&mut self, pattern: &oxc_ast::ast::ObjectPattern<'a>) {
        if pattern.rest.is_some() {
            self.opaque = true;
        }
        walk::walk_object_pattern(self, pattern);
    }

    fn visit_object_assignment_target(
        &mut self,
        target: &oxc_ast::ast::ObjectAssignmentTarget<'a>,
    ) {
        if target.rest.is_some() {
            self.opaque = true;
        }
        walk::walk_object_assignment_target(self, target);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        let observes_arguments = match call.callee.without_parentheses() {
            Expression::StaticMemberExpression(member) => {
                let namespace = matches!(
                    member.object.without_parentheses(),
                    Expression::Identifier(ident)
                        if matches!(ident.name.as_str(), "Object" | "JSON" | "Reflect")
                );
                let name = member.property.name.as_str();
                if matches!(name, "hasOwnProperty" | "propertyIsEnumerable") {
                    self.operand(&member.object);
                }
                namespace || matches!(name, "call" | "apply")
            }
            Expression::ComputedMemberExpression(_) => true,
            Expression::Identifier(ident) => ident.name == "structuredClone",
            _ => false,
        };
        if observes_arguments {
            for argument in &call.arguments {
                if let Some(expression) = argument.as_expression() {
                    self.operand(expression);
                } else if let oxc_ast::ast::Argument::SpreadElement(spread) = argument {
                    self.operand(&spread.argument);
                }
            }
        }
        walk::walk_call_expression(self, call);
    }
}
