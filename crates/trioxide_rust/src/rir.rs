//! RIR, the Rust IR (PLAN §6): a small typed tree IR for one island.
//!
//! Every value has a [`Ty`] and every operation states the JavaScript semantics it implements
//! ([`BinOp`], [`MathFn`], …), so a backend never has to rediscover them. The emitter in
//! [`crate::emit`] prints it as Rust; nothing here is Rust-specific beyond the shape of the
//! types, so a direct `wasm-encoder` backend stays possible (D6).

use trioxide_wasm::{AbiType, TypedArrayElement};

/// A value type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ty {
    /// A JS number.
    F64,
    /// A JS number proven to be an int32 (and never `-0`).
    I32,
    /// A JS number proven to be a uint32 (and never `-0`).
    U32,
    /// A JS boolean.
    Bool,
    /// A non-negative array index kept by the backend itself (`for…of` counters).
    Index,
    /// An array.
    Array(ArrayTy),
    /// A struct value, by index into [`Island::structs`].
    Struct(StructRef),
    /// A reference to a struct the caller owns (a boundary struct parameter).
    StructRef(StructRef, Access),
    /// No value (`undefined` returns).
    Unit,
}

impl Ty {
    /// Whether the type is a number representation.
    pub fn is_number(self) -> bool {
        matches!(self, Self::F64 | Self::I32 | Self::U32)
    }
}

/// An array type: its elements and how the holder may use it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArrayTy {
    /// The element type.
    pub elem: Elem,
    /// How it is held.
    pub access: Access,
}

/// How an array or struct reference is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Access {
    /// Read only, borrowed (`&[E]`, `&S`).
    Shared,
    /// Written through, borrowed from the caller (`&mut [E]`, `&mut S`).
    Mut,
    /// Created during this call and owned until the arena resets (`&'static mut [E]`).
    Fresh,
}

/// An array element type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Elem {
    /// A typed array element.
    Typed(TypedArrayElement),
    /// A `number[]` element: stored as `f64`; holes read as `NaN`.
    Number,
}

impl Elem {
    /// The element as stored in linear memory.
    pub fn storage(self) -> TypedArrayElement {
        match self {
            Self::Typed(element) => element,
            Self::Number => TypedArrayElement::F64,
        }
    }
}

/// A struct: index into [`Island::structs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StructRef(pub usize);

/// A function: index into [`Island::functions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FuncRef(pub usize);

/// A local variable or parameter: index into [`Function::locals`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalId(pub usize);

/// A module-level constant: index into [`Island::consts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstRef(pub usize);

/// A loop, for labelled `break` and `continue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LoopId(pub usize);

/// A whole island: one crate.
#[derive(Debug, Clone, Default)]
pub struct Island {
    /// The island id (`IslandSpec::id`).
    pub id: String,
    /// The module the island comes from, for the crate's doc comment.
    pub module: String,
    /// Structs, boundary ones first in spec order.
    pub structs: Vec<StructDef>,
    /// Module-level constants the island reads.
    pub consts: Vec<ConstDef>,
    /// Every function, internal or exported.
    pub functions: Vec<Function>,
    /// The boundary.
    pub exports: Vec<Export>,
}

/// A struct definition.
#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    /// Its Rust (and spec) name.
    pub name: String,
    /// Fields in order.
    pub fields: Vec<(String, Ty)>,
    /// What it stands for in the source, for the doc comment.
    pub doc: String,
    /// For structs crossing the boundary: the index in `IslandSpec::structs` (the layout is
    /// C's and asserted). `None` for internal classes, whose layout is Rust's.
    pub abi: Option<usize>,
    /// For classes: their constructor and methods.
    pub methods: Vec<FuncRef>,
}

/// A module-level constant.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstDef {
    /// Its Rust name.
    pub name: String,
    /// Its value.
    pub value: Lit,
}

/// A literal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Lit {
    /// A number.
    F64(f64),
    /// An int32.
    I32(i32),
    /// A uint32.
    U32(u32),
    /// A boolean.
    Bool(bool),
}

impl Lit {
    /// The literal's type.
    pub fn ty(self) -> Ty {
        match self {
            Self::F64(_) => Ty::F64,
            Self::I32(_) => Ty::I32,
            Self::U32(_) => Ty::U32,
            Self::Bool(_) => Ty::Bool,
        }
    }
}

/// What kind of function a [`Function`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionKind {
    /// A plain function.
    Free,
    /// A class method, taking the instance by value as `self`.
    Method(StructRef),
    /// A class constructor, returning the instance.
    Constructor(StructRef),
}

/// A function.
#[derive(Debug, Clone)]
pub struct Function {
    /// Its Rust name, unique in the island.
    pub name: String,
    /// What it is.
    pub kind: FunctionKind,
    /// Its parameters, as locals (for methods, `self` is not among them).
    pub params: Vec<LocalId>,
    /// Every local, parameters included.
    pub locals: Vec<Local>,
    /// The return type.
    pub ret: Ty,
    /// The body.
    pub body: Vec<Stmt>,
}

/// A local variable.
#[derive(Debug, Clone, PartialEq)]
pub struct Local {
    /// Its Rust name, unique in the function.
    pub name: String,
    /// Its type.
    pub ty: Ty,
    /// Whether it is assigned after its declaration.
    pub mutable: bool,
}

/// An exported function: one ABI shim.
#[derive(Debug, Clone, PartialEq)]
pub struct Export {
    /// The function.
    pub func: FuncRef,
    /// The JS (and wasm) name.
    pub name: String,
    /// Each parameter's JS name, ABI type and whether the island writes to it.
    pub params: Vec<(String, AbiType, bool)>,
    /// The ABI return type.
    pub ret: Option<AbiType>,
}

/// A statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// Declares a local with its initial value.
    Let(LocalId, Expr),
    /// Assigns a local.
    Assign(LocalId, Expr),
    /// `a[i] = v` on an array.
    ArraySet {
        /// The array.
        array: Expr,
        /// The index.
        index: Index,
        /// The value, a number.
        value: Expr,
    },
    /// `s.f = v` on a mutable struct reference.
    FieldSet {
        /// The struct.
        base: Expr,
        /// The field.
        field: String,
        /// The value.
        value: Expr,
    },
    /// Evaluates for effects.
    Expr(Expr),
    /// `if`.
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    /// A loop: runs `body` while `test` holds, then `update` after each iteration
    /// (`continue` included).
    Loop(Loop),
    /// `for (const x of xs)` over an array: binds each element to `local`.
    ForOf {
        /// The loop.
        id: LoopId,
        /// The counter (a [`Ty::Index`] local).
        counter: LocalId,
        /// The array.
        array: Expr,
        /// The element local.
        local: LocalId,
        /// How the element is read.
        elem: Elem,
        /// The body.
        body: Vec<Stmt>,
    },
    /// `break`, out of the given loop (`None`: the innermost).
    Break(LoopId),
    /// `continue` the given loop.
    Continue(LoopId),
    /// `return`.
    Return(Option<Expr>),
    /// Code the analysis proves never runs: traps if it does.
    Unreachable,
    /// A labelled block, exited by `break` from inside (a labelled non-loop statement).
    Block(Option<LoopId>, Vec<Stmt>),
}

/// A `for`/`while`/`do…while` loop.
#[derive(Debug, Clone, PartialEq)]
pub struct Loop {
    /// Its id, for `break` and `continue`.
    pub id: LoopId,
    /// The condition, checked before each iteration (after the first, for `do…while`).
    pub test: Option<Expr>,
    /// Whether the test runs after the body (`do…while`).
    pub test_after: bool,
    /// The body.
    pub body: Vec<Stmt>,
    /// The update, run after the body and after `continue`.
    pub update: Vec<Stmt>,
    /// Whether a `break` or `continue` names it from a nested loop.
    pub labelled: bool,
    /// Whether the body contains a `continue` of this loop.
    pub has_continue: bool,
}

/// How an array index is computed and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Index {
    /// The index value.
    pub value: Expr,
    /// Its type: [`Ty::F64`], [`Ty::I32`], [`Ty::U32`] or [`Ty::Index`].
    pub ty: Ty,
    /// The analysis proved it in bounds (and the array has no holes), so no check is needed.
    pub in_bounds: bool,
}

/// A unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    /// `-x` on `f64`.
    NegF64,
    /// `-x` on `i32`, proven not to overflow or produce `-0`.
    NegI32,
    /// `!b`.
    Not,
    /// `~x` on `i32`.
    BitNot,
}

/// A binary operator, with the representation it works on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// `+` on `f64`.
    Add,
    /// `-` on `f64`.
    Sub,
    /// `*` on `f64`.
    Mul,
    /// `/` on `f64`.
    Div,
    /// `%` on `f64` (`fmod`, exact).
    Rem,
    /// `+` on `i32` or `u32` operands whose result is proven in range (so it never wraps).
    IntAdd,
    /// `-`, as [`BinOp::IntAdd`].
    IntSub,
    /// `*`, as [`BinOp::IntAdd`].
    IntMul,
    /// `+` on `i32`s (or `u32`s) that may wrap: the result goes straight through `ToInt32`, or
    /// an operand was reinterpreted between `i32` and `u32` while the result is proven in
    /// range, so arithmetic modulo 2^32 gives the JS result.
    WrapAdd,
    /// `-`, as [`BinOp::WrapAdd`].
    WrapSub,
    /// `*`, as [`BinOp::WrapAdd`].
    WrapMul,
    /// `%` on two int32s: an `f64`, since the result may be `-0` or `NaN`.
    RemI32,
    /// `(a / b) | 0` on two int32s.
    DivTruncI32,
    /// `&` on `i32`.
    BitAnd,
    /// `|` on `i32`.
    BitOr,
    /// `^` on `i32`.
    BitXor,
    /// `<<` on `i32` (count mod 32).
    Shl,
    /// `>>` on `i32` (count mod 32).
    Shr,
    /// `>>>` on `i32`, giving `u32`.
    Ushr,
    /// `<`.
    Lt,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
    /// `===` / `==` on numbers or booleans.
    Eq,
    /// `!==` / `!=`.
    Ne,
    /// `&&` on booleans.
    And,
    /// `||` on booleans.
    Or,
}

/// A `Math` function, with JS semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathFn {
    /// `Math.abs`.
    Abs,
    /// `Math.floor`.
    Floor,
    /// `Math.ceil`.
    Ceil,
    /// `Math.trunc`.
    Trunc,
    /// `Math.sqrt`.
    Sqrt,
    /// `Math.round`.
    Round,
    /// `Math.sign`.
    Sign,
    /// `Math.fround`.
    Fround,
    /// `Math.clz32`, returning `u32`.
    Clz32,
    /// `Math.imul` on two `f64`s, returning `i32`.
    Imul,
    /// `Math.imul` on two `i32`s.
    ImulI32,
    /// `Math.max` of two arguments; more fold left.
    Max,
    /// `Math.min` of two arguments; more fold left.
    Min,
    /// `Math.hypot` of two arguments.
    Hypot2,
    /// `Math.hypot` of three arguments.
    Hypot3,
    /// `Math.tan`.
    Tan,
    /// `Math.asin`.
    Asin,
    /// `Math.acos`.
    Acos,
    /// `Math.atan`.
    Atan,
    /// `Math.atan2`.
    Atan2,
    /// `Math.sinh`.
    Sinh,
    /// `Math.atanh`.
    Atanh,
    /// `Math.expm1`.
    Expm1,
    /// `Math.log1p`.
    Log1p,
    /// `Math.log2`.
    Log2,
}

/// A numeric conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conv {
    /// `ToInt32` of an `f64`.
    ToInt32,
    /// `ToUint32` of an `f64`.
    ToUint32,
    /// An exact conversion: the value is representable in the target (`i32` → `f64`, or an
    /// `f64` the analysis proves is an int32, …), or a bit reinterpretation between `i32`
    /// and `u32` where JS would apply `ToInt32`/`ToUint32`.
    Exact,
}

/// An expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A literal.
    Lit(Lit),
    /// A local.
    Local(LocalId),
    /// A module-level constant.
    Const(ConstRef),
    /// `self` inside a method.
    SelfValue,
    /// A unary operation.
    Unary(UnOp, Box<Expr>),
    /// A binary operation.
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// A conversion between number representations.
    Convert {
        /// The conversion.
        conv: Conv,
        /// From.
        from: Ty,
        /// To.
        to: Ty,
        /// The value.
        arg: Box<Expr>,
    },
    /// JS truthiness of a number.
    Truthy(Ty, Box<Expr>),
    /// `a || b` / `a && b` on numbers: the first operand if it is truthy (`||`) or falsy
    /// (`&&`), else the second.
    NumberLogic {
        /// `true` for `||`.
        or: bool,
        /// The number type of both operands.
        ty: Ty,
        /// The first operand.
        lhs: Box<Expr>,
        /// The second operand.
        rhs: Box<Expr>,
    },
    /// A `Math` call.
    Math(MathFn, Vec<Expr>),
    /// A call to an island function.
    Call(FuncRef, Vec<Expr>),
    /// A method call on a class instance.
    MethodCall(FuncRef, Box<Expr>, Vec<Expr>),
    /// A field read.
    Field(Box<Expr>, String),
    /// A struct value from field values in declaration order.
    StructLit(StructRef, Vec<(String, Expr)>),
    /// An array's length, as the given number type.
    Len(Box<Expr>, Ty),
    /// An element read, as `f64` (`NaN` when out of bounds unless proven in bounds).
    Get {
        /// The array.
        array: Box<Expr>,
        /// The index.
        index: Box<Index>,
    },
    /// A new zero-filled array.
    NewArray {
        /// Its elements.
        elem: Elem,
        /// Its length.
        len: Box<Expr>,
        /// `true`: `len` is a [`Ty::Index`] that is already a valid length; `false`: an `f64`
        /// that goes through `ToIndex` and traps on invalid lengths, as the constructor throws.
        exact: bool,
    },
    /// `test ? then : otherwise`.
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Runs statements, then evaluates to the value.
    Block(Vec<Stmt>, Box<Expr>),
}

impl Expr {
    /// Whether `pred` holds for this expression or one inside it (not looking into the
    /// statements of blocks).
    pub fn any(&self, pred: &impl Fn(&Expr) -> bool) -> bool {
        if pred(self) {
            return true;
        }
        match self {
            Expr::Lit(_) | Expr::Local(_) | Expr::Const(_) | Expr::SelfValue => false,
            Expr::Unary(_, a)
            | Expr::Truthy(_, a)
            | Expr::Field(a, _)
            | Expr::Len(a, _)
            | Expr::Convert { arg: a, .. }
            | Expr::NewArray { len: a, .. }
            | Expr::Block(_, a) => a.any(pred),
            Expr::Binary(_, a, b) | Expr::NumberLogic { lhs: a, rhs: b, .. } => {
                a.any(pred) || b.any(pred)
            }
            Expr::Get { array, index } => array.any(pred) || index.value.any(pred),
            Expr::Math(_, args) | Expr::Call(_, args) => args.iter().any(|e| e.any(pred)),
            Expr::MethodCall(_, receiver, args) => {
                receiver.any(pred) || args.iter().any(|e| e.any(pred))
            }
            Expr::StructLit(_, fields) => fields.iter().any(|(_, e)| e.any(pred)),
            Expr::Cond(a, b, c) => a.any(pred) || b.any(pred) || c.any(pred),
        }
    }

    /// Whether evaluating it may call a function, allocate or run statements (which may
    /// assign the function's locals).
    pub fn has_effects(&self) -> bool {
        self.any(&|e| {
            matches!(
                e,
                Expr::Call(..) | Expr::MethodCall(..) | Expr::NewArray { .. } | Expr::Block(..)
            )
        })
    }

    /// Whether evaluating it runs statements of the function, which may assign its locals
    /// (calls can't).
    pub fn runs_statements(&self) -> bool {
        self.any(&|e| matches!(e, Expr::Block(..)))
    }
}
