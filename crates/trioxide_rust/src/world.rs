//! The program as the backend sees it: every module parsed with oxc, with its functions,
//! classes and `"use wasm"` / `"use js"` directives indexed.

use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::{
    AstKind,
    ast::{
        Class, ClassElement, Declaration, ExportDefaultDeclarationKind, Expression, Function,
        FunctionType, Program, Statement,
    },
};
use oxc_parser::Parser;
use oxc_semantic::{Semantic, SemanticBuilder};
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::{node::NodeId, operator::UnaryOperator, reference::ReferenceId, symbol::SymbolId};
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::{ModuleId, NodeRef, SymbolRef};
use trioxide_types::{ClassId, FunctionId};

/// One module of the program.
#[derive(Debug, Clone, Copy)]
pub struct SourceModule<'s> {
    /// Its id in the module graph (the one [`Facts`](trioxide_analysis::Facts) uses).
    pub id: ModuleId,
    /// Its path.
    pub path: &'s Path,
    /// Its source.
    pub source: &'s str,
    /// How to parse it.
    pub source_type: SourceType,
    /// The graph modules it imports (statically, re-exports included).
    pub imports: &'s [ModuleId],
}

/// A source directive about where a function goes (PLAN D11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// `"use wasm"` or `@trioxide wasm`: must become WASM.
    Wasm,
    /// `"use js"` or `@trioxide js`: must stay JS.
    Js,
}

/// Where a directive was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Directive {
    /// What it asks for.
    pub placement: Placement,
    /// The directive or tag.
    pub span: Span,
    /// Whether it is a whole-file directive.
    pub file: bool,
}

/// A parsed module.
pub(crate) struct Module<'a> {
    pub id: ModuleId,
    pub path: &'a Path,
    pub source: &'a str,
    pub program: &'a Program<'a>,
    pub semantic: Semantic<'a>,
    /// The file-level directive, if any.
    pub directive: Option<Directive>,
}

impl<'a> Module<'a> {
    /// The names the module exports the binding `symbol` under (`export function f`,
    /// `export { f as g }`, `export default f`), and whether anything assigns it.
    pub fn exported_names(&self, symbol: SymbolId) -> (Vec<String>, bool) {
        let scoping = self.semantic.scoping();
        let reassigned = scoping
            .get_resolved_references(symbol)
            .any(oxc_semantic::Reference::is_write);
        let mut names = Vec::new();
        for statement in &self.program.body {
            match statement {
                Statement::ExportDeclaration(export) => {
                    if let Declaration::FunctionDeclaration(f) = &export.declaration
                        && f.id.as_ref().and_then(|id| id.symbol_id.get()) == Some(symbol)
                        && let Some(id) = &f.id
                    {
                        names.push(id.name.to_string());
                    }
                }
                Statement::ExportNamedDeclaration(export) => {
                    for specifier in &export.specifiers {
                        if let oxc_ast::ast::ModuleExportName::IdentifierReference(local) =
                            &specifier.local
                            && self.reference_symbol(local.reference_id.get()) == Some(symbol)
                        {
                            names.push(specifier.exported.name().to_string());
                        }
                    }
                }
                Statement::ExportDefaultDeclaration(export) => {
                    let local = match &export.declaration {
                        ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                            f.id.as_ref().and_then(|id| id.symbol_id.get())
                        }
                        ExportDefaultDeclarationKind::Identifier(id) => {
                            self.reference_symbol(id.reference_id.get())
                        }
                        _ => None,
                    };
                    if local == Some(symbol) {
                        names.push("default".into());
                    }
                }
                _ => {}
            }
        }
        (names, reassigned)
    }

    /// The symbol an identifier reference resolves to in this module.
    pub fn reference_symbol(&self, reference: Option<ReferenceId>) -> Option<SymbolId> {
        self.semantic
            .scoping()
            .get_reference(reference?)
            .symbol_id()
    }

    /// The node declaring `symbol`.
    pub fn declaration(&self, symbol: SymbolId) -> (NodeId, AstKind<'a>) {
        let node = self.semantic.symbol_declaration(symbol);
        (node.id(), node.kind())
    }

    pub fn node(&self, node: NodeId) -> NodeRef {
        NodeRef {
            module: self.id,
            node,
        }
    }

    pub fn symbol(&self, symbol: SymbolId) -> SymbolRef {
        SymbolRef {
            module: self.id,
            symbol,
        }
    }
}

/// How a function sits in a class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Free,
    Constructor(ClassId),
    Method(ClassId),
    /// Static members, getters, setters and other class code.
    OtherMember,
}

/// A function declaration, expression or method.
pub(crate) struct FnInfo<'a> {
    pub id: FunctionId,
    pub func: &'a Function<'a>,
    pub name: Option<String>,
    /// A `function` declaration at the top level of its module (exported or not).
    pub top_level: bool,
    pub role: Role,
    /// Its own directive (function directive or `JSDoc` tag).
    pub directive: Option<Directive>,
}

/// A class.
pub(crate) struct ClassInfo<'a> {
    pub class: &'a Class<'a>,
    pub name: Option<String>,
}

/// Whether evaluating a top-level statement runs no code: it only declares.
fn declares_only(statement: &Statement<'_>) -> bool {
    fn literal(expression: &Expression<'_>) -> bool {
        match expression.get_inner_expression() {
            Expression::NumericLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::NullLiteral(_) => true,
            Expression::UnaryExpression(u) => {
                matches!(
                    u.operator,
                    UnaryOperator::UnaryNegation | UnaryOperator::UnaryPlus
                ) && literal(&u.argument)
            }
            _ => false,
        }
    }
    fn plain_class(class: &Class<'_>) -> bool {
        class.heritage.is_none()
            && class.decorators.is_empty()
            && class.body.body.iter().all(|element| match element {
                ClassElement::MethodDefinition(m) => !m.r#static && !m.computed,
                ClassElement::PropertyDefinition(p) => !p.r#static && !p.computed,
                ClassElement::TSIndexSignature(_) => true,
                _ => false,
            })
    }
    fn declaration(declaration: &Declaration<'_>) -> bool {
        match declaration {
            Declaration::FunctionDeclaration(_)
            | Declaration::TSTypeAliasDeclaration(_)
            | Declaration::TSInterfaceDeclaration(_)
            | Declaration::TSExternalModuleDeclaration(_)
            | Declaration::TSGlobalDeclaration(_) => true,
            Declaration::ClassDeclaration(class) => plain_class(class),
            Declaration::VariableDeclaration(variables) => variables.declarations.iter().all(|d| {
                d.id.get_binding_identifier().is_some() && d.init.as_ref().is_none_or(literal)
            }),
            Declaration::TSEnumDeclaration(e) => e.declare,
            _ => false,
        }
    }
    match statement {
        Statement::EmptyStatement(_)
        | Statement::ImportDeclaration(_)
        | Statement::ExportNamedDeclaration(_)
        | Statement::ExportFromDeclaration(_) => true,
        Statement::ExportDeclaration(export) => declaration(&export.declaration),
        Statement::ExportDefaultDeclaration(export) => matches!(
            &export.declaration,
            ExportDefaultDeclarationKind::FunctionDeclaration(_)
                | ExportDefaultDeclarationKind::TSInterfaceDeclaration(_)
        ),
        statement => statement.as_declaration().is_some_and(declaration),
    }
}

/// The modules that can reach themselves through imports.
fn cyclic_modules(modules: &[SourceModule<'_>]) -> FxHashSet<ModuleId> {
    let imports: FxHashMap<ModuleId, &[ModuleId]> =
        modules.iter().map(|m| (m.id, m.imports)).collect();
    let mut cyclic = FxHashSet::default();
    for module in modules {
        let mut seen = FxHashSet::default();
        let mut stack: Vec<ModuleId> = module.imports.to_vec();
        while let Some(next) = stack.pop() {
            if next == module.id {
                cyclic.insert(module.id);
                break;
            }
            if seen.insert(next) {
                stack.extend(imports.get(&next).copied().unwrap_or_default());
            }
        }
    }
    cyclic
}

/// Every module, parsed and indexed.
pub(crate) struct World<'a> {
    pub modules: Vec<Module<'a>>,
    /// Directives in functions that can't be islands (arrow functions).
    pub misplaced: Vec<(ModuleId, Directive)>,
    pub functions: FxHashMap<FunctionId, FnInfo<'a>>,
    pub classes: FxHashMap<ClassId, ClassInfo<'a>>,
    /// Modules in an import cycle: one of their importers may run before their body does.
    pub cyclic: FxHashSet<ModuleId>,
}

impl<'a> World<'a> {
    /// Parses `modules` into `allocators` (one per module).
    pub fn build(allocators: &'a [Allocator], modules: &[SourceModule<'a>]) -> Self {
        let mut world = Self {
            modules: Vec::with_capacity(modules.len()),
            misplaced: Vec::new(),
            functions: FxHashMap::default(),
            classes: FxHashMap::default(),
            cyclic: cyclic_modules(modules),
        };
        for (allocator, input) in allocators.iter().zip(modules) {
            let parsed = Parser::new(allocator, input.source, input.source_type).parse();
            let program: &'a Program<'a> = allocator.alloc(parsed.program);
            let semantic = SemanticBuilder::new()
                .with_enum_eval(true)
                .with_build_nodes(true)
                .build(program)
                .semantic;
            let directive = program.directives.iter().find_map(|d| {
                placement(d.directive.as_str()).map(|placement| Directive {
                    placement,
                    span: d.span,
                    file: true,
                })
            });
            let module = Module {
                id: input.id,
                path: input.path,
                source: input.source,
                program,
                semantic,
                directive,
            };
            world.index(&module);
            world.modules.push(module);
        }
        world
    }

    /// Whether no code of its module can run before the class declaration `class` (a
    /// top-level statement): everything before it only declares (imports, functions, types,
    /// literal constants, plain classes).
    pub fn declared_first(&self, class: ClassId) -> bool {
        let Some(module) = self.module(class.module) else {
            return false;
        };
        for statement in &module.program.body {
            if statement.span().start >= module.semantic.nodes().get_node(class.node).span().start {
                return true;
            }
            if !declares_only(statement) {
                return false;
            }
        }
        false
    }

    pub fn module(&self, id: ModuleId) -> Option<&Module<'a>> {
        self.modules.iter().find(|module| module.id == id)
    }

    fn index(&mut self, module: &Module<'a>) {
        let program = module.program;
        let tags = jsdoc_tags(program, module.source);
        // Leading comments attach to `export` when there is one.
        let mut export_starts: FxHashMap<u32, u32> = FxHashMap::default();
        for statement in &program.body {
            match statement {
                Statement::ExportDeclaration(export) => {
                    export_starts.insert(export.declaration.span().start, export.span.start);
                }
                Statement::ExportDefaultDeclaration(export) => {
                    if let ExportDefaultDeclarationKind::FunctionDeclaration(function) =
                        &export.declaration
                    {
                        export_starts.insert(function.span.start, export.span.start);
                    }
                }
                _ => {}
            }
        }

        let nodes = module.semantic.nodes();
        for node in nodes.iter() {
            match node.kind() {
                AstKind::Function(func) => {
                    let parent = nodes.parent_kind(node.id());
                    let top_level = func.r#type == FunctionType::FunctionDeclaration
                        && matches!(parent, AstKind::Program(_) | AstKind::ExportDeclaration(_));
                    let name = func.id.as_ref().map(|id| id.name.to_string());
                    let role = match parent {
                        AstKind::MethodDefinition(method) => {
                            let class = nodes
                                .ancestors(node.id())
                                .find(|n| matches!(n.kind(), AstKind::Class(_)))
                                .map(|n| module.node(n.id()));
                            match (class, method.kind, method.r#static) {
                                (
                                    Some(class),
                                    oxc_ast::ast::MethodDefinitionKind::Constructor,
                                    _,
                                ) => Role::Constructor(class),
                                (
                                    Some(class),
                                    oxc_ast::ast::MethodDefinitionKind::Method,
                                    false,
                                ) => Role::Method(class),
                                _ => Role::OtherMember,
                            }
                        }
                        _ => Role::Free,
                    };
                    let name = match (&name, parent) {
                        (None, AstKind::MethodDefinition(method)) => {
                            method.key.static_name().map(|key| key.to_string())
                        }
                        _ => name,
                    };
                    let mut directive = func.body.as_ref().and_then(|body| {
                        body.directives.iter().find_map(|d| {
                            placement(d.directive.as_str()).map(|placement| Directive {
                                placement,
                                span: d.span,
                                file: false,
                            })
                        })
                    });
                    if directive.is_none() {
                        let start = export_starts
                            .get(&func.span.start)
                            .copied()
                            .unwrap_or(func.span.start);
                        let starts = [start, func.span.start, parent_start(parent)];
                        directive = tags
                            .iter()
                            .find(|(at, _)| starts.contains(at))
                            .map(|(_, directive)| *directive);
                    }
                    let id = module.node(node.id());
                    self.functions.insert(
                        id,
                        FnInfo {
                            id,
                            func,
                            name,
                            top_level,
                            role,
                            directive,
                        },
                    );
                }
                AstKind::ArrowFunctionExpression(arrow) => {
                    let oxc_ast::ast::ArrowFunctionBody::FunctionBody(body) = &arrow.body else {
                        continue;
                    };
                    if let Some(directive) = body.directives.iter().find_map(|d| {
                        placement(d.directive.as_str()).map(|placement| Directive {
                            placement,
                            span: d.span,
                            file: false,
                        })
                    }) {
                        self.misplaced.push((module.id, directive));
                    }
                }
                AstKind::Class(class) => {
                    self.classes.insert(
                        module.node(node.id()),
                        ClassInfo {
                            class,
                            name: class.id.as_ref().map(|id| id.name.to_string()),
                        },
                    );
                }
                _ => {}
            }
        }
    }

    /// The directive in force for `function`: its own, else its module's (for top-level
    /// function declarations).
    pub fn directive(&self, function: FunctionId) -> Option<Directive> {
        let info = self.functions.get(&function)?;
        info.directive.or_else(|| {
            let module = self.module(function.module)?;
            module.directive.filter(|_| info.top_level)
        })
    }
}

fn parent_start(parent: AstKind<'_>) -> u32 {
    match parent {
        AstKind::MethodDefinition(method) => method.span.start,
        _ => u32::MAX,
    }
}

fn placement(directive: &str) -> Option<Placement> {
    match directive {
        "use wasm" => Some(Placement::Wasm),
        "use js" => Some(Placement::Js),
        _ => None,
    }
}

/// `/** @trioxide wasm|js */` tags, by the offset of the token they precede.
fn jsdoc_tags(program: &Program<'_>, source: &str) -> Vec<(u32, Directive)> {
    let mut tags = Vec::new();
    for comment in &program.comments {
        let text = &source[comment.span.start as usize..comment.span.end as usize];
        if !text.starts_with("/**") {
            continue;
        }
        let mut words = text.split(|c: char| c.is_whitespace() || c == '*' || c == '/');
        while let Some(word) = words.next() {
            if word != "@trioxide" {
                continue;
            }
            let placement = match words.find(|w| !w.is_empty()) {
                Some("wasm") => Placement::Wasm,
                Some("js") => Placement::Js,
                _ => continue,
            };
            tags.push((
                comment.attached_to,
                Directive {
                    placement,
                    span: comment.span,
                    file: false,
                },
            ));
            break;
        }
    }
    tags
}
