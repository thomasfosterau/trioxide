//! A data table of types and effects for the JavaScript builtins trioxide models (PLAN §2
//! step 4, "builtin models").
//!
//! The models assume the builtins are intact: nothing in the program replaces `Math.floor`,
//! patches `Array.prototype`, and so on. The analysis checks the program for such writes and
//! disables every model of an object that is written to (see `Analysis::patched_builtins`).
//!
//! An entry's effects describe the builtin itself, called with arguments of the documented
//! types. Coercing an argument that is not a primitive may run user code (`valueOf`), which
//! the analysis accounts for separately.

/// What a builtin call evaluates to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ret {
    /// A number, refined by [`NumberModel`].
    Number(NumberModel),
    /// `true` or `false`.
    Boolean,
    /// A primitive string.
    String,
    /// `undefined`.
    Undefined,
    /// A new array whose elements are the receiver's (`slice`, `filter`, `concat` of arrays).
    ReceiverElements,
    /// A new array whose element type is unknown (`map` returns the callback's results).
    NewArray,
    /// A new array of strings (`split`, `Object.keys`).
    StringArray,
    /// One of the receiver's elements, or `undefined` (`pop`, `find`, `at`).
    ElementOrUndefined,
    /// The receiver itself (`sort`, `reverse`, `fill`).
    Receiver,
    /// The first argument (`Object.freeze`).
    FirstArgument,
    /// A new typed array of the receiver's kind (`subarray` shares the buffer, `slice` copies).
    ReceiverTypedArray,
    /// Something trioxide does not model further.
    Unknown,
}

/// How the numeric result relates to the arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberModel {
    /// Any number, NaN included.
    Any,
    /// Any number except NaN.
    NonNaN,
    /// An int32.
    Int32,
    /// A uint32.
    Uint32,
    /// An integer in `[min, max]`.
    Int(i64, i64),
    /// An integer in `[-1, 2^32 - 2]`: an index or -1 (`indexOf`).
    IndexOrMinusOne,
    /// An array length, `[0, 2^32 - 1]`.
    Length,
    /// A UTF-16 code unit, or NaN when the index is out of range (`charCodeAt`).
    CodeUnitOrNaN,
    /// An integer or NaN (`parseInt`).
    IntOrNaN,
    /// `[0, 1)` (`Math.random`).
    Unit,
    /// Computed from the arguments by the interval rules in `numeric`.
    Math(MathOp),
}

/// The `Math` functions with interval rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathOp {
    Abs,
    Ceil,
    Floor,
    Round,
    Trunc,
    Sign,
    Sqrt,
    Min,
    Max,
    Wave,
    Fround,
}

/// What calling a builtin does besides returning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[expect(clippy::struct_excessive_bools, reason = "independent effect flags")]
pub struct BuiltinEffects {
    /// Reads mutable state (the receiver's elements, a clock, the PRNG).
    pub reads: bool,
    /// Writes the receiver (`push`, `sort`, `fill`).
    pub writes_receiver: bool,
    /// Writes state other than the receiver (the PRNG).
    pub writes_other: bool,
    /// Returns a new object.
    pub allocates: bool,
    /// May throw even with well-typed arguments (`JSON.parse`, `new Array(-1)`).
    pub throws: bool,
    /// Stores its arguments where they outlive the call (`push` into the receiver).
    pub retains_args: bool,
    /// Calls its function arguments.
    pub calls_args: bool,
}

/// The parameters a builtin passes to a callback argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackParam {
    /// An element of the receiver.
    Element,
    /// The element's index, `[0, 2^32 - 2]`.
    Index,
    /// The receiver.
    Receiver,
    /// The accumulator of `reduce`: the initial value or a callback result.
    Accumulator,
    /// Something not modelled.
    Unknown,
}

/// What kind of value a method is looked up on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receiver {
    /// A global function or value (`parseInt`, `NaN`) or a namespace member (`Math.floor`).
    Global,
    /// A proven `Array`.
    Array,
    /// A proven typed array.
    TypedArray,
    /// A proven primitive string.
    String,
    /// A proven number.
    Number,
}

/// One modelled builtin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Builtin {
    /// `Math.floor`, `parseInt`, or a method name for receiver kinds other than `Global`.
    pub name: &'static str,
    /// What it is looked up on.
    pub receiver: Receiver,
    /// `false` for values such as `Math.PI` (the `Ret` is then the value's type).
    pub callable: bool,
    /// The result.
    pub ret: Ret,
    /// The effects of a call.
    pub effects: BuiltinEffects,
    /// The parameters it passes to its first argument, if that is a callback.
    pub callback: &'static [CallbackParam],
}

const PURE: BuiltinEffects = BuiltinEffects {
    reads: false,
    writes_receiver: false,
    writes_other: false,
    allocates: false,
    throws: false,
    retains_args: false,
    calls_args: false,
};

const READS: BuiltinEffects = BuiltinEffects {
    reads: true,
    ..PURE
};

const COPY: BuiltinEffects = BuiltinEffects {
    reads: true,
    allocates: true,
    ..PURE
};

const MUTATES: BuiltinEffects = BuiltinEffects {
    reads: true,
    writes_receiver: true,
    ..PURE
};

const ITERATES: BuiltinEffects = BuiltinEffects {
    reads: true,
    throws: true,
    calls_args: true,
    ..PURE
};

const ELEMENT_CALLBACK: &[CallbackParam] =
    &[CallbackParam::Element, CallbackParam::Index, CallbackParam::Receiver];
const REDUCE_CALLBACK: &[CallbackParam] = &[
    CallbackParam::Accumulator,
    CallbackParam::Element,
    CallbackParam::Index,
    CallbackParam::Receiver,
];
const SORT_CALLBACK: &[CallbackParam] = &[CallbackParam::Element, CallbackParam::Element];

const fn value(name: &'static str, ret: Ret) -> Builtin {
    Builtin {
        name,
        receiver: Receiver::Global,
        callable: false,
        ret,
        effects: PURE,
        callback: &[],
    }
}

const fn function(name: &'static str, ret: Ret, effects: BuiltinEffects) -> Builtin {
    Builtin {
        name,
        receiver: Receiver::Global,
        callable: true,
        ret,
        effects,
        callback: &[],
    }
}

const fn math(name: &'static str, op: MathOp) -> Builtin {
    function(name, Ret::Number(NumberModel::Math(op)), PURE)
}

const fn method(
    receiver: Receiver,
    name: &'static str,
    ret: Ret,
    effects: BuiltinEffects,
    callback: &'static [CallbackParam],
) -> Builtin {
    Builtin {
        name,
        receiver,
        callable: true,
        ret,
        effects,
        callback,
    }
}

const fn number(model: NumberModel) -> Ret {
    Ret::Number(model)
}

use MathOp as M;
use NumberModel as N;
use Receiver as R;

/// Every modelled builtin.
pub static BUILTINS: &[Builtin] = &[
    // Global values and functions.
    value("NaN", number(N::Any)),
    value("Infinity", number(N::NonNaN)),
    value("undefined", Ret::Undefined),
    function("isNaN", Ret::Boolean, PURE),
    function("isFinite", Ret::Boolean, PURE),
    function("parseInt", number(N::IntOrNaN), PURE),
    function("parseFloat", number(N::Any), PURE),
    function("String", Ret::String, PURE),
    function("Number", number(N::Any), PURE),
    function("Boolean", Ret::Boolean, PURE),
    function("encodeURIComponent", Ret::String, BuiltinEffects { throws: true, ..PURE }),
    function("decodeURIComponent", Ret::String, BuiltinEffects { throws: true, ..PURE }),
    // Math.
    value("Math.PI", number(N::NonNaN)),
    value("Math.E", number(N::NonNaN)),
    value("Math.LN2", number(N::NonNaN)),
    value("Math.LN10", number(N::NonNaN)),
    value("Math.LOG2E", number(N::NonNaN)),
    value("Math.LOG10E", number(N::NonNaN)),
    value("Math.SQRT2", number(N::NonNaN)),
    value("Math.SQRT1_2", number(N::NonNaN)),
    math("Math.abs", M::Abs),
    math("Math.ceil", M::Ceil),
    math("Math.floor", M::Floor),
    math("Math.round", M::Round),
    math("Math.trunc", M::Trunc),
    math("Math.sign", M::Sign),
    math("Math.sqrt", M::Sqrt),
    math("Math.min", M::Min),
    math("Math.max", M::Max),
    math("Math.sin", M::Wave),
    math("Math.cos", M::Wave),
    math("Math.fround", M::Fround),
    function("Math.imul", number(N::Int32), PURE),
    function("Math.clz32", number(N::Int(0, 32)), PURE),
    function("Math.random", number(N::Unit), BuiltinEffects { reads: true, writes_other: true, ..PURE }),
    function("Math.tan", number(N::Any), PURE),
    function("Math.asin", number(N::Any), PURE),
    function("Math.acos", number(N::Any), PURE),
    function("Math.atan", number(N::Any), PURE),
    function("Math.atan2", number(N::Any), PURE),
    function("Math.exp", number(N::Any), PURE),
    function("Math.expm1", number(N::Any), PURE),
    function("Math.log", number(N::Any), PURE),
    function("Math.log1p", number(N::Any), PURE),
    function("Math.log2", number(N::Any), PURE),
    function("Math.log10", number(N::Any), PURE),
    function("Math.pow", number(N::Any), PURE),
    function("Math.cbrt", number(N::Any), PURE),
    function("Math.hypot", number(N::Any), PURE),
    function("Math.sinh", number(N::Any), PURE),
    function("Math.cosh", number(N::Any), PURE),
    function("Math.tanh", number(N::Any), PURE),
    // Number.
    value("Number.MAX_SAFE_INTEGER", number(N::Int(9_007_199_254_740_991, 9_007_199_254_740_991))),
    value("Number.MIN_SAFE_INTEGER", number(N::Int(-9_007_199_254_740_991, -9_007_199_254_740_991))),
    value("Number.EPSILON", number(N::NonNaN)),
    value("Number.MAX_VALUE", number(N::NonNaN)),
    value("Number.MIN_VALUE", number(N::NonNaN)),
    value("Number.POSITIVE_INFINITY", number(N::NonNaN)),
    value("Number.NEGATIVE_INFINITY", number(N::NonNaN)),
    value("Number.NaN", number(N::Any)),
    function("Number.isInteger", Ret::Boolean, PURE),
    function("Number.isSafeInteger", Ret::Boolean, PURE),
    function("Number.isFinite", Ret::Boolean, PURE),
    function("Number.isNaN", Ret::Boolean, PURE),
    function("Number.parseInt", number(N::IntOrNaN), PURE),
    function("Number.parseFloat", number(N::Any), PURE),
    method(R::Number, "toFixed", Ret::String, BuiltinEffects { throws: true, ..PURE }, &[]),
    method(R::Number, "toString", Ret::String, BuiltinEffects { throws: true, ..PURE }, &[]),
    method(R::Number, "toPrecision", Ret::String, BuiltinEffects { throws: true, ..PURE }, &[]),
    // JSON.
    function("JSON.stringify", Ret::Unknown, BuiltinEffects { reads: true, throws: true, calls_args: true, ..PURE }),
    function("JSON.parse", Ret::Unknown, BuiltinEffects { allocates: true, throws: true, calls_args: true, ..PURE }),
    // Object.
    function("Object.keys", Ret::StringArray, BuiltinEffects { throws: true, ..COPY }),
    function("Object.values", Ret::NewArray, BuiltinEffects { throws: true, ..COPY }),
    function("Object.entries", Ret::NewArray, BuiltinEffects { throws: true, ..COPY }),
    function("Object.is", Ret::Boolean, PURE),
    function("Object.freeze", Ret::FirstArgument, BuiltinEffects { writes_other: true, ..PURE }),
    function("Object.isFrozen", Ret::Boolean, READS),
    // Array and String statics.
    function("Array.isArray", Ret::Boolean, PURE),
    function("String.fromCharCode", Ret::String, PURE),
    function("Date.now", number(N::NonNaN), BuiltinEffects { reads: true, ..PURE }),
    // Array.prototype.
    method(R::Array, "at", Ret::ElementOrUndefined, READS, &[]),
    method(R::Array, "includes", Ret::Boolean, READS, &[]),
    method(R::Array, "indexOf", number(N::IndexOrMinusOne), READS, &[]),
    method(R::Array, "lastIndexOf", number(N::IndexOrMinusOne), READS, &[]),
    method(R::Array, "join", Ret::String, BuiltinEffects { throws: true, calls_args: true, ..READS }, &[]),
    method(R::Array, "slice", Ret::ReceiverElements, COPY, &[]),
    method(R::Array, "concat", Ret::NewArray, COPY, &[]),
    method(R::Array, "map", Ret::NewArray, BuiltinEffects { allocates: true, ..ITERATES }, ELEMENT_CALLBACK),
    method(R::Array, "filter", Ret::ReceiverElements, BuiltinEffects { allocates: true, ..ITERATES }, ELEMENT_CALLBACK),
    method(R::Array, "forEach", Ret::Undefined, ITERATES, ELEMENT_CALLBACK),
    method(R::Array, "some", Ret::Boolean, ITERATES, ELEMENT_CALLBACK),
    method(R::Array, "every", Ret::Boolean, ITERATES, ELEMENT_CALLBACK),
    method(R::Array, "find", Ret::ElementOrUndefined, ITERATES, ELEMENT_CALLBACK),
    method(R::Array, "findIndex", number(N::IndexOrMinusOne), ITERATES, ELEMENT_CALLBACK),
    method(R::Array, "reduce", Ret::Unknown, ITERATES, REDUCE_CALLBACK),
    method(R::Array, "push", number(N::Length), BuiltinEffects { retains_args: true, throws: true, ..MUTATES }, &[]),
    method(R::Array, "pop", Ret::ElementOrUndefined, MUTATES, &[]),
    method(R::Array, "shift", Ret::ElementOrUndefined, MUTATES, &[]),
    method(R::Array, "unshift", number(N::Length), BuiltinEffects { retains_args: true, throws: true, ..MUTATES }, &[]),
    method(R::Array, "reverse", Ret::Receiver, MUTATES, &[]),
    method(R::Array, "sort", Ret::Receiver, BuiltinEffects { throws: true, calls_args: true, ..MUTATES }, SORT_CALLBACK),
    method(R::Array, "fill", Ret::Receiver, BuiltinEffects { retains_args: true, ..MUTATES }, &[]),
    // Typed arrays (the `%TypedArray%.prototype` methods).
    method(R::TypedArray, "at", Ret::ElementOrUndefined, READS, &[]),
    method(R::TypedArray, "includes", Ret::Boolean, READS, &[]),
    method(R::TypedArray, "indexOf", number(N::IndexOrMinusOne), READS, &[]),
    method(R::TypedArray, "fill", Ret::Receiver, MUTATES, &[]),
    method(R::TypedArray, "set", Ret::Undefined, BuiltinEffects { throws: true, ..MUTATES }, &[]),
    method(R::TypedArray, "subarray", Ret::ReceiverTypedArray, BuiltinEffects { allocates: true, ..PURE }, &[]),
    method(R::TypedArray, "slice", Ret::ReceiverTypedArray, COPY, &[]),
    method(R::TypedArray, "forEach", Ret::Undefined, ITERATES, ELEMENT_CALLBACK),
    method(R::TypedArray, "reduce", Ret::Unknown, ITERATES, REDUCE_CALLBACK),
    method(R::TypedArray, "sort", Ret::Receiver, BuiltinEffects { throws: true, calls_args: true, ..MUTATES }, SORT_CALLBACK),
    // String.prototype (on primitive strings, which nothing can mutate).
    method(R::String, "charAt", Ret::String, PURE, &[]),
    method(R::String, "charCodeAt", number(N::CodeUnitOrNaN), PURE, &[]),
    method(R::String, "codePointAt", Ret::Unknown, PURE, &[]),
    method(R::String, "indexOf", number(N::Int(-1, 9_007_199_254_740_990)), PURE, &[]),
    method(R::String, "lastIndexOf", number(N::Int(-1, 9_007_199_254_740_990)), PURE, &[]),
    method(R::String, "includes", Ret::Boolean, BuiltinEffects { throws: true, ..PURE }, &[]),
    method(R::String, "startsWith", Ret::Boolean, BuiltinEffects { throws: true, ..PURE }, &[]),
    method(R::String, "endsWith", Ret::Boolean, BuiltinEffects { throws: true, ..PURE }, &[]),
    method(R::String, "slice", Ret::String, PURE, &[]),
    method(R::String, "substring", Ret::String, PURE, &[]),
    method(R::String, "toUpperCase", Ret::String, PURE, &[]),
    method(R::String, "toLowerCase", Ret::String, PURE, &[]),
    method(R::String, "trim", Ret::String, PURE, &[]),
    method(R::String, "trimStart", Ret::String, PURE, &[]),
    method(R::String, "trimEnd", Ret::String, PURE, &[]),
    method(R::String, "padStart", Ret::String, BuiltinEffects { throws: true, ..PURE }, &[]),
    method(R::String, "padEnd", Ret::String, BuiltinEffects { throws: true, ..PURE }, &[]),
    method(R::String, "repeat", Ret::String, BuiltinEffects { throws: true, ..PURE }, &[]),
    method(R::String, "split", Ret::StringArray, BuiltinEffects { allocates: true, throws: true, ..PURE }, &[]),
    method(R::String, "localeCompare", number(N::NonNaN), PURE, &[]),
    method(R::String, "concat", Ret::String, PURE, &[]),
    method(R::String, "at", Ret::Unknown, PURE, &[]),
];

/// The global namespace objects whose members are modelled; a write to any of them (or a
/// shadowing binding) disables their models.
pub const NAMESPACES: [&str; 7] = ["Math", "Number", "JSON", "Object", "Array", "String", "Date"];

/// The model of a global or namespace member, e.g. `"Math.floor"`.
pub fn global(name: &str) -> Option<&'static Builtin> {
    BUILTINS
        .iter()
        .find(|builtin| builtin.receiver == Receiver::Global && builtin.name == name)
}

/// The model of a method on a proven receiver kind.
pub fn method_of(receiver: Receiver, name: &str) -> Option<&'static Builtin> {
    BUILTINS
        .iter()
        .find(|builtin| builtin.receiver == receiver && builtin.name == name)
}

/// Properties every plain object inherits from `Object.prototype`. Reading one that is not an
/// own property yields a function, not `undefined`.
pub const OBJECT_PROTOTYPE: [&str; 12] = [
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
    "__proto__",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_per_receiver() {
        for (index, builtin) in BUILTINS.iter().enumerate() {
            assert!(
                !BUILTINS[..index]
                    .iter()
                    .any(|other| other.receiver == builtin.receiver && other.name == builtin.name),
                "{} is listed twice",
                builtin.name
            );
        }
        assert_eq!(global("Math.floor").unwrap().ret, Ret::Number(NumberModel::Math(MathOp::Floor)));
        assert!(method_of(Receiver::Array, "push").unwrap().effects.writes_receiver);
        assert_eq!(method_of(Receiver::Array, "map").unwrap().callback[1], CallbackParam::Index);
    }
}
