//! [`IslandSpec`]: the boundary contract of one island (`docs/ABI.md`).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// The ABI version that [`IslandSpec`] describes. Bump on any incompatible change to how
/// values cross the boundary; generated glue checks it against the module's
/// `__trioxide_abi_version` export, and `trioxide_rt::ABI_VERSION` must match.
pub const ABI_VERSION: u32 = 1;

/// A stable identifier for an island, unique within a build. It names the island's files
/// (`<id>.wasm`, `<id>.js`) and its virtual glue module, `\0trioxide:island/<id>`, so it may
/// only contain ASCII letters, digits, `-` and `_`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IslandId(pub String);

impl IslandId {
    /// The specifier JS code uses to import this island's glue module (resolved by the Vite
    /// plugin or written to disk by `trioxide build`).
    pub fn glue_specifier(&self) -> String {
        format!("\0trioxide:island/{}", self.0)
    }

    /// Whether the id is non-empty, at most 64 bytes, and only uses `[A-Za-z0-9_-]`.
    pub fn is_valid(&self) -> bool {
        !self.0.is_empty()
            && self.0.len() <= 64
            && self
                .0
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }
}

impl std::fmt::Display for IslandId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a value crosses the JS↔WASM boundary (PLAN §5 eligibility, §6 "ABI and glue").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AbiType {
    /// A JS number passed as `f64`.
    F64,
    /// A JS number proven to be an int32, passed as `i32`.
    I32,
    /// A JS number proven to be a uint32, passed as `i32` and read as `u32`.
    U32,
    /// A JS boolean, passed as `i32` 0/1.
    Bool,
    /// A typed array, copied into linear memory and passed as pointer + length.
    TypedArray {
        /// Element type.
        element: TypedArrayElement,
    },
    /// A `number[]`, copied into an `f64` buffer and passed as pointer + length.
    NumberArray,
    /// A plain-data struct, by index into [`IslandSpec::structs`], copied into linear memory
    /// and passed as a pointer.
    Struct {
        /// Index into [`IslandSpec::structs`].
        index: usize,
    },
}

impl AbiType {
    /// Whether the value is a number or boolean, passed directly.
    pub fn is_scalar(&self) -> bool {
        matches!(self, Self::F64 | Self::I32 | Self::U32 | Self::Bool)
    }

    /// Whether the value is a typed array or `number[]`.
    pub fn is_array(&self) -> bool {
        matches!(self, Self::TypedArray { .. } | Self::NumberArray)
    }

    /// The element type of an array, `number[]` being [`TypedArrayElement::F64`].
    pub fn array_element(&self) -> Option<TypedArrayElement> {
        match self {
            Self::TypedArray { element } => Some(*element),
            Self::NumberArray => Some(TypedArrayElement::F64),
            _ => None,
        }
    }

    /// The WebAssembly value types this type occupies as a parameter.
    pub fn wasm_params(&self) -> &'static [WasmType] {
        match self {
            Self::F64 => &[WasmType::F64],
            Self::I32 | Self::U32 | Self::Bool | Self::Struct { .. } => &[WasmType::I32],
            Self::TypedArray { .. } | Self::NumberArray => &[WasmType::I32, WasmType::I32],
        }
    }

    /// The WebAssembly value type this type is returned as.
    pub fn wasm_result(&self) -> WasmType {
        match self {
            Self::F64 => WasmType::F64,
            _ => WasmType::I32,
        }
    }

    /// The TypeScript spelling of the type, for comments in the glue.
    pub fn ts_name(&self, spec: &IslandSpec) -> String {
        match self {
            Self::F64 | Self::I32 | Self::U32 => "number".into(),
            Self::Bool => "boolean".into(),
            Self::TypedArray { element } => element.js_constructor().into(),
            Self::NumberArray => "number[]".into(),
            Self::Struct { index } => spec
                .structs
                .get(*index)
                .map_or_else(|| "object".into(), |s| s.name.clone()),
        }
    }
}

/// A WebAssembly value type at the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WasmType {
    /// `i32`
    I32,
    /// `f64`
    F64,
}

/// The element type of a typed array crossing the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TypedArrayElement {
    /// `Int8Array`
    I8,
    /// `Uint8Array`
    U8,
    /// `Uint8ClampedArray`
    U8Clamped,
    /// `Int16Array`
    I16,
    /// `Uint16Array`
    U16,
    /// `Int32Array`
    I32,
    /// `Uint32Array`
    U32,
    /// `Float32Array`
    F32,
    /// `Float64Array`
    F64,
}

impl TypedArrayElement {
    /// The JS constructor, e.g. `Uint8ClampedArray`.
    pub fn js_constructor(self) -> &'static str {
        match self {
            Self::I8 => "Int8Array",
            Self::U8 => "Uint8Array",
            Self::U8Clamped => "Uint8ClampedArray",
            Self::I16 => "Int16Array",
            Self::U16 => "Uint16Array",
            Self::I32 => "Int32Array",
            Self::U32 => "Uint32Array",
            Self::F32 => "Float32Array",
            Self::F64 => "Float64Array",
        }
    }

    /// The Rust element type in `trioxide_rt`, e.g. `U8Clamped`.
    pub fn rust_type(self) -> &'static str {
        match self {
            Self::I8 => "i8",
            Self::U8 => "u8",
            Self::U8Clamped => "U8Clamped",
            Self::I16 => "i16",
            Self::U16 => "u16",
            Self::I32 => "i32",
            Self::U32 => "u32",
            Self::F32 => "f32",
            Self::F64 => "f64",
        }
    }

    /// Bytes per element, which is also the element's alignment.
    pub fn bytes(self) -> u32 {
        match self {
            Self::I8 | Self::U8 | Self::U8Clamped => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }
}

/// One parameter of an exported island function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AbiParam {
    /// The JS parameter name (for glue readability and `Function.length`).
    pub name: String,
    /// How it crosses the boundary.
    pub ty: AbiType,
    /// For arrays and structs: whether the island writes to it, so the glue must copy it
    /// back.
    #[serde(default)]
    pub mutated: bool,
}

/// A function the island exports to JS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandExport {
    /// The JS-visible name the glue exports, matching the original function.
    pub name: String,
    /// The symbol the `.wasm` module exports for it.
    pub wasm_name: String,
    /// Parameters in order.
    pub params: Vec<AbiParam>,
    /// The return type, or `None` for `void` (the wrapper returns `undefined`).
    pub ret: Option<AbiType>,
    /// The name the `fallback` module exports the JS version under, when it isn't `name`
    /// (`export { f as g }`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_name: Option<String>,
}

/// One field of a [`StructSpec`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructField {
    /// The JS property name.
    pub name: String,
    /// A scalar or a nested [`AbiType::Struct`]; arrays can't be fields.
    pub ty: AbiType,
}

/// A plain-data struct shared by the Rust side and the glue: a class or object type whose
/// instances only hold these own data properties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructSpec {
    /// The class or type name it came from (the Rust struct's name).
    pub name: String,
    /// Fields in declaration order, which is also the layout order and the property order
    /// of objects the glue creates.
    pub fields: Vec<StructField>,
}

/// Where each field of a struct lives in linear memory: C layout (`#[repr(C)]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructLayout {
    /// Total size in bytes, a multiple of `align`.
    pub size: u32,
    /// Alignment in bytes.
    pub align: u32,
    /// Fields in order with their byte offsets.
    pub fields: Vec<FieldLayout>,
}

/// One field of a [`StructLayout`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldLayout {
    /// The JS property name.
    pub name: String,
    /// Its type.
    pub ty: AbiType,
    /// Byte offset from the start of the struct.
    pub offset: u32,
}

/// The boundary contract of one island: what it exports and how values cross.
///
/// This is the interface between the Rust backend (S6), which generates the crate and the
/// spec, and the builder and glue generator (S7). Everything the glue needs is here; the glue
/// never inspects the Rust source. `docs/ABI.md` specifies the Rust each spec implies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandSpec {
    /// The island's id.
    pub id: IslandId,
    /// The [`ABI_VERSION`] the spec was produced for.
    pub abi_version: u32,
    /// Functions callable from JS.
    pub exports: Vec<IslandExport>,
    /// Struct layouts referenced by [`AbiType::Struct`].
    #[serde(default)]
    pub structs: Vec<StructSpec>,
    /// A module specifier, relative to the glue module, exporting the JS versions of the
    /// island's functions under the same names. The glue calls them when a call passes
    /// overlapping arrays (or the same struct twice) and at least one is mutated, which
    /// separate copies in linear memory can't reproduce. Without it such calls throw a
    /// `TypeError`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

/// Why an [`IslandSpec`] is malformed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid island spec `{island}`: {message}")]
pub struct SpecError {
    /// The island.
    pub island: String,
    /// What is wrong.
    pub message: String,
}

/// JavaScript reserved words, which can't be function or parameter names.
const RESERVED: &[&str] = &[
    "arguments",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "eval",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// An ASCII JS identifier that doesn't start with `$` (the glue's own names do) and isn't
/// reserved.
pub(crate) fn is_js_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
        && !RESERVED.contains(&name)
}

/// A symbol name usable as a wasm export and a Rust `export_name`.
fn is_wasm_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !name.starts_with("__trioxide_")
        && name != "memory"
}

impl IslandSpec {
    /// Parses a spec from JSON (the `island.json` format).
    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        serde_json::from_str(json)
    }

    /// The spec as pretty-printed JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("specs serialise") + "\n"
    }

    /// The `<id>.wasm` file name the glue loads by default.
    pub fn wasm_file_name(&self) -> String {
        format!("{}.wasm", self.id.0)
    }

    /// Checks everything the glue and builder rely on: the ABI version, names (valid,
    /// unique, not reserved), struct references and layouts, and that only arrays and structs
    /// are marked `mutated`.
    pub fn validate(&self) -> Result<(), SpecError> {
        let error = |message: String| SpecError {
            island: self.id.0.clone(),
            message,
        };
        if self.abi_version != ABI_VERSION {
            return Err(error(format!(
                "ABI version {} is not supported (expected {ABI_VERSION})",
                self.abi_version
            )));
        }
        if !self.id.is_valid() {
            return Err(error("ids may only use [A-Za-z0-9_-]".into()));
        }
        if self.exports.is_empty() {
            return Err(error("an island must export at least one function".into()));
        }
        let check_type = |ty: &AbiType, context: &str| -> Result<(), SpecError> {
            match ty {
                AbiType::Struct { index } if *index >= self.structs.len() => Err(error(format!(
                    "{context} refers to struct {index}, but there are {}",
                    self.structs.len()
                ))),
                _ => Ok(()),
            }
        };

        let mut names = BTreeSet::new();
        let mut wasm_names = BTreeSet::new();
        for export in &self.exports {
            if !is_js_name(&export.name)
                || export.name == "ready"
                || !names.insert(export.name.as_str())
            {
                return Err(error(format!(
                    "export name `{}` is invalid, reserved or duplicated",
                    export.name
                )));
            }
            if !is_wasm_name(&export.wasm_name) || !wasm_names.insert(export.wasm_name.as_str()) {
                return Err(error(format!(
                    "wasm name `{}` is invalid, reserved or duplicated",
                    export.wasm_name
                )));
            }
            let mut params = BTreeSet::new();
            for param in &export.params {
                let context = format!("parameter `{}` of `{}`", param.name, export.name);
                if !is_js_name(&param.name) || !params.insert(param.name.as_str()) {
                    return Err(error(format!("{context} has an invalid or duplicate name")));
                }
                check_type(&param.ty, &context)?;
                if param.mutated && param.ty.is_scalar() {
                    return Err(error(format!(
                        "{context} is a scalar and can't be `mutated`"
                    )));
                }
            }
            if let Some(ret) = &export.ret {
                check_type(ret, &format!("the return type of `{}`", export.name))?;
            }
        }

        for (index, spec) in self.structs.iter().enumerate() {
            if !is_js_name(&spec.name) {
                return Err(error(format!("struct name `{}` is invalid", spec.name)));
            }
            if spec.fields.is_empty() {
                return Err(error(format!("struct `{}` has no fields", spec.name)));
            }
            let mut fields = BTreeSet::new();
            for field in &spec.fields {
                let context = format!("field `{}` of `{}`", field.name, spec.name);
                if !is_js_name(&field.name) || !fields.insert(field.name.as_str()) {
                    return Err(error(format!("{context} has an invalid or duplicate name")));
                }
                if field.ty.is_array() {
                    return Err(error(format!(
                        "{context} is an array; fields must be plain data"
                    )));
                }
                check_type(&field.ty, &context)?;
            }
            if self.contains_struct(index, index, 0) {
                return Err(error(format!("struct `{}` contains itself", spec.name)));
            }
        }
        Ok(())
    }

    /// Whether struct `outer` (transitively) has a field of struct type `target`.
    fn contains_struct(&self, outer: usize, target: usize, depth: usize) -> bool {
        depth <= self.structs.len()
            && self.structs[outer]
                .fields
                .iter()
                .any(|field| match field.ty {
                    AbiType::Struct { index } => {
                        index == target || self.contains_struct(index, target, depth + 1)
                    }
                    _ => false,
                })
    }

    /// The C layout of struct `index`, which the Rust side declares as `#[repr(C)]` with the
    /// fields in the same order. The spec must be [valid](Self::validate).
    pub fn struct_layout(&self, index: usize) -> StructLayout {
        let mut offset = 0_u32;
        let mut align = 1_u32;
        let mut fields = Vec::new();
        for field in &self.structs[index].fields {
            let (size, field_align) = self.size_align(&field.ty);
            offset = offset.next_multiple_of(field_align);
            fields.push(FieldLayout {
                name: field.name.clone(),
                ty: field.ty.clone(),
                offset,
            });
            offset += size;
            align = align.max(field_align);
        }
        StructLayout {
            size: offset.next_multiple_of(align),
            align,
            fields,
        }
    }

    fn size_align(&self, ty: &AbiType) -> (u32, u32) {
        match ty {
            AbiType::F64 => (8, 8),
            AbiType::I32 | AbiType::U32 => (4, 4),
            AbiType::Bool => (1, 1),
            AbiType::Struct { index } => {
                let layout = self.struct_layout(*index);
                (layout.size, layout.align)
            }
            AbiType::TypedArray { .. } | AbiType::NumberArray => {
                unreachable!("arrays are not struct fields")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(json: &str) -> IslandSpec {
        IslandSpec::from_json(json).unwrap()
    }

    const BASE: &str = r#"{
        "id": "demo", "abiVersion": 1,
        "exports": [{ "name": "f", "wasmName": "f", "params": [{ "name": "x", "ty": { "kind": "f64" } }], "ret": { "kind": "f64" } }]
    }"#;

    #[test]
    fn layouts_follow_c_rules() {
        let spec = spec(
            r#"{
            "id": "demo", "abiVersion": 1,
            "exports": [{ "name": "f", "wasmName": "f", "params": [], "ret": { "kind": "struct", "index": 1 } }],
            "structs": [
                { "name": "Inner", "fields": [{ "name": "flag", "ty": { "kind": "bool" } }, { "name": "n", "ty": { "kind": "i32" } }] },
                { "name": "Outer", "fields": [
                    { "name": "a", "ty": { "kind": "bool" } },
                    { "name": "x", "ty": { "kind": "f64" } },
                    { "name": "inner", "ty": { "kind": "struct", "index": 0 } },
                    { "name": "b", "ty": { "kind": "bool" } }
                ] }
            ]
        }"#,
        );
        spec.validate().unwrap();
        let inner = spec.struct_layout(0);
        assert_eq!((inner.size, inner.align), (8, 4));
        assert_eq!(inner.fields[1].offset, 4);
        let outer = spec.struct_layout(1);
        let offsets: Vec<u32> = outer.fields.iter().map(|f| f.offset).collect();
        assert_eq!(offsets, [0, 8, 16, 24]);
        assert_eq!((outer.size, outer.align), (32, 8));
    }

    #[test]
    fn validation() {
        spec(BASE).validate().unwrap();
        let invalid = [
            (r#""abiVersion": 1"#, r#""abiVersion": 0"#, "ABI version"),
            (r#""id": "demo""#, r#""id": "a/b""#, "ids"),
            (
                r#""wasmName": "f""#,
                r#""wasmName": "__trioxide_alloc""#,
                "wasm name",
            ),
            (r#""wasmName": "f""#, r#""wasmName": "memory""#, "wasm name"),
            (r#""name": "f""#, r#""name": "delete""#, "export name"),
            (r#""name": "f""#, r#""name": "ready""#, "export name"),
            (r#""name": "x""#, r#""name": "$x""#, "parameter"),
            (
                r#""ty": { "kind": "f64" } }]"#,
                r#""ty": { "kind": "f64" }, "mutated": true }]"#,
                "scalar",
            ),
            (
                r#""ret": { "kind": "f64" }"#,
                r#""ret": { "kind": "struct", "index": 0 }"#,
                "struct 0",
            ),
        ];
        for (from, to, expected) in invalid {
            let json = BASE.replacen(from, to, 1);
            assert_ne!(json, BASE, "{from}");
            let error = spec(&json).validate().unwrap_err();
            assert!(error.message.contains(expected), "{to}: {error}");
        }
        let recursive = r#"{
            "id": "demo", "abiVersion": 1,
            "exports": [{ "name": "f", "wasmName": "f", "params": [] }],
            "structs": [{ "name": "S", "fields": [{ "name": "s", "ty": { "kind": "struct", "index": 0 } }] }]
        }"#;
        assert!(spec(recursive).validate().is_err());
    }
}
