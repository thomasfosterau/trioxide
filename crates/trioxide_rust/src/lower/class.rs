//! Classes used inside an island: immutable plain-data values.

use oxc_ast::ast::{
    AssignmentOperator, AssignmentTarget, ClassElement, Expression, Function, MethodDefinitionKind,
    PropertyKey, Statement,
};
use oxc_span::{GetSpan, Span};
use rustc_hash::FxHashMap;
use trioxide_analysis::Facts;
use trioxide_types::{ClassId, FunctionId};

use crate::world::World;

/// What the backend needs to know about a class: its fields, how they are initialised, and
/// its methods.
///
/// Instances are compiled as Rust values (`Copy` structs), which is exact only because the
/// backend never lets them change after construction or be compared by identity.
pub(crate) struct ClassModel<'a> {
    /// Field names in definition order.
    pub fields: Vec<String>,
    /// Initialisers in evaluation order: field declarations, then constructor assignments.
    pub inits: Vec<(usize, &'a Expression<'a>)>,
    /// The constructor's id (the class's own id when it has none).
    pub ctor: FunctionId,
    /// The constructor, if written.
    pub ctor_func: Option<&'a Function<'a>>,
    /// Instance methods by name.
    pub methods: FxHashMap<String, FunctionId>,
}

impl<'a> ClassModel<'a> {
    /// Builds the model, or says why the class can't be compiled.
    pub fn build(
        world: &World<'a>,
        facts: &Facts,
        class_id: ClassId,
    ) -> Result<Self, (Span, String)> {
        let class = world.classes[&class_id].class;
        let module = world.module(class_id.module).expect("class in a module");
        if let Some(heritage) = &class.heritage {
            return Err((heritage.span(), "extends another class".into()));
        }
        if let Some(decorator) = class.decorators.first() {
            return Err((decorator.span, "has decorators".into()));
        }
        if let Some(id) = &class.id
            && let Some(symbol) = id.symbol_id.get()
            && module.semantic.scoping().symbol_is_mutated(symbol)
        {
            return Err((id.span, "the class binding is reassigned".into()));
        }
        let shape = facts.class_shape(class_id);
        if !facts.is_usable(shape.trust) || !shape.value.is_stable() {
            return Err((
                class.id.as_ref().map_or(class.span, |id| id.span),
                "can't prove its instances keep one shape (properties added, deleted or \
                 written with computed keys, or instances escape)"
                    .into(),
            ));
        }
        let mut model = Self {
            fields: Vec::new(),
            inits: Vec::new(),
            ctor: class_id,
            ctor_func: None,
            methods: FxHashMap::default(),
        };
        let mut declared_without_value = Vec::new();
        for element in &class.body.body {
            match element {
                ClassElement::PropertyDefinition(property) => {
                    if property.r#static {
                        return Err((property.span, "has a static field".into()));
                    }
                    let Some(name) = static_key(&property.key, property.computed) else {
                        return Err((
                            property.key.span(),
                            "has a computed or private field".into(),
                        ));
                    };
                    if property.declare {
                        model.fields.push(name);
                        declared_without_value.push(model.fields.len() - 1);
                        continue;
                    }
                    model.fields.push(name);
                    let index = model.fields.len() - 1;
                    match &property.value {
                        Some(value) => model.inits.push((index, value)),
                        None => declared_without_value.push(index),
                    }
                }
                ClassElement::MethodDefinition(method) => {
                    let function_id =
                        module.node(oxc_syntax::node::GetNodeId::node_id(&*method.value));
                    match method.kind {
                        MethodDefinitionKind::Constructor => {
                            model.ctor = function_id;
                            model.ctor_func = Some(&method.value);
                        }
                        MethodDefinitionKind::Method if !method.r#static && !method.computed => {
                            let Some(name) = static_key(&method.key, false) else {
                                return Err((method.key.span(), "has a private method".into()));
                            };
                            model.methods.insert(name, function_id);
                        }
                        _ => {
                            return Err((
                                method.span,
                                "has a static member, getter, setter or computed method".into(),
                            ));
                        }
                    }
                }
                ClassElement::TSIndexSignature(_) => {}
                other => return Err((other.span(), "has a static block or accessor".into())),
            }
        }
        if let Some(ctor) = model.ctor_func {
            for param in &ctor.params.items {
                if param.accessibility.is_some() || param.readonly || param.r#override {
                    return Err((param.span, "has a parameter property".into()));
                }
            }
            let body = ctor.body.as_ref().expect("constructors have bodies");
            for statement in &body.statements {
                let Statement::ExpressionStatement(statement) = statement else {
                    return Err((
                        statement.span(),
                        "its constructor does more than assign fields (`this.x = …;`)".into(),
                    ));
                };
                let Expression::AssignmentExpression(assignment) = &statement.expression else {
                    return Err((
                        statement.span,
                        "its constructor does more than assign fields (`this.x = …;`)".into(),
                    ));
                };
                let field = match &assignment.left {
                    AssignmentTarget::StaticMemberExpression(member)
                        if assignment.operator == AssignmentOperator::Assign
                            && matches!(member.object, Expression::ThisExpression(_)) =>
                    {
                        member.property.name.to_string()
                    }
                    _ => {
                        return Err((
                            assignment.span,
                            "its constructor does more than assign fields (`this.x = …;`)".into(),
                        ));
                    }
                };
                let index = if let Some(index) = model.fields.iter().position(|f| *f == field) {
                    index
                } else {
                    model.fields.push(field);
                    model.fields.len() - 1
                };
                if model.inits.iter().any(|(i, _)| *i == index) {
                    return Err((assignment.span, "assigns a field twice".into()));
                }
                model.inits.push((index, &assignment.right));
            }
        }
        for index in declared_without_value {
            if !model.inits.iter().any(|(i, _)| *i == index) {
                return Err((
                    class.span,
                    format!("its field `{}` may be undefined", model.fields[index]),
                ));
            }
        }
        if model.fields.is_empty() {
            return Err((class.span, "has no fields".into()));
        }
        Ok(model)
    }
}

fn static_key(key: &PropertyKey<'_>, computed: bool) -> Option<String> {
    if computed {
        return None;
    }
    match key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
        _ => None,
    }
}
