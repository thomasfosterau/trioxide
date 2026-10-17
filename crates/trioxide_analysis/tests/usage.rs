//! The usage queries: call-site completeness, expression and loop effects, points-to sets,
//! callers and reachability, on the `usage-facts` fixture.

mod common;

use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::GetSpan;
use trioxide_analysis::Trust;
use trioxide_common::NodeRef;

/// The node of `kind` whose source text is `text`, with the analysis.
struct Fixture {
    loaded: common::Loaded,
    source: String,
}

impl Fixture {
    fn load() -> Self {
        let entry = common::fixtures()
            .into_iter()
            .find(|entry| entry.parent().unwrap().ends_with("usage-facts"))
            .unwrap();
        let loaded = common::load(&entry);
        let source = loaded.graph.modules()[0].source.to_string();
        Self { loaded, source }
    }

    /// The largest node whose text is exactly `text` and that `matches`.
    fn node(&self, text: &str, matches: impl Fn(AstKind<'_>) -> bool) -> NodeRef {
        let module = &self.loaded.graph.modules()[0];
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, &self.source, module.source_type).parse();
        let program = allocator.alloc(parsed.program);
        let semantic = SemanticBuilder::new()
            .with_build_nodes(true)
            .build(program)
            .semantic;
        let node = semantic
            .nodes()
            .iter()
            .filter(|node| {
                matches(node.kind()) && node.kind().span().source_text(&self.source) == text
            })
            .max_by_key(|node| node.kind().span().size())
            .unwrap_or_else(|| panic!("no node `{text}`"));
        NodeRef {
            module: module.id,
            node: node.id(),
        }
    }
}

fn call(kind: AstKind<'_>) -> bool {
    matches!(kind, AstKind::CallExpression(_))
}

#[test]
fn usage_queries() {
    let fixture = Fixture::load();
    let facts = &fixture.loaded.analysis.facts;

    // Call sites.
    let square_call = fixture.node("square(3)", call);
    let resolution = facts.site_resolution(square_call);
    assert_eq!(resolution.callees.len(), 1);
    assert_eq!(
        (resolution.complete.value, resolution.complete.trust),
        (true, Trust::Proven)
    );
    let outside = fixture.node("(globalThis as any).unknown?.(1)", |kind| {
        matches!(kind, AstKind::ChainExpression(_))
    });
    let outside_call = fixture.node("(globalThis as any).unknown?.(1)", call);
    assert!(!facts.site_resolution(outside_call).complete.value);

    // Expression effects.
    assert!(facts.is_pure(square_call).value);
    assert_eq!(facts.is_pure(square_call).trust, Trust::Proven);
    let bump_call = fixture.node("bump(2)", call);
    assert!(facts.expr_effects(bump_call).value.writes);
    assert!(!facts.is_pure(bump_call).value);
    assert!(facts.expr_effects(outside).value.calls_unknown);

    // Callers.
    let square_unit = resolution.callees[0];
    assert_eq!(facts.call_graph().callers(square_unit).count(), 1);

    // Points-to.
    let literal = fixture.node("{ x: 1 }", |kind| {
        matches!(kind, AstKind::ObjectExpression(_))
    });
    let point = fixture.node("point", |kind| {
        matches!(kind, AstKind::IdentifierReference(_))
    });
    let sites = facts.points_to(point);
    assert_eq!(sites.trust, Trust::Proven);
    assert_eq!(sites.value, Some(&[literal][..]));

    // Loops.
    let pure_loop = fixture.node(
        "for (let i = 0; i < values.length; i++) {\n    total += values[i] * values[i];\n  }",
        |kind| matches!(kind, AstKind::ForStatement(_)),
    );
    let facts_of = facts.loop_facts(pure_loop).expect("the loop ran");
    assert!(!facts_of.effects.0.writes, "{facts_of:?}");
    assert_eq!(facts_of.length_changed, Vec::new());
    assert_eq!(facts_of.written.len(), 2, "{facts_of:?}");
    assert!(
        facts_of
            .induction
            .iter()
            .any(|(_, range)| range.value.int && range.value.range.min >= 0.0)
    );
    let popping = fixture.node(
        "for (let i = 0; i < values.length; i++) {\n    values.pop();\n    total += i;\n  }",
        |kind| matches!(kind, AstKind::ForStatement(_)),
    );
    let popping = facts.loop_facts(popping).expect("the loop ran");
    assert_eq!(popping.length_changed.len(), 1, "{popping:?}");

    let invariant_loop = fixture.node(
        "for (let i = 0; i < values.length; i++) {\n    total += values[i] + k * 3;\n  }",
        |kind| matches!(kind, AstKind::ForStatement(_)),
    );
    let invariant_loop = facts.loop_facts(invariant_loop).expect("the loop ran");
    let k_times_3 = fixture.node("k * 3", |kind| matches!(kind, AstKind::BinaryExpression(_)));
    assert!(
        invariant_loop.invariant.contains(&k_times_3),
        "{invariant_loop:?}"
    );
    assert!(
        !invariant_loop
            .invariant
            .contains(&fixture.node("values[i] + k * 3", |kind| {
                matches!(kind, AstKind::BinaryExpression(_))
            })),
        "{invariant_loop:?}"
    );

    // Reachability.
    let never = fixture.node("function never(): number {\n  return 1;\n}", |kind| {
        matches!(kind, AstKind::Function(_))
    });
    assert!(!facts.reachable(never).value);
    assert_eq!(facts.reachable(never).trust, Trust::Proven);
    let dead = fixture.node("log = 99;", |kind| {
        matches!(kind, AstKind::ExpressionStatement(_))
    });
    assert!(!facts.reachable(dead).value);
    let live = fixture.node("return 2;", |kind| {
        matches!(kind, AstKind::ReturnStatement(_))
    });
    assert_eq!(facts.reachable(live).trust, Trust::Unknown);
    let square_fn = fixture.node(
        "function square(x: number): number {\n  return x * x;\n}",
        |kind| matches!(kind, AstKind::Function(_)),
    );
    assert_eq!(facts.reachable(square_fn).trust, Trust::Unknown);
}
