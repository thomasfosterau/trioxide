//! What the speed passes (PLAN §4 passes 5, 8, 11 and 12) share: rebuilding the scoping
//! without losing node ids, fresh readable names, and the island checks.
//!
//! The speed passes run as one fact-reading group. Each rewrites the AST directly and then
//! rebuilds the scoping with [`rescope`], which keeps the node ids of the nodes it kept, so the
//! next pass of the group still gets facts about them (nodes a pass made stay
//! [`NodeId::DUMMY`](trioxide_common::NodeId::DUMMY) and answer `Unknown`).

use oxc_ast::{AstKind, ast::Program};
use oxc_ast_visit::Visit;
use oxc_semantic::Scoping;
use oxc_syntax::{scope::ScopeId, symbol::SymbolId};
use rustc_hash::FxHashSet;
use trioxide_common::{NodeId, NodeRef};

use crate::{Keep, rebuild_scoping};

/// Rebuilds `program`'s scoping, keeping every node's id. A semantic build numbers nodes
/// afresh; this puts the old ids back, so facts keep answering for the nodes passes kept.
/// The scoping's own records of nodes (a scope's or symbol's node) are in the new numbering,
/// which the passes don't use.
pub fn rescope(program: &Program<'_>) -> Scoping {
    #[derive(Default)]
    struct Ids(Vec<NodeId>);
    impl<'a> Visit<'a> for Ids {
        fn enter_node(&mut self, kind: AstKind<'a>) {
            self.0.push(kind.node_id());
        }
    }
    struct Restore<'i>(std::slice::Iter<'i, NodeId>);
    impl<'a> Visit<'a> for Restore<'_> {
        fn enter_node(&mut self, kind: AstKind<'a>) {
            if let Some(&id) = self.0.next() {
                kind.set_node_id(id);
            }
        }
    }
    let mut ids = Ids::default();
    ids.visit_program(program);
    let scoping = rebuild_scoping(program);
    Restore(ids.0.iter()).visit_program(program);
    scoping
}

/// Sets every node id under a node (a clone) to [`NodeId::DUMMY`], so it has no facts.
pub struct ForgetIds;

impl<'a> Visit<'a> for ForgetIds {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        kind.set_node_id(NodeId::DUMMY);
    }
}

/// Picks names for new bindings that no reference can confuse with another binding.
pub struct Namer {
    /// Names given out so far, and globals the module reads: never reused.
    taken: FxHashSet<String>,
}

impl Namer {
    /// A namer for a module with `scoping`.
    pub fn new(scoping: &Scoping) -> Self {
        Self {
            taken: scoping
                .root_unresolved_references()
                .keys()
                .map(|name| name.to_string())
                .collect(),
        }
    }

    /// A name based on `base` for a new binding in `scope`: not bound in `scope`, its ancestors
    /// or its descendants, not a global the module reads, not reserved, and not given out
    /// before. So no existing reference starts resolving to it, and every reference made to it
    /// from inside `scope` reaches it. `base`, else `base_1`, `base_2`, ….
    pub fn fresh(&mut self, base: &str, scope: ScopeId, scoping: &Scoping) -> String {
        self.fresh_replacing(base, scope, scoping, None)
    }

    /// [`fresh`](Self::fresh), where the binding `replaced` (which the new one takes over,
    /// references and all) doesn't count as taking its name.
    pub fn fresh_replacing(
        &mut self,
        base: &str,
        scope: ScopeId,
        scoping: &Scoping,
        replaced: Option<SymbolId>,
    ) -> String {
        let base = sanitize(base);
        let other = |symbol: SymbolId| Some(symbol) != replaced;
        let free = |name: &str, taken: &FxHashSet<String>| {
            !taken.contains(name)
                && !oxc_syntax::keyword::is_reserved_keyword_or_global_object(name)
                && !matches!(
                    name,
                    "arguments" | "eval" | "let" | "static" | "yield" | "await"
                )
                && scoping
                    .find_binding(scope, name.into())
                    .is_none_or(|symbol| !other(symbol))
                && !scoping.iter_bindings().any(|(inner, bindings)| {
                    bindings.get(name).is_some_and(|&symbol| other(symbol))
                        && scoping.scope_is_descendant_of(inner, scope)
                })
        };
        let mut name = base.clone();
        let mut counter = 1;
        while !free(&name, &self.taken) {
            name = format!("{base}_{counter}");
            counter += 1;
        }
        self.taken.insert(name.clone());
        name
    }
}

/// `text` as an identifier: other characters become `_`, and a leading digit gets a `_`.
fn sanitize(text: &str) -> String {
    let mut name: String = text
        .chars()
        .map(|c| {
            if c == '$' || c == '_' || c.is_ascii_alphanumeric() {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert(0, '_');
    }
    name
}

/// Whether the function `unit` must stay as written: it is compiled into a WASM island (or
/// forced into one by `"use wasm"` / `@trioxide wasm`, which `forced` says).
pub fn is_island(keep: &Keep, unit: Option<NodeRef>, forced: bool) -> bool {
    forced || unit.is_some_and(|unit| keep.functions.contains(&unit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize("x"), "x");
        assert_eq!(sanitize("0"), "_0");
        assert_eq!(sanitize("foo-bar"), "foo_bar");
        assert_eq!(sanitize(""), "_");
    }
}
