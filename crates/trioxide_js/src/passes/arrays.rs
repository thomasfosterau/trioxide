//! What the speed passes (PLAN §4 passes 6, 7, 9 and 10) share: fresh readable names, the
//! functions they leave alone, and the proof that a value is a plain array.

use oxc_ast::ast::{Expression, Program, Statement};
use oxc_ast_visit::{Visit, walk};
use oxc_span::Span;
use oxc_syntax::scope::ScopeFlags;
use rustc_hash::FxHashSet;
use trioxide_analysis::{AllocKind, Escape, Trust};
use trioxide_common::NodeRef;
use trioxide_types::Type;

use super::util::{Query, weak};
use crate::Keep;

/// Words a binding can't be named.
fn is_reserved(name: &str) -> bool {
    matches!(
        name,
        "arguments"
            | "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "eval"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "implements"
            | "import"
            | "in"
            | "instanceof"
            | "interface"
            | "let"
            | "new"
            | "null"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "return"
            | "static"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
            | "undefined"
            | "NaN"
            | "Infinity"
    )
}

/// `name` with its first letter upper-cased, for camel-case compounds (`items` → `Items`).
pub fn capitalized(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The spans of the functions the speed passes leave alone: WASM island functions (whose JS
/// versions are the islands' fallbacks) and functions forced into an island.
pub struct Kept(Vec<Span>);

impl Kept {
    /// The kept functions of `program`.
    pub fn new(program: &Program<'_>, keep: &Keep, query: Query<'_>) -> Self {
        struct Find<'k, 'f> {
            keep: &'k Keep,
            query: Query<'f>,
            tagged: FxHashSet<u32>,
            spans: Vec<Span>,
            export_start: Option<u32>,
        }
        impl<'a> Visit<'a> for Find<'_, '_> {
            fn visit_statement(&mut self, statement: &Statement<'a>) {
                self.export_start = match statement {
                    Statement::ExportNamedDeclaration(export) => Some(export.span.start),
                    Statement::ExportDefaultDeclaration(export) => Some(export.span.start),
                    _ => None,
                };
                walk::walk_statement(self, statement);
            }

            fn visit_function(&mut self, function: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
                let starts: Vec<u32> = std::iter::once(function.span.start)
                    .chain(self.export_start.take())
                    .collect();
                let island = self
                    .query
                    .node(function.node_id.get())
                    .is_some_and(|node| self.keep.functions.contains(&node));
                if island || super::forced_island(function, &self.tagged, &starts) {
                    self.spans.push(function.span);
                    return;
                }
                walk::walk_function(self, function, flags);
            }
        }
        let mut find = Find {
            keep,
            query,
            tagged: super::wasm_tags(program),
            spans: Vec::new(),
            export_start: None,
        };
        find.visit_program(program);
        Self(find.spans)
    }

    /// Whether `span` lies in a kept function.
    pub fn contains(&self, span: Span) -> bool {
        self.0
            .iter()
            .any(|kept| kept.start <= span.start && span.end <= kept.end)
    }
}

/// What is proven about a value the passes treat as a plain array.
pub struct ArrayProof {
    /// No array it may be ever has a hole.
    pub packed: bool,
    /// The allocation sites it may be, when they are known.
    pub sites: Option<Vec<NodeRef>>,
}

/// Whether `expression` evaluates to a plain `Array` whose builtins are intact: a usable
/// array (or tuple) type that is never `null` or `undefined`, `Array` and `Object` unpatched,
/// and, when the type is `Proven`, every allocation it may be an array the analysis follows
/// (not escaping to unknown code, so nothing it can't see gives it own methods or an own
/// iterator) with a stable shape. A `Declared` type (usable with `trustTypes`) is taken at its
/// word: TypeScript's `T[]` then also rules out subclasses and proxies. The error is the reason
/// for the decision log.
pub fn prove_array(query: Query<'_>, expression: &Expression<'_>) -> Result<ArrayProof, String> {
    let inner = expression.get_inner_expression();
    let fact = query.ty(inner.node_id());
    if !query.usable(fact.trust) {
        return Err(weak(fact.trust, "the array's type"));
    }
    let arrays = !matches!(fact.value, Type::Unknown | Type::Never)
        && fact
            .value
            .members()
            .iter()
            .all(|member| matches!(member, Type::Array(_) | Type::Tuple(_)));
    if !arrays {
        return Err("the value may not be an array".to_string());
    }
    for name in ["Array", "Object"] {
        let intact = query.facts.builtin_intact(name);
        if !(intact.value && intact.trust == Trust::Proven) {
            return Err(format!(
                "the program may patch `{name}` or its prototype, so the builtin models are off"
            ));
        }
    }
    let points_to = query
        .node(inner.node_id())
        .map(|node| query.facts.points_to(node));
    let sites = points_to.and_then(|fact| fact.value.map(<[NodeRef]>::to_vec));
    if fact.trust == Trust::Proven {
        let followed = sites.as_ref().is_some_and(|sites| {
            !sites.is_empty()
                && sites.iter().all(|site| {
                    query.facts.allocation(*site).is_some_and(|allocation| {
                        allocation.kind == AllocKind::Array
                            && allocation.escape.value != Escape::Unknown
                            && allocation.stable_shape.value
                    })
                })
        });
        if !followed {
            return Err(
                "an array it may be is not followed by the analysis (it reaches unknown code, \
                 or gets properties added)"
                    .to_string(),
            );
        }
    }
    let packed = sites.as_ref().is_some_and(|sites| {
        !sites.is_empty()
            && sites.iter().all(|site| {
                let holes = query.facts.may_have_holes(*site);
                !holes.value && holes.trust == Trust::Proven
            })
    });
    Ok(ArrayProof { packed, sites })
}

/// Every binding and reference name in some subtree, for names that only need to be fresh
/// there (a loop's own index, which nothing outside the loop sees).
#[derive(Default)]
pub struct NamesIn(pub FxHashSet<String>);

impl<'a> Visit<'a> for NamesIn {
    fn visit_identifier_reference(&mut self, ident: &oxc_ast::ast::IdentifierReference<'a>) {
        self.0.insert(ident.name.to_string());
    }

    fn visit_binding_identifier(&mut self, ident: &oxc_ast::ast::BindingIdentifier<'a>) {
        self.0.insert(ident.name.to_string());
    }
}

impl NamesIn {
    /// A loop index name: `i`, `j`, `k`, else `i2`, `i3`, …, one that is free here and that
    /// no enclosing binding has (`visible`), so nested loops read conventionally.
    pub fn fresh_index(&mut self, visible: impl Fn(&str) -> bool) -> String {
        for name in ["i", "j", "k"] {
            if !self.0.contains(name) && !visible(name) {
                self.0.insert(name.to_string());
                return name.to_string();
            }
        }
        self.fresh("i")
    }

    /// `base` if no name here is `base`, else `base2`, `base3`, …; taken from then on.
    pub fn fresh(&mut self, base: &str) -> String {
        let mut name = base.to_string();
        let mut counter = 2;
        while self.0.contains(&name) || is_reserved(&name) {
            name = format!("{base}{counter}");
            counter += 1;
        }
        self.0.insert(name.clone());
        name
    }
}

/// Names that only need to be fresh within the function (or module top level) where a binding
/// goes: no binding or reference anywhere in that function, nested functions included, uses
/// them, so a new binding in any of its blocks shadows nothing and collides with nothing.
pub struct FunctionNames {
    /// Each function's (and the program's) span with the names in it, innermost last among
    /// those that contain one another.
    functions: Vec<(Span, FxHashSet<String>)>,
}

impl FunctionNames {
    /// The names of every function of `program`.
    pub fn new(program: &Program<'_>) -> Self {
        #[derive(Default)]
        struct Collect {
            functions: Vec<(Span, FxHashSet<String>)>,
            open: Vec<usize>,
        }
        impl Collect {
            fn name(&mut self, name: &str) {
                for &index in &self.open {
                    self.functions[index].1.insert(name.to_string());
                }
            }

            fn enter(&mut self, span: Span) {
                self.open.push(self.functions.len());
                self.functions.push((span, FxHashSet::default()));
            }
        }
        impl<'a> Visit<'a> for Collect {
            fn visit_identifier_reference(
                &mut self,
                ident: &oxc_ast::ast::IdentifierReference<'a>,
            ) {
                self.name(&ident.name);
            }

            fn visit_binding_identifier(&mut self, ident: &oxc_ast::ast::BindingIdentifier<'a>) {
                self.name(&ident.name);
            }

            fn visit_function(&mut self, function: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
                self.enter(function.span);
                walk::walk_function(self, function, flags);
                self.open.pop();
            }

            fn visit_arrow_function_expression(
                &mut self,
                arrow: &oxc_ast::ast::ArrowFunctionExpression<'a>,
            ) {
                self.enter(arrow.span);
                walk::walk_arrow_function_expression(self, arrow);
                self.open.pop();
            }

            fn visit_static_block(&mut self, block: &oxc_ast::ast::StaticBlock<'a>) {
                self.enter(block.span);
                walk::walk_static_block(self, block);
                self.open.pop();
            }
        }
        let mut collect = Collect::default();
        collect.enter(program.span);
        walk::walk_program(&mut collect, program);
        Self {
            functions: collect.functions,
        }
    }

    /// A name based on `base`, fresh in the innermost function containing `at`; taken there
    /// (and in the functions around it) from then on.
    pub fn fresh(&mut self, base: &str, at: Span) -> String {
        let containing: Vec<usize> = (0..self.functions.len())
            .filter(|&index| {
                let span = self.functions[index].0;
                span.start <= at.start && at.end <= span.end
            })
            .collect();
        let Some(&innermost) = containing
            .iter()
            .max_by_key(|&&index| self.functions[index].0.start)
        else {
            return base.to_string();
        };
        let mut name = base.to_string();
        let mut counter = 2;
        while self.functions[innermost].1.contains(&name) || is_reserved(&name) {
            name = format!("{base}{counter}");
            counter += 1;
        }
        for index in containing {
            self.functions[index].1.insert(name.clone());
        }
        name
    }
}
