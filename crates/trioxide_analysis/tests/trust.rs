//! Trust expectations written into the analysis fixtures.
//!
//! A block comment `/*@<trust> [<kind>] [<flag>…]*/` right after an expression (or binding)
//! states what the analysis must report for the largest fact-bearing node ending there:
//! `proven`, `declared` or `unknown`, optionally a kind (`number`, `string`, `boolean`,
//! `undefined`, `null`, `array`, `typed-array`, `object`, `instance`, `function`) and flags
//! (`int`, `nonNaN`, `exact`, `in-bounds`, `escape=none|known|unknown`). Every adversarial
//! fixture carries such annotations; this test checks all of them.

mod common;

use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::GetSpan;
use trioxide_analysis::{Escape, Facts, Trust};
use trioxide_common::NodeRef;
use trioxide_types::Type;

struct Expectation {
    line: usize,
    text: String,
    comment_start: u32,
}

fn expectations(source: &str) -> Vec<Expectation> {
    let mut found = Vec::new();
    let mut rest = source;
    let mut offset = 0usize;
    while let Some(at) = rest.find("/*@") {
        let start = offset + at;
        let Some(end) = source[start..].find("*/") else { break };
        let text = source[start + 3..start + end].trim().to_string();
        found.push(Expectation {
            line: source[..start].lines().count().max(1),
            text,
            comment_start: u32::try_from(start).unwrap(),
        });
        offset = start + end + 2;
        rest = &source[offset..];
    }
    found
}

fn fact_bearing(kind: AstKind<'_>) -> bool {
    matches!(
        kind,
        AstKind::IdentifierReference(_)
            | AstKind::BindingIdentifier(_)
            | AstKind::StaticMemberExpression(_)
            | AstKind::ComputedMemberExpression(_)
            | AstKind::PrivateFieldExpression(_)
            | AstKind::CallExpression(_)
            | AstKind::NewExpression(_)
            | AstKind::BinaryExpression(_)
            | AstKind::LogicalExpression(_)
            | AstKind::UnaryExpression(_)
            | AstKind::UpdateExpression(_)
            | AstKind::AssignmentExpression(_)
            | AstKind::ConditionalExpression(_)
            | AstKind::ThisExpression(_)
            | AstKind::ArrayExpression(_)
            | AstKind::ObjectExpression(_)
            | AstKind::TSAsExpression(_)
            | AstKind::TSTypeAssertion(_)
            | AstKind::TSNonNullExpression(_)
            | AstKind::ParenthesizedExpression(_)
            | AstKind::AwaitExpression(_)
            | AstKind::ArrowFunctionExpression(_)
            | AstKind::Function(_)
    )
}

fn kind_matches(kind: &str, ty: &Type) -> bool {
    match kind {
        "number" => matches!(ty, Type::Number(_)),
        "string" => *ty == Type::String,
        "boolean" => *ty == Type::Boolean,
        "undefined" => *ty == Type::Undefined,
        "null" => *ty == Type::Null,
        "bigint" => *ty == Type::BigInt,
        "array" => matches!(ty, Type::Array(_) | Type::Tuple(_)),
        "typed-array" => matches!(ty, Type::TypedArray(_)),
        "object" => matches!(ty, Type::Object(_)),
        "instance" => matches!(ty, Type::Instance(_)),
        "function" => matches!(ty, Type::Function(..)),
        "union" => matches!(ty, Type::Union(_)),
        "never" => *ty == Type::Never,
        _ => panic!("unknown kind `{kind}` in an expectation"),
    }
}

fn check(facts: &Facts, node: NodeRef, expectation: &str) -> Result<(), String> {
    let fact = facts.ty(node);
    let mut words = expectation.split_whitespace();
    let trust = match words.next() {
        Some("proven") => Trust::Proven,
        Some("declared") => Trust::Declared,
        Some("unknown") => Trust::Unknown,
        other => return Err(format!("bad trust {other:?}")),
    };
    if fact.trust != trust {
        return Err(format!("expected {trust:?}, got {:?} ({:?})", fact.trust, fact.value));
    }
    for word in words {
        let ok = match word {
            "int" => matches!(fact.value, Type::Number(n) if n.int),
            "nonNaN" => matches!(fact.value, Type::Number(n) if n.non_nan),
            "exact" => matches!(fact.value, Type::Object(shape) if shape.exact),
            "in-bounds" => facts.in_bounds(node).trust >= Trust::Declared,
            "escape=none" => facts.escapes(node).value == Escape::None,
            "escape=known" => facts.escapes(node).value == Escape::Known,
            "escape=unknown" => facts.escapes(node).value == Escape::Unknown,
            kind => kind_matches(kind, fact.value),
        };
        if !ok {
            return Err(format!("`{word}` does not hold for {:?} @{:?}", fact.value, fact.trust));
        }
    }
    Ok(())
}

#[test]
fn expectations_hold() {
    let mut failures = Vec::new();
    let mut count = 0;
    for entry in common::fixtures() {
        let loaded = common::load(&entry);
        for module in loaded.graph.modules() {
            let found = expectations(&module.source);
            if found.is_empty() {
                continue;
            }
            let allocator = Allocator::default();
            let parsed = Parser::new(&allocator, &module.source, module.source_type).parse();
            let program = allocator.alloc(parsed.program);
            let semantic = SemanticBuilder::new().with_enum_eval(true).with_build_nodes(true).build(program).semantic;
            for expectation in found {
                count += 1;
                let before = module.source[..expectation.comment_start as usize].trim_end();
                let end = u32::try_from(before.len()).unwrap();
                let node = semantic
                    .nodes()
                    .iter()
                    .filter(|node| fact_bearing(node.kind()) && node.kind().span().end == end)
                    .max_by_key(|node| node.kind().span().size());
                let place = format!("{}:{}", module.path.display(), expectation.line);
                let Some(node) = node else {
                    failures.push(format!("{place}: no expression ends before `/*@{}*/`", expectation.text));
                    continue;
                };
                let node_ref = NodeRef { module: module.id, node: node.id() };
                if let Err(error) = check(&loaded.analysis.facts, node_ref, &expectation.text) {
                    let text = node.kind().span().source_text(&module.source);
                    failures.push(format!("{place}: `{text}` /*@{}*/: {error}", expectation.text));
                }
            }
        }
    }
    assert!(count > 0, "no expectations found");
    assert!(failures.is_empty(), "{} of {count} expectations failed:\n{}", failures.len(), failures.join("\n"));
}
