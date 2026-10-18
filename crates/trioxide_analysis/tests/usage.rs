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
    let outside = fixture.node("external.unknown?.(1)", |kind| {
        matches!(kind, AstKind::ChainExpression(_))
    });
    let outside_call = fixture.node("external.unknown?.(1)", call);
    assert!(!facts.site_resolution(outside_call).complete.value);

    // Expression effects.
    assert!(facts.is_pure(square_call).value);
    assert_eq!(facts.is_pure(square_call).trust, Trust::Proven);
    let bump_call = fixture.node("bump(2)", call);
    assert!(facts.expr_effects(bump_call).value.writes);
    assert!(!facts.is_pure(bump_call).value);
    assert!(facts.expr_effects(outside).value.calls_unknown);

    // Sites in units the engine calls, and `new Promise(executor)`.
    for site in [
        fixture.node("r(1)", call),
        fixture.node("this.k()", call),
        fixture.node("Math.floor(o as unknown as number)", call),
        fixture.node("[o].join()", call),
        fixture.node(
            "new Promise((resolve) => {\n    log += 1;\n    resolve(1);\n  })",
            |kind| matches!(kind, AstKind::NewExpression(_)),
        ),
    ] {
        let resolution = facts.site_resolution(site);
        assert!(
            !(resolution.complete.value && resolution.complete.trust == Trust::Proven),
            "{resolution:?}"
        );
        assert!(!facts.is_pure(site).value || facts.is_pure(site).trust == Trust::Unknown);
    }

    // Callers.
    let square_unit = resolution.callees[0];
    assert_eq!(facts.call_graph().callers(square_unit).count(), 1);
    assert_eq!(
        facts.callers_complete(square_unit),
        trioxide_analysis::Fact::proven(true)
    );
    let unit = |text: &str| fixture.node(text, |kind| matches!(kind, AstKind::Function(_)));
    // A builtin callback is called at the builtin's site; `call` isn't modelled.
    let triple = unit("function triple(x: number): number {\n  return x * 3;\n}");
    assert_eq!(facts.call_graph().callers(triple).count(), 1);
    assert_eq!(
        facts.callers_complete(triple),
        trioxide_analysis::Fact::proven(true)
    );
    let quadruple = unit("function quadruple(x: number): number {\n  return x * 4;\n}");
    assert_eq!(facts.call_graph().callers(quadruple).count(), 0);
    assert_eq!(
        facts.callers_complete(quadruple),
        trioxide_analysis::Fact::proven(false)
    );

    // A literal allocates, at expression level.
    let object = fixture.node("{ x: 1 }", |kind| {
        matches!(kind, AstKind::ObjectExpression(_))
    });
    assert!(facts.expr_effects(object).value.allocates);

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
    assert!(facts_of.resized.is_empty() && !facts_of.resizes_untracked);
    // `i` counts up by one; `total` accumulates.
    let [counter, total] = facts_of.induction.as_slice() else {
        panic!("{facts_of:?}");
    };
    let (counter, total) = if counter.counter {
        (counter, total)
    } else {
        (total, counter)
    };
    assert!(counter.range.value.int && counter.range.value.range.min >= 0.0);
    assert_eq!((counter.step, counter.counter), (Some(1.0), true));
    assert_eq!((total.step, total.counter), (None, false));
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

    // Not invariant: guarded, volatile, a written object or captured local, a callee, or in a
    // loop with an early exit.
    let for_loop = |text: &str| {
        let node = fixture.node(text, |kind| matches!(kind, AstKind::ForStatement(_)));
        facts.loop_facts(node).expect("the loop ran").clone()
    };
    let binary =
        |text: &str| fixture.node(text, |kind| matches!(kind, AstKind::BinaryExpression(_)));
    let cases = [
        (
            "for (let i = 0; i < 3; i++) {\n    t += getK() * 1;\n    k++;\n  }",
            binary("getK() * 1"),
        ),
        (
            "for (let i = 0; i < values.length; i++) {\n    if (o !== null) {\n      total += o.x * 2;\n    }\n  }",
            binary("o.x * 2"),
        ),
        (
            "for (let i = 0; i < values.length; i++) {\n    total += Date.now() * 0;\n  }",
            binary("Date.now() * 0"),
        ),
        (
            "for (let i = 0; i < values.length; i++) {\n    total += box.v * 2;\n    box.v++;\n  }",
            binary("box.v * 2"),
        ),
        (
            "for (let i = 0; i < values.length; i++) {\n    total += c * 2;\n    inc();\n  }",
            binary("c * 2"),
        ),
        (
            "for (let i = 0; i < values.length; i++) {\n    total += scale.apply(i);\n  }",
            fixture.node("scale.apply", |kind| {
                matches!(kind, AstKind::StaticMemberExpression(_))
            }),
        ),
        (
            "for (let i = 0; i < values.length; i++) {\n    if (i > 3) {\n      break;\n    }\n    total += k * 5;\n  }",
            binary("k * 5"),
        ),
    ];
    for (text, expression) in cases {
        let loop_facts = for_loop(text);
        assert!(
            !loop_facts.invariant.contains(&expression),
            "{text}: {loop_facts:?}"
        );
    }
    let boxed = for_loop(
        "for (let i = 0; i < values.length; i++) {\n    total += box.v * 2;\n    box.v++;\n  }",
    );
    assert!(boxed.effects.0.writes, "{boxed:?}");

    // Writes to locals and local objects are effects of the expression.
    let assign = |text: &str| {
        fixture.node(text, |kind| {
            matches!(
                kind,
                AstKind::AssignmentExpression(_) | AstKind::UpdateExpression(_)
            )
        })
    };
    for expression in [
        assign("t += arr[0] * 2"),
        assign("arr[0] = i + 10"),
        fixture.node("arr.push(i)", call),
    ] {
        assert!(!facts.is_pure(expression).value, "{expression:?}");
    }
    let locals_loop = for_loop(
        "for (let i = 0; i < 3; i++) {\n    t += arr[0] * 2;\n    arr[0] = i + 10;\n    arr.push(i);\n  }",
    );
    assert!(locals_loop.effects.0.writes, "{locals_loop:?}");
    let arr = fixture.node("[7, 8, 9]", |kind| {
        matches!(kind, AstKind::ArrayExpression(_))
    });
    assert_eq!(locals_loop.resized, vec![arr], "{locals_loop:?}");

    // A nested loop's counter isn't the outer loop's; a callee's resize counts.
    let outer = for_loop(
        "for (let i = 10; i > 0; i -= 2) {\n    for (let j = 0; j < i; j++) {\n      grow(grown);\n    }\n  }",
    );
    let [counter] = outer.induction.as_slice() else {
        panic!("{outer:?}");
    };
    assert_eq!(
        (counter.step, counter.counter),
        (Some(-2.0), true),
        "{outer:?}"
    );
    let grown = fixture.node("[11]", |kind| matches!(kind, AstKind::ArrayExpression(_)));
    assert_eq!(outer.resized, vec![grown], "{outer:?}");
    assert!(
        !locals_loop.invariant.contains(&binary("arr[0] * 2")),
        "{locals_loop:?}"
    );

    // Only whole steps are reported.
    let fractional = for_loop("for (let x = 0; x < 1; x += 0.25) {\n    count += 1;\n  }");
    // `x` steps by 0.25: no step; `count` by 1.
    let steps: Vec<Option<f64>> = fractional
        .induction
        .iter()
        .map(|induction| induction.step)
        .collect();
    assert!(
        steps.contains(&None) && steps.contains(&Some(1.0)),
        "{fractional:?}"
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

#[test]
fn forgetting_a_module_opens_the_sites_that_called_into_it() {
    let entry = common::fixtures()
        .into_iter()
        .find(|entry| entry.parent().unwrap().ends_with("module-linking"))
        .unwrap();
    let loaded = common::load(&entry);
    let mut facts = loaded.analysis.facts.clone();
    let edge = facts
        .call_graph()
        .edges()
        .iter()
        .find(|edge| edge.site.module != edge.callee.module)
        .copied()
        .expect("a cross-module call");
    let before = facts.site_resolution(edge.site);
    assert_eq!(
        (before.complete.value, before.complete.trust),
        (true, Trust::Proven)
    );
    facts.forget_module(edge.callee.module);
    let after = facts.site_resolution(edge.site);
    assert_eq!(after.callees, Vec::new());
    assert!(!after.complete.value, "{after:?}");
}

/// With `trustTypes`, a `for…of` over an `Iterable<T>` still runs user iterator code: its
/// effects never rest on the declared type alone.
#[test]
fn user_iterators_are_not_summarised_from_declared_types() {
    let entry = common::fixtures()
        .into_iter()
        .find(|entry| entry.parent().unwrap().ends_with("protocol-methods"))
        .unwrap();
    let loaded = common::load_trusting_types(&entry);
    let facts = &loaded.analysis.facts;
    let source = loaded.graph.modules()[0].source.to_string();
    let module = &loaded.graph.modules()[0];
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &source, module.source_type).parse();
    let program = allocator.alloc(parsed.program);
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(program)
        .semantic;
    let for_of = semantic
        .nodes()
        .iter()
        .find(|node| matches!(node.kind(), AstKind::ForOfStatement(_)))
        .expect("the loop in `sum`");
    let loop_facts = facts
        .loop_facts(NodeRef {
            module: module.id,
            node: for_of.id(),
        })
        .expect("the loop ran");
    for effects in [loop_facts.effects.0, loop_facts.effects.1] {
        assert!(effects.calls_unknown && effects.writes, "{loop_facts:?}");
    }
    let sum = semantic
        .nodes()
        .iter()
        .find(|node| {
            matches!(node.kind(), AstKind::Function(function)
                if function.id.as_ref().is_some_and(|id| id.name == "sum"))
        })
        .expect("`sum`");
    let effects = facts.effects(NodeRef {
        module: module.id,
        node: sum.id(),
    });
    assert!(effects.value.calls_unknown, "{effects:?}");

    // The engine calls protocol methods: none of them has a complete set of callers.
    let mut checked = 0;
    for node in semantic.nodes().iter() {
        let protocol = match semantic.nodes().parent_kind(node.id()) {
            AstKind::ObjectProperty(property) if property.method => {
                property.computed
                    || property
                        .key
                        .static_name()
                        .is_some_and(|name| name == "next" || name == "toJSON")
            }
            AstKind::MethodDefinition(method) => method.computed,
            _ => false,
        };
        if protocol && matches!(node.kind(), AstKind::Function(_)) {
            let unit = NodeRef {
                module: module.id,
                node: node.id(),
            };
            assert_eq!(
                facts.callers_complete(unit),
                trioxide_analysis::Fact::proven(false),
                "{}",
                node.kind().span().source_text(&source)
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 6);
}
