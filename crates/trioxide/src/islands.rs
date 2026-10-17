//! Building WASM islands and falling back to JS when that fails (PLAN D5, §6 "Build driver").

use std::path::{Path, PathBuf};

use trioxide_common::Diagnostic;
use trioxide_wasm::{
    BuildError, BuilderOptions, CargoIslandBuilder, GlueOptions, IslandArtifact, IslandBuilder,
    IslandSource, WasmUrl,
};

use crate::{GlueMode, Options};

/// Where islands are cached: `Options::cache_dir` (relative to the root), else
/// `node_modules/.trioxide` under the root.
pub(crate) fn cache_dir(root: &Path, options: &Options) -> PathBuf {
    options.cache_dir.as_ref().map_or_else(
        || root.join("node_modules").join(".trioxide"),
        |dir| root.join(dir),
    )
}

/// The cargo builder configured from `options`.
pub(crate) fn cargo_builder(root: &Path, options: &Options) -> CargoIslandBuilder {
    CargoIslandBuilder::new(BuilderOptions {
        cache_dir: cache_dir(root, options),
        wasm_opt: options.wasm.wasm_opt,
        glue: GlueOptions {
            mode: match options.wasm.glue {
                GlueMode::Tla => trioxide_wasm::GlueMode::Tla,
                GlueMode::Ready => trioxide_wasm::GlueMode::Ready,
            },
            wasm_url: WasmUrl::Placeholder,
        },
        path: None,
    })
}

/// Builds `sources`, returning the artifacts that built and diagnostics for the rest.
///
/// A missing toolchain is reported once for all islands. Without `require` every failure is
/// a warning and the island's functions stay JS; with it they are errors.
pub(crate) fn build(
    builder: &dyn IslandBuilder,
    sources: &[IslandSource],
    require: bool,
) -> (Vec<IslandArtifact>, Vec<Diagnostic>) {
    let mut artifacts = Vec::new();
    let mut diagnostics = Vec::new();
    let mut toolchain: Option<(String, Vec<String>)> = None;
    let report = |message: String| {
        if require {
            Diagnostic::error(message)
        } else {
            Diagnostic::warning(message)
        }
    };
    for (source, result) in sources.iter().zip(builder.build_all(sources)) {
        match result {
            Ok(artifact) => {
                diagnostics.extend(artifact.warnings.iter().map(|warning| {
                    Diagnostic::warning(warning.clone()).with_code("trioxide(wasm-opt)")
                }));
                artifacts.push(artifact);
            }
            Err(BuildError::ToolchainUnavailable(reason)) => {
                toolchain
                    .get_or_insert_with(|| (reason, Vec::new()))
                    .1
                    .push(source.spec.id.0.clone());
            }
            Err(error) => diagnostics.push(
                report(format!(
                    "WASM island `{}` could not be built{}: {error}",
                    source.spec.id,
                    if require {
                        ""
                    } else {
                        "; its functions stay JS"
                    }
                ))
                .with_code("trioxide(wasm-build)"),
            ),
        }
    }
    if let Some((reason, islands)) = toolchain {
        let message = if require {
            format!(
                "{reason}; WASM is required (--require-wasm) but {} island(s) can't be built: {}",
                islands.len(),
                islands.join(", ")
            )
        } else {
            format!(
                "{reason}; {} WASM island(s) fall back to JS: {}",
                islands.len(),
                islands.join(", ")
            )
        };
        diagnostics.insert(
            0,
            report(message)
                .with_code("trioxide(wasm-toolchain)")
                .with_help(
                    "install Rust (https://rustup.rs) and run `rustup target add wasm32-unknown-unknown`; `trioxide check-toolchain` shows what is missing. --no-wasm turns islands off.",
                ),
        );
    }
    (artifacts, diagnostics)
}

/// A built island that modules' JS may call.
#[derive(Debug, Clone)]
pub(crate) struct IslandLink {
    pub id: trioxide_wasm::IslandId,
    /// The module defining its exports.
    pub module: trioxide_common::ModuleId,
    /// The exports calls may be pointed at, with the names their module exports them under.
    pub bindings: Vec<trioxide_partition::ExportBinding>,
    /// Every function compiled into it.
    pub members: Vec<trioxide_types::FunctionId>,
    /// The defining module's own calls may go through the glue (it imports the glue).
    pub local_calls: bool,
    /// The glue calls the JS versions by their exported names, so only exported functions'
    /// calls may go through it.
    pub fallback: bool,
}

/// What rewriting one module's calls needs.
pub(crate) struct Rewrite<'r> {
    pub module: trioxide_common::ModuleId,
    pub links: &'r [IslandLink],
    /// The module's import specifiers and the graph modules they resolved to.
    pub imports: Vec<(String, Option<trioxide_common::ModuleId>)>,
    /// Spans of island members in this module: their JS versions keep calling each other in
    /// JS, so the fallback stays pure JS.
    pub member_spans: Vec<oxc_span::Span>,
}

/// The `fallback` specifier for the JS module at `path`: its absolute path (with `/`), which
/// bundlers resolve and compile; `trioxide build` rewrites it to the compiled module's path
/// relative to the glue.
pub(crate) fn fallback_specifier(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Points direct calls of island exports at the island's glue: imports of exports from
/// their module move to an import from the glue (the original import stays, specifier-less,
/// so evaluation order doesn't change), and calls in the defining module go through an
/// import of the glue. Runs on the stripped JavaScript. Returns the islands the module now
/// imports.
pub(crate) fn rewrite_calls<'a>(
    allocator: &'a oxc_allocator::Allocator,
    program: &mut oxc_ast::ast::Program<'a>,
    scoping: &oxc_semantic::Scoping,
    rewrite: &Rewrite<'_>,
) -> Vec<trioxide_wasm::IslandRef> {
    use oxc_ast::{
        ast::{
            BindingIdentifier, IdentifierName, ImportDeclaration, ImportDeclarationSpecifier,
            ImportOrExportKind, ImportSpecifier, ModuleExportName, Statement, StringLiteral,
        },
        builder::AstBuilder,
    };
    use oxc_span::SPAN;
    use rustc_hash::{FxHashMap, FxHashSet};

    let builder = AstBuilder::new(allocator);
    let calls = call_references(program);
    let only_called = |symbol: oxc_syntax::symbol::SymbolId| {
        let references = scoping.get_resolved_reference_ids(symbol);
        !references.is_empty() && references.iter().all(|r| calls.contains(r))
    };
    let mut refs: Vec<trioxide_wasm::IslandRef> = Vec::new();
    let mut add_ref = |id: &trioxide_wasm::IslandId| {
        if !refs.iter().any(|r| r.id == *id) {
            refs.push(trioxide_wasm::IslandRef {
                id: id.clone(),
                specifier: id.glue_specifier(),
            });
        }
    };
    let glue_import = |specifiers: oxc_allocator::Vec<'a, ImportDeclarationSpecifier<'a>>,
                       id: &trioxide_wasm::IslandId| {
        let source = StringLiteral::new(
            SPAN,
            allocator.alloc_str(&id.glue_specifier()),
            None,
            &builder,
        );
        Statement::ImportDeclaration(oxc_allocator::Box::new_in(
            ImportDeclaration::new(
                SPAN,
                Some(specifiers),
                source,
                None,
                None,
                ImportOrExportKind::Value,
                &builder,
            ),
            &allocator,
        ))
    };

    // Imports of island exports from other modules.
    let mut index = 0;
    while index < program.body.len() {
        let Statement::ImportDeclaration(declaration) = &mut program.body[index] else {
            index += 1;
            continue;
        };
        let target = rewrite
            .imports
            .iter()
            .find(|(specifier, _)| specifier == declaration.source.value.as_str())
            .and_then(|(_, target)| *target);
        let link = target.and_then(|target| {
            rewrite
                .links
                .iter()
                .find(|link| link.module == target && target != rewrite.module)
        });
        let (Some(link), Some(specifiers)) = (link, declaration.specifiers.as_mut()) else {
            index += 1;
            continue;
        };
        let mut moved = oxc_allocator::Vec::new_in(&allocator);
        let mut kept = oxc_allocator::Vec::new_in(&allocator);
        for mut specifier in specifiers.drain(..) {
            // The island export the imported name refers to (exported names can swap).
            let binding = match &specifier {
                ImportDeclarationSpecifier::ImportSpecifier(named)
                    if named.import_kind == ImportOrExportKind::Value
                        && named.local.symbol_id.get().is_some_and(only_called) =>
                {
                    let imported = named.imported.name();
                    link.bindings
                        .iter()
                        .find(|b| b.exported_as.iter().any(|n| *n == imported.as_str()))
                }
                _ => None,
            };
            match (binding, &mut specifier) {
                (Some(binding), ImportDeclarationSpecifier::ImportSpecifier(named)) => {
                    named.imported = ModuleExportName::IdentifierName(IdentifierName::new(
                        SPAN,
                        allocator.alloc_str(&binding.name),
                        &builder,
                    ));
                    moved.push(specifier);
                }
                _ => kept.push(specifier),
            }
        }
        if moved.is_empty() {
            *specifiers = kept;
            index += 1;
            continue;
        }
        declaration.specifiers = if kept.is_empty() { None } else { Some(kept) };
        add_ref(&link.id);
        program.body.insert(index + 1, glue_import(moved, &link.id));
        index += 2;
    }

    // Calls in the defining module, when it may import the glue.
    let mut renames: FxHashMap<oxc_syntax::reference::ReferenceId, &'a str> = FxHashMap::default();
    let mut added: Vec<Statement<'a>> = Vec::new();
    for link in rewrite
        .links
        .iter()
        .filter(|l| l.module == rewrite.module && l.local_calls)
    {
        let mut specifiers = oxc_allocator::Vec::new_in(&allocator);
        let taken: FxHashSet<String> = scoping.symbol_names().map(str::to_string).collect();
        for name in link
            .bindings
            .iter()
            .filter(|b| !link.fallback || !b.exported_as.is_empty())
            .map(|b| &b.name)
        {
            let Some(symbol) = scoping.get_root_binding(name.as_str().into()) else {
                continue;
            };
            let sites: Vec<_> = scoping
                .get_resolved_reference_ids(symbol)
                .iter()
                .copied()
                .filter(|r| calls.contains(r))
                .collect();
            let sites: Vec<_> = sites
                .into_iter()
                .filter(|r| {
                    let span = call_spans(program).get(r).copied().unwrap_or_default();
                    !rewrite
                        .member_spans
                        .iter()
                        .any(|m| m.start <= span.start && span.end <= m.end)
                })
                .collect();
            if sites.is_empty() {
                continue;
            }
            let mut alias = format!("{name}$wasm");
            let mut n = 2;
            while taken.contains(&alias)
                || scoping
                    .root_unresolved_references()
                    .contains_key(alias.as_str())
            {
                alias = format!("{name}$wasm{n}");
                n += 1;
            }
            let alias: &'a str = allocator.alloc_str(&alias);
            for site in sites {
                renames.insert(site, alias);
            }
            let imported = ModuleExportName::IdentifierName(IdentifierName::new(
                SPAN,
                allocator.alloc_str(name),
                &builder,
            ));
            let local = BindingIdentifier::new(SPAN, alias, &builder);
            specifiers.push(ImportDeclarationSpecifier::ImportSpecifier(
                oxc_allocator::Box::new_in(
                    ImportSpecifier::new(
                        SPAN,
                        imported,
                        local,
                        ImportOrExportKind::Value,
                        &builder,
                    ),
                    &allocator,
                ),
            ));
        }
        if !specifiers.is_empty() {
            add_ref(&link.id);
            added.push(glue_import(specifiers, &link.id));
        }
    }
    if !renames.is_empty() {
        let mut renamer = Renamer { renames: &renames };
        oxc_ast_visit::VisitMut::visit_program(&mut renamer, program);
        let at = program
            .body
            .iter()
            .rposition(|s| matches!(s, Statement::ImportDeclaration(_)))
            .map_or(0, |i| i + 1);
        for (offset, statement) in added.into_iter().enumerate() {
            program.body.insert(at + offset, statement);
        }
    }
    refs
}

/// References that are the callee of a (non-optional) call.
fn call_references(
    program: &oxc_ast::ast::Program<'_>,
) -> rustc_hash::FxHashSet<oxc_syntax::reference::ReferenceId> {
    call_spans(program).into_keys().collect()
}

fn call_spans(
    program: &oxc_ast::ast::Program<'_>,
) -> rustc_hash::FxHashMap<oxc_syntax::reference::ReferenceId, oxc_span::Span> {
    use oxc_ast::ast::{CallExpression, Expression};
    struct Calls(rustc_hash::FxHashMap<oxc_syntax::reference::ReferenceId, oxc_span::Span>);
    impl<'a> oxc_ast_visit::Visit<'a> for Calls {
        fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
            if !call.optional
                && let Expression::Identifier(id) = &call.callee
                && let Some(reference) = id.reference_id.get()
            {
                self.0.insert(reference, id.span);
            }
            oxc_ast_visit::walk::walk_call_expression(self, call);
        }
    }
    let mut calls = Calls(rustc_hash::FxHashMap::default());
    oxc_ast_visit::Visit::visit_program(&mut calls, program);
    calls.0
}

struct Renamer<'r, 'a> {
    renames: &'r rustc_hash::FxHashMap<oxc_syntax::reference::ReferenceId, &'a str>,
}

impl<'a> oxc_ast_visit::VisitMut<'a> for Renamer<'_, 'a> {
    fn visit_identifier_reference(&mut self, id: &mut oxc_ast::ast::IdentifierReference<'a>) {
        if let Some(reference) = id.reference_id.get()
            && let Some(&alias) = self.renames.get(&reference)
        {
            // Printed by name: the glue binding has no semantic data.
            id.reference_id.set(None);
            id.name = alias.into();
        }
    }
}
