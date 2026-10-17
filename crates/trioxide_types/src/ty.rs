use serde::{Deserialize, Serialize};
use trioxide_common::NodeRef;

/// Identifies a class by the node of its declaration or expression.
pub type ClassId = NodeRef;

/// Identifies a function by the node of its declaration, expression or arrow.
pub type FunctionId = NodeRef;

/// A trioxide type (PLAN §3).
///
/// This is a lattice with [`Type::Unknown`] at the top and [`Type::Never`] at the bottom. It
/// describes *values*, not TS syntax: aliases, generics and type parameters are resolved away
/// by the oracle. How far a type can be trusted is tracked separately, by the trust level on
/// each analysis fact.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Type {
    /// Any value: the top of the lattice, and the answer whenever trioxide can't tell.
    #[default]
    Unknown,
    /// No value (unreachable code, a function that always throws).
    Never,
    /// A JS number, refined by range analysis.
    Number(NumberType),
    /// `true` or `false`.
    Boolean,
    /// A primitive string.
    String,
    /// A primitive bigint.
    BigInt,
    /// A primitive symbol.
    Symbol,
    /// `null`.
    Null,
    /// `undefined`.
    Undefined,
    /// An `Array` whose elements have the given type.
    Array(Box<Type>),
    /// A typed array view such as `Float64Array`.
    TypedArray(TypedArrayKind),
    /// A fixed-length array with per-position types.
    Tuple(Vec<Type>),
    /// A plain object with a closed shape.
    Object(Shape),
    /// An instance of a class whose fields are known and not mutated dynamically.
    Instance(ClassId),
    /// A callable, with the function it is known to be, if any.
    Function(Box<Signature>, Option<FunctionId>),
    /// One of several types. Never nested and never a single member.
    Union(Vec<Type>),
}

/// What is known about a number beyond "it is a number".
///
/// tsrs supplies `number` and numeric literal types; trioxide's range analysis narrows the rest.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NumberType {
    /// The values it may take. NaN is tracked by `non_nan`, not by the interval.
    pub range: Interval,
    /// Always an integer (or ±Infinity if the interval allows).
    pub int: bool,
    /// Never NaN.
    pub non_nan: bool,
    /// Never `-0`.
    pub non_neg_zero: bool,
}

/// A closed interval of numbers. `[-inf, +inf]` is "any number".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    /// Inclusive lower bound.
    pub min: f64,
    /// Inclusive upper bound.
    pub max: f64,
}

impl Default for Interval {
    fn default() -> Self {
        Self::FULL
    }
}

impl Interval {
    /// Every number.
    pub const FULL: Self = Self {
        min: f64::NEG_INFINITY,
        max: f64::INFINITY,
    };

    /// The single value `value`.
    pub const fn point(value: f64) -> Self {
        Self {
            min: value,
            max: value,
        }
    }
}

/// The element kind of a typed array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TypedArrayKind {
    /// `Int8Array`
    Int8,
    /// `Uint8Array`
    Uint8,
    /// `Uint8ClampedArray`
    Uint8Clamped,
    /// `Int16Array`
    Int16,
    /// `Uint16Array`
    Uint16,
    /// `Int32Array`
    Int32,
    /// `Uint32Array`
    Uint32,
    /// `Float32Array`
    Float32,
    /// `Float64Array`
    Float64,
    /// `BigInt64Array`
    BigInt64,
    /// `BigUint64Array`
    BigUint64,
}

impl TypedArrayKind {
    /// The global constructor's name, e.g. `"Float64Array"`.
    pub const fn constructor_name(self) -> &'static str {
        match self {
            Self::Int8 => "Int8Array",
            Self::Uint8 => "Uint8Array",
            Self::Uint8Clamped => "Uint8ClampedArray",
            Self::Int16 => "Int16Array",
            Self::Uint16 => "Uint16Array",
            Self::Int32 => "Int32Array",
            Self::Uint32 => "Uint32Array",
            Self::Float32 => "Float32Array",
            Self::Float64 => "Float64Array",
            Self::BigInt64 => "BigInt64Array",
            Self::BigUint64 => "BigUint64Array",
        }
    }
}

/// The known properties of a closed object shape, in declaration order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shape {
    /// `(key, type)` pairs. Keys are unique.
    pub properties: Vec<(String, Type)>,
}

/// A resolved call signature.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Signature {
    /// Parameter types in order. A rest parameter is the last entry, typed as its array.
    pub params: Vec<Type>,
    /// Whether the last parameter is a rest parameter.
    pub rest: bool,
    /// The return type.
    pub ret: Type,
}
