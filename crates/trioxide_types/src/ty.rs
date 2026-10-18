use std::sync::Arc;

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
///
/// Nested parts are shared (`Arc`), so cloning a type is cheap.
#[derive(Debug, Clone, Default)]
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
    Array(Arc<Type>),
    /// A typed array view such as `Float64Array`.
    TypedArray(TypedArrayKind),
    /// A fixed-length array with per-position types.
    Tuple(Arc<[Type]>),
    /// A plain object with a closed shape.
    Object(Arc<Shape>),
    /// An instance of a class whose fields are known and not mutated dynamically.
    Instance(ClassId),
    /// A callable, with the function it is known to be, if any.
    Function(Arc<Signature>, Option<FunctionId>),
    /// One of several types. Never nested and never a single member.
    Union(Arc<[Type]>),
}

impl Type {
    /// The type with every class and function id it names passed through `map`, or `None` if
    /// `map` has no answer for one of them.
    pub fn try_map_ids(&self, map: &impl Fn(NodeRef) -> Option<NodeRef>) -> Option<Self> {
        let all = |types: &[Self]| -> Option<Vec<Self>> {
            types.iter().map(|ty| ty.try_map_ids(map)).collect()
        };
        Some(match self {
            Self::Array(element) => Self::Array(Arc::new(element.try_map_ids(map)?)),
            Self::Tuple(items) => Self::Tuple(all(items)?.into()),
            Self::Union(members) => Self::Union(all(members)?.into()),
            Self::Object(shape) => Self::Object(Arc::new(Shape {
                properties: shape
                    .properties
                    .iter()
                    .map(|(key, ty)| Some((key.clone(), ty.try_map_ids(map)?)))
                    .collect::<Option<_>>()?,
                exact: shape.exact,
            })),
            Self::Instance(class) => Self::Instance(map(*class)?),
            Self::Function(signature, id) => Self::Function(
                Arc::new(Signature {
                    params: all(&signature.params)?,
                    rest: signature.rest,
                    ret: signature.ret.try_map_ids(map)?,
                }),
                match id {
                    Some(id) => Some(map(*id)?),
                    None => None,
                },
            ),
            other => other.clone(),
        })
    }

    /// A function with nothing known about its signature, known to be `id` if given.
    pub fn opaque_function(id: Option<FunctionId>) -> Self {
        static UNKNOWN: std::sync::LazyLock<Arc<Signature>> =
            std::sync::LazyLock::new(Arc::default);
        Self::Function(Arc::clone(&UNKNOWN), id)
    }

    /// An object with no known properties.
    pub fn empty_object() -> Self {
        static EMPTY: std::sync::LazyLock<Arc<Shape>> = std::sync::LazyLock::new(Arc::default);
        Self::Object(Arc::clone(&EMPTY))
    }
}

impl PartialEq for Type {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        if std::mem::discriminant(self) != std::mem::discriminant(other) {
            return false;
        }
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a == b,
            (Self::Array(a), Self::Array(b)) => Arc::ptr_eq(a, b) || a == b,
            (Self::TypedArray(a), Self::TypedArray(b)) => a == b,
            (Self::Tuple(a), Self::Tuple(b)) | (Self::Union(a), Self::Union(b)) => {
                Arc::ptr_eq(a, b) || a == b
            }
            (Self::Object(a), Self::Object(b)) => Arc::ptr_eq(a, b) || a == b,
            (Self::Instance(a), Self::Instance(b)) => a == b,
            (Self::Function(a, x), Self::Function(b, y)) => x == y && (Arc::ptr_eq(a, b) || a == b),
            (
                Self::Unknown
                | Self::Never
                | Self::Boolean
                | Self::String
                | Self::BigInt
                | Self::Symbol
                | Self::Null
                | Self::Undefined,
                _,
            ) => true,
            _ => false,
        }
    }
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

    /// `[min, max]`.
    pub const fn new(min: f64, max: f64) -> Self {
        Self { min, max }
    }

    /// The smallest interval containing both.
    #[must_use]
    pub const fn hull(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    /// Whether every value of `other` is in `self`.
    pub fn contains(self, other: Self) -> bool {
        self.min <= other.min && other.max <= self.max
    }

    /// Whether this is `[-inf, +inf]`.
    pub fn is_full(self) -> bool {
        self.min == f64::NEG_INFINITY && self.max == f64::INFINITY
    }

    /// The single value, if the interval holds exactly one.
    pub fn as_point(self) -> Option<f64> {
        #[expect(clippy::float_cmp, reason = "an exact point")]
        (self.min == self.max).then_some(self.min)
    }
}

impl NumberType {
    /// Any number, NaN and `-0` included.
    pub const ANY: Self = Self {
        range: Interval::FULL,
        int: false,
        non_nan: false,
        non_neg_zero: false,
    };

    /// Exactly `value`.
    pub fn constant(value: f64) -> Self {
        if value.is_nan() {
            return Self::ANY;
        }
        Self {
            range: Interval::point(value),
            int: value.fract() == 0.0 || value.is_infinite(),
            non_nan: true,
            non_neg_zero: !(value == 0.0 && value.is_sign_negative()),
        }
    }

    /// The single value this type allows, if any (NaN excluded).
    pub fn as_constant(self) -> Option<f64> {
        if !self.non_nan {
            return None;
        }
        let value = self.range.as_point()?;
        // `[0, 0]` without `nonNegZero` may be `-0`.
        (value != 0.0 || self.non_neg_zero).then_some(value)
    }

    /// A number that is either of these.
    #[must_use]
    pub const fn join(self, other: Self) -> Self {
        Self {
            range: self.range.hull(other.range),
            int: self.int && other.int,
            non_nan: self.non_nan && other.non_nan,
            non_neg_zero: self.non_neg_zero && other.non_neg_zero,
        }
    }

    /// Whether every value of `other` is allowed by `self`.
    pub fn contains(self, other: Self) -> bool {
        self.range.contains(other.range)
            && (!self.int || other.int)
            && (!self.non_nan || other.non_nan)
            && (!self.non_neg_zero || other.non_neg_zero)
    }
}

impl Type {
    /// Any number.
    pub const NUMBER: Self = Self::Number(NumberType::ANY);

    /// A value that is one of `types`: flattens nested unions, drops `Never` and duplicates,
    /// merges every number into one [`NumberType`], and is `Unknown` if any member is.
    pub fn union(types: impl IntoIterator<Item = Self>) -> Self {
        let mut members: Vec<Self> = Vec::new();
        let mut number: Option<NumberType> = None;
        let mut pending: Vec<Self> = types.into_iter().collect();
        while let Some(ty) = pending.pop() {
            match ty {
                Self::Unknown => return Self::Unknown,
                Self::Never => {}
                Self::Union(inner) => pending.extend(inner.iter().cloned()),
                Self::Number(n) => number = Some(number.map_or(n, |m| m.join(n))),
                other => {
                    if !members.contains(&other) {
                        members.push(other);
                    }
                }
            }
        }
        if let Some(number) = number {
            members.push(Self::Number(number));
        }
        members.reverse();
        // A canonical order, so equal unions compare equal whatever order they were built in.
        members.sort_by(Self::canonical_cmp);
        match members.len() {
            0 => Self::Never,
            1 => members.pop().expect("one member"),
            _ => Self::Union(members.into()),
        }
    }

    /// A total order on types: primitives first, in a fixed order, then structured types by
    /// variant and contents. It agrees with `==` except on number bounds, which it compares
    /// with `f64::total_cmp` (so `-0` sorts before `0`, which `==` treats as equal).
    fn canonical_cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        fn number(a: &NumberType, b: &NumberType) -> Ordering {
            a.range
                .min
                .total_cmp(&b.range.min)
                .then(a.range.max.total_cmp(&b.range.max))
                .then(a.int.cmp(&b.int))
                .then(a.non_nan.cmp(&b.non_nan))
                .then(a.non_neg_zero.cmp(&b.non_neg_zero))
        }
        fn list(a: &[Type], b: &[Type]) -> Ordering {
            a.len().cmp(&b.len()).then_with(|| {
                a.iter()
                    .zip(b)
                    .map(|(a, b)| a.canonical_cmp(b))
                    .find(|o| o.is_ne())
                    .unwrap_or(Ordering::Equal)
            })
        }
        fn variant(ty: &Type) -> u8 {
            match ty {
                Type::Unknown => 0,
                Type::Never => 1,
                Type::Undefined => 2,
                Type::Null => 3,
                Type::Boolean => 4,
                Type::Number(_) => 5,
                Type::BigInt => 6,
                Type::String => 7,
                Type::Symbol => 8,
                Type::Array(_) => 9,
                Type::TypedArray(_) => 10,
                Type::Tuple(_) => 11,
                Type::Object(_) => 12,
                Type::Instance(_) => 13,
                Type::Function(..) => 14,
                Type::Union(_) => 15,
            }
        }
        let by_variant = self
            .union_rank()
            .cmp(&other.union_rank())
            .then(variant(self).cmp(&variant(other)));
        if by_variant.is_ne() {
            return by_variant;
        }
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => number(a, b),
            (Self::Array(a), Self::Array(b)) => a.canonical_cmp(b),
            (Self::TypedArray(a), Self::TypedArray(b)) => (*a as u8).cmp(&(*b as u8)),
            (Self::Tuple(a), Self::Tuple(b)) | (Self::Union(a), Self::Union(b)) => list(a, b),
            (Self::Object(a), Self::Object(b)) => a.exact.cmp(&b.exact).then_with(|| {
                a.properties.len().cmp(&b.properties.len()).then_with(|| {
                    a.properties
                        .iter()
                        .zip(&b.properties)
                        .map(|((ka, ta), (kb, tb))| ka.cmp(kb).then_with(|| ta.canonical_cmp(tb)))
                        .find(|o| o.is_ne())
                        .unwrap_or(Ordering::Equal)
                })
            }),
            (Self::Instance(a), Self::Instance(b)) => a.cmp(b),
            (Self::Function(a, x), Self::Function(b, y)) => x.cmp(y).then_with(|| {
                a.rest
                    .cmp(&b.rest)
                    .then_with(|| list(&a.params, &b.params))
                    .then_with(|| a.ret.canonical_cmp(&b.ret))
            }),
            _ => Ordering::Equal,
        }
    }

    fn union_rank(&self) -> u8 {
        match self {
            Self::Undefined => 0,
            Self::Null => 1,
            Self::Boolean => 2,
            Self::Number(_) => 3,
            Self::BigInt => 4,
            Self::String => 5,
            Self::Symbol => 6,
            _ => 7,
        }
    }

    /// The least type containing both (the lattice join). Arrays join element-wise; tuples of
    /// one length join position-wise; anything else becomes a union.
    #[must_use]
    pub fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Never, ty) | (ty, Self::Never) => ty.clone(),
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (a, b) if a == b => a.clone(),
            (Self::Number(a), Self::Number(b)) => Self::Number(a.join(*b)),
            (Self::Array(a), Self::Array(b)) => Self::Array(Arc::new(a.join(b))),
            (Self::Tuple(a), Self::Tuple(b)) if a.len() == b.len() => {
                Self::Tuple(a.iter().zip(b.iter()).map(|(a, b)| a.join(b)).collect())
            }
            (Self::Tuple(a), Self::Array(b)) | (Self::Array(b), Self::Tuple(a)) => Self::Array(
                Arc::new(a.iter().fold((**b).clone(), |acc, ty| acc.join(ty))),
            ),
            (a, b) => Self::union([a.clone(), b.clone()]),
        }
    }

    /// The members of a union, or the type itself.
    pub fn members(&self) -> &[Self] {
        match self {
            Self::Union(members) => members,
            other => std::slice::from_ref(other),
        }
    }

    /// The number part of the type, if every member is a number.
    pub fn as_number(&self) -> Option<NumberType> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Whether every value of this type is a primitive other than a bigint (so arithmetic on it
    /// never throws, runs user code or yields a bigint).
    pub fn is_non_bigint_primitive(&self) -> bool {
        self.members().iter().all(|ty| {
            matches!(
                ty,
                Self::Number(_) | Self::Boolean | Self::String | Self::Null | Self::Undefined
            )
        }) && !matches!(self, Self::Never)
    }

    /// Whether the type allows `null` or `undefined`.
    pub fn may_be_nullish(&self) -> bool {
        self.members()
            .iter()
            .any(|ty| matches!(ty, Self::Unknown | Self::Null | Self::Undefined))
    }

    /// The type without `null` and `undefined`.
    #[must_use]
    pub fn non_nullish(&self) -> Self {
        Self::union(
            self.members()
                .iter()
                .filter(|ty| !matches!(ty, Self::Null | Self::Undefined))
                .cloned(),
        )
    }

    /// How deep the type nests (a primitive is 1).
    pub fn depth(&self) -> usize {
        match self {
            Self::Array(elem) => 1 + elem.depth(),
            Self::Tuple(items) | Self::Union(items) => {
                1 + items.iter().map(Self::depth).max().unwrap_or(0)
            }
            Self::Object(shape) => {
                1 + shape
                    .properties
                    .iter()
                    .map(|(_, ty)| ty.depth())
                    .max()
                    .unwrap_or(0)
            }
            Self::Function(signature, _) => {
                1 + signature
                    .params
                    .iter()
                    .chain([&signature.ret])
                    .map(Self::depth)
                    .max()
                    .unwrap_or(0)
            }
            _ => 1,
        }
    }

    /// The type with everything nested deeper than `depth` replaced by `Unknown`, which keeps
    /// recursive types finite.
    #[must_use]
    pub fn truncate(&self, depth: usize) -> Self {
        if depth == 0 {
            return Self::Unknown;
        }
        let inner = |ty: &Self| ty.truncate(depth - 1);
        match self {
            Self::Array(elem) => Self::Array(Arc::new(inner(elem))),
            Self::Tuple(items) => Self::Tuple(items.iter().map(inner).collect()),
            Self::Union(items) => Self::union(items.iter().map(|ty| ty.truncate(depth))),
            Self::Object(shape) => Self::Object(Arc::new(Shape {
                properties: shape
                    .properties
                    .iter()
                    .map(|(key, ty)| (key.clone(), inner(ty)))
                    .collect(),
                exact: shape.exact,
            })),
            Self::Function(signature, id) => Self::Function(
                Arc::new(Signature {
                    params: signature.params.iter().map(inner).collect(),
                    rest: signature.rest,
                    ret: inner(&signature.ret),
                }),
                *id,
            ),
            other => other.clone(),
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
    /// Every kind, in declaration order.
    pub const ALL: [Self; 11] = [
        Self::Int8,
        Self::Uint8,
        Self::Uint8Clamped,
        Self::Int16,
        Self::Uint16,
        Self::Int32,
        Self::Uint32,
        Self::Float32,
        Self::Float64,
        Self::BigInt64,
        Self::BigUint64,
    ];

    /// The kind whose constructor is called `name`.
    pub fn from_constructor_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.constructor_name() == name)
    }

    /// The values an element read yields, or `None` for the bigint kinds.
    pub fn element(self) -> Option<NumberType> {
        let int = |min: f64, max: f64| NumberType {
            range: Interval::new(min, max),
            int: true,
            non_nan: true,
            non_neg_zero: true,
        };
        Some(match self {
            Self::Int8 => int(-128.0, 127.0),
            Self::Uint8 | Self::Uint8Clamped => int(0.0, 255.0),
            Self::Int16 => int(-32768.0, 32767.0),
            Self::Uint16 => int(0.0, 65535.0),
            Self::Int32 => int(-2_147_483_648.0, 2_147_483_647.0),
            Self::Uint32 => int(0.0, 4_294_967_295.0),
            Self::Float32 | Self::Float64 => NumberType::ANY,
            Self::BigInt64 | Self::BigUint64 => return None,
        })
    }

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

/// The known properties of an object, in declaration order.
///
/// TypeScript object types are open (a value may carry more properties than its type names),
/// so a shape from the type oracle is never `exact`. Only the analysis, which sees where an
/// object literal is created and every use of it, can prove a shape exact.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shape {
    /// `(key, type)` pairs. Keys are unique.
    pub properties: Vec<(String, Type)>,
    /// The object has exactly these own properties, plain data (no getters or setters), and
    /// `Object.prototype` as its prototype.
    pub exact: bool,
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
