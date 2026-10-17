//! The JS glue module for an island (`docs/ABI.md`, "Glue").
//!
//! The glue instantiates the `.wasm` and exports one wrapper per island function with the
//! original name and parameters. Each wrapper copies arrays and structs into the island's
//! arena, calls the export, converts the result, copies mutated arguments back, and resets
//! the arena. The glue's own names start with `$`, which spec names may not.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::spec::{ABI_VERSION, AbiType, IslandExport, IslandSpec, TypedArrayElement};

/// The trap code `trioxide_rt::abi::TRAP_OUT_OF_MEMORY`.
pub(crate) const TRAP_OUT_OF_MEMORY: u32 = 1;
/// The trap code `trioxide_rt::abi::TRAP_INVALID_LENGTH`.
pub(crate) const TRAP_INVALID_LENGTH: u32 = 2;

/// How the glue waits for the module to instantiate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GlueMode {
    /// Top-level `await`: the island is ready when the glue module has evaluated, which
    /// delays the evaluation of every module importing it past other pending tasks.
    Tla,
    /// Export `ready()`, a promise that resolves once the island is instantiated. Until then
    /// wrappers call the spec's `fallback` (or throw without one). Module evaluation order is
    /// unchanged.
    #[default]
    Ready,
}

/// Where the glue loads the `.wasm` from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WasmUrl {
    /// `new URL("__TRIOXIDE_WASM_URL__", import.meta.url)`: the emitter replaces
    /// [`WASM_URL_PLACEHOLDER`](crate::WASM_URL_PLACEHOLDER) with the `.wasm`'s URL relative
    /// to the glue. This is what [`IslandArtifact::glue`](crate::IslandArtifact::glue) holds.
    #[default]
    Placeholder,
    /// `new URL("./<id>.wasm", import.meta.url)`: the `.wasm` sits next to the glue. Vite
    /// recognises this pattern and emits the file as a hashed asset.
    Sibling,
    /// `new URL("<path>", import.meta.url)` for a relative path or absolute URL.
    Path(String),
    /// `new URL(<expression>, import.meta.url)` for a JS expression, e.g. a Vite
    /// `import.meta.ROLLUP_FILE_URL_<ref>` in a virtual glue module.
    Expression(String),
}

/// Options for [`generate_glue`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlueOptions {
    /// How the glue waits for instantiation.
    pub mode: GlueMode,
    /// Where the `.wasm` is.
    pub wasm_url: WasmUrl,
}

/// What the glue needs besides the wrappers, so unused helpers are left out.
#[derive(Default)]
struct Needs {
    copy_in: bool,
    copy_in_numbers: bool,
    copy_back: bool,
    copy_back_numbers: bool,
    take_array: bool,
    take_numbers: bool,
    overlaps: bool,
    structs: Vec<StructNeeds>,
}

#[derive(Default, Clone, Copy)]
struct StructNeeds {
    write: bool,
    read: bool,
    update: bool,
}

impl Needs {
    fn of(spec: &IslandSpec) -> Self {
        let mut needs = Self {
            structs: vec![StructNeeds::default(); spec.structs.len()],
            ..Self::default()
        };
        for export in &spec.exports {
            for param in &export.params {
                match &param.ty {
                    AbiType::TypedArray { .. } => {
                        needs.copy_in = true;
                        needs.copy_back |= param.mutated;
                    }
                    AbiType::NumberArray => {
                        needs.copy_in_numbers = true;
                        needs.copy_back_numbers |= param.mutated;
                    }
                    AbiType::Struct { index } => {
                        needs.mark(spec, *index, |s| s.write = true);
                        if param.mutated {
                            needs.mark(spec, *index, |s| s.update = true);
                        }
                    }
                    _ => {}
                }
            }
            needs.overlaps |= alias_checks(export)
                .iter()
                .any(|&(a, _)| export.params[a].ty.is_array());
            match &export.ret {
                Some(AbiType::TypedArray { .. }) => needs.take_array = true,
                Some(AbiType::NumberArray) => needs.take_numbers = true,
                Some(AbiType::Struct { index }) => needs.mark(spec, *index, |s| s.read = true),
                _ => {}
            }
        }
        needs
    }

    /// Marks struct `index` and every struct nested in it.
    fn mark(&mut self, spec: &IslandSpec, index: usize, set: fn(&mut StructNeeds)) {
        set(&mut self.structs[index]);
        for field in &spec.structs[index].fields {
            if let AbiType::Struct { index } = field.ty {
                self.mark(spec, index, set);
            }
        }
    }
}

/// Pairs of by-reference parameters that must not alias: both arrays (or both structs) and
/// at least one mutated.
fn alias_checks(export: &IslandExport) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for (i, a) in export.params.iter().enumerate() {
        for (j, b) in export.params.iter().enumerate().skip(i + 1) {
            let comparable = (a.ty.is_array() && b.ty.is_array())
                || (matches!(a.ty, AbiType::Struct { .. })
                    && matches!(b.ty, AbiType::Struct { .. }));
            if comparable && (a.mutated || b.mutated) {
                pairs.push((i, j));
            }
        }
    }
    pairs
}

/// Generates the ESM glue module for `spec`, which must be
/// [valid](IslandSpec::validate).
pub fn generate_glue(spec: &IslandSpec, options: &GlueOptions) -> String {
    let needs = Needs::of(spec);
    let id = &spec.id.0;
    let ready = options.mode == GlueMode::Ready;
    let mut out = String::new();

    writeln!(
        out,
        "// Glue for the WASM island \"{id}\" (trioxide ABI {ABI_VERSION}). Generated by trioxide; do not edit.\n//"
    )
    .unwrap();
    for export in &spec.exports {
        writeln!(out, "//   {}", signature(spec, export)).unwrap();
    }
    out.push('\n');
    if let Some(fallback) = &spec.fallback {
        writeln!(out, "import * as $fallback from {};\n", js_string(fallback)).unwrap();
    }

    let url = match &options.wasm_url {
        WasmUrl::Placeholder => js_string(crate::WASM_URL_PLACEHOLDER),
        WasmUrl::Sibling => js_string(&format!("./{}", spec.wasm_file_name())),
        WasmUrl::Path(path) => js_string(path),
        WasmUrl::Expression(expression) => expression.clone(),
    };
    writeln!(out, "const $url = new URL({url}, import.meta.url);").unwrap();
    write!(
        out,
        r#"
async function $instantiate() {{
  let bytes;
  if ($url.protocol === "file:") {{
    const fs = "node:fs/promises";
    bytes = await (await import(/* @vite-ignore */ fs)).readFile($url);
  }} else {{
    const response = await fetch($url);
    if (!response.ok) throw new Error(`trioxide: cannot load ${{$url}}: HTTP ${{response.status}}`);
    bytes = await response.arrayBuffer();
  }}
  const {{ instance }} = await WebAssembly.instantiate(bytes, {{}});
  const version = instance.exports.__trioxide_abi_version();
  if (version !== {ABI_VERSION}) throw new Error(`trioxide: ${{$url}} implements island ABI ${{version}}, expected {ABI_VERSION}`);
  return instance.exports;
}}
"#
    )
    .unwrap();

    if ready {
        write!(
            out,
            r#"
// `var`: a module this glue imports (its fallback) may call a wrapper before this body runs.
var $wasm;
var $ready = $instantiate().then((exports) => {{
  $wasm = exports;
}});
// Rejections surface through ready(); don't also report them as unhandled.
$ready.catch(() => {{}});

/** Resolves once the island is instantiated. Until then its functions run their JS versions
 * (the spec's fallback), or throw if there is none. */
export function ready() {{
  return $ready;
}}

function $check() {{
  if ($wasm === undefined) throw new Error('trioxide: island "{id}" is not ready; await ready() first');
}}
"#
        )
        .unwrap();
    } else {
        out.push_str("\nconst $wasm = await $instantiate();\n");
    }

    write_helpers(&mut out, spec, &needs);
    for export in &spec.exports {
        write_wrapper(&mut out, spec, export, ready);
    }
    out
}

/// `name(a: T, …): R`, with copied-back parameters marked.
fn signature(spec: &IslandSpec, export: &IslandExport) -> String {
    let params: Vec<String> = export
        .params
        .iter()
        .map(|param| format!("{}: {}", param.name, param.ty.ts_name(spec)))
        .collect();
    let ret = export
        .ret
        .as_ref()
        .map_or_else(|| "void".into(), |ret| ret.ts_name(spec));
    let mutated: Vec<&str> = export
        .params
        .iter()
        .filter(|param| param.mutated)
        .map(|param| param.name.as_str())
        .collect();
    let mut line = format!("{}({}): {ret}", export.name, params.join(", "));
    if !mutated.is_empty() {
        write!(line, "  (copies back {})", mutated.join(", ")).unwrap();
    }
    line
}

fn js_string(text: &str) -> String {
    serde_json::to_string(text).expect("strings serialise")
}

fn write_helpers(out: &mut String, spec: &IslandSpec, needs: &Needs) {
    let id = &spec.id.0;
    write!(
        out,
        r#"
function $alloc(bytes, align) {{
  const ptr = $wasm.__trioxide_alloc(bytes, align);
  if (ptr === 0) throw new RangeError('trioxide: island "{id}" is out of memory');
  return ptr;
}}

function $trap(error, name) {{
  if (!(error instanceof WebAssembly.RuntimeError)) return error;
  const code = $wasm.__trioxide_trap_code();
  if (code === {TRAP_OUT_OF_MEMORY}) return new RangeError(`trioxide: island "{id}" ran out of memory in ${{name}}()`);
  if (code === {TRAP_INVALID_LENGTH}) return new RangeError(`Invalid typed array length: ${{$wasm.__trioxide_trap_value()}}`);
  return new Error(`trioxide: island "{id}" trapped in ${{name}}(); this is a trioxide bug`, {{ cause: error }});
}}
"#
    )
    .unwrap();
    if needs.copy_in {
        out.push_str(
            r"
// Copies a typed array into the arena as View elements. (A detached one has no elements,
// and `set` would throw.)
function $copyIn(array, View) {
  const ptr = $alloc(array.length * View.BYTES_PER_ELEMENT, View.BYTES_PER_ELEMENT);
  if (array.length !== 0) new View($wasm.memory.buffer, ptr, array.length).set(array);
  return ptr;
}
",
        );
    }
    if needs.copy_in_numbers {
        out.push_str(
            r"
// A number[] element that is `undefined` (a hole) enters the island as a signalling NaN
// (trioxide_rt `array::HOLE`), which arithmetic turns into an ordinary NaN and which the
// island never stores: elements still holding it weren't written.
const $HOLE_HI = 0x7ff47472;
const $HOLE_LO = 0x696f7869;

function $isHole(words, i) {
  return words[2 * i] === $HOLE_LO && words[2 * i + 1] === $HOLE_HI;
}
",
        );
    }
    if needs.copy_in_numbers {
        out.push_str(
            r"
function $copyInNumbers(array) {
  const ptr = $alloc(array.length * 8, 8);
  if (array.length !== 0) {
    new Float64Array($wasm.memory.buffer, ptr, array.length).set(array);
    const words = new Uint32Array($wasm.memory.buffer, ptr, array.length * 2);
    for (let i = 0; i < array.length; i++) {
      if (array[i] === undefined) {
        words[2 * i] = $HOLE_LO;
        words[2 * i + 1] = $HOLE_HI;
      }
    }
  }
  return ptr;
}
",
        );
    }
    if needs.copy_back {
        out.push_str(
            r"
function $copyBack(array, View, ptr) {
  if (array.length !== 0) array.set(new View($wasm.memory.buffer, ptr, array.length));
}
",
        );
    }
    if needs.copy_back_numbers {
        out.push_str(
            r"
// Writes back only the elements that changed, so holes the island didn't write stay holes.
function $copyBackNumbers(array, ptr) {
  const view = new Float64Array($wasm.memory.buffer, ptr, array.length);
  const words = new Uint32Array($wasm.memory.buffer, ptr, array.length * 2);
  for (let i = 0; i < view.length; i++) {
    if (!$isHole(words, i) && !Object.is(view[i], array[i])) array[i] = view[i];
  }
}
",
        );
    }
    if needs.take_array {
        out.push_str(
            r"
function $takeArray(ret, View) {
  const [ptr, length] = new Uint32Array($wasm.memory.buffer, ret, 2);
  return new View($wasm.memory.buffer, ptr, length).slice();
}
",
        );
    }
    if needs.take_numbers {
        out.push_str(
            r"
function $takeNumbers(ret) {
  const [ptr, length] = new Uint32Array($wasm.memory.buffer, ret, 2);
  return Array.from(new Float64Array($wasm.memory.buffer, ptr, length));
}
",
        );
    }
    if needs.overlaps {
        out.push_str(
            r"
function $overlaps(a, b) {
  if (a === b) return true;
  if (Array.isArray(a) || Array.isArray(b) || a.buffer !== b.buffer) return false;
  return a.byteOffset < b.byteOffset + b.byteLength && b.byteOffset < a.byteOffset + a.byteLength;
}
",
        );
    }
    if needs.structs.iter().any(|s| s.write) {
        out.push_str(
            r"
// An own data property (a writable one if the island may write it): copying it in and back
// then reads and writes it as JS would. Otherwise the call runs the JS version.
function $isData(object, name, writable) {
  const property =
    typeof object === 'object' && object !== null ? Object.getOwnPropertyDescriptor(object, name) : undefined;
  return property !== undefined && 'value' in property && (!writable || property.writable);
}
",
        );
    }
    if needs.structs.iter().any(|s| s.update) {
        out.push_str(
            r"
// Whether the island changed `size` bytes at `at` since `before` (from `from`) was taken: only
// fields it wrote are copied back.
function $changed(view, at, before, from, size) {
  for (let i = 0; i < size; i++) if (view.getUint8(at + i) !== before[from + i]) return true;
  return false;
}
",
        );
    }
    for (index, struct_needs) in needs.structs.iter().enumerate() {
        write_struct_helpers(out, spec, index, *struct_needs);
    }
}

/// `(DataView getter, setter)` suffix and how values convert, for a scalar field.
fn accessor(ty: &AbiType) -> &'static str {
    match ty {
        AbiType::F64 => "Float64",
        AbiType::I32 => "Int32",
        AbiType::U32 => "Uint32",
        AbiType::Bool => "Uint8",
        _ => unreachable!("not a scalar field"),
    }
}

fn write_struct_helpers(out: &mut String, spec: &IslandSpec, index: usize, needs: StructNeeds) {
    let name = &spec.structs[index].name;
    let layout = spec.struct_layout(index);
    if needs.write {
        let checks: Vec<String> = layout
            .fields
            .iter()
            .map(|field| match &field.ty {
                AbiType::Struct { index } => format!(
                    "$isData(value, {0}, false) && $plain{1}(value.{2}, writable)",
                    js_string(&field.name),
                    spec.structs[*index].name,
                    field.name
                ),
                _ => format!("$isData(value, {}, writable)", js_string(&field.name)),
            })
            .collect();
        writeln!(
            out,
            "\nfunction $plain{name}(value, writable) {{\n  return {};\n}}",
            if checks.is_empty() {
                "typeof value === 'object' && value !== null".to_string()
            } else {
                checks.join(" && ")
            }
        )
        .unwrap();
        writeln!(out, "\nfunction $write{name}(view, ptr, value) {{").unwrap();
        for field in &layout.fields {
            let at = offset(field.offset);
            match &field.ty {
                AbiType::Struct { index } => writeln!(
                    out,
                    "  $write{}(view, ptr{at}, value.{});",
                    spec.structs[*index].name, field.name
                ),
                AbiType::Bool => writeln!(
                    out,
                    "  view.setUint8(ptr{at}, value.{} ? 1 : 0);",
                    field.name
                ),
                ty => writeln!(
                    out,
                    "  view.set{}(ptr{at}, value.{}, true);",
                    accessor(ty),
                    field.name
                ),
            }
            .unwrap();
        }
        out.push_str("}\n");
    }
    if needs.read {
        writeln!(out, "\nfunction $read{name}(view, ptr) {{\n  return {{").unwrap();
        for field in &layout.fields {
            writeln!(out, "    {}: {},", field.name, read_field(spec, field)).unwrap();
        }
        out.push_str("  };\n}\n");
    }
    if needs.update {
        writeln!(
            out,
            "\nfunction $update{name}(view, ptr, value, before, base) {{"
        )
        .unwrap();
        for field in &layout.fields {
            let at = offset(field.offset);
            match &field.ty {
                AbiType::Struct { index } => writeln!(
                    out,
                    "  $update{}(view, ptr{at}, value.{}, before, base{at});",
                    spec.structs[*index].name, field.name
                ),
                ty => writeln!(
                    out,
                    "  if ($changed(view, ptr{at}, before, base{at}, {})) value.{} = {};",
                    scalar_size(ty),
                    field.name,
                    read_field(spec, field)
                ),
            }
            .unwrap();
        }
        out.push_str("}\n");
    }
}

fn scalar_size(ty: &AbiType) -> u32 {
    match ty {
        AbiType::F64 => 8,
        AbiType::Bool => 1,
        _ => 4,
    }
}

fn offset(offset: u32) -> String {
    if offset == 0 {
        String::new()
    } else {
        format!(" + {offset}")
    }
}

fn read_field(spec: &IslandSpec, field: &crate::spec::FieldLayout) -> String {
    let at = offset(field.offset);
    match &field.ty {
        AbiType::Struct { index } => format!("$read{}(view, ptr{at})", spec.structs[*index].name),
        AbiType::Bool => format!("view.getUint8(ptr{at}) !== 0"),
        AbiType::F64 => format!("view.getFloat64(ptr{at}, true)"),
        ty => format!("view.get{}(ptr{at}, true)", accessor(ty)),
    }
}

/// The JS version of an export in the `fallback` module.
fn fallback(export: &IslandExport) -> String {
    let name = export.fallback_name.as_deref().unwrap_or(&export.name);
    if crate::spec::is_js_name(name) {
        format!("$fallback.{name}")
    } else {
        format!("$fallback[{}]", js_string(name))
    }
}

/// How an argument's temporary is introduced: mutated ones are declared before the `try` so
/// the `finally` can copy them back.
fn declare(mutated: bool) -> &'static str {
    if mutated { "" } else { "const " }
}

fn view_of(element: TypedArrayElement) -> &'static str {
    element.js_constructor()
}

fn write_wrapper(out: &mut String, spec: &IslandSpec, export: &IslandExport, ready: bool) {
    let params: Vec<&str> = export.params.iter().map(|p| p.name.as_str()).collect();
    let params = params.join(", ");
    writeln!(out, "\nexport function {}({params}) {{", export.name).unwrap();
    if ready {
        match &spec.fallback {
            Some(_) => writeln!(
                out,
                "  if ($wasm === undefined) return {}({params});",
                fallback(export)
            )
            .unwrap(),
            None => out.push_str("  $check();\n"),
        }
    }

    let checks = alias_checks(export);
    if !checks.is_empty() {
        let conditions: Vec<String> = checks
            .iter()
            .map(|&(i, j)| {
                let (a, b) = (&export.params[i].name, &export.params[j].name);
                if export.params[i].ty.is_array() {
                    format!("$overlaps({a}, {b})")
                } else {
                    format!("{a} === {b}")
                }
            })
            .collect();
        writeln!(out, "  if ({}) {{", conditions.join(" || ")).unwrap();
        match &spec.fallback {
            Some(_) => writeln!(out, "    return {}({params});", fallback(export)),
            None => writeln!(
                out,
                "    throw new TypeError('trioxide: {}() was called with overlapping arguments, which island \"{}\" cannot handle');",
                export.name, spec.id.0
            ),
        }
        .unwrap();
        out.push_str("  }\n");
    }

    let plain: Vec<String> = export
        .params
        .iter()
        .filter_map(|param| match &param.ty {
            AbiType::Struct { index } => Some(format!(
                "!$plain{}({}, {})",
                spec.structs[*index].name, param.name, param.mutated
            )),
            _ => None,
        })
        .collect();
    if !plain.is_empty() {
        writeln!(out, "  if ({}) {{", plain.join(" || ")).unwrap();
        match &spec.fallback {
            Some(_) => writeln!(out, "    return {}({params});", fallback(export)),
            None => writeln!(
                out,
                "    throw new TypeError('trioxide: {}() got an object whose fields aren\\'t plain data properties, which island \"{}\" cannot handle');",
                export.name, spec.id.0
            ),
        }
        .unwrap();
        out.push_str("  }\n");
    }

    let mut body = String::new();
    let mut args = Vec::new();
    let mut copy_back = Vec::new();
    for (index, param) in export.params.iter().enumerate() {
        let name = &param.name;
        match &param.ty {
            AbiType::F64 | AbiType::I32 | AbiType::U32 => args.push(name.clone()),
            AbiType::Bool => args.push(format!("{name} ? 1 : 0")),
            AbiType::TypedArray { .. } | AbiType::NumberArray => {
                let element = param.ty.array_element().expect("arrays have elements");
                let view = view_of(element);
                let copy_in = if param.ty == AbiType::NumberArray {
                    format!("$copyInNumbers({name})")
                } else {
                    format!("$copyIn({name}, {view})")
                };
                writeln!(body, "    {}${index} = {copy_in};", declare(param.mutated)).unwrap();
                args.push(format!("${index}"));
                args.push(format!("{name}.length"));
                if param.mutated {
                    copy_back.push(if param.ty == AbiType::NumberArray {
                        format!("$copyBackNumbers({name}, ${index});")
                    } else {
                        format!("$copyBack({name}, {view}, ${index});")
                    });
                }
            }
            AbiType::Struct {
                index: struct_index,
            } => {
                let layout = spec.struct_layout(*struct_index);
                let struct_name = &spec.structs[*struct_index].name;
                writeln!(
                    body,
                    "    {}${index} = $alloc({}, {});\n    $write{struct_name}(new DataView($wasm.memory.buffer), ${index}, {name});",
                    declare(param.mutated),
                    layout.size,
                    layout.align
                )
                .unwrap();
                args.push(format!("${index}"));
                if param.mutated {
                    writeln!(
                        body,
                        "    ${index}b = new Uint8Array($wasm.memory.buffer, ${index}, {}).slice();",
                        layout.size
                    )
                    .unwrap();
                    copy_back.push(format!(
                        "$update{struct_name}(new DataView($wasm.memory.buffer), ${index}, {name}, ${index}b, 0);"
                    ));
                }
            }
        }
    }

    let call = format!("$wasm.{}({})", export.wasm_name, args.join(", "));
    if !copy_back.is_empty() {
        body.push_str("    $called = true;\n");
    }
    let result = match &export.ret {
        None => None,
        Some(AbiType::F64 | AbiType::I32) => Some("$ret".to_string()),
        Some(AbiType::U32) => Some("$ret >>> 0".to_string()),
        Some(AbiType::Bool) => Some("$ret !== 0".to_string()),
        Some(AbiType::TypedArray { element }) => {
            Some(format!("$takeArray($ret, {})", view_of(*element)))
        }
        Some(AbiType::NumberArray) => Some("$takeNumbers($ret)".to_string()),
        Some(AbiType::Struct { index }) => Some(format!(
            "$read{}(new DataView($wasm.memory.buffer), $ret)",
            spec.structs[*index].name
        )),
    };
    match &result {
        None => writeln!(body, "    {call};").unwrap(),
        Some(result) if result == "$ret" => writeln!(body, "    return {call};").unwrap(),
        Some(_) => writeln!(body, "    const $ret = {call};").unwrap(),
    }
    if let Some(result) = &result
        && result != "$ret"
    {
        writeln!(body, "    return {result};").unwrap();
    }

    // Mutated arguments are copied back once the island has run, even if it trapped: the
    // writes it made before are visible in JS too.
    let mut cleanup = String::new();
    if !copy_back.is_empty() {
        let temps: Vec<String> = export
            .params
            .iter()
            .enumerate()
            .filter(|(_, param)| param.mutated)
            .flat_map(|(index, param)| {
                let before =
                    matches!(param.ty, AbiType::Struct { .. }).then(|| format!("${index}b"));
                std::iter::once(format!("${index}")).chain(before)
            })
            .collect();
        writeln!(out, "  let {};\n  let $called = false;", temps.join(", ")).unwrap();
        cleanup.push_str("    if ($called) {\n");
        for line in &copy_back {
            writeln!(cleanup, "      {line}").unwrap();
        }
        cleanup.push_str("    }\n");
    }
    write!(
        out,
        "  try {{\n{body}  }} catch ($error) {{\n    throw $trap($error, {});\n  }} finally {{\n{cleanup}    $wasm.__trioxide_reset();\n  }}\n}}\n",
        js_string(&export.name)
    )
    .unwrap();
}
