//! Abstract values: a type with a trust level, plus the allocation sites the value may point
//! to.

use trioxide_common::NodeRef;
use trioxide_types::{NumberType, Type};

use crate::facts::{Fact, Trust};

/// An allocation site: the object literal, array literal or `new` expression node.
pub type AllocId = NodeRef;

/// How many sites a points-to set holds before it gives up and becomes [`Pts::Any`].
const MAX_SITES: usize = 8;

/// Which tracked allocations a value may be.
///
/// `Sites(s)` means: if the value is an object, it was created at one of `s` (so the empty set
/// means "not an object trioxide could be tracking"; primitives have it). `Any` means it may be
/// any object, including ones created by code trioxide does not see.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Pts {
    /// One of these sites, or not an object.
    Sites(Vec<AllocId>),
    /// Possibly any object.
    #[default]
    Any,
}

impl Pts {
    /// Not an object.
    pub const NONE: Self = Self::Sites(Vec::new());

    /// Exactly `site`.
    pub fn site(site: AllocId) -> Self {
        Self::Sites(vec![site])
    }

    /// Either. Sites that the join can no longer name (it gives `Any`) are noted in
    /// [`take_lost`]: a value that may be them is no longer followed, so they must be treated
    /// as escaping.
    #[must_use]
    pub fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Sites(a), Self::Sites(b)) => {
                let mut sites = a.clone();
                for site in b {
                    if let Err(at) = sites.binary_search(site) {
                        sites.insert(at, *site);
                    }
                }
                if sites.len() > MAX_SITES {
                    lose(&sites);
                    Self::Any
                } else {
                    Self::Sites(sites)
                }
            }
            (Self::Sites(sites), Self::Any) | (Self::Any, Self::Sites(sites)) => {
                lose(sites);
                Self::Any
            }
            (Self::Any, Self::Any) => Self::Any,
        }
    }

    /// The sites, if the set is known.
    pub fn sites(&self) -> Option<&[AllocId]> {
        match self {
            Self::Sites(sites) => Some(sites),
            Self::Any => None,
        }
    }

    /// Whether the value is known not to be any object.
    pub fn is_none(&self) -> bool {
        matches!(self, Self::Sites(sites) if sites.is_empty())
    }
}

thread_local! {
    static LOST: std::cell::RefCell<Vec<AllocId>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn lose(sites: &[AllocId]) {
    if !sites.is_empty() {
        LOST.with(|lost| lost.borrow_mut().extend_from_slice(sites));
    }
}

/// The sites joins on this thread have dropped into [`Pts::Any`] since the last call.
pub fn take_lost() -> Vec<AllocId> {
    LOST.with(|lost| std::mem::take(&mut *lost.borrow_mut()))
}

/// An abstract value.
#[derive(Debug, Clone, PartialEq)]
pub struct Val {
    /// What the value is.
    pub ty: Type,
    /// How far `ty` can be trusted.
    pub trust: Trust,
    /// The objects it may be.
    pub pts: Pts,
    /// Something the type checker can't see may have changed the value (eval, `with`,
    /// sloppy `arguments` aliasing, a proxy, `Object.defineProperty`): not even TypeScript's
    /// declared type applies.
    pub poisoned: bool,
}

impl Default for Val {
    fn default() -> Self {
        Self::unknown()
    }
}

impl Val {
    /// Anything.
    pub fn unknown() -> Self {
        Self {
            ty: Type::Unknown,
            trust: Trust::Unknown,
            pts: Pts::Any,
            poisoned: false,
        }
    }

    /// Anything, and TypeScript's type can't be trusted either.
    pub fn poisoned() -> Self {
        Self {
            poisoned: true,
            ..Self::unknown()
        }
    }

    /// No value: unreachable, or a binding not yet initialised.
    pub fn never() -> Self {
        Self {
            ty: Type::Never,
            trust: Trust::Proven,
            pts: Pts::NONE,
            poisoned: false,
        }
    }

    /// A proven primitive.
    pub fn proven(ty: Type) -> Self {
        Self {
            ty,
            trust: Trust::Proven,
            pts: Pts::NONE,
            poisoned: false,
        }
    }

    /// A proven number.
    pub fn number(number: NumberType) -> Self {
        Self::proven(Type::Number(number))
    }

    /// A value TypeScript declares to have `ty`. Its objects are untracked.
    pub fn declared(ty: Type) -> Self {
        let pts = if is_primitive(&ty) {
            Pts::NONE
        } else {
            Pts::Any
        };
        Self {
            trust: if ty == Type::Unknown {
                Trust::Unknown
            } else {
                Trust::Declared
            },
            ty,
            pts,
            poisoned: false,
        }
    }

    /// A proven object of `ty` created at `site`.
    pub fn object(ty: Type, site: AllocId) -> Self {
        Self {
            ty,
            trust: Trust::Proven,
            pts: Pts::site(site),
            poisoned: false,
        }
    }

    /// Either value.
    #[must_use]
    pub fn join(&self, other: &Self) -> Self {
        if self.ty == Type::Never && self.pts.is_none() {
            return other.clone();
        }
        if (other.ty == Type::Never && other.pts.is_none()) || self == other {
            return self.clone();
        }
        let mut ty = self.ty.join(&other.ty);
        if ty.depth() > MAX_TYPE_DEPTH {
            ty = ty.truncate(MAX_TYPE_DEPTH);
        }
        Self {
            trust: if ty == Type::Unknown {
                Trust::Unknown
            } else {
                self.trust.min(other.trust)
            },
            ty,
            pts: self.pts.join(&other.pts),
            poisoned: self.poisoned || other.poisoned,
        }
    }

    /// The same value at no more than `trust`.
    #[must_use]
    pub fn at_most(mut self, trust: Trust) -> Self {
        self.trust = self.trust.min(trust);
        if self.trust == Trust::Unknown {
            self.ty = Type::Unknown;
        }
        self
    }

    /// The value as a fact.
    pub fn fact(&self) -> Fact<Type> {
        Fact {
            value: if self.trust == Trust::Unknown {
                Type::Unknown
            } else {
                self.ty.clone()
            },
            trust: if self.ty == Type::Unknown {
                Trust::Unknown
            } else {
                self.trust
            },
        }
    }

    /// Whether the value is proven to be a primitive (so it can't run user code when coerced or
    /// be mutated), at `Proven` or, with `declared`, at `Declared`.
    pub fn is_primitive(&self, declared: bool) -> bool {
        let usable = self.trust == Trust::Proven || (declared && self.trust >= Trust::Declared);
        usable && is_primitive(&self.ty)
    }
}

/// How deep types in abstract values may nest; deeper parts become `Unknown`.
pub const MAX_TYPE_DEPTH: usize = 4;

/// Whether every value of `ty` is a primitive.
pub fn is_primitive(ty: &Type) -> bool {
    !matches!(ty, Type::Never)
        && ty.members().iter().all(|member| {
            matches!(
                member,
                Type::Number(_)
                    | Type::Boolean
                    | Type::String
                    | Type::BigInt
                    | Type::Symbol
                    | Type::Null
                    | Type::Undefined
            )
        })
}
