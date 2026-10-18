//! PLAN §4 pass 10: hidden-class stabilisation.
//!
//! Engines give objects that get the same properties in the same order one hidden class, and
//! fall back to slow dictionary objects after a `delete`. For a class whose instances' keys no
//! code can observe, this pass:
//!
//! - **declares late properties up front**: a property instances get after construction
//!   ([`ClassShape::added_later`]) or only on some constructor paths becomes a class field
//!   initialised to `undefined`, after the class's own fields, so every instance has every
//!   property, in one order, from the start;
//! - **replaces `delete`**: a `delete o.p;` statement whose object can only be such an
//!   instance becomes `o.p = undefined;`.
//!
//! Both change which own properties an instance has, and in which order, before the property
//! is assigned (or after it is deleted): what `Object.keys`, `for…in`, `JSON.stringify`,
//! spread, `in` and `hasOwnProperty` would report. So the class must be one the analysis
//! follows entirely ([`ClassShape`] `Proven`, so no instance reaches unknown code) whose
//! instances no such observer may see and which nothing extends
//! ([`Facts::instance_keys_observed`]); it must have no `extends` clause, no computed instance
//! keys, and the property must not exist on its prototype chain (its methods and accessors,
//! and `Object.prototype`'s), so reading it while unset gives `undefined` either way and no
//! setter runs.
//!
//! [`ClassShape::added_later`]: trioxide_analysis::ClassShape::added_later
//! [`ClassShape`]: trioxide_analysis::ClassShape
//! [`Facts::instance_keys_observed`]: trioxide_analysis::Facts::instance_keys_observed

use oxc_allocator::{ArenaVec, TakeIn};
use oxc_ast::ast::{
    AssignmentOperator, AssignmentTarget, Class, ClassElement, Expression, IdentifierName,
    MethodDefinitionKind, Program, PropertyDefinitionType, PropertyKey, Statement, UnaryOperator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::SPAN;
use oxc_syntax::scope::ScopeFlags;
use oxc_traverse::{Traverse, TraverseCtx};
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_analysis::{AllocKind, Constant, Trust};
use trioxide_common::NodeRef;

use super::{
    Decisions, Position,
    arrays::Kept,
    track_position,
    util::{Query, constant_expr},
};
use crate::{Pass, PassContext};

/// PLAN §4 pass 10 (`"hidden-class"`).
pub struct HiddenClass;

/// What every object inherits from `Object.prototype`, besides what a class adds.
const OBJECT_PROTOTYPE: [&str; 12] = [
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
    "__proto__",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
];

impl Pass for HiddenClass {
    fn name(&self) -> &'static str {
        "hidden-class"
    }

    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
        let query = Query {
            facts: ctx.facts,
            module: ctx.module,
        };
        let mut decisions = Decisions::new(self.name());
        let classes = eligible_classes(program, query, &mut decisions);
        if classes.is_empty() {
            return decisions.flush(ctx.log);
        }
        let mut visitor = Visitor {
            query,
            kept: Kept::new(program, ctx.keep, query),
            position: Position::default(),
            decisions,
            classes,
        };
        ctx.traverse(&mut visitor, program, ());
        visitor.decisions.flush(ctx.log)
    }
}

/// What the pass may do to one class.
struct Plan {
    /// Names on the instances' prototype chain.
    inherited: FxHashSet<String>,
    /// Properties to declare as fields, in order.
    declare: Vec<String>,
}

/// The classes of `program` whose instances' keys nothing observes, with their plans.
fn eligible_classes(
    program: &Program<'_>,
    query: Query<'_>,
    decisions: &mut Decisions,
) -> FxHashMap<NodeRef, Plan> {
    struct Find<'q, 'f, 'd> {
        query: Query<'f>,
        decisions: &'d mut Decisions,
        found: FxHashMap<NodeRef, Plan>,
        _marker: std::marker::PhantomData<&'q ()>,
    }
    impl<'a> Visit<'a> for Find<'_, '_, '_> {
        fn visit_class(&mut self, class: &Class<'a>) {
            if let Some(id) = self.query.node(class.node_id.get())
                && let Some(plan) = plan(class, id, self.query, self.decisions)
            {
                self.found.insert(id, plan);
            }
            walk::walk_class(self, class);
        }
    }
    let mut find = Find {
        query,
        decisions,
        found: FxHashMap::default(),
        _marker: std::marker::PhantomData,
    };
    find.visit_program(program);
    find.found
}

/// The plan for `class`, if the pass may touch it.
fn plan(
    class: &Class<'_>,
    id: NodeRef,
    query: Query<'_>,
    decisions: &mut Decisions,
) -> Option<Plan> {
    let shape = query.facts.class_shape(id);
    let unstable = !shape.value.is_stable();
    let span = class.id.as_ref().map_or(class.span, |id| id.span);
    let decline = |decisions: &mut Decisions, reason: &str| {
        // Only classes the pass would change are worth a word.
        if unstable {
            decisions.skipped(span, reason.to_string());
        }
    };
    if query.dead(class.node_id.get()) {
        return None;
    }
    if shape.trust != Trust::Proven {
        decline(
            decisions,
            "the class's instances may reach code the analysis doesn't see",
        );
        return None;
    }
    if shape.value.dynamic {
        decline(
            decisions,
            "instance properties are written with computed keys",
        );
        return None;
    }
    let observed = query.facts.instance_keys_observed(id);
    if observed.value || observed.trust != Trust::Proven {
        decline(
            decisions,
            "code may observe its instances' keys (`Object.keys`, `for…in`, `in`, spread, …), \
             or the class is extended",
        );
        return None;
    }
    if class.heritage.is_some() {
        decline(decisions, "the class extends another");
        return None;
    }
    let mut inherited: FxHashSet<String> = OBJECT_PROTOTYPE
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let mut fields: FxHashSet<String> = FxHashSet::default();
    let mut constructor = None;
    for element in &class.body.body {
        match element {
            ClassElement::MethodDefinition(method) if !method.r#static => {
                if method.computed {
                    decline(decisions, "the class has a computed method name");
                    return None;
                }
                if method.kind == MethodDefinitionKind::Constructor {
                    constructor = Some(method);
                } else if let Some(name) = method.key.static_name() {
                    inherited.insert(name.to_string());
                }
            }
            // `declare` and abstract fields don't exist at run time.
            ClassElement::PropertyDefinition(property)
                if !property.r#static
                    && !property.declare
                    && property.r#type == PropertyDefinitionType::PropertyDefinition =>
            {
                if property.computed {
                    decline(decisions, "the class has a computed field name");
                    return None;
                }
                if let Some(name) = property.key.static_name() {
                    fields.insert(name.to_string());
                }
            }
            ClassElement::AccessorProperty(_) => {
                decline(decisions, "the class has an auto-accessor");
                return None;
            }
            _ => {}
        }
    }
    // What the constructor assigns before anything else runs, and later.
    let mut declare: Vec<String> = Vec::new();
    if let Some(constructor) = constructor {
        for parameter in &constructor.value.params.items {
            if parameter.has_modifier() {
                parameter.pattern.all_binding_identifiers(&mut |ident| {
                    fields.insert(ident.name.to_string());
                    true
                });
            }
        }
        if let Some(body) = &constructor.value.body {
            let mut leading = body.statements.iter();
            for statement in leading.by_ref() {
                match this_assignment(statement) {
                    Some(name) => {
                        fields.insert(name.to_string());
                    }
                    None => break,
                }
            }
            let mut later = ThisAssignments(Vec::new());
            for statement in &body.statements {
                if this_assignment(statement).is_some_and(|name| fields.contains(name)) {
                    continue;
                }
                later.visit_statement(statement);
            }
            for name in later.0 {
                if !declare.contains(&name) {
                    declare.push(name);
                }
            }
        }
    }
    for name in &shape.value.added_later {
        if !declare.contains(name) {
            declare.push(name.clone());
        }
    }
    declare.retain(|name| !fields.contains(name));
    let inherited_names: Vec<&String> = declare
        .iter()
        .filter(|name| inherited.contains(*name))
        .collect();
    if !inherited_names.is_empty() {
        decisions.skipped(
            span,
            format!(
                "`{}` is on the prototype chain, so declaring it would hide it",
                inherited_names[0]
            ),
        );
    }
    declare.retain(|name| !inherited.contains(name));
    Some(Plan { inherited, declare })
}

/// The property a statement `this.name = …;` assigns.
fn this_assignment<'s>(statement: &'s Statement<'_>) -> Option<&'s str> {
    let Statement::ExpressionStatement(statement) = statement else {
        return None;
    };
    let Expression::AssignmentExpression(assignment) = &statement.expression else {
        return None;
    };
    match &assignment.left {
        AssignmentTarget::StaticMemberExpression(member)
            if matches!(member.object, Expression::ThisExpression(_)) =>
        {
            Some(member.property.name.as_str())
        }
        _ => None,
    }
}

/// The properties `this.name = …` assigns, in order, looking through arrows (whose `this` is
/// the same) but not functions or classes.
struct ThisAssignments(Vec<String>);

impl<'a> Visit<'a> for ThisAssignments {
    fn visit_assignment_expression(&mut self, assignment: &oxc_ast::ast::AssignmentExpression<'a>) {
        if let AssignmentTarget::StaticMemberExpression(member) = &assignment.left
            && matches!(member.object, Expression::ThisExpression(_))
        {
            let name = member.property.name.to_string();
            if !self.0.contains(&name) {
                self.0.push(name);
            }
        }
        walk::walk_assignment_expression(self, assignment);
    }

    fn visit_function(&mut self, _: &oxc_ast::ast::Function<'a>, _: ScopeFlags) {}

    fn visit_class(&mut self, _: &Class<'a>) {}
}

struct Visitor<'f> {
    query: Query<'f>,
    kept: Kept,
    position: Position,
    decisions: Decisions,
    classes: FxHashMap<NodeRef, Plan>,
}

impl<'a> Traverse<'a, ()> for Visitor<'_> {
    track_position!(after_statement);

    fn exit_class(&mut self, class: &mut Class<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        if !self.position.live() || self.kept.contains(class.span) {
            return;
        }
        let Some(id) = self.query.node(class.node_id.get()) else {
            return;
        };
        let Some(plan) = self.classes.get(&id) else {
            return;
        };
        if plan.declare.is_empty() {
            return;
        }
        let at = class
            .body
            .body
            .iter()
            .rposition(|element| {
                matches!(
                    element,
                    ClassElement::PropertyDefinition(property)
                        if !property.r#static
                            && !property.declare
                            && property.r#type == PropertyDefinitionType::PropertyDefinition
                )
            })
            .map_or(0, |last| last + 1);
        let span = class.id.as_ref().map_or(class.span, |id| id.span);
        for (offset, name) in plan.declare.iter().enumerate() {
            let key = PropertyKey::StaticIdentifier(oxc_allocator::ArenaBox::new_in(
                IdentifierName::new(SPAN, oxc_ast::ast::Ident::from_str_in(name, ctx), ctx),
                ctx,
            ));
            let value = constant_expr(&Constant::Undefined, SPAN, ctx);
            let field = ClassElement::new_property_definition(
                SPAN,
                PropertyDefinitionType::PropertyDefinition,
                ArenaVec::new_in(&ctx.ast),
                key,
                None,
                Some(value),
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                None,
                ctx,
            );
            class.body.body.insert(at + offset, field);
        }
        self.decisions.applied(span);
    }
}

impl<'a> Visitor<'_> {
    fn after_statement(&mut self, statement: &mut Statement<'a>, ctx: &mut TraverseCtx<'a, ()>) {
        if !self.position.live() || self.query.dead(statement.node_id()) {
            return;
        }
        let Statement::ExpressionStatement(expression_statement) = statement else {
            return;
        };
        if self.kept.contains(expression_statement.span) {
            return;
        }
        let Expression::UnaryExpression(unary) = &mut expression_statement.expression else {
            return;
        };
        if unary.operator != UnaryOperator::Delete {
            return;
        }
        let Expression::StaticMemberExpression(member) = unary.argument.without_parentheses()
        else {
            return;
        };
        if member.optional {
            return;
        }
        let span = unary.span;
        let name = member.property.name.as_str();
        let sites = self
            .query
            .node(member.object.get_inner_expression().node_id())
            .map(|node| self.query.facts.points_to(node));
        let Some(sites) = sites.and_then(|fact| fact.value) else {
            self.decisions
                .skipped(span, "the objects it may delete from are unknown");
            return;
        };
        let classes: Option<Vec<NodeRef>> = sites
            .iter()
            .map(
                |site| match self.query.facts.allocation(*site).map(|a| a.kind) {
                    Some(AllocKind::Instance(class)) => Some(class),
                    _ => None,
                },
            )
            .collect();
        let Some(classes) = classes.filter(|classes| !classes.is_empty()) else {
            self.decisions.skipped(
                span,
                "the object may be something other than a class instance",
            );
            return;
        };
        for class in &classes {
            match self.classes.get(class) {
                None => {
                    self.decisions.skipped(
                        span,
                        "the class isn't one whose instances' keys are proven unobserved",
                    );
                    return;
                }
                Some(plan) if plan.inherited.contains(name) => {
                    self.decisions.skipped(
                        span,
                        "the property is on the prototype chain, which a delete uncovers",
                    );
                    return;
                }
                Some(_) => {}
            }
        }
        let Expression::UnaryExpression(unary) = expression_statement.expression.take_in(&ctx.ast)
        else {
            unreachable!("matched above")
        };
        let mut argument = unary.unbox().argument;
        while let Expression::ParenthesizedExpression(paren) = argument {
            argument = paren.unbox().expression;
        }
        let Expression::StaticMemberExpression(member) = argument else {
            unreachable!("matched above")
        };
        let value = constant_expr(&Constant::Undefined, SPAN, ctx);
        expression_statement.expression = Expression::new_assignment_expression(
            span,
            AssignmentOperator::Assign,
            AssignmentTarget::StaticMemberExpression(member),
            value,
            ctx,
        );
        self.decisions.applied(span);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_assignments() {
        let allocator = oxc_allocator::Allocator::default();
        let program = oxc_parser::Parser::new(
            &allocator,
            "this.a = 1; if (x) { this.b = 2; } f(() => { this.c = 3; }); g(function () { this.d = 4; });",
            oxc_span::SourceType::ts(),
        )
        .parse()
        .program;
        assert_eq!(this_assignment(&program.body[0]), Some("a"));
        let mut found = ThisAssignments(Vec::new());
        for statement in &program.body {
            found.visit_statement(statement);
        }
        assert_eq!(found.0, ["a", "b", "c"]);
    }
}
