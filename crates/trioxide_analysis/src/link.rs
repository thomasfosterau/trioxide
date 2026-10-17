//! The linker: resolves every import binding to the binding it imports, through re-exports
//! (PLAN §2 step 3).

use oxc_ast::ast::{ExportDefaultDeclarationKind, Statement};
use oxc_syntax::module_record::{
    ExportExportName, ExportImportName, ExportLocalName, ImportImportName,
};
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::{ModuleId, NodeRef, SymbolRef};

use crate::world::{Module, World};

/// What an import (or an export) names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// A binding in a module of the graph.
    Symbol(SymbolRef),
    /// The namespace object of a module of the graph (`import * as ns`).
    Namespace(ModuleId),
    /// The value of an `export default <expression>`: the expression's node.
    Expression(NodeRef),
    /// A module outside the graph (a package, or a file trioxide doesn't compile).
    External,
    /// Nothing: the export doesn't exist, is ambiguous, or the module failed to parse.
    Unresolved,
}

/// Import bindings and module exports, resolved.
#[derive(Debug, Default)]
pub struct Links {
    imports: FxHashMap<SymbolRef, Target>,
}

impl Links {
    /// Links every module of `world`.
    pub fn build(world: &World<'_>) -> Self {
        let mut links = Self::default();
        for module in &world.modules {
            for entry in &module.record.import_entries {
                if entry.is_type {
                    continue;
                }
                let Some(local) = module.root_binding(entry.local_name.name.as_str()) else {
                    continue;
                };
                let target = match module.imports.get(entry.module_request.name.as_str()) {
                    Some(Some(target)) => {
                        let target = world.module(*target);
                        match &entry.import_name {
                            ImportImportName::NamespaceObject => Target::Namespace(target.id),
                            ImportImportName::Default(_) => {
                                resolve_export(world, target, "default", &mut FxHashSet::default())
                            }
                            ImportImportName::Name(name) => resolve_export(
                                world,
                                target,
                                name.name.as_str(),
                                &mut FxHashSet::default(),
                            ),
                        }
                    }
                    _ => Target::External,
                };
                links.imports.insert(module.symbol(local), target);
            }
        }
        links
    }

    /// What the import binding `symbol` refers to, if it is one.
    pub fn import(&self, symbol: SymbolRef) -> Option<Target> {
        self.imports.get(&symbol).copied()
    }

    /// Every resolved import, sorted.
    pub fn imports(&self) -> Vec<(SymbolRef, Target)> {
        let mut imports: Vec<_> = self.imports.iter().map(|(k, v)| (*k, *v)).collect();
        imports.sort_by_key(|(symbol, _)| *symbol);
        imports
    }
}

/// Resolves the export `name` of `module`.
pub fn resolve_export(
    world: &World<'_>,
    module: &Module<'_>,
    name: &str,
    visiting: &mut FxHashSet<(ModuleId, String)>,
) -> Target {
    if module.broken || !visiting.insert((module.id, name.to_string())) {
        return Target::Unresolved;
    }
    for entry in &module.record.local_export_entries {
        if entry.is_type || !export_name_is(&entry.export_name, name) {
            continue;
        }
        return match &entry.local_name {
            ExportLocalName::Name(local) => match module.root_binding(local.name.as_str()) {
                Some(symbol) => follow_local(world, module, symbol, visiting),
                None => Target::Unresolved,
            },
            ExportLocalName::Default(_) | ExportLocalName::Null => default_expression(module),
        };
    }
    for entry in &module.record.indirect_export_entries {
        if entry.is_type || !export_name_is(&entry.export_name, name) {
            continue;
        }
        let Some(target) = entry
            .module_request
            .as_ref()
            .and_then(|request| module.imports.get(request.name.as_str()).copied().flatten())
        else {
            return Target::External;
        };
        let target = world.module(target);
        return match &entry.import_name {
            ExportImportName::Name(imported) => {
                resolve_export(world, target, imported.name.as_str(), visiting)
            }
            ExportImportName::All | ExportImportName::AllButDefault => Target::Namespace(target.id),
            ExportImportName::Null => Target::Unresolved,
        };
    }
    if name == "default" {
        return Target::Unresolved;
    }
    let mut found = None;
    for entry in &module.record.star_export_entries {
        let Some(target) = entry
            .module_request
            .as_ref()
            .and_then(|request| module.imports.get(request.name.as_str()).copied().flatten())
        else {
            // `export *` from outside the graph may supply any name.
            return Target::External;
        };
        let resolved = resolve_export(world, world.module(target), name, visiting);
        if resolved == Target::Unresolved {
            continue;
        }
        match found {
            None => found = Some(resolved),
            Some(previous) if previous == resolved => {}
            // Ambiguous star exports are not exported at all.
            Some(_) => return Target::Unresolved,
        }
    }
    found.unwrap_or(Target::Unresolved)
}

/// A local binding that is exported: itself, or what it imports.
fn follow_local(
    world: &World<'_>,
    module: &Module<'_>,
    symbol: oxc_syntax::symbol::SymbolId,
    visiting: &mut FxHashSet<(ModuleId, String)>,
) -> Target {
    if !module.info(symbol).import {
        return Target::Symbol(module.symbol(symbol));
    }
    let name = module.semantic.scoping().symbol_name(symbol);
    for entry in &module.record.import_entries {
        if entry.local_name.name.as_str() != name {
            continue;
        }
        let Some(Some(target)) = module.imports.get(entry.module_request.name.as_str()) else {
            return Target::External;
        };
        let target = world.module(*target);
        return match &entry.import_name {
            ImportImportName::NamespaceObject => Target::Namespace(target.id),
            ImportImportName::Default(_) => resolve_export(world, target, "default", visiting),
            ImportImportName::Name(imported) => {
                resolve_export(world, target, imported.name.as_str(), visiting)
            }
        };
    }
    Target::Unresolved
}

/// The value of the module's `export default`.
fn default_expression(module: &Module<'_>) -> Target {
    for statement in &module.program.body {
        if let Statement::ExportDefaultDeclaration(export) = statement {
            let node = match &export.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                    if let Some(symbol) = function.id.as_ref().and_then(|id| id.symbol_id.get()) {
                        return Target::Symbol(module.symbol(symbol));
                    }
                    function.node_id.get()
                }
                ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                    if let Some(symbol) = class.id.as_ref().and_then(|id| id.symbol_id.get()) {
                        return Target::Symbol(module.symbol(symbol));
                    }
                    class.node_id.get()
                }
                ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => {
                    return Target::Unresolved;
                }
                expression => match expression.as_expression() {
                    Some(expression) => oxc_ast::ast::Expression::node_id(expression),
                    None => return Target::Unresolved,
                },
            };
            return Target::Expression(module.node(node));
        }
    }
    Target::Unresolved
}

fn export_name_is(export: &ExportExportName<'_>, name: &str) -> bool {
    match export {
        ExportExportName::Name(export) => export.name.as_str() == name,
        ExportExportName::Default(_) => name == "default",
        ExportExportName::Null => false,
    }
}
