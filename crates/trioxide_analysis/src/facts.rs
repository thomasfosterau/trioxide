use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use trioxide_common::{NodeRef, SymbolRef};
use trioxide_types::{FunctionId, Type};

/// How far a fact can be relied on (PLAN D1).
///
/// Ordered from weakest to strongest, so `trust >= Trust::Declared` reads naturally.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Trust {
    /// Nothing is known; the value is a placeholder (e.g. [`Type::Unknown`]).
    #[default]
    Unknown,
    /// TypeScript says so, but the claim rests on an annotation, cast, `any` or an open-world
    /// boundary. Only usable as proof with `trustTypes: true`.
    Declared,
    /// Derived from literals, operators, builtin models and flow narrowing with no unsound
    /// construct on the path. Always usable as proof.
    Proven,
}

/// A value together with how far it can be trusted.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Fact<T> {
    /// What is claimed.
    pub value: T,
    /// How far the claim can be relied on.
    pub trust: Trust,
}

impl<T> Fact<T> {
    /// A fact proven sound.
    pub const fn proven(value: T) -> Self {
        Self {
            value,
            trust: Trust::Proven,
        }
    }

    /// A fact that rests on TS declarations.
    pub const fn declared(value: T) -> Self {
        Self {
            value,
            trust: Trust::Declared,
        }
    }

    /// Borrows the value.
    pub const fn as_ref(&self) -> Fact<&T> {
        Fact {
            value: &self.value,
            trust: self.trust,
        }
    }
}

impl<T: Default> Fact<T> {
    /// The "nothing known" fact: the default value at [`Trust::Unknown`].
    pub fn unknown() -> Self {
        Self {
            value: T::default(),
            trust: Trust::Unknown,
        }
    }
}

/// What a function may do when called (PLAN §2 step 4, "effect and purity summaries").
///
/// Every flag is conservative: `true` means "may", `false` means "proven not to" at the
/// fact's trust level. [`Effects::default`] is the "anything" summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(clippy::struct_excessive_bools, reason = "independent effect flags")]
pub struct Effects {
    /// Reads mutable state outside its own locals.
    pub reads: bool,
    /// Writes state outside its own locals.
    pub writes: bool,
    /// Allocates objects that may outlive the call.
    pub allocates: bool,
    /// May throw.
    pub throws: bool,
    /// Calls a function whose effects are unknown.
    pub calls_unknown: bool,
}

impl Default for Effects {
    fn default() -> Self {
        Self::ANY
    }
}

impl Effects {
    /// May do anything.
    pub const ANY: Self = Self {
        reads: true,
        writes: true,
        allocates: true,
        throws: true,
        calls_unknown: true,
    };
    /// Does nothing observable.
    pub const PURE: Self = Self {
        reads: false,
        writes: false,
        allocates: false,
        throws: false,
        calls_unknown: false,
    };

    /// No writes, throws or unknown calls: removable when the result is unused.
    pub const fn is_side_effect_free(self) -> bool {
        !self.writes && !self.throws && !self.calls_unknown
    }
}

/// The analysis side tables for a whole project (PLAN §2 step 4).
///
/// Tables are keyed by [`NodeRef`] or [`SymbolRef`], i.e. `(ModuleId, NodeId/SymbolId)`. A
/// missing entry means "nothing known" and is returned as [`Fact::unknown`], so passes never
/// have to distinguish "not analysed" from "analysed, unknown".
///
/// Passes must check trust before acting: use [`Facts::is_usable`], which honours
/// `trustTypes`. S2 owns this type and will add tables (ranges, escape, shapes, call graph);
/// adding a table is not a breaking change for readers.
#[derive(Debug, Default)]
pub struct Facts {
    trust_types: bool,
    node_types: FxHashMap<NodeRef, Fact<Type>>,
    symbol_types: FxHashMap<SymbolRef, Fact<Type>>,
    effects: FxHashMap<FunctionId, Fact<Effects>>,
}

impl Facts {
    /// Empty tables. `trust_types` decides whether `Declared` facts are usable as proof.
    pub fn new(trust_types: bool) -> Self {
        Self {
            trust_types,
            ..Self::default()
        }
    }

    /// Whether a fact at `trust` may justify a semantics-changing rewrite: always for
    /// `Proven`, for `Declared` only with `trustTypes`, never for `Unknown`.
    pub const fn is_usable(&self, trust: Trust) -> bool {
        match trust {
            Trust::Proven => true,
            Trust::Declared => self.trust_types,
            Trust::Unknown => false,
        }
    }

    /// The type of the value an expression or binding node evaluates to.
    pub fn ty(&self, node: NodeRef) -> Fact<&Type> {
        self.node_types
            .get(&node)
            .map_or(UNKNOWN_TYPE.as_ref(), Fact::as_ref)
    }

    /// The type of every value a symbol (binding) can hold.
    pub fn symbol_ty(&self, symbol: SymbolRef) -> Fact<&Type> {
        self.symbol_types
            .get(&symbol)
            .map_or(UNKNOWN_TYPE.as_ref(), Fact::as_ref)
    }

    /// What calling `function` may do.
    pub fn effects(&self, function: FunctionId) -> Fact<Effects> {
        self.effects
            .get(&function)
            .cloned()
            .unwrap_or_else(Fact::unknown)
    }

    /// Records the type of a node. Replaces any previous fact.
    pub fn set_ty(&mut self, node: NodeRef, fact: Fact<Type>) {
        self.node_types.insert(node, fact);
    }

    /// Records the type of a symbol. Replaces any previous fact.
    pub fn set_symbol_ty(&mut self, symbol: SymbolRef, fact: Fact<Type>) {
        self.symbol_types.insert(symbol, fact);
    }

    /// Records a function's effect summary. Replaces any previous fact.
    pub fn set_effects(&mut self, function: FunctionId, fact: Fact<Effects>) {
        self.effects.insert(function, fact);
    }
}

static UNKNOWN_TYPE: Fact<Type> = Fact {
    value: Type::Unknown,
    trust: Trust::Unknown,
};

#[cfg(test)]
mod tests {
    use trioxide_common::{ModuleId, NodeId};
    use trioxide_types::NumberType;

    use super::*;

    fn node(index: u32) -> NodeRef {
        NodeRef {
            module: ModuleId::new(0),
            node: NodeId::from_usize(index as usize),
        }
    }

    #[test]
    fn missing_facts_are_unknown() {
        let facts = Facts::new(false);
        assert_eq!(
            facts.ty(node(1)),
            Fact {
                value: &Type::Unknown,
                trust: Trust::Unknown
            }
        );
        assert_eq!(
            facts.effects(node(1)),
            Fact {
                value: Effects::ANY,
                trust: Trust::Unknown
            }
        );
    }

    #[test]
    fn trust_types_gates_declared_facts() {
        let mut facts = Facts::new(false);
        facts.set_ty(node(1), Fact::declared(Type::Number(NumberType::default())));
        let fact = facts.ty(node(1));
        assert!(!facts.is_usable(fact.trust));
        assert!(facts.is_usable(Trust::Proven));
        assert!(Facts::new(true).is_usable(fact.trust));
        assert!(!Facts::new(true).is_usable(Trust::Unknown));
    }
}
