//! A human-readable dump of the facts for one module (`trioxide --emit=facts`).

use std::fmt::Write as _;

use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::{GetSpan, Span};
use trioxide_common::{ModuleId, NodeId, NodeRef};
use trioxide_types::{NumberType, Type};

use crate::{
    ModuleInput,
    facts::{AllocKind, Effects, Escape, Fact, Facts, Trust},
};

/// Renders every fact about `module`: functions with their effects and callees, classes
/// with their shapes, then the type of each identifier, member access, call and operator in
/// source order, with `@trust` levels.
pub fn render(facts: &Facts, module: &ModuleInput<'_>, all: &[ModuleInput<'_>]) -> String {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, module.source, module.source_type).parse();
    let program = allocator.alloc(parsed.program);
    let semantic = SemanticBuilder::new()
        .with_enum_eval(true)
        .with_build_nodes(true)
        .build(program)
        .semantic;
    let nodes = semantic.nodes();
    let names = Names::new(module.id, module.source, &semantic, all);
    let lines = LineIndex::new(module.source);
    let at = |node: NodeId| NodeRef {
        module: module.id,
        node,
    };
    let mut out = String::new();

    let mut units: Vec<NodeId> = nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind(),
                AstKind::Function(_)
                    | AstKind::ArrowFunctionExpression(_)
                    | AstKind::StaticBlock(_)
                    | AstKind::Program(_)
            ) || matches!(node.kind(), AstKind::ClassBody(_))
        })
        .map(oxc_semantic::AstNode::id)
        .filter(|&id| facts.function(at(id)).is_some())
        .collect();
    units.sort_by_key(|&id| nodes.kind(id).span().start);
    if !units.is_empty() {
        out.push_str("functions:\n");
    }
    for unit in units {
        let info = facts.function(at(unit)).expect("filtered");
        let name = info.name.clone().unwrap_or_else(|| match nodes.kind(unit) {
            AstKind::Program(_) => "<module>".into(),
            _ => "<anonymous>".into(),
        });
        let (line, column) = lines.position(info.span.start);
        write!(out, "  {name} ({line}:{column})").unwrap();
        if info.open {
            out.push_str(" open");
        }
        match facts.effect_summaries(at(unit)) {
            Some((proven, declared)) => {
                write!(out, ": {}", effects(proven)).unwrap();
                if declared != proven {
                    write!(out, " (declared: {})", effects(declared)).unwrap();
                }
            }
            None => out.push_str(": no summary"),
        }
        out.push('\n');
        let mut callees: Vec<String> = facts
            .call_graph()
            .edges_from(at(unit))
            .map(|edge| {
                let (line, column) = lines.position(names.span(edge.site).start);
                let trust = if edge.trust == Trust::Proven {
                    ""
                } else {
                    " @declared"
                };
                format!(
                    "{line}:{column} -> {}{trust}",
                    names.unit(facts, edge.callee)
                )
            })
            .collect();
        callees.dedup();
        for callee in callees {
            writeln!(out, "    calls {callee}").unwrap();
        }
    }

    let mut classes: Vec<(NodeId, Span)> = nodes
        .iter()
        .filter_map(|node| match node.kind() {
            AstKind::Class(class) => Some((node.id(), class.span)),
            _ => None,
        })
        .collect();
    classes.sort_by_key(|(_, span)| span.start);
    if !classes.is_empty() {
        out.push_str("classes:\n");
    }
    for (class, span) in classes {
        let shape = facts.class_shape(at(class));
        let (line, column) = lines.position(span.start);
        write!(
            out,
            "  {} ({line}:{column}): fields [{}]",
            names.class(at(class)),
            shape.value.fields.join(", ")
        )
        .unwrap();
        if shape.trust == Trust::Unknown {
            out.push_str(" shape unknown");
        } else if shape.value.is_stable() {
            out.push_str(" stable");
        } else {
            if !shape.value.added_later.is_empty() {
                write!(out, " added later [{}]", shape.value.added_later.join(", ")).unwrap();
            }
            if shape.value.deletes {
                out.push_str(" deletes");
            }
            if shape.value.dynamic {
                out.push_str(" dynamic");
            }
        }
        out.push('\n');
    }

    let mut rows: Vec<(u32, u32, String)> = Vec::new();
    for node in nodes.iter() {
        let Some(label) = label(node.kind()) else {
            continue;
        };
        let id = node.id();
        let span = node.kind().span();
        let fact = facts.ty(at(id));
        let mut row = format!(
            "{label} `{}`: {}",
            snippet(span.source_text(module.source)),
            names.fact(&fact)
        );
        if facts.in_bounds(at(id)).trust != Trust::Unknown {
            write!(
                out_of(&mut row),
                " in-bounds{}",
                trust_suffix(facts.in_bounds(at(id)).trust)
            )
            .unwrap();
        }
        if let Some(allocation) = facts.allocation(at(id)) {
            let escape = match allocation.escape.value {
                Escape::None => "none",
                Escape::Known => "known",
                Escape::Unknown => "unknown",
            };
            write!(out_of(&mut row), " escape={escape}").unwrap();
            if matches!(allocation.kind, AllocKind::Object | AllocKind::Array)
                && allocation.stable_shape.value
            {
                row.push_str(" stable-shape");
            }
        }
        rows.push((span.start, span.end, row));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    if !rows.is_empty() {
        out.push_str("nodes:\n");
    }
    for (start, _, row) in rows {
        let (line, column) = lines.position(start);
        writeln!(out, "  {line}:{column} {row}").unwrap();
    }
    out
}

fn out_of(row: &mut String) -> &mut String {
    row
}

/// Which nodes get a row, and what to call them.
fn label(kind: AstKind<'_>) -> Option<&'static str> {
    Some(match kind {
        AstKind::IdentifierReference(_) => "ref",
        AstKind::BindingIdentifier(_) => "bind",
        AstKind::StaticMemberExpression(_)
        | AstKind::ComputedMemberExpression(_)
        | AstKind::PrivateFieldExpression(_) => "member",
        AstKind::CallExpression(_) => "call",
        AstKind::NewExpression(_) => "new",
        AstKind::BinaryExpression(_) | AstKind::LogicalExpression(_) => "binary",
        AstKind::UnaryExpression(_) | AstKind::UpdateExpression(_) => "unary",
        AstKind::AssignmentExpression(_) => "assign",
        AstKind::ConditionalExpression(_) => "cond",
        AstKind::ThisExpression(_) => "this",
        AstKind::ArrayExpression(_) => "array",
        AstKind::ObjectExpression(_) => "object",
        AstKind::TSAsExpression(_) | AstKind::TSTypeAssertion(_) => "cast",
        AstKind::TSNonNullExpression(_) => "non-null",
        AstKind::AwaitExpression(_) => "await",
        _ => return None,
    })
}

fn snippet(text: &str) -> String {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > 40 {
        let cut: String = text.chars().take(37).collect();
        format!("{cut}...")
    } else {
        text
    }
}

fn trust_suffix(trust: Trust) -> &'static str {
    match trust {
        Trust::Proven => "@proven",
        Trust::Declared => "@declared",
        Trust::Unknown => "@unknown",
    }
}

fn effects(effects: Effects) -> String {
    if effects == Effects::PURE {
        return "pure".into();
    }
    let mut parts = Vec::new();
    for (flag, name) in [
        (effects.reads, "reads"),
        (effects.writes, "writes"),
        (effects.allocates, "allocates"),
        (effects.throws, "throws"),
        (effects.calls_unknown, "calls-unknown"),
    ] {
        if flag {
            parts.push(name);
        }
    }
    parts.join(" ")
}

/// Renders a number type: `number`, `5`, or `number[0, 10] int nonNaN nonNegZero`.
pub fn number(number: &NumberType) -> String {
    if let Some(constant) = number.as_constant() {
        return format!("{constant}");
    }
    let mut out = String::from("number");
    if !number.range.is_full() {
        let bound = |value: f64| {
            if value == f64::INFINITY {
                "inf".to_string()
            } else if value == f64::NEG_INFINITY {
                "-inf".to_string()
            } else {
                format!("{value}")
            }
        };
        write!(
            out,
            "[{}, {}]",
            bound(number.range.min),
            bound(number.range.max)
        )
        .unwrap();
    }
    for (flag, name) in [
        (number.int, "int"),
        (number.non_nan, "nonNaN"),
        (number.non_neg_zero, "nonNegZero"),
    ] {
        if flag {
            out.push(' ');
            out.push_str(name);
        }
    }
    out
}

/// Names of classes and functions, for rendering types and edges.
struct Names<'s> {
    module: ModuleId,
    spans: rustc_hash::FxHashMap<NodeId, Span>,
    classes: rustc_hash::FxHashMap<NodeRef, String>,
    _source: &'s str,
}

impl<'s> Names<'s> {
    fn new(
        module: ModuleId,
        source: &'s str,
        semantic: &oxc_semantic::Semantic<'_>,
        all: &[ModuleInput<'_>],
    ) -> Self {
        let mut spans = rustc_hash::FxHashMap::default();
        let mut classes = rustc_hash::FxHashMap::default();
        for node in semantic.nodes().iter() {
            spans.insert(node.id(), node.kind().span());
            if let AstKind::Class(class) = node.kind() {
                let name = class
                    .id
                    .as_ref()
                    .map_or_else(|| "<class>".to_string(), |id| id.name.to_string());
                classes.insert(
                    NodeRef {
                        module,
                        node: node.id(),
                    },
                    name,
                );
            }
        }
        // Classes of other modules, by parsing them too (only their names are needed).
        for other in all.iter().filter(|other| other.id != module) {
            let allocator = Allocator::default();
            let parsed = Parser::new(&allocator, other.source, other.source_type).parse();
            let program = allocator.alloc(parsed.program);
            let semantic = SemanticBuilder::new()
                .with_enum_eval(true)
                .with_build_nodes(true)
                .build(program)
                .semantic;
            for node in semantic.nodes().iter() {
                if let AstKind::Class(class) = node.kind() {
                    let name = class
                        .id
                        .as_ref()
                        .map_or_else(|| "<class>".to_string(), |id| id.name.to_string());
                    classes.insert(
                        NodeRef {
                            module: other.id,
                            node: node.id(),
                        },
                        name,
                    );
                }
            }
        }
        Self {
            module,
            spans,
            classes,
            _source: source,
        }
    }

    fn span(&self, node: NodeRef) -> Span {
        if node.module == self.module {
            self.spans.get(&node.node).copied().unwrap_or_default()
        } else {
            Span::default()
        }
    }

    fn class(&self, class: NodeRef) -> String {
        self.classes
            .get(&class)
            .cloned()
            .unwrap_or_else(|| "<class>".into())
    }

    fn unit(&self, facts: &Facts, unit: NodeRef) -> String {
        let name = facts
            .function(unit)
            .and_then(|info| info.name.clone())
            .unwrap_or_else(|| "<anonymous>".into());
        if unit.module == self.module {
            name
        } else {
            format!("{name} (m{})", unit.module.index())
        }
    }

    fn fact(&self, fact: &Fact<&Type>) -> String {
        if fact.trust == Trust::Unknown {
            return "unknown".into();
        }
        format!("{}{}", self.ty(fact.value), trust_suffix(fact.trust))
    }

    fn ty(&self, ty: &Type) -> String {
        match ty {
            Type::Unknown => "unknown".into(),
            Type::Never => "never".into(),
            Type::Number(n) => number(n),
            Type::Boolean => "boolean".into(),
            Type::String => "string".into(),
            Type::BigInt => "bigint".into(),
            Type::Symbol => "symbol".into(),
            Type::Null => "null".into(),
            Type::Undefined => "undefined".into(),
            Type::Array(element) => format!("{}[]", self.wrapped(element)),
            Type::TypedArray(kind) => kind.constructor_name().into(),
            Type::Tuple(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(|item| self.ty(item))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Type::Object(shape) => {
                let body = shape
                    .properties
                    .iter()
                    .map(|(key, ty)| format!("{key}: {}", self.ty(ty)))
                    .collect::<Vec<_>>()
                    .join(", ");
                if shape.exact {
                    format!("{{| {body} |}}")
                } else if body.is_empty() {
                    "object".into()
                } else {
                    format!("{{ {body} }}")
                }
            }
            Type::Instance(class) => self.class(*class),
            Type::Function(_, _) => "function".into(),
            Type::Union(members) => members
                .iter()
                .map(|member| self.wrapped(member))
                .collect::<Vec<_>>()
                .join(" | "),
        }
    }

    fn wrapped(&self, ty: &Type) -> String {
        let text = self.ty(ty);
        if matches!(ty, Type::Union(_))
            || (matches!(ty, Type::Number(n) if n.as_constant().is_none() && (!n.range.is_full() || n.int || n.non_nan)))
        {
            format!("({text})")
        } else {
            text
        }
    }
}

/// Byte offset to 1-based line and column (in characters).
struct LineIndex {
    starts: Vec<u32>,
    source: String,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut starts = vec![0];
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(u32::try_from(index + 1).unwrap_or(u32::MAX));
            }
        }
        Self {
            starts,
            source: source.to_string(),
        }
    }

    fn position(&self, offset: u32) -> (usize, usize) {
        let line = self
            .starts
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);
        let start = self.starts[line] as usize;
        let end = (offset as usize).min(self.source.len());
        let column = self
            .source
            .get(start..end)
            .map_or(0, |text| text.chars().count());
        (line + 1, column + 1)
    }
}
