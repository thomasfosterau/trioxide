//! What the lowering infers about each value before choosing Rust types: a small lattice
//! joined over every assignment, argument and return in the island.

use std::rc::Rc;

use trioxide_common::NodeRef;
use trioxide_types::{ClassId, NumberType};

use crate::rir::Elem;

/// Which struct an object value belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum StructKey {
    /// An instance of a program class used only inside the island.
    Class(ClassId),
    /// Objects from one object literal.
    Literal(NodeRef),
}

/// The kind of a value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    /// No value seen yet.
    Bottom,
    /// `undefined` from a function without a return value.
    Unit,
    /// A boolean.
    Bool,
    /// A number; with `undef`, possibly `undefined` (an unchecked array read), which the
    /// island represents as `NaN` and may only use where JS converts it to a number.
    Num {
        /// May be `undefined`.
        undef: bool,
    },
    /// An array: created in the island (`fresh`) or one of the current function's parameters
    /// (bit `i` of `params`).
    Array {
        /// The element type.
        elem: Elem,
        /// May be an array created during the call.
        fresh: bool,
        /// May be one of these parameters.
        params: u64,
    },
    /// An immutable object (class instance or object literal) with the kinds of its fields
    /// in declaration order.
    Value {
        /// Which struct.
        key: StructKey,
        /// Field kinds.
        fields: Rc<[Kind]>,
    },
    /// A plain-data object from outside the island, passed by reference: boundary struct
    /// parameter group `group`.
    ParamStruct {
        /// The group (one Rust struct).
        group: usize,
    },
    /// Conflicting kinds.
    Error,
}

impl Kind {
    /// The least kind covering both.
    pub fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Bottom, k) | (k, Self::Bottom) => k.clone(),
            (Self::Unit, Self::Unit) | (Self::Bool, Self::Bool) => self.clone(),
            (Self::Num { undef: a }, Self::Num { undef: b }) => Self::Num { undef: *a || *b },
            (
                Self::Array {
                    elem: a,
                    fresh: fa,
                    params: pa,
                },
                Self::Array {
                    elem: b,
                    fresh: fb,
                    params: pb,
                },
            ) if a == b => Self::Array {
                elem: *a,
                fresh: *fa || *fb,
                params: pa | pb,
            },
            (Self::Value { key: a, fields: fa }, Self::Value { key: b, fields: fb })
                if a == b && fa.len() == fb.len() =>
            {
                Self::Value {
                    key: *a,
                    fields: fa.iter().zip(fb.iter()).map(|(x, y)| x.join(y)).collect(),
                }
            }
            (Self::ParamStruct { group: a }, Self::ParamStruct { group: b }) if a == b => {
                Self::ParamStruct { group: *a }
            }
            _ => Self::Error,
        }
    }

    /// A short description for messages.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Bottom => "nothing",
            Self::Unit => "undefined",
            Self::Bool => "a boolean",
            Self::Num { undef: false } => "a number",
            Self::Num { undef: true } => "a number or undefined",
            Self::Array { .. } => "an array",
            Self::Value { .. } => "an object",
            Self::ParamStruct { .. } => "an object parameter",
            Self::Error => "an unknown value",
        }
    }

    /// A number that is never `undefined`.
    pub const NUM: Self = Self::Num { undef: false };
}

/// `-2^31`.
const I32_MIN: f64 = -2_147_483_648.0;
/// `2^31 - 1`.
const I32_MAX: f64 = 2_147_483_647.0;
/// `2^32 - 1`.
const U32_MAX: f64 = 4_294_967_295.0;

/// Whether every value of `n` is an int32 other than `-0` (so `i32` holds it exactly).
pub(crate) fn is_int32(n: NumberType) -> bool {
    n.int && n.non_nan && n.non_neg_zero && n.range.min >= I32_MIN && n.range.max <= I32_MAX
}

/// Whether every value of `n` is a uint32 other than `-0`.
pub(crate) fn is_uint32(n: NumberType) -> bool {
    n.int && n.non_nan && n.non_neg_zero && n.range.min >= 0.0 && n.range.max <= U32_MAX
}

/// Whether every value of `n` is a valid array index below 2^32 - 1 (so it fits `usize` on
/// wasm32 and is exact as `u32`).
pub(crate) fn is_index(n: NumberType) -> bool {
    n.int && n.non_nan && n.range.min >= 0.0 && n.range.max < U32_MAX
}
