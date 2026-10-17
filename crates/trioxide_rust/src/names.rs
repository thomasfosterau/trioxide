//! Turning JS names into Rust and ABI names.

use oxc_ast::ast::{BindingPattern, FormalParameter, TSType};

/// Rust's strict and reserved keywords (2024 edition).
const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "dyn", "else", "enum", "extern", "false",
    "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "static", "struct", "trait", "true", "type", "unsafe", "use", "where",
    "while", "abstract", "become", "box", "do", "final", "macro", "override", "priv", "try",
    "typeof", "unsized", "virtual", "yield",
];

/// Names that can't even be raw identifiers.
const NOT_RAW: &[&str] = &["self", "Self", "super", "crate", "_"];

/// Type names the generated crate already uses.
const RESERVED_TYPES: &[&str] = &[
    "Element",
    "U8Clamped",
    "Clone",
    "Copy",
    "Option",
    "Some",
    "None",
    "Result",
    "Ok",
    "Err",
    "Default",
    "Drop",
    "Fn",
    "FnMut",
    "FnOnce",
    "Iterator",
    "Sized",
    "Send",
    "Sync",
    "PartialEq",
    "PartialOrd",
    "Eq",
    "Ord",
    "From",
    "Into",
    "Debug",
];

/// JS reserved words (and names the glue can't use as parameters).
const JS_RESERVED: &[&str] = &[
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
    "arguments",
    "eval",
];

/// A JS identifier as a Rust identifier: `$` becomes `_`, keywords are raw (`r#type`), and
/// `self`-like names get a trailing `_`.
pub(crate) fn rust_ident(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| if c == '$' { '_' } else { c })
        .collect();
    if NOT_RAW.contains(&name.as_str()) {
        format!("{name}_")
    } else if KEYWORDS.contains(&name.as_str()) {
        format!("r#{name}")
    } else {
        name
    }
}

/// The name without a raw prefix, for collision checks.
pub(crate) fn bare(name: &str) -> &str {
    name.strip_prefix("r#").unwrap_or(name)
}

/// A struct name from a JS name: `stats` → `Stats`.
pub(crate) fn type_name(name: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if upper {
                out.extend(c.to_uppercase());
                upper = false;
            } else {
                out.push(c);
            }
        } else {
            upper = true;
        }
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, "Struct");
    }
    out
}

/// Whether a struct can't take this name.
pub(crate) fn is_reserved_type(name: &str) -> bool {
    RESERVED_TYPES.contains(&name) || KEYWORDS.contains(&name) || NOT_RAW.contains(&name)
}

/// Whether `name` can be a JS name in the spec: an ASCII identifier, not reserved, not
/// starting with `$` (`docs/ABI.md`).
pub(crate) fn is_abi_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        && !JS_RESERVED.contains(&name)
}

/// The spec name of a parameter: its JS name, or a safe replacement (only
/// `Function.prototype.toString` could tell).
pub(crate) fn abi_param_name(param: &FormalParameter<'_>, index: usize) -> String {
    if let BindingPattern::BindingIdentifier(id) = &param.pattern {
        let name = id.name.as_str();
        if is_abi_name(name) {
            return name.to_string();
        }
        let stripped = name.trim_start_matches('$');
        if is_abi_name(stripped) {
            return stripped.to_string();
        }
    }
    format!("p{index}")
}

/// The name of a TS type that is a plain reference (`Body`, not `Body[]` or `A.B`).
pub(crate) fn type_reference_name(ty: &TSType<'_>) -> Option<String> {
    match ty {
        TSType::TSTypeReference(reference) if reference.type_arguments.is_none() => {
            match &reference.type_name {
                oxc_ast::ast::TSTypeName::IdentifierReference(id) => Some(id.name.to_string()),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_names() {
        assert_eq!(rust_ident("total"), "total");
        assert_eq!(rust_ident("type"), "r#type");
        assert_eq!(rust_ident("self"), "self_");
        assert_eq!(rust_ident("$scale"), "_scale");
        assert_eq!(type_name("stats"), "Stats");
        assert_eq!(type_name("boxBlur_src"), "BoxBlurSrc");
        assert_eq!(type_name("2d"), "Struct2d");
        assert!(is_reserved_type("Option") && !is_reserved_type("Body"));
    }

    #[test]
    fn abi_names() {
        assert!(is_abi_name("dt") && is_abi_name("x$1"));
        assert!(!is_abi_name("$x") && !is_abi_name("default") && !is_abi_name("é"));
    }
}
