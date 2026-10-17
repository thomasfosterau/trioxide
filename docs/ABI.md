# Trioxide island ABI (version 1)

An **island** is a group of functions compiled together into one Rust crate and one `.wasm`
module (PLAN D4). This document is the contract between:

- the **Rust backend** (S6), which turns an island's TypeScript functions into Rust and
  describes their boundary in an [`IslandSpec`](#the-spec-islandjson);
- the **builder** (`trioxide_wasm::CargoIslandBuilder`), which compiles the crate;
- the **glue** (`trioxide_wasm::generate_glue`), a small ESM module that loads the `.wasm`
  and exports JS functions with the original signatures;
- the **runtime** (`crates/trioxide_rt`), which every island depends on.

The JS that calls an island can't tell it apart from the original functions: same names,
same parameters, same results, same mutations, same output. Anything that would observably
differ makes a function ineligible (PLAN §5) instead.

`tests/islands/*/island/` holds hand-written islands in exactly the shape described here, one
per ABI feature. Read them alongside this document.

Contents: [Spec](#the-spec-islandjson) · [Types](#types-at-the-boundary) ·
[Memory](#memory-and-the-arena) · [Arrays](#arrays) · [Structs](#plain-data-structs) ·
[Module interface](#module-interface) · [Rust shape](#the-rust-shape-of-an-island) ·
[Numerics](#numerics) · [Traps](#traps-and-errors) · [Re-entrancy](#re-entrancy) ·
[Glue](#glue) · [Building](#building) · [Versioning](#versioning) · [Not in v1](#not-in-v1)

## The spec (`island.json`)

`trioxide_wasm::IslandSpec`, serialised as camelCase JSON:

```json
{
  "id": "particles",
  "abiVersion": 1,
  "exports": [
    {
      "name": "step",
      "wasmName": "step",
      "params": [
        { "name": "body", "ty": { "kind": "struct", "index": 0 }, "mutated": true },
        { "name": "dt", "ty": { "kind": "f64" } }
      ],
      "ret": null
    }
  ],
  "structs": [
    {
      "name": "Body",
      "fields": [
        { "name": "x", "ty": { "kind": "f64" } },
        { "name": "layer", "ty": { "kind": "i32" } },
        { "name": "pinned", "ty": { "kind": "bool" } }
      ]
    }
  ],
  "fallback": "../kernel.fallback.js"
}
```

| Field | Meaning |
|-------|---------|
| `id` | Unique within a build; 1–64 bytes of `[A-Za-z0-9_-]`. Names the files (`<id>.wasm`, `<id>.js`) and the virtual module `\0trioxide:island/<id>`. |
| `abiVersion` | Must equal `ABI_VERSION` (1). |
| `exports[].name` | The JS name the glue exports: the original function's name. |
| `exports[].wasmName` | The `.wasm` export implementing it. By convention equal to `name`. |
| `exports[].params[]` | In order: `name` (the JS parameter name), `ty`, and `mutated` (arrays and structs only: the island writes to it, so the glue copies it back). |
| `exports[].ret` | The return type, or `null` for functions returning `undefined`. |
| `structs[]` | Plain-data struct layouts referenced by `{ "kind": "struct", "index": i }`. |
| `fallback` | Optional. A module specifier, relative to the glue, that exports the JS version of every island function under the same name. See [aliasing](#aliasing). |

`IslandSpec::validate` enforces:

- **JS names** (exports, parameters, structs, fields): ASCII identifiers
  `[A-Za-z_][A-Za-z0-9_$]*`, not a reserved word, and **not starting with `$`**. The glue's
  own names start with `$`, so they can't clash. Unique within their scope. The backend
  renames a `$x` parameter; only `Function.prototype.toString` could tell.
- **Export names** also can't be `ready`, which `ready()`-mode glue exports itself.
- **wasm names**: `[A-Za-z_][A-Za-z0-9_]*`, unique, not `memory`, not starting with
  `__trioxide_`.
- Struct indices in range; struct fields are scalars or nested structs (no arrays); no struct
  contains itself; every struct has a field.
- Only arrays and structs may be `mutated`.

An island also needs Rust files; together they form `trioxide_wasm::IslandSource` (spec plus
files by path, which must include `src/lib.rs`, all under `src/`). `IslandSource::load(dir)`
reads `dir/island.json` and `dir/src/**/*.rs`.

## Types at the boundary

| `ty.kind` | TS type | wasm params | Shim parameter (Rust) | Function parameter (Rust) | Returned as |
|-----------|---------|-------------|-----------------------|---------------------------|-------------|
| `f64` | `number` | `f64` | `x: f64` | `f64` | `f64` |
| `i32` | `number` proven int32 | `i32` | `x: i32` | `i32` | `i32` |
| `u32` | `number` proven uint32 | `i32` | `x: u32` | `u32` | `i32` holding the `u32` bits |
| `bool` | `boolean` | `i32` | `x: i32` (0 or 1) | `bool` (`x != 0`) | `i32` 0/1 |
| `typedArray` | `Int8Array` … `Float64Array` | `i32, i32` | `x_ptr: u32, x_len: u32` | `&[E]` / `&mut [E]` | `u32` → `ArrayRet` |
| `numberArray` | `number[]` | `i32, i32` | `x_ptr: u32, x_len: u32` | `&[f64]` / `&mut [f64]` | `u32` → `ArrayRet` |
| `struct` | plain-data class or object type | `i32` | `x_ptr: u32` | `&S` / `&mut S` | `u32` → `S` |

`len` is an element count, not bytes. The typed array element types `E` are, by
`element`:

| `element` | JS | Rust `E` | Bytes (= alignment) |
|-----------|----|----------|---------------------|
| `i8` | `Int8Array` | `i8` | 1 |
| `u8` | `Uint8Array` | `u8` | 1 |
| `u8Clamped` | `Uint8ClampedArray` | `trioxide_rt::array::U8Clamped` | 1 |
| `i16` | `Int16Array` | `i16` | 2 |
| `u16` | `Uint16Array` | `u16` | 2 |
| `i32` | `Int32Array` | `i32` | 4 |
| `u32` | `Uint32Array` | `u32` | 4 |
| `f32` | `Float32Array` | `f32` | 4 |
| `f64` | `Float64Array` | `f64` | 8 |

**Scalars pass directly.** The WebAssembly JS API converts arguments: `f64` parameters get
ToNumber (so a missing argument is `NaN`, as `undefined` is in JS arithmetic) and `i32`
parameters get ToInt32. That's why `i32` and `u32` are only allowed for values the analysis
proves to be int32 / uint32: then the conversion is the identity. A `u32` above 2^31 crosses
as a negative `i32` and Rust reads the same bits back as `u32`. The glue passes booleans as
`b ? 1 : 0`, turns `u32` results back into numbers with `>>> 0`, and `bool` results with
`!== 0`.

`i32` is only for values that are int32 throughout: S6 uses `i32` for a local or parameter
only when range analysis proves every value it holds is an int32 *and* -0 can't occur (see
[Numerics](#numerics)).

## Memory and the arena

Each island instance has one linear memory (exported as `memory`). Rust's linker lays it out
as stack, then static data, then `__heap_base`. Everything from `__heap_base` up belongs to
the **arena**, a bump allocator in `trioxide_rt::abi`:

- `__trioxide_alloc(size, align) -> ptr` reserves `size` bytes aligned to `align` (a power of
  two), growing memory by whole pages as needed. It returns 0 when memory can't grow; the glue
  throws a `RangeError`.
- `__trioxide_reset()` frees everything at once (and clears the trap code). Memory never
  shrinks.

Every call through the glue follows the same sequence:

1. **Copy in**: allocate and fill each array and struct argument, in parameter order.
2. **Call** the export. Island code allocates its own temporaries (`new Float64Array(n)`,
   returned structs and arrays) from the same arena.
3. **Read the result** and **copy back** mutated arguments.
4. **Reset** the arena (in a `finally`, so it also happens after a trap).

So nothing an island allocates outlives the call. Islands have no global allocator and no
state that persists between calls. Rust `static`s are allowed only for constants.

Growing memory replaces `memory.buffer` and detaches every view of the old one. The glue
therefore creates a fresh view of `memory.buffer` for each access and never keeps one across
an allocation or a call.

## Arrays

**Copy-in.** The glue allocates `length * BYTES_PER_ELEMENT` bytes aligned to the element
size and copies the argument with `TypedArray.prototype.set`:

- A typed array is copied into a view of the same kind: a byte copy, NaN payloads and -0
  included. A `subarray` view copies just its elements.
- A `number[]` is copied into a `Float64Array` view, which converts each element with
  ToNumber. The analysis must prove the elements are numbers. A hole reads as `NaN`; the
  island must not observe `undefined` other than through arithmetic (see
  [`array`](../crates/trioxide_rt/src/array.rs)).

Empty arrays pass `len = 0` with some aligned pointer; `abi::slice` returns `&[]` without
touching it.

**Copy-back** happens only for parameters marked `mutated`, after the call returns:

- typed arrays: `array.set(view)`, a bulk copy into the original object (so views and
  `subarray`s update the right part of their buffer);
- `number[]`: only elements whose value changed (compared with `Object.is` against the
  element's numeric value) are assigned, so untouched holes stay holes. Lengths never change.

The analysis sets `mutated` whenever the island may write to the array, directly or through
a callee. Unmarked arrays get `&[E]`, so writing to them doesn't compile.

**Returns.** An export returning an array returns the address of an `ArrayRet { ptr: u32,
len: u32 }` (`#[repr(C)]`, 8 bytes) in the arena, made by `abi::return_array(xs)`. The glue
copies the elements out (`view.slice()` for typed arrays, which gives a fresh array with its
own `ArrayBuffer`, offset 0; `Array.from(view)` for `number[]`, which gives a dense array).
The returned array must be one the JS function would also have *created*, such as
`new Float64Array(n)`, `new Array(n).fill(0)` or a call to a function that creates one. A
function that may return one of its parameters, or an array it stores elsewhere, is not
eligible in v1: the glue can't preserve identity.

### Aliasing

Two array arguments may share memory in JS (the same object twice, or views of one
`ArrayBuffer`); two struct arguments may be the same object. Separate copies in linear memory
can't reproduce writes through one being visible through the other. Before copying in, the
wrapper checks every pair of array (or struct) parameters where at least one is `mutated`:

- arrays overlap if they are the same object, or typed arrays on the same `buffer` whose byte
  ranges intersect;
- structs alias if they are the same object.

On overlap the wrapper calls the JS version from the spec's `fallback` module, if it has one,
and otherwise throws a `TypeError`. Read-only arguments never need the check. Because the
glue guarantees it, the Rust side may hold `&mut` to one argument alongside `&` or `&mut` to
another.

## Plain-data structs

A struct describes a class or object type whose values, as far as the island sees, are just
their listed own data properties: no getters or setters, no private fields, nothing read
through the prototype. Field types are `f64`, `i32`, `u32`, `bool` or a nested struct (stored
inline).

**Layout** is C's (`#[repr(C)]`, `IslandSpec::struct_layout`), fields in spec order:

| Field type | Size | Alignment | Stored as |
|------------|------|-----------|-----------|
| `f64` | 8 | 8 | IEEE double, little-endian |
| `i32`, `u32` | 4 | 4 | little-endian |
| `bool` | 1 | 1 | byte 0 or 1 (Rust `bool`) |
| struct `S` | `size_of::<S>()` | `align_of::<S>()` | inline |

Each field sits at the next offset aligned for its type; the struct's alignment is its
largest field alignment, and its size is rounded up to that. For example
`{ x: f64, y: f64, vx: f64, vy: f64, mass: f64, layer: i32, pinned: bool }` puts `layer` at
40 and `pinned` at 44, for a size of 48 with alignment 8.

**Parameters** are copied in field by field (`DataView`, little-endian) into an arena block
of the struct's size and alignment, and passed by address. A `mutated` struct is copied back
by assigning every field to the original object in place: its identity and any other
properties are kept, and nested structs are updated in place too.

**Returns**: the export returns the address of the struct in the arena
(`abi::return_struct(value)`). The glue builds a new plain object with the fields in spec
order, which is what an object literal in the original code produces (`{ min, max, mean }`).
Returning an instance of a class needs its prototype and is not supported in v1; such
functions stay JS.

Classes used only *inside* the island (never as a parameter or result) don't appear in the
spec at all. They become ordinary Rust structs with whatever layout Rust chooses (see
`Vec2` in `tests/islands/particles`).

## Module interface

An island module **imports nothing** (the glue instantiates it with `{}`) and exports:

| Export | Signature | Provided by |
|--------|-----------|-------------|
| `memory` | the linear memory | rustc |
| `__trioxide_abi_version` | `() -> i32` | `trioxide_rt` |
| `__trioxide_alloc` | `(size: i32, align: i32) -> i32` | `trioxide_rt` |
| `__trioxide_reset` | `() -> ()` | `trioxide_rt` |
| `__trioxide_trap_code` | `() -> i32` | `trioxide_rt` |
| each `exports[].wasmName` | per [the type table](#types-at-the-boundary) | the island |

## The Rust shape of an island

### Crate layout

```
<crate>/
  island.json      the IslandSpec
  Cargo.toml       optional, for editing in place; the builder ignores it
  src/lib.rs       the island; more modules under src/ are allowed
```

The builder writes the manifest it builds with (see [Building](#building)): package
`island_<hash>`, edition 2024, `crate-type = ["cdylib"]`, and one dependency,
`trioxide_rt` with the `std` feature. Islands can't use any other crate.

### `lib.rs`

```rust
//! WASM island `<id>`, generated by trioxide from `<module path>`. Do not edit.

#![no_std]
#![allow(non_snake_case, clippy::all)]

use trioxide_rt::prelude::*;

// 1. Struct definitions, in spec order, each followed by its layout assertions.
// 2. Every island function, internal or exported, as a plain Rust fn.
// 3. One ABI shim per export.
```

- `#![no_std]` is required. The `std` feature of `trioxide_rt` links `std` only for the
  hardware float instructions (`f64.sqrt`, `floor`, `ceil`, `trunc`); island code must not use
  anything else from `std` (no allocation, I/O or formatting).
- `use trioxide_rt::prelude::*;` brings in the modules `abi`, `array`, `conv`, `math` and
  `ops`, plus `Element` and `U8Clamped`. Call helpers by module path (`math::round(x)`,
  `ops::shl(a, b)`, `array::get(xs, i)`) so the source reads like the JS.
- **Names.** Each TS function `f` becomes a Rust `fn f` with the same name (hence
  `non_snake_case`), escaped as a raw identifier if it is a Rust keyword (`r#type`). Locals
  and parameters keep their TS names. An internal function that would collide with a shim
  can't: shims are named `__abi_<name>`.
- **Types.** `number` is `f64` unless proven int32 (`i32`) or uint32 (`u32`); `boolean` is
  `bool`; arrays are slices; plain-data structs are `#[repr(C)]` structs; internal classes are
  plain structs, with methods becoming `impl` methods taking `self` by value or reference.
- **No panics.** Generated code must not contain a reachable panic: they pull `std`'s panic
  formatting into the module (about 18 KB) and would make a JS-valid program trap. Use the
  `trioxide_rt` helpers, which are panic-free: `array::get*`/`set*` for indices not proven in
  bounds, `get_unchecked`/`set_unchecked` (in `unsafe` with a `// SAFETY:` comment naming the
  proof) or `slice::get_unchecked` for proven ones, `wrapping_*` and `ops::*_i32` for integer
  operators, and never `/` or `%` on Rust integers (use `ops::div_trunc_i32`, `ops::rem_i32`).
  `xs[i]` indexing is acceptable only where LLVM can see the bound (e.g. `for &x in xs`).
- **Allocation.** `new XArray(n)` with an arbitrary `n` is `abi::new_array::<E>(n)` (ToIndex
  semantics, `RangeError` traps); with a length that is already a valid index (another
  array's `.length`) it is `abi::alloc_array::<E>(len)`. Both are zero-filled, as JS arrays
  are, and live until the call ends. `new Array<number>(n).fill(0)` is `alloc_array::<f64>`.
- **Internal calls** go straight to the Rust functions, never through a shim, so values
  aren't copied between functions of one island.

### Structs

For each `structs[i]`, in order:

```rust
/// `Body` (`structs[0]` in the spec).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Body {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub mass: f64,
    pub layer: i32,
    pub pinned: bool,
}

const _: () = {
    assert!(size_of::<Body>() == 48 && align_of::<Body>() == 8);
    assert!(core::mem::offset_of!(Body, layer) == 40);
    assert!(core::mem::offset_of!(Body, pinned) == 44);
};
```

The struct's name and field names are the spec's (escaped if they are Rust keywords). The
assertions state the size, alignment and every non-zero offset from `struct_layout`, so a
mismatch fails to compile instead of corrupting memory.

### ABI shims

One per export, after the functions:

```rust
#[unsafe(export_name = "<wasmName>")]
pub unsafe extern "C" fn __abi_<name>(<params>) -> <ret> {
    // decode parameters, call the function, encode the result
}
```

- **Parameters**, in spec order: `f64` → `name: f64`; `i32` → `name: i32`; `u32` →
  `name: u32`; `bool` → `name: i32`; arrays → `name_ptr: u32, name_len: u32`; structs →
  `name_ptr: u32`.
- **Decoding**: arrays with `abi::slice::<E>(ptr, len)`, or `abi::slice_mut::<E>` when
  `mutated`; structs with `abi::struct_ref::<S>(ptr)`, or `abi::struct_mut::<S>` when
  `mutated`; booleans with `name != 0`. These are `unsafe`. Put them in one `unsafe` block
  with `// SAFETY: the glue passes fresh copies` (and, for several mutable arguments, `and
  checks they don't overlap`).
- **Result**: `f64`, `i32`, `u32` are returned as is; `bool` as `i32::from(b)`; arrays with
  `abi::return_array(xs)` and structs with `abi::return_struct(value)`, both returning `u32`.
  No result: no return type.
- A shim without pointer parameters is a safe `pub extern "C" fn`.

Complete example, from `tests/islands/image-filter`:

```rust
fn histogram(pixels: &[U8Clamped], bins: &mut [u32]) {
    let mut i = 0usize;
    while i < pixels.len() {
        if (i as i32 & 3) != 3 {
            // SAFETY: i < pixels.len().
            let bin = ops::shr(unsafe { array::get_unchecked(pixels, i) }, 4.0);
            array::set_i32(bins, bin, array::get_i32(bins, bin) + 1.0);
        }
        i += 1;
    }
}

#[unsafe(export_name = "histogram")]
pub unsafe extern "C" fn __abi_histogram(pixels_ptr: u32, pixels_len: u32, bins_ptr: u32, bins_len: u32) {
    // SAFETY: the glue passes fresh copies and checks they don't overlap in JS.
    let pixels = unsafe { abi::slice::<U8Clamped>(pixels_ptr, pixels_len) };
    let bins = unsafe { abi::slice_mut::<u32>(bins_ptr, bins_len) };
    histogram(pixels, bins);
}
```

## Numerics

The runtime's golden tables (`crates/trioxide_rt/tests/golden/`, generated from Node by
`generate.mjs`) check every helper below against V8 bit for bit, on NaN, ±0, ±Infinity,
subnormals, 2^31 and 2^32 boundaries, rounding ties and pseudo-random values.

| JS | Rust |
|----|------|
| `a + b`, `a - b`, `a * b`, `a / b` on numbers | the same `f64` operators (Rust never fuses or reorders float operations) |
| `a % b` | `ops::rem(a, b)` (`f64 %`, C's `fmod`, which is exact) |
| `a < b` etc., `===`, `!==` on numbers | the same `f64` comparisons; NaN compares false except `!=`. Keep `!(a < b)` as written. |
| `-x` | `-x` (can produce -0; see below) |
| `x \| 0`, `x >>> 0` | `conv::to_int32(x)`, `conv::to_uint32(x)` |
| `&`, `\|`, `^`, `~` | `ops::bit_and/bit_or/bit_xor/bit_not` on `f64`, or Rust's operators on `i32` |
| `<<`, `>>`, `>>>` | `ops::shl/shr/ushr` on `f64`; `ops::shl_i32/shr_i32/ushr_i32` on `i32` (count taken mod 32) |
| `(a / b) \| 0` on int32 | `ops::div_trunc_i32(a, b)` (0 when `b` is 0) |
| `a % b` on int32 | `ops::rem_i32(a, b)`, which returns `f64` (NaN for `b == 0`, -0 for a zero remainder of a negative `a`) |
| truthiness of a number | `conv::to_boolean(x)` (false for ±0 and NaN) |
| `Math.abs/floor/ceil/trunc/sqrt` | `math::abs/floor/ceil/trunc/sqrt` |
| `Math.round` | `math::round` (ties toward +∞, -0 for [-0.5, -0]) |
| `Math.sign`, `Math.fround`, `Math.clz32`, `Math.imul` | `math::sign`, `math::fround`, `math::clz32`, `math::imul` / `math::imul_i32` |
| `Math.max(a, b)`, `Math.min(a, b)` | `math::max`, `math::min` (NaN wins; +0 > -0). More arguments fold left; none is `math::max_of(&[])`. |
| `Math.hypot(a, b[, c])` | `math::hypot2`, `math::hypot3`, `math::hypot(&[…])` (V8's algorithm) |
| `Math.pow(x, y)`, `x ** y` | `math::pow` (JS rules for NaN exponents and \|x\| = 1 with infinite `y`) |
| typed array element read / write | `Element::to_f64` / `Element::from_f64` (ToInt8, ToUint8Clamp, …, `f32` rounding), via the `array` helpers |

**Integers.** An `i32` local may only hold values JS would also produce as int32. Arithmetic
on `i32` must be proven not to overflow (JS would produce a larger number, not wrap) unless
the result goes straight through ToInt32 (`(a + b) | 0`, `Math.imul`, `x & mask`), in which
case use `wrapping_*`. JS has -0 and int32 doesn't: a negation, `0 * -x` or `%` result that
can be -0 needs `f64` unless it is consumed by ToInt32 or a comparison.

**Transcendental functions** (`sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `sinh`,
`cosh`, `tanh`, `asinh`, `acosh`, `atanh`, `exp`, `expm1`, `log`, `log1p`, `log2`, `log10`,
`cbrt`, `pow`) come from `libm` (a port of musl), while V8 uses fdlibm, and glibc for `sin`
and `cos`. ECMAScript only asks for implementation-approximated results. Special cases (NaN,
±0, ±Infinity, out-of-domain arguments) always agree. On the golden inputs:

- **exact** on every input: `tan`, `asin`, `acos`, `atan`, `atan2`, `sinh`, `atanh`,
  `expm1`, `log1p`, `log2`;
- **within 1 ulp**: `sin`, `cos`, `cosh`, `asinh`, `acosh`, `exp`, `log`, `log10`, `cbrt`,
  `pow` (e.g. `Math.exp(1)` is 2.718281828459045 in V8 and 2.7182818284590455 in libm);
- **within 2 ulps**: `tanh` (e.g. `Math.tanh(0.7)`).

`pow` is exact for the cases the lowering relies on (`x ** 2` is `x * x` in both). Since a
last-bit difference can change printed output, S6 lowers code using the inexact functions
only when their results can't reach observable output exactly (for example, when they are
rounded, compared against thresholds, or written to integer typed arrays), or when the user
opts in. Otherwise the function stays JS.

## Traps and errors

Island code doesn't throw, and JS functions in the compilable subset throw nothing either,
apart from the cases below. A WebAssembly trap ends the call with a
`WebAssembly.RuntimeError`, which the wrapper converts, after checking `__trioxide_trap_code()`:

| Trap code | Raised by | The glue throws |
|-----------|-----------|-----------------|
| 1 `TRAP_OUT_OF_MEMORY` | an arena allocation inside the island failed (`alloc_array`, `new_array`, `return_*`) | `RangeError`, as JS does when an allocation is too large |
| 2 `TRAP_INVALID_LENGTH` | `abi::new_array` got a negative or too-large length | `RangeError`, as the typed array constructor does |
| 0 (anything else) | `unreachable`, a panic, a stack overflow: a trioxide bug | `Error("trioxide: island \"<id>\" trapped in <name>(); this is a trioxide bug")` with the `RuntimeError` as `cause` |

`$alloc` failing during copy-in throws a `RangeError` before the call. Errors thrown while
reading arguments (say, a struct argument that is `undefined`) propagate unchanged. The
arena is reset in every case, and the instance stays usable: a later call works normally.

## Re-entrancy

Version 1 has no JS callbacks: an island imports nothing, so island code can't call back
into JS, and a call can't re-enter the island. Calls are synchronous and the module is
single-threaded, so the arena belongs to exactly one call at a time. Exports never call each
other through the boundary (internal calls are plain Rust calls). Callbacks would need
imports and a re-entrant arena (a stack of arena marks instead of one reset), and would make
the ABI version 2.

## Glue

`generate_glue(spec, options)` emits an ESM module. The output for every hand-written island
is snapshotted in `crates/trioxide_wasm/tests/snapshots/`.

- **Loading.** `const $url = new URL("./<id>.wasm", import.meta.url)`. That is the pattern
  Vite turns into a hashed asset. With a `file:` URL (Node) the bytes come from
  `node:fs/promises`, which the glue imports dynamically through a variable so bundlers leave
  it alone. Otherwise they come from `fetch`. `GlueOptions::wasm_url` can instead name a
  path (`WasmUrl::Path`) or any JS expression (`WasmUrl::Expression`, e.g.
  `import.meta.ROLLUP_FILE_URL_<ref>` in a virtual module).
- **Instantiation** with an empty import object, then a check that
  `__trioxide_abi_version()` equals the ABI version the glue was generated for (it throws
  otherwise).
- **Modes.** `GlueMode::Tla` (default, `wasm.glue: "tla"`) awaits instantiation at the top
  level, so the island is ready once the module is imported. `GlueMode::Ready`
  (`wasm.glue: "ready"`) for targets without top-level await exports `ready()`, a promise.
  Wrappers throw until it resolves.
- **Wrappers.** One `export function <name>(<params>)` per export, with the original
  parameter names (so `name` and `length` match the original function). Each one checks
  aliasing, then, in `try`, copies arguments in, calls the export, copies mutated arguments
  back, and converts the result. `catch` converts traps; `finally` resets the arena.
- **Size.** Helpers (`$copyIn`, `$copyBack`, `$takeArray`, struct readers and writers, …) are
  emitted only when some export needs them.

`trioxide build` writes the glue and module as `<out-dir>/_islands/<id>.js` and
`<id>.wasm`. `BuildOutput.islands` carries both (`IslandArtifact { spec, wasm, glue,
warnings }`) for the Vite plugin, which serves the glue as `\0trioxide:island/<id>`.

## Building

`CargoIslandBuilder` (PLAN §6 "Build driver"):

1. **Cache key**: a 128-bit xxh3 hash of the island's files, the embedded `trioxide_rt`
   sources and version, the generated manifests (which pin the profile), the target, the
   full `rustc -vV` output, and the `wasm-opt --version` output when `wasm-opt` will run.
   A module already at `<cache_dir>/wasm/<key>.wasm` is used without running cargo; only
   `rustc -vV` runs.
2. **Staging**: misses are written into one cargo workspace at `<cache_dir>/build/`, with
   the runtime crate (embedded in the trioxide binary, so installed copies need no checkout)
   in `trioxide_rt/` and each island in `islands/island_<key prefix>/`. Files are only
   rewritten when they change, so cargo's own freshness checks keep working.
3. **Compile**: one `cargo build --release --target wasm32-unknown-unknown --keep-going -p …`
   for all misses. Cargo builds them in parallel and shares the target directory, so the
   runtime and `libm` compile once. The profile is `opt-level = 3`, `lto = "fat"`,
   `codegen-units = 1`, `panic = "abort"`, `strip = true`, no debug info or overflow checks.
   Environment variables that would change the build (`RUSTFLAGS`, `CARGO_TARGET_DIR`,
   `CARGO_PROFILE_*`, …) are removed. A lock file serialises concurrent builds of one cache.
4. **Optimise**: `wasm-opt -O3` when `wasm.wasmOpt` is on (the default) and `wasm-opt` is on
   `PATH`. If it fails, the unoptimised module is used with a warning (and not cached).

`cache_dir` defaults to `node_modules/.trioxide` under the project root. It should stay
inside the project so the project's `rust-toolchain.toml`, if any, picks the toolchain
(rustup looks for it from the build directory).

**Toolchain.** `trioxide_wasm::toolchain` finds `cargo`, `rustc`, the wasm32 standard library
(in `rustc --print sysroot`) and `wasm-opt` on `PATH`, for the builder and for
`trioxide check-toolchain` (`toolchain::detect`). When something is missing the build goes
on: the islands' functions stay JS and the report gets **one** warning
(`trioxide(wasm-toolchain)`) naming the islands and how to fix it. `--require-wasm`
(`wasm.require`) makes that, and any failed island build, an error. `--no-wasm`
(`wasm.enabled: false`) turns islands off.

## Versioning

`ABI_VERSION` is defined in `trioxide_wasm` (the spec and glue) and in `trioxide_rt` (the
module's `__trioxide_abi_version` export); a test keeps them equal. The spec's `abiVersion`
must match the compiler's, and the glue refuses a module that reports another version.
Cache keys include it, so a version change rebuilds every island.

Bump it for any change that makes old glue and new modules, or the reverse, disagree: the
meaning of a type or its parameter passing, struct layout rules, the arena protocol, export
names or signatures, or trap codes. Additive changes that old glue never sees (a new `kind`,
a new runtime helper) don't need a bump.

## Not in v1

Strings, callbacks into JS, exceptions crossing the boundary, `number[]` that change length
(`push`, out-of-bounds writes), arrays inside structs, returning a parameter or a stored
array, returning class instances, `bigint`, shared memory and threads. Functions that need
any of these stay JS.
