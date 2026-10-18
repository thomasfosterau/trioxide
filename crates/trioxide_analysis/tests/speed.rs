//! The queries the speed passes add: `builtin_intact`, `may_have_holes` and
//! `instance_keys_observed`, on small programs written to a temporary directory.

mod common;

use oxc_allocator::Allocator;
use oxc_ast::ast::{ArrayExpression, Class};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use trioxide_analysis::{Fact, Trust};
use trioxide_common::NodeRef;

/// Analyses `source` as the only module of a program.
fn analyse(source: &str) -> (tempfile::TempDir, common::Loaded) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("input.ts"), source).unwrap();
    let loaded = common::load(&dir.path().join("input.ts"));
    (dir, loaded)
}

/// The array literals and classes of the program's module, in source order.
fn nodes(loaded: &common::Loaded) -> (Vec<NodeRef>, Vec<(String, NodeRef)>) {
    #[derive(Default)]
    struct Find {
        arrays: Vec<oxc_syntax::node::NodeId>,
        classes: Vec<(String, oxc_syntax::node::NodeId)>,
    }
    impl<'a> Visit<'a> for Find {
        fn visit_array_expression(&mut self, array: &ArrayExpression<'a>) {
            self.arrays.push(array.node_id.get());
            walk::walk_array_expression(self, array);
        }

        fn visit_class(&mut self, class: &Class<'a>) {
            let name = class
                .id
                .as_ref()
                .map_or_else(String::new, |id| id.name.to_string());
            self.classes.push((name, class.node_id.get()));
            walk::walk_class(self, class);
        }
    }
    let module = &loaded.graph.modules()[0];
    let allocator = Allocator::default();
    let program = Parser::new(&allocator, &module.source, module.source_type)
        .parse()
        .program;
    let _ = SemanticBuilder::new().build(&program);
    let mut find = Find::default();
    find.visit_program(&program);
    let node = |node| NodeRef {
        module: module.id,
        node,
    };
    (
        find.arrays.into_iter().map(node).collect(),
        find.classes
            .into_iter()
            .map(|(name, id)| (name, node(id)))
            .collect(),
    )
}

#[test]
fn builtin_integrity() {
    let (_dir, loaded) = analyse("export function main() { console.log([1].map((x) => x)); }\n");
    let facts = &loaded.analysis.facts;
    assert_eq!(facts.builtin_intact("Array"), Fact::proven(true));
    let (_dir, loaded) = analyse(
        "Array.prototype.map = Array.prototype.filter;\nexport function main() { console.log([1].map((x) => x)); }\n",
    );
    assert_eq!(
        loaded.analysis.facts.builtin_intact("Array"),
        Fact::proven(false)
    );
    // Before any analysis there is nothing to go on.
    assert_eq!(
        trioxide_analysis::Facts::new(false).builtin_intact("Array"),
        Fact::unknown()
    );
}

#[test]
fn holes() {
    let (_dir, loaded) = analyse(
        "function f(): number {\n  const packed = [1, 2];\n  packed.push(3);\n  // eslint-disable-next-line no-sparse-arrays\n  const sparse = [1, , 3];\n  const deleted = [4, 5];\n  delete deleted[0];\n  const grown = [6];\n  grown.length = 4;\n  return packed.length + sparse.length + deleted.length + grown.length;\n}\nexport function main() { console.log(f()); }\n",
    );
    let facts = &loaded.analysis.facts;
    let (arrays, _) = nodes(&loaded);
    assert_eq!(facts.may_have_holes(arrays[0]), Fact::proven(false));
    for &holey in &arrays[1..] {
        let fact = facts.may_have_holes(holey);
        assert!(fact.value && fact.trust == Trust::Unknown, "{fact:?}");
    }
}

#[test]
fn observed_keys() {
    let (_dir, loaded) = analyse(
        "class Hidden { a = 1; }\nclass Listed { a = 1; }\nclass Tested { a = 1; }\nclass Parent { a = 1; }\nclass Child extends Parent {}\nfunction f(): string {\n  const h = new Hidden();\n  (h as any).b = 2;\n  const l = new Listed();\n  const t = new Tested();\n  const c = new Child();\n  return `${h.a} ${Object.keys(l).length} ${\"a\" in t} ${c.a}`;\n}\nexport function main() { console.log(f()); }\n",
    );
    let facts = &loaded.analysis.facts;
    let (_, classes) = nodes(&loaded);
    let observed = |name: &str| {
        let class = classes.iter().find(|(n, _)| n == name).unwrap().1;
        facts.instance_keys_observed(class)
    };
    assert_eq!(observed("Hidden"), Fact::proven(false));
    for name in ["Listed", "Tested", "Parent"] {
        let fact = observed(name);
        assert!(
            fact.value && fact.trust == Trust::Unknown,
            "{name}: {fact:?}"
        );
    }
}
