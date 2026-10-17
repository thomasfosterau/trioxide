//! The JS glue module for an island (`docs/ABI.md`, "Glue").
//!
//! The glue instantiates the `.wasm` and exports one wrapper per island function with the
//! original name and parameters. Each wrapper copies arrays and structs into the island's
//! arena, calls the export, converts the result, copies mutated arguments back, and resets
//! the arena. The glue's own names start with `$`, which spec names may not.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::spec::{ABI_VERSION, AbiType, IslandExport, IslandSpec, TypedArrayElement};

/// How the glue waits for the module to instantiate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GlueMode {
    /// Top-level `await`: the island is ready when the glue module has evaluated.
    #[default]
    Tla,
    /// Export `ready()`, a promise that resolves once the island is instantiated, for targets
    /// without top-level await. Wrappers throw until then.
    Ready,
}

/// Where the glue loads the `.wasm` from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WasmUrl {
    /// `new URL("./<id>.wasm", import.meta.url)`: the `.wasm` sits next to the glue. Vite
    /// recognises this pattern and emits the file as a hashed asset.
    #[default]
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
                        needs.copy_in = true;
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
let $wasm;
const $ready = $instantiate().then((exports) => {{
  $wasm = exports;
}});
// Rejections surface through ready(); don't also report them as unhandled.
$ready.catch(() => {{}});

/** Resolves once the island is instantiated. Its functions throw until then. */
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
  if ($wasm.__trioxide_trap_code() === 1) return new RangeError(`trioxide: island "{id}" ran out of memory in ${{name}}()`);
  return new Error(`trioxide: island "{id}" trapped in ${{name}}(); this is a trioxide bug`, {{ cause: error }});
}}
"#
    )
    .unwrap();
    if needs.copy_in {
        out.push_str(
            r"
// Copies a typed array or number[] into the arena as View elements.
function $copyIn(array, View) {
  const ptr = $alloc(array.length * View.BYTES_PER_ELEMENT, View.BYTES_PER_ELEMENT);
  new View($wasm.memory.buffer, ptr, array.length).set(array);
  return ptr;
}
",
        );
    }
    if needs.copy_back {
        out.push_str(
            r"
function $copyBack(array, View, ptr) {
  array.set(new View($wasm.memory.buffer, ptr, array.length));
}
",
        );
    }
    if needs.copy_back_numbers {
        out.push_str(
            r"
// Writes back only the elements that changed, so holes stay holes.
function $copyBackNumbers(array, ptr) {
  const view = new Float64Array($wasm.memory.buffer, ptr, array.length);
  for (let i = 0; i < view.length; i++) if (!Object.is(view[i], +array[i])) array[i] = view[i];
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
        writeln!(out, "\nfunction $update{name}(view, ptr, value) {{").unwrap();
        for field in &layout.fields {
            match &field.ty {
                AbiType::Struct { index } => writeln!(
                    out,
                    "  $update{}(view, ptr{}, value.{});",
                    spec.structs[*index].name,
                    offset(field.offset),
                    field.name
                ),
                _ => writeln!(out, "  value.{} = {};", field.name, read_field(spec, field)),
            }
            .unwrap();
        }
        out.push_str("}\n");
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

fn view_of(element: TypedArrayElement) -> &'static str {
    element.js_constructor()
}

fn write_wrapper(out: &mut String, spec: &IslandSpec, export: &IslandExport, ready: bool) {
    let params: Vec<&str> = export.params.iter().map(|p| p.name.as_str()).collect();
    let params = params.join(", ");
    writeln!(out, "\nexport function {}({params}) {{", export.name).unwrap();
    if ready {
        out.push_str("  $check();\n");
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
            Some(_) => writeln!(out, "    return $fallback.{}({params});", export.name),
            None => writeln!(
                out,
                "    throw new TypeError('trioxide: {}() was called with overlapping arguments, which island \"{}\" cannot handle');",
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
                writeln!(body, "    const ${index} = $copyIn({name}, {view});").unwrap();
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
                    "    const ${index} = $alloc({}, {});\n    $write{struct_name}(new DataView($wasm.memory.buffer), ${index}, {name});",
                    layout.size, layout.align
                )
                .unwrap();
                args.push(format!("${index}"));
                if param.mutated {
                    copy_back.push(format!(
                        "$update{struct_name}(new DataView($wasm.memory.buffer), ${index}, {name});"
                    ));
                }
            }
        }
    }

    let call = format!("$wasm.{}({})", export.wasm_name, args.join(", "));
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
    match (&result, copy_back.is_empty()) {
        (None, _) => writeln!(body, "    {call};").unwrap(),
        (Some(result), true) if result == "$ret" => writeln!(body, "    return {call};").unwrap(),
        (Some(_), _) => writeln!(body, "    const $ret = {call};").unwrap(),
    }
    for line in &copy_back {
        writeln!(body, "    {line}").unwrap();
    }
    if let Some(result) = &result
        && !(result == "$ret" && copy_back.is_empty())
    {
        writeln!(body, "    return {result};").unwrap();
    }

    write!(
        out,
        "  try {{\n{body}  }} catch ($error) {{\n    throw $trap($error, {});\n  }} finally {{\n    $wasm.__trioxide_reset();\n  }}\n}}\n",
        js_string(&export.name)
    )
    .unwrap();
}
