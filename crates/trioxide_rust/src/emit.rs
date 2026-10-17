//! Printing RIR as an island crate in the shape `docs/ABI.md` ("The Rust shape of an island")
//! prescribes, and its [`IslandSpec`].
//!
//! The source is built as text, checked and normalised by parsing it with `syn`, and printed
//! with `prettyplease`. `prettyplease` drops ordinary comments, so `// SAFETY:` comments are
//! written as `__trioxide_comment!("…");` statements and turned into comments afterwards.

use std::fmt::Write as _;

use trioxide_wasm::{
    ABI_VERSION, AbiParam, AbiType, IslandExport, IslandId, IslandSpec, StructField, StructSpec,
};

use crate::rir::{
    Access, BinOp, Conv, Elem, Export, Expr, FuncRef, Function, FunctionKind, Index, Island, Lit,
    LocalId, Loop, LoopId, MathFn, Stmt, StructDef, Ty, UnOp,
};

const COMMENT_MACRO: &str = "__trioxide_comment!";

/// The boundary contract of `island`.
pub fn spec(island: &Island) -> IslandSpec {
    let mut abi_structs: Vec<(usize, &StructDef)> = island
        .structs
        .iter()
        .filter_map(|s| s.abi.map(|index| (index, s)))
        .collect();
    abi_structs.sort_by_key(|(index, _)| *index);
    let structs = abi_structs
        .iter()
        .map(|(_, s)| StructSpec {
            name: s.name.clone(),
            fields: s
                .fields
                .iter()
                .map(|(name, ty)| StructField {
                    name: name.clone(),
                    ty: crate::lower::abi_type(*ty, &island.structs).unwrap_or(AbiType::F64),
                })
                .collect(),
        })
        .collect();
    IslandSpec {
        id: IslandId(island.id.clone()),
        abi_version: ABI_VERSION,
        exports: island
            .exports
            .iter()
            .map(|export| IslandExport {
                name: export.name.clone(),
                wasm_name: export.name.clone(),
                params: export
                    .params
                    .iter()
                    .map(|(name, ty, mutated)| AbiParam {
                        name: name.clone(),
                        ty: ty.clone(),
                        mutated: *mutated,
                    })
                    .collect(),
                ret: export.ret.clone(),
            })
            .collect(),
        structs,
        fallback: None,
    }
}

/// The island's `src/lib.rs`.
pub fn lib_rs(island: &Island, spec: &IslandSpec) -> String {
    let mut printer = Printer {
        island,
        out: String::new(),
        comments: std::cell::RefCell::default(),
    };
    printer.file(spec);
    // Comments are numbered markers in `printer.out`: short, so they never wrap.
    let comments = printer.comments.into_inner();
    let file = match syn::parse_file(&printer.out) {
        Ok(file) => file,
        Err(error) => {
            // A backend bug; return the unformatted text so the build error points at it.
            return format!(
                "// trioxide: generated code failed to parse: {error}\n{}",
                printer.out
            );
        }
    };
    let pretty = prettyplease::unparse(&file);
    let mut out = String::with_capacity(pretty.len());
    for line in pretty.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(COMMENT_MACRO) {
            let indent = &line[..line.len() - trimmed.len()];
            let index: usize = rest
                .trim_start_matches('(')
                .trim_end_matches(");")
                .parse()
                .unwrap_or(usize::MAX);
            let text = comments.get(index).map_or("", String::as_str);
            let _ = writeln!(out, "{indent}// {text}");
        } else if trimmed.starts_with("assert!(") {
            // Macro arguments print as loose tokens.
            out.push_str(&line.replace("::< ", "::<").replace(" > ()", ">()"));
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    space_items(&out)
}

/// Blank lines between items and between methods, as people write them.
fn space_items(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len() + lines.len());
    for (index, line) in lines.iter().enumerate() {
        out.push_str(line);
        out.push('\n');
        let Some(next) = lines.get(index + 1) else {
            break;
        };
        let ends_item = matches!(*line, "}" | "};")
            || (line.starts_with("use ")
                || line.starts_with("const ")
                || line.starts_with("#![allow"))
                && line.ends_with(';')
            || line.starts_with("#![allow")
            || (line.starts_with("//!") && !next.starts_with("//!"))
            || (*line == "    }" && (next.starts_with("    fn ") || next.starts_with("    ///")));
        if ends_item && !next.is_empty() && *next != "}" {
            out.push('\n');
        }
    }
    out
}

struct Printer<'i> {
    island: &'i Island,
    out: String,
    /// Comment texts, referred to by number from `__trioxide_comment!(n);` markers.
    comments: std::cell::RefCell<Vec<String>>,
}

/// Rust operator precedence, high binds tighter.
mod prec {
    pub const ATOM: u8 = 20;
    pub const POSTFIX: u8 = 16;
    pub const UNARY: u8 = 14;
    pub const CAST: u8 = 13;
    pub const MUL: u8 = 12;
    pub const ADD: u8 = 11;
    pub const BIT_AND: u8 = 9;
    pub const BIT_XOR: u8 = 8;
    pub const BIT_OR: u8 = 7;
    pub const CMP: u8 = 6;
    pub const AND: u8 = 5;
    pub const OR: u8 = 4;
    pub const BLOCK: u8 = 1;
}

/// Per-function printing state.
struct Fn<'f> {
    function: &'f Function,
    /// Loops being printed, innermost last.
    loops: Vec<(LoopId, bool)>,
    /// Labels of loops that need one, and loops whose `continue` is `break 'cN`.
    labelled: Vec<LoopId>,
    continue_blocks: Vec<LoopId>,
    /// An unchecked access was printed in the current statement.
    unsafe_used: bool,
    temp: usize,
}

impl Printer<'_> {
    fn safety_text(&self, text: &str) -> String {
        let mut comments = self.comments.borrow_mut();
        comments.push(format!("SAFETY: {text}."));
        format!("{COMMENT_MACRO}({});\n", comments.len() - 1)
    }

    fn safety_comment(&self) -> String {
        self.safety_text("the analysis proves the index is in bounds")
    }

    fn file(&mut self, spec: &IslandSpec) {
        let island = self.island;
        let _ = writeln!(
            self.out,
            "//! WASM island `{}`, generated by trioxide from `{}`. Do not edit.\n",
            island.id, island.module
        );
        self.out
            .push_str("#![no_std]\n#![allow(non_snake_case, clippy::all)]\n\n");
        self.out.push_str("use trioxide_rt::prelude::*;\n\n");
        for (index, def) in island.structs.iter().enumerate() {
            self.struct_def(index, def, spec);
        }
        for constant in &island.consts {
            let (ty, value) = lit_parts(constant.value);
            let _ = writeln!(self.out, "const {}: {ty} = {value};\n", constant.name);
        }
        for function in &island.functions {
            if matches!(function.kind, FunctionKind::Free) {
                self.function(function, "");
            }
        }
        for export in &island.exports {
            self.shim(export);
        }
    }

    fn struct_def(&mut self, index: usize, def: &StructDef, spec: &IslandSpec) {
        let doc = match def.abi {
            Some(abi) => format!("{} (`structs[{abi}]` in the spec).", def.doc),
            None => format!("{}.", def.doc),
        };
        let _ = writeln!(self.out, "#[doc = {}]", rust_string(&format!(" {doc}")));
        let public = if def.abi.is_some() { "pub " } else { "" };
        if def.abi.is_some() {
            self.out.push_str("#[repr(C)]\n");
        }
        self.out.push_str("#[derive(Clone, Copy)]\n");
        let _ = writeln!(self.out, "{public}struct {} {{", def.name);
        for (name, ty) in &def.fields {
            let _ = writeln!(
                self.out,
                "    {public}{}: {},",
                crate::names::rust_ident(name),
                self.ty(*ty)
            );
        }
        self.out.push_str("}\n\n");
        if let Some(abi) = def.abi {
            let layout = spec.struct_layout(abi);
            let _ = write!(
                self.out,
                "const _: () = {{ assert!(size_of::<{name}>() == {} && align_of::<{name}>() == {});",
                layout.size,
                layout.align,
                name = def.name
            );
            for field in layout.fields.iter().filter(|f| f.offset != 0) {
                let _ = write!(
                    self.out,
                    " assert!(core::mem::offset_of!({}, {}) == {});",
                    def.name,
                    crate::names::rust_ident(&field.name),
                    field.offset
                );
            }
            self.out.push_str(" };\n\n");
        }
        let _ = index;
        if !def.methods.is_empty() {
            let _ = writeln!(self.out, "impl {} {{", def.name);
            for &FuncRef(f) in &def.methods {
                self.function(&self.island.functions[f], "");
            }
            self.out.push_str("}\n\n");
        }
    }

    fn ty(&self, ty: Ty) -> String {
        match ty {
            Ty::F64 => "f64".into(),
            Ty::I32 => "i32".into(),
            Ty::U32 => "u32".into(),
            Ty::Bool => "bool".into(),
            Ty::Index => "usize".into(),
            Ty::Unit => "()".into(),
            Ty::Array(array) => {
                let elem = array.elem.storage().rust_type();
                match array.access {
                    Access::Shared => format!("&[{elem}]"),
                    Access::Mut => format!("&mut [{elem}]"),
                    Access::Fresh => format!("&'static mut [{elem}]"),
                }
            }
            Ty::Struct(s) => self.island.structs[s.0].name.clone(),
            Ty::StructRef(s, access) => {
                let name = &self.island.structs[s.0].name;
                match access {
                    Access::Mut => format!("&mut {name}"),
                    _ => format!("&{name}"),
                }
            }
        }
    }

    fn function(&mut self, function: &Function, _indent: &str) {
        let mut params = Vec::new();
        if let FunctionKind::Method(_) = function.kind {
            params.push("self".to_string());
        }
        for &LocalId(local) in &function.params {
            let local = &function.locals[local];
            let mutable = if local.mutable { "mut " } else { "" };
            params.push(format!("{mutable}{}: {}", local.name, self.ty(local.ty)));
        }
        let ret = match function.ret {
            Ty::Unit => String::new(),
            Ty::Struct(_) if matches!(function.kind, FunctionKind::Constructor(_)) => {
                " -> Self".into()
            }
            ty => format!(" -> {}", self.ty(ty)),
        };
        let _ = write!(
            self.out,
            "fn {}({}){ret} ",
            function.name,
            params.join(", ")
        );
        let mut cx = Fn {
            function,
            loops: Vec::new(),
            labelled: Vec::new(),
            continue_blocks: Vec::new(),
            unsafe_used: false,
            temp: 0,
        };
        collect_labels(&function.body, &mut cx);
        let mut body = String::new();
        let stmts = &function.body;
        // A final `return x;` becomes the tail expression.
        match stmts.split_last() {
            Some((Stmt::Return(Some(value)), rest)) => {
                body.push_str(&self.stmts(rest, &mut cx));
                cx.unsafe_used = false;
                let value = self.expr(value, &mut cx).0;
                if cx.unsafe_used {
                    body.push_str(&self.safety_comment());
                }
                body.push_str(&value);
                body.push('\n');
            }
            _ => body.push_str(&self.stmts(stmts, &mut cx)),
        }
        let _ = writeln!(self.out, "{{\n{body}}}\n");
    }

    fn stmts(&self, stmts: &[Stmt], cx: &mut Fn<'_>) -> String {
        let mut out = String::new();
        for stmt in stmts {
            cx.unsafe_used = false;
            let text = self.stmt(stmt, cx);
            out.push_str(&text);
        }
        out
    }

    /// One statement. Expressions are printed first, so `unsafe_used` tells whether the
    /// statement itself (not a nested block) needs a `// SAFETY:` comment.
    fn stmt(&self, stmt: &Stmt, cx: &mut Fn<'_>) -> String {
        let mut head = match stmt {
            Stmt::Let(LocalId(local), value) => {
                let local = &cx.function.locals[*local];
                let value = self.expr(value, cx).0;
                let mutable = if local.mutable { "mut " } else { "" };
                let annotate = match local.ty {
                    Ty::I32 | Ty::U32 | Ty::Index => true,
                    Ty::Array(array) => array.access != Access::Fresh,
                    _ => false,
                };
                let ty = if annotate {
                    format!(": {}", self.ty(local.ty))
                } else {
                    String::new()
                };
                format!("let {mutable}{}{ty} = {value};\n", local.name)
            }
            Stmt::Assign(LocalId(local), value) => {
                let name = &cx.function.locals[*local].name;
                if let Some((op, rhs)) = compound(value, &Expr::Local(LocalId(*local))) {
                    let rhs = self.expr(rhs, cx).0;
                    format!("{name} {op}= {rhs};\n")
                } else {
                    let value = self.expr(value, cx).0;
                    format!("{name} = {value};\n")
                }
            }
            Stmt::ArraySet {
                array,
                index,
                value,
            } => {
                let array = self.expr(array, cx).0;
                let value = self.expr(value, cx).0;
                if index.in_bounds {
                    let index = self.usize_index(index, cx);
                    cx.unsafe_used = true;
                    format!("unsafe {{ array::set_unchecked({array}, {index}, {value}) }};\n")
                } else {
                    let (helper, index) = self.checked_index(index, cx);
                    format!("array::set{helper}({array}, {index}, {value});\n")
                }
            }
            Stmt::FieldSet { base, field, value } => {
                let base_text = self.operand(base, prec::POSTFIX, cx);
                let field_name = crate::names::rust_ident(field);
                let target = Expr::Field(Box::new(base.clone()), field.clone());
                if let Some((op, rhs)) = compound(value, &target) {
                    let rhs = self.expr(rhs, cx).0;
                    format!("{base_text}.{field_name} {op}= {rhs};\n")
                } else {
                    let value = self.expr(value, cx).0;
                    format!("{base_text}.{field_name} = {value};\n")
                }
            }
            Stmt::Expr(expr) => {
                let unit_call = match expr {
                    Expr::Call(FuncRef(f), _) | Expr::MethodCall(FuncRef(f), _, _) => {
                        self.island.functions[*f].ret == Ty::Unit
                    }
                    _ => false,
                };
                let text = self.expr(expr, cx).0;
                if unit_call {
                    format!("{text};\n")
                } else {
                    format!("let _ = {text};\n")
                }
            }
            Stmt::If(test, then, otherwise) => {
                let test = self.expr(test, cx).0;
                let unsafe_used = cx.unsafe_used;
                let then = self.stmts(then, cx);
                let mut text = format!("if {test} {{\n{then}}}");
                let mut otherwise = otherwise.as_slice();
                // `else if` chains.
                while let [Stmt::If(test, then, rest)] = otherwise {
                    cx.unsafe_used = false;
                    let test = self.expr(test, cx).0;
                    let then = self.stmts(then, cx);
                    let _ = write!(text, " else if {test} {{\n{then}}}");
                    otherwise = rest.as_slice();
                }
                if !otherwise.is_empty() {
                    let body = self.stmts(otherwise, cx);
                    let _ = write!(text, " else {{\n{body}}}");
                }
                text.push('\n');
                cx.unsafe_used = unsafe_used;
                text
            }
            Stmt::Loop(lp) => self.loop_stmt(lp, cx),
            Stmt::ForOf {
                id,
                counter,
                array,
                local,
                elem: _,
                body,
            } => {
                let array = self.expr(array, cx).0;
                let unsafe_used = cx.unsafe_used;
                let counter = &cx.function.locals[counter.0].name;
                let value = &cx.function.locals[local.0].name;
                let label = Self::loop_label(*id, cx);
                cx.loops.push((*id, false));
                let body = self.stmts(body, cx);
                cx.loops.pop();
                cx.unsafe_used = unsafe_used;
                format!(
                    "{label}for {counter} in 0..{array}.len() {{\n{}let {value} = unsafe {{ array::get_unchecked({array}, {counter}) }};\n{body}}}\n",
                    self.safety_text("the loop keeps the index below the length")
                )
            }
            Stmt::Break(id) => {
                let innermost = cx.loops.last().map(|(id, _)| *id);
                if innermost == Some(*id) {
                    "break;\n".into()
                } else {
                    format!("break 'l{};\n", id.0)
                }
            }
            Stmt::Continue(id) => {
                if cx.continue_blocks.contains(id) {
                    format!("break 'c{};\n", id.0)
                } else if cx.loops.last().map(|(id, _)| *id) == Some(*id) {
                    "continue;\n".into()
                } else {
                    format!("continue 'l{};\n", id.0)
                }
            }
            Stmt::Return(None) => "return;\n".into(),
            Stmt::Return(Some(value)) => {
                let value = self.expr(value, cx).0;
                format!("return {value};\n")
            }
            Stmt::Block(label, body) => {
                let body = self.stmts(body, cx);
                match label {
                    Some(id) => format!("'l{}: {{\n{body}}}\n", id.0),
                    None => format!("{{\n{body}}}\n"),
                }
            }
        };
        if cx.unsafe_used {
            head.insert_str(0, &self.safety_comment());
        }
        head
    }

    fn loop_label(id: LoopId, cx: &Fn<'_>) -> String {
        if cx.labelled.contains(&id) {
            format!("'l{}: ", id.0)
        } else {
            String::new()
        }
    }

    fn loop_stmt(&self, lp: &Loop, cx: &mut Fn<'_>) -> String {
        let Loop {
            id,
            test,
            test_after,
            body,
            update,
            ..
        } = lp;
        let test_text = if *test_after {
            None
        } else {
            test.as_ref().map(|test| self.expr(test, cx).0)
        };
        let unsafe_used = cx.unsafe_used;
        let label = Self::loop_label(*id, cx);
        cx.loops.push((*id, true));
        let body_text = self.stmts(body, cx);
        let body_text = if cx.continue_blocks.contains(id) {
            format!("'c{}: {{\n{body_text}}}\n", id.0)
        } else {
            body_text
        };
        let mut tail = self.stmts(update, cx);
        if *test_after && let Some(test) = test {
            cx.unsafe_used = false;
            let test = self.expr(test, cx).0;
            let comment = if cx.unsafe_used {
                self.safety_comment()
            } else {
                String::new()
            };
            let _ = write!(tail, "{comment}if !({test}) {{\nbreak;\n}}\n");
        }
        cx.loops.pop();
        cx.unsafe_used = unsafe_used;
        match test_text {
            Some(test) => format!("{label}while {test} {{\n{body_text}{tail}}}\n"),
            None => format!("{label}loop {{\n{body_text}{tail}}}\n"),
        }
    }

    /// An index for an unchecked access: a `usize`.
    fn usize_index(&self, index: &Index, cx: &mut Fn<'_>) -> String {
        match index.ty {
            Ty::Index => self.expr(&index.value, cx).0,
            _ => format!("{} as usize", self.operand(&index.value, prec::CAST, cx)),
        }
    }

    /// The helper suffix and index for a checked access.
    fn checked_index(&self, index: &Index, cx: &mut Fn<'_>) -> (&'static str, String) {
        match index.ty {
            Ty::I32 => ("_i32", self.expr(&index.value, cx).0),
            Ty::Index => ("_usize", self.expr(&index.value, cx).0),
            Ty::U32 => ("", format!("f64::from({})", self.expr(&index.value, cx).0)),
            _ => ("", self.expr(&index.value, cx).0),
        }
    }

    /// `expr`, parenthesised if it binds looser than `min`.
    fn operand(&self, expr: &Expr, min: u8, cx: &mut Fn<'_>) -> String {
        let (text, p) = self.expr(expr, cx);
        if p < min { format!("({text})") } else { text }
    }

    #[expect(clippy::too_many_lines, reason = "one arm per expression kind")]
    fn expr(&self, expr: &Expr, cx: &mut Fn<'_>) -> (String, u8) {
        use prec::{
            ADD, AND, ATOM, BIT_AND, BIT_OR, BIT_XOR, BLOCK, CAST, CMP, MUL, OR, POSTFIX, UNARY,
        };
        match expr {
            Expr::Lit(lit) => {
                let (_, text) = lit_parts(*lit);
                let p = if text.starts_with('-') { UNARY } else { ATOM };
                (text, p)
            }
            Expr::Local(LocalId(local)) => (cx.function.locals[*local].name.clone(), ATOM),
            Expr::Const(c) => (self.island.consts[c.0].name.clone(), ATOM),
            Expr::SelfValue => ("self".into(), ATOM),
            Expr::Unary(op, arg) => {
                let symbol = match op {
                    UnOp::NegF64 | UnOp::NegI32 => "-",
                    UnOp::Not | UnOp::BitNot => "!",
                };
                let operand = self.operand(arg, UNARY, cx);
                // `--x` reads as a decrement.
                let operand = if symbol == "-" && operand.starts_with('-') {
                    format!("({operand})")
                } else {
                    operand
                };
                (format!("{symbol}{operand}"), UNARY)
            }
            Expr::Binary(op, a, b) => {
                let infix = |sym: &str, p: u8, cx: &mut Fn<'_>| {
                    // Comparisons don't chain; a cast before `<` would read as generics.
                    let left_min = if p == CMP { CMP + 1 } else { p };
                    let mut left = self.operand(a, left_min, cx);
                    // `x as i32 < y` reads `<` as the start of generic arguments.
                    if sym.starts_with('<')
                        && [" as i32", " as u32", " as f64", " as usize"]
                            .iter()
                            .any(|t| left.ends_with(t))
                    {
                        left = format!("({left})");
                    }
                    let right = self.operand(b, p + 1, cx);
                    (format!("{left} {sym} {right}"), p)
                };
                let call = |name: &str, cx: &mut Fn<'_>| {
                    let a = self.expr(a, cx).0;
                    let b = self.expr(b, cx).0;
                    (format!("{name}({a}, {b})"), POSTFIX)
                };
                match op {
                    BinOp::Add | BinOp::IntAdd => infix("+", ADD, cx),
                    BinOp::Sub | BinOp::IntSub => infix("-", ADD, cx),
                    BinOp::Mul | BinOp::IntMul => infix("*", MUL, cx),
                    BinOp::WrapAdd | BinOp::WrapSub | BinOp::WrapMul => {
                        let name = match op {
                            BinOp::WrapAdd => "wrapping_add",
                            BinOp::WrapSub => "wrapping_sub",
                            _ => "wrapping_mul",
                        };
                        let argument = self.expr(b, cx).0;
                        // Literals and negations have no type of their own to call a method on.
                        if matches!(
                            **a,
                            Expr::Local(_)
                                | Expr::Field(..)
                                | Expr::Call(..)
                                | Expr::MethodCall(..)
                                | Expr::Const(_)
                        ) {
                            let receiver = self.operand(a, POSTFIX, cx);
                            return (format!("{receiver}.{name}({argument})"), POSTFIX);
                        }
                        let receiver = self.expr(a, cx).0;
                        let ty = if int_ty(a, cx) == Ty::U32 {
                            "u32"
                        } else {
                            "i32"
                        };
                        (format!("{ty}::{name}({receiver}, {argument})"), POSTFIX)
                    }
                    BinOp::Div => {
                        // rustc 1.99.1's LLVM rewrites `0.0 / sitofp(x)` to `copysign(0.0, x)`
                        // when the quotient feeds a multiplication, losing the NaN of 0 / 0;
                        // inlining can expose any division to it, so divisors are opaque.
                        if matches!(**b, Expr::Lit(Lit::F64(x)) if x != 0.0 && !x.is_nan())
                            || matches!(**b, Expr::Lit(Lit::I32(x)) if x != 0)
                        {
                            return infix("/", MUL, cx);
                        }
                        let left = self.operand(a, MUL, cx);
                        let right = self.expr(b, cx).0;
                        (format!("{left} / core::hint::black_box({right})"), MUL)
                    }
                    BinOp::Rem => call("ops::rem", cx),
                    BinOp::RemI32 => call("ops::rem_i32", cx),
                    BinOp::DivTruncI32 => call("ops::div_trunc_i32", cx),
                    BinOp::BitAnd => infix("&", BIT_AND, cx),
                    BinOp::BitOr => infix("|", BIT_OR, cx),
                    BinOp::BitXor => infix("^", BIT_XOR, cx),
                    BinOp::Shl => call("ops::shl_i32", cx),
                    BinOp::Shr => call("ops::shr_i32", cx),
                    BinOp::Ushr => call("ops::ushr_i32", cx),
                    BinOp::Lt => infix("<", CMP, cx),
                    BinOp::Le => infix("<=", CMP, cx),
                    BinOp::Gt => infix(">", CMP, cx),
                    BinOp::Ge => infix(">=", CMP, cx),
                    BinOp::Eq => infix("==", CMP, cx),
                    BinOp::Ne => infix("!=", CMP, cx),
                    BinOp::And => infix("&&", AND, cx),
                    BinOp::Or => infix("||", OR, cx),
                }
            }
            Expr::Convert {
                conv,
                from,
                to,
                arg,
            } => match (conv, from, to) {
                (Conv::ToInt32, _, _) => {
                    (format!("conv::to_int32({})", self.expr(arg, cx).0), POSTFIX)
                }
                (Conv::ToUint32, _, _) => (
                    format!("conv::to_uint32({})", self.expr(arg, cx).0),
                    POSTFIX,
                ),
                (Conv::Exact, Ty::I32 | Ty::U32, Ty::F64) => {
                    (format!("f64::from({})", self.expr(arg, cx).0), POSTFIX)
                }
                (Conv::Exact, Ty::I32 | Ty::U32, Ty::Index) if matches!(**arg, Expr::Lit(_)) => {
                    let (_, text) = lit_parts(match **arg {
                        Expr::Lit(Lit::U32(v)) => Lit::I32(i32::try_from(v).unwrap_or(i32::MAX)),
                        Expr::Lit(lit) => lit,
                        _ => unreachable!("matched"),
                    });
                    (text, ATOM)
                }
                (Conv::Exact, _, to) => {
                    let to = match to {
                        Ty::F64 => "f64",
                        Ty::I32 => "i32",
                        Ty::U32 => "u32",
                        _ => "usize",
                    };
                    // An unsuffixed literal would take the cast's type.
                    let mut operand = self.operand(arg, CAST, cx);
                    let digits = operand.strip_prefix('-').unwrap_or(&operand);
                    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                        operand.push_str(match from {
                            Ty::Index => "_usize",
                            Ty::U32 => "_u32",
                            _ => "_i32",
                        });
                    }
                    (format!("{operand} as {to}"), CAST)
                }
            },
            Expr::Truthy(_, arg) => (
                format!("conv::to_boolean({})", self.expr(arg, cx).0),
                POSTFIX,
            ),
            Expr::NumberLogic { or, ty, lhs, rhs } => {
                let name = Self::fresh(cx, "lhs");
                let lhs = self.expr(lhs, cx).0;
                let rhs = self.expr(rhs, cx).0;
                let test = match ty {
                    Ty::F64 => format!("conv::to_boolean({name})"),
                    _ => format!("{name} != 0"),
                };
                let (then, otherwise) = if *or {
                    (name.clone(), rhs)
                } else {
                    (rhs, name.clone())
                };
                (
                    format!(
                        "{{ let {name} = {lhs}; if {test} {{ {then} }} else {{ {otherwise} }} }}"
                    ),
                    ATOM,
                )
            }
            Expr::Math(function, args) => {
                let args: Vec<String> = args.iter().map(|a| self.expr(a, cx).0).collect();
                (
                    format!("math::{}({})", math_name(*function), args.join(", ")),
                    POSTFIX,
                )
            }
            Expr::Call(FuncRef(f), args) => {
                let function = &self.island.functions[*f];
                let args: Vec<String> = args.iter().map(|a| self.expr(a, cx).0).collect();
                let path = match function.kind {
                    FunctionKind::Constructor(s) => {
                        format!("{}::{}", self.island.structs[s.0].name, function.name)
                    }
                    _ => function.name.clone(),
                };
                (format!("{path}({})", args.join(", ")), POSTFIX)
            }
            Expr::MethodCall(FuncRef(f), receiver, args) => {
                let receiver = self.operand(receiver, POSTFIX, cx);
                let args: Vec<String> = args.iter().map(|a| self.expr(a, cx).0).collect();
                (
                    format!(
                        "{receiver}.{}({})",
                        self.island.functions[*f].name,
                        args.join(", ")
                    ),
                    POSTFIX,
                )
            }
            Expr::Field(base, field) => (
                format!(
                    "{}.{}",
                    self.operand(base, POSTFIX, cx),
                    crate::names::rust_ident(field)
                ),
                POSTFIX,
            ),
            Expr::StructLit(s, fields) => {
                let name = &self.island.structs[s.0].name;
                let fields: Vec<String> = fields
                    .iter()
                    .map(|(field, value)| {
                        let field = crate::names::rust_ident(field);
                        let value = self.expr(value, cx).0;
                        if value == field {
                            field
                        } else {
                            format!("{field}: {value}")
                        }
                    })
                    .collect();
                (format!("{name} {{ {} }}", fields.join(", ")), ATOM)
            }
            Expr::Len(array, ty) => {
                let array_text = self.operand(array, POSTFIX, cx);
                match ty {
                    Ty::Index => (format!("{array_text}.len()"), POSTFIX),
                    Ty::I32 => (format!("{array_text}.len() as i32"), CAST),
                    _ => (format!("array::len({})", self.expr(array, cx).0), POSTFIX),
                }
            }
            Expr::Get { array, index } => {
                let array = self.expr(array, cx).0;
                if index.in_bounds {
                    let index = self.usize_index(index, cx);
                    cx.unsafe_used = true;
                    (
                        format!("unsafe {{ array::get_unchecked({array}, {index}) }}"),
                        ATOM,
                    )
                } else {
                    let (helper, index) = self.checked_index(index, cx);
                    (format!("array::get{helper}({array}, {index})"), POSTFIX)
                }
            }
            Expr::NewArray { elem, len, exact } => {
                let elem = elem_type(*elem);
                let len = self.expr(len, cx).0;
                let helper = if *exact { "alloc_array" } else { "new_array" };
                (format!("abi::{helper}::<{elem}>({len})"), POSTFIX)
            }
            Expr::Cond(test, a, b) => {
                let test = self.expr(test, cx).0;
                let a = self.expr(a, cx).0;
                let b = self.expr(b, cx).0;
                (format!("if {test} {{ {a} }} else {{ {b} }}"), BLOCK)
            }
            Expr::Block(stmts, value) => {
                let outer = cx.unsafe_used;
                let stmts = self.stmts(stmts, cx);
                cx.unsafe_used = false;
                let value = self.expr(value, cx).0;
                cx.unsafe_used |= outer;
                (format!("{{\n{stmts}{value}\n}}"), ATOM)
            }
        }
    }

    fn fresh(cx: &mut Fn<'_>, base: &str) -> String {
        loop {
            cx.temp += 1;
            let name = if cx.temp == 1 {
                base.to_string()
            } else {
                format!("{base}_{}", cx.temp)
            };
            if !cx.function.locals.iter().any(|l| l.name == name) {
                return name;
            }
        }
    }

    fn shim(&mut self, export: &Export) {
        let function = &self.island.functions[export.func.0];
        let mut params = Vec::new();
        let mut decode = Vec::new();
        let mut args = Vec::new();
        let mut pointers = false;
        let mut mutable_aggregates = 0;
        let mut used: Vec<String> = Vec::new();
        let mut unique = |name: String| {
            let mut candidate = name.clone();
            let mut n = 2;
            while used.contains(&candidate) {
                candidate = format!("{}_{n}", crate::names::bare(&name));
                n += 1;
            }
            used.push(candidate.clone());
            candidate
        };
        for (index, (name, ty, mutated)) in export.params.iter().enumerate() {
            let local_ty = function.locals[function.params[index].0].ty;
            let base = crate::names::bare(&crate::names::rust_ident(name)).to_string();
            match ty {
                AbiType::F64 | AbiType::I32 | AbiType::U32 => {
                    let rust = unique(crate::names::rust_ident(name));
                    params.push(format!("{rust}: {}", self.ty(local_ty)));
                    args.push(rust);
                }
                AbiType::Bool => {
                    let rust = unique(crate::names::rust_ident(name));
                    params.push(format!("{rust}: i32"));
                    args.push(format!("{rust} != 0"));
                }
                AbiType::TypedArray { .. } | AbiType::NumberArray => {
                    pointers = true;
                    let ptr = unique(format!("{base}_ptr"));
                    let len = unique(format!("{base}_len"));
                    params.push(format!("{ptr}: u32, {len}: u32"));
                    let element = ty.array_element().expect("an array").rust_type();
                    let helper = if *mutated {
                        mutable_aggregates += 1;
                        "slice_mut"
                    } else {
                        "slice"
                    };
                    let rust = unique(crate::names::rust_ident(name));
                    decode.push(format!(
                        "let {rust} = unsafe {{ abi::{helper}::<{element}>({ptr}, {len}) }};"
                    ));
                    args.push(rust);
                }
                AbiType::Struct { .. } => {
                    pointers = true;
                    let ptr = unique(format!("{base}_ptr"));
                    params.push(format!("{ptr}: u32"));
                    let Ty::StructRef(s, _) = local_ty else {
                        continue;
                    };
                    let struct_name = &self.island.structs[s.0].name;
                    let helper = if *mutated {
                        mutable_aggregates += 1;
                        "struct_mut"
                    } else {
                        "struct_ref"
                    };
                    let rust = unique(crate::names::rust_ident(name));
                    decode.push(format!(
                        "let {rust} = unsafe {{ abi::{helper}::<{struct_name}>({ptr}) }};"
                    ));
                    args.push(rust);
                }
            }
        }
        let call = format!("{}({})", function.name, args.join(", "));
        let (ret, body) = match &export.ret {
            None => (String::new(), format!("{call};")),
            Some(AbiType::F64) => (" -> f64".into(), call),
            Some(AbiType::I32) => (" -> i32".into(), call),
            Some(AbiType::U32) => (" -> u32".into(), call),
            Some(AbiType::Bool) => (" -> i32".into(), format!("i32::from({call})")),
            Some(AbiType::TypedArray { .. } | AbiType::NumberArray) => {
                (" -> u32".into(), format!("abi::return_array({call})"))
            }
            Some(AbiType::Struct { .. }) => {
                (" -> u32".into(), format!("abi::return_struct({call})"))
            }
        };
        let safety = if decode.is_empty() {
            String::new()
        } else if mutable_aggregates > 0
            && export
                .params
                .iter()
                .filter(|(_, ty, _)| !ty.is_scalar())
                .count()
                > 1
        {
            self.safety_text("the glue passes fresh copies and checks they don't overlap")
        } else {
            self.safety_text("the glue passes fresh copies")
        };
        let qualifiers = if pointers {
            "pub unsafe extern \"C\""
        } else {
            "pub extern \"C\""
        };
        let _ = writeln!(
            self.out,
            "#[unsafe(export_name = {})]\n{qualifiers} fn __abi_{}({}){ret} {{\n{safety}{}\n{body}\n}}\n",
            rust_string(&export.name),
            export.name,
            params.join(", "),
            decode.join("\n"),
        );
    }
}

/// The integer type of an operand of an integer operator (`i32` unless it is clearly `u32`).
fn int_ty(expr: &Expr, cx: &Fn<'_>) -> Ty {
    match expr {
        Expr::Lit(Lit::U32(_)) | Expr::Binary(BinOp::Ushr, ..) => Ty::U32,
        Expr::Convert { to, .. } => *to,
        Expr::Local(LocalId(local)) => cx.function.locals[*local].ty,
        Expr::Binary(
            BinOp::IntAdd
            | BinOp::IntSub
            | BinOp::IntMul
            | BinOp::WrapAdd
            | BinOp::WrapSub
            | BinOp::WrapMul,
            a,
            _,
        ) => int_ty(a, cx),
        _ => Ty::I32,
    }
}

/// `x = x op y` as `(op, y)`, for compound assignment.
fn compound<'e>(value: &'e Expr, target: &Expr) -> Option<(&'static str, &'e Expr)> {
    let Expr::Binary(op, a, b) = value else {
        return None;
    };
    if **a != *target {
        return None;
    }
    let symbol = match op {
        BinOp::Add | BinOp::IntAdd => "+",
        BinOp::Sub | BinOp::IntSub => "-",
        BinOp::Mul | BinOp::IntMul => "*",
        BinOp::Div => "/",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        _ => return None,
    };
    Some((symbol, b))
}

fn collect_labels(stmts: &[Stmt], cx: &mut Fn<'_>) {
    for stmt in stmts {
        match stmt {
            Stmt::Loop(lp) => {
                if lp.labelled {
                    cx.labelled.push(lp.id);
                }
                if lp.has_continue && (!lp.update.is_empty() || lp.test_after) {
                    cx.continue_blocks.push(lp.id);
                }
                collect_labels(&lp.body, cx);
                collect_labels(&lp.update, cx);
            }
            Stmt::ForOf { body, .. } | Stmt::Block(_, body) => {
                for inner in body {
                    if let Stmt::Loop(_) | Stmt::ForOf { .. } = inner {}
                }
                collect_labels(body, cx);
            }
            Stmt::If(_, a, b) => {
                collect_labels(a, cx);
                collect_labels(b, cx);
            }
            _ => {}
        }
    }
    // `for…of` loops targeted from nested loops are labelled too.
    for stmt in stmts {
        if let Stmt::ForOf { id, body, .. } = stmt
            && targets_from_nested(*id, body, false)
        {
            cx.labelled.push(*id);
        }
    }
}

/// Whether a `break`/`continue` of `id` occurs inside a loop nested in `stmts`.
fn targets_from_nested(id: LoopId, stmts: &[Stmt], nested: bool) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Break(target) | Stmt::Continue(target) => nested && *target == id,
        Stmt::Loop(lp) => {
            targets_from_nested(id, &lp.body, true) || targets_from_nested(id, &lp.update, true)
        }
        Stmt::ForOf { body, .. } => targets_from_nested(id, body, true),
        Stmt::If(_, a, b) => {
            targets_from_nested(id, a, nested) || targets_from_nested(id, b, nested)
        }
        Stmt::Block(_, body) => targets_from_nested(id, body, nested),
        _ => false,
    })
}

fn rust_string(text: &str) -> String {
    format!("{text:?}")
}

fn elem_type(elem: Elem) -> &'static str {
    elem.storage().rust_type()
}

/// A literal's Rust type and spelling.
fn lit_parts(lit: Lit) -> (&'static str, String) {
    match lit {
        Lit::F64(value) => ("f64", f64_literal(value)),
        Lit::I32(value) => ("i32", value.to_string()),
        Lit::U32(value) => ("u32", format!("{value}_u32")),
        Lit::Bool(value) => ("bool", value.to_string()),
    }
}

fn f64_literal(value: f64) -> String {
    if value.is_nan() {
        "f64::NAN".into()
    } else if value == f64::INFINITY {
        "f64::INFINITY".into()
    } else if value == f64::NEG_INFINITY {
        "f64::NEG_INFINITY".into()
    } else {
        // `{:?}` round-trips and always has a `.` or exponent.
        let text = format!("{value:?}");
        if text.contains('.') || text.contains('e') || text.contains("inf") {
            text
        } else {
            format!("{text}.0")
        }
    }
}

fn math_name(function: MathFn) -> &'static str {
    match function {
        MathFn::Abs => "abs",
        MathFn::Floor => "floor",
        MathFn::Ceil => "ceil",
        MathFn::Trunc => "trunc",
        MathFn::Sqrt => "sqrt",
        MathFn::Round => "round",
        MathFn::Sign => "sign",
        MathFn::Fround => "fround",
        MathFn::Clz32 => "clz32",
        MathFn::Imul => "imul",
        MathFn::ImulI32 => "imul_i32",
        MathFn::Max => "max",
        MathFn::Min => "min",
        MathFn::Hypot2 => "hypot2",
        MathFn::Hypot3 => "hypot3",
        MathFn::Tan => "tan",
        MathFn::Asin => "asin",
        MathFn::Acos => "acos",
        MathFn::Atan => "atan",
        MathFn::Atan2 => "atan2",
        MathFn::Sinh => "sinh",
        MathFn::Atanh => "atanh",
        MathFn::Expm1 => "expm1",
        MathFn::Log1p => "log1p",
        MathFn::Log2 => "log2",
    }
}
