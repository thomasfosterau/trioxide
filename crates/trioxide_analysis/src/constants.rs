//! Facts that follow from declarations and references alone: `const` bindings with literal
//! values, enum members whose objects nothing can change, and exports no module uses (PLAN §4
//! passes 1 and 3).

use oxc_ast::{
    AstKind,
    ast::{
        Declaration, Expression, Statement, TSEnumMemberName, UnaryOperator,
        VariableDeclarationKind,
    },
};
use oxc_semantic::Scoping;
use oxc_span::GetSpan;
use oxc_syntax::symbol::{SymbolFlags, SymbolId};
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::{ModuleId, NodeId, SymbolRef};

use crate::{
    AnalysisOptions,
    facts::{Facts, Trust},
    link::{Links, Target},
    world::{Module, World},
};

/// A primitive value known at compile time.
#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    /// A number (any `f64`, `-0` and NaN included).
    Number(f64),
    /// A string.
    String(String),
    /// `true` or `false`.
    Boolean(bool),
    /// A bigint, in base 10 without the `n` suffix.
    BigInt(String),
    /// `null`.
    Null,
    /// `undefined`.
    Undefined,
}

/// Records [`Facts::symbol_constant`], [`Facts::enum_member`] and [`Facts::export_used`].
pub fn record(world: &World<'_>, links: &Links, options: &AnalysisOptions, facts: &mut Facts) {
    let cycles = in_cycle(world);
    for module in &world.modules {
        if module.broken {
            continue;
        }
        constant_bindings(module, facts);
    }
    for module in &world.modules {
        if module.broken {
            continue;
        }
        sound_enums(world, links, module, &cycles, options, facts);
    }
    if options.closed_world {
        unused_exports(world, links, options, facts);
    }
}

/// The literal an initialiser is, looking through parentheses and TypeScript-only wrappers.
fn literal(module: &Module<'_>, expression: &Expression<'_>) -> Option<Constant> {
    Some(match expression {
        Expression::NumericLiteral(literal) => Constant::Number(literal.value),
        Expression::StringLiteral(literal) => Constant::String(literal.value.to_string()),
        Expression::BooleanLiteral(literal) => Constant::Boolean(literal.value),
        Expression::NullLiteral(_) => Constant::Null,
        Expression::BigIntLiteral(literal) => Constant::BigInt(literal.value.to_string()),
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
            Constant::String(template.quasis.first()?.value.cooked?.to_string())
        }
        Expression::Identifier(ident) if ident.name == "undefined" => {
            let reference = module
                .semantic
                .scoping()
                .get_reference(ident.reference_id.get()?);
            if reference.symbol_id().is_some() {
                return None;
            }
            Constant::Undefined
        }
        Expression::UnaryExpression(unary) => match unary.operator {
            UnaryOperator::UnaryNegation => match literal(module, &unary.argument)? {
                Constant::Number(value) => Constant::Number(-value),
                _ => return None,
            },
            UnaryOperator::Void if is_literal(&unary.argument) => Constant::Undefined,
            _ => return None,
        },
        Expression::ParenthesizedExpression(paren) => literal(module, &paren.expression)?,
        Expression::TSAsExpression(cast) => literal(module, &cast.expression)?,
        Expression::TSSatisfiesExpression(cast) => literal(module, &cast.expression)?,
        Expression::TSTypeAssertion(cast) => literal(module, &cast.expression)?,
        Expression::TSNonNullExpression(cast) => literal(module, &cast.expression)?,
        _ => return None,
    })
}

fn is_literal(expression: &Expression<'_>) -> bool {
    matches!(
        expression.without_parentheses(),
        Expression::NumericLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
    )
}

/// `const x = <literal>`: every read that doesn't throw (it may be in the temporal dead zone)
/// sees the literal.
fn constant_bindings(module: &Module<'_>, facts: &mut Facts) {
    for node in module.semantic.nodes().iter() {
        let AstKind::VariableDeclaration(declaration) = node.kind() else {
            continue;
        };
        if declaration.kind != VariableDeclarationKind::Const || declaration.declare {
            continue;
        }
        for declarator in &declaration.declarations {
            let (Some(ident), Some(init)) =
                (declarator.id.get_binding_identifier(), &declarator.init)
            else {
                continue;
            };
            if !matches!(
                declarator.id,
                oxc_ast::ast::BindingPattern::BindingIdentifier(_)
            ) {
                continue;
            }
            if let (Some(symbol), Some(value)) = (ident.symbol_id.get(), literal(module, init)) {
                facts.set_symbol_constant(module.symbol(symbol), value);
            }
        }
    }
}

/// Which modules are in an import cycle (and so may run each other's code before they finish).
fn in_cycle(world: &World<'_>) -> Vec<bool> {
    let count = world.modules.len();
    let successors = |index: usize| -> Vec<usize> {
        world.modules[index]
            .imports
            .values()
            .copied()
            .flatten()
            .map(ModuleId::index)
            .filter(|&target| target < count)
            .collect()
    };
    (0..count)
        .map(|start| {
            let mut seen = vec![false; count];
            let mut pending = successors(start);
            while let Some(next) = pending.pop() {
                if next == start {
                    return true;
                }
                if !std::mem::replace(&mut seen[next], true) {
                    pending.extend(successors(next));
                }
            }
            false
        })
        .collect()
}

/// Enum members whose reads yield their declared values: the enum is declared once at the top
/// of a module that is in no import cycle, after only code that can't call anything (so no
/// read runs before the enum object exists). See [`enum_trust`] for how far that holds.
fn sound_enums(
    world: &World<'_>,
    links: &Links,
    module: &Module<'_>,
    cycles: &[bool],
    options: &AnalysisOptions,
    facts: &mut Facts,
) {
    if cycles[module.id.index()] {
        return;
    }
    let scoping = module.semantic.scoping();
    let mut inert = true;
    for statement in &module.program.body {
        let declaration = match statement {
            Statement::TSEnumDeclaration(declaration) => Some(&**declaration),
            Statement::ExportDeclaration(export) => match &export.declaration {
                Declaration::TSEnumDeclaration(declaration) => Some(&**declaration),
                _ => None,
            },
            _ => None,
        };
        if let Some(declaration) = declaration
            && inert
            && !declaration.declare
            && let Some(symbol) = declaration.id.symbol_id.get()
        {
            let exported = matches!(statement, Statement::ExportDeclaration(_));
            if let Some(trust) = enum_trust(world, links, module, symbol, exported, cycles, options)
            {
                for member in &declaration.body.members {
                    let name = match &member.id {
                        TSEnumMemberName::Identifier(ident) => ident.name.as_str(),
                        TSEnumMemberName::String(name) | TSEnumMemberName::ComputedString(name) => {
                            name.value.as_str()
                        }
                        TSEnumMemberName::ComputedTemplateString(_) => continue,
                    };
                    let Some(member_symbol) = enum_member_symbol(scoping, symbol, name) else {
                        continue;
                    };
                    let value = match scoping.get_enum_member_value(member_symbol) {
                        Some(oxc_syntax::constant_value::ConstantValue::Number(value)) => {
                            Constant::Number(*value)
                        }
                        Some(oxc_syntax::constant_value::ConstantValue::String(value)) => {
                            Constant::String(value.to_string())
                        }
                        None => continue,
                    };
                    facts.set_enum_member(module.symbol(symbol), name.to_string(), value, trust);
                }
            }
        }
        inert &= statement_is_inert(statement);
    }
}

fn enum_member_symbol(scoping: &Scoping, symbol: SymbolId, name: &str) -> Option<SymbolId> {
    let scopes = scoping.get_enum_body_scopes(symbol)?;
    let [scope] = scopes else {
        return None;
    };
    scoping
        .iter_bindings_in(*scope)
        .find(|&member| scoping.symbol_name(member) == name)
}

/// Whether running `statement` (at a module's top level) can't call any code.
fn statement_is_inert(statement: &Statement<'_>) -> bool {
    match statement {
        Statement::ImportDeclaration(_)
        | Statement::FunctionDeclaration(_)
        | Statement::TSTypeAliasDeclaration(_)
        | Statement::TSInterfaceDeclaration(_)
        | Statement::EmptyStatement(_)
        | Statement::ExportNamedDeclaration(_)
        | Statement::ExportFromDeclaration(_) => true,
        Statement::ExportDeclaration(export) => declaration_is_inert(&export.declaration),
        Statement::TSEnumDeclaration(declaration) => enum_is_inert(declaration),
        Statement::VariableDeclaration(declaration) => variables_are_inert(declaration),
        _ => false,
    }
}

fn declaration_is_inert(declaration: &Declaration<'_>) -> bool {
    match declaration {
        Declaration::FunctionDeclaration(_)
        | Declaration::TSTypeAliasDeclaration(_)
        | Declaration::TSInterfaceDeclaration(_) => true,
        Declaration::TSEnumDeclaration(declaration) => enum_is_inert(declaration),
        Declaration::VariableDeclaration(declaration) => variables_are_inert(declaration),
        _ => false,
    }
}

fn enum_is_inert(declaration: &oxc_ast::ast::TSEnumDeclaration<'_>) -> bool {
    declaration.declare
        || declaration
            .body
            .members
            .iter()
            .all(|member| member.initializer.as_ref().is_none_or(literal_initializer))
}

fn variables_are_inert(declaration: &oxc_ast::ast::VariableDeclaration<'_>) -> bool {
    declaration.declare
        || (!matches!(
            declaration.kind,
            VariableDeclarationKind::Using | VariableDeclarationKind::AwaitUsing
        ) && declaration.declarations.iter().all(|declarator| {
            matches!(
                declarator.id,
                oxc_ast::ast::BindingPattern::BindingIdentifier(_)
            ) && declarator.init.as_ref().is_none_or(literal_initializer)
        }))
}

fn literal_initializer(expression: &Expression<'_>) -> bool {
    match expression.without_parentheses() {
        Expression::UnaryExpression(unary) => {
            unary.operator == UnaryOperator::UnaryNegation && is_literal(&unary.argument)
        }
        Expression::BigIntLiteral(_) => true,
        Expression::TemplateLiteral(template) => template.expressions.is_empty(),
        other => is_literal(other),
    }
}

/// How far the members of the enum `symbol` of `module` can be relied on, given that it is
/// declared before anything can run: `Proven` when every reference to it, and to every import
/// of it, is a member read and no namespace object, re-export or open-world export exposes it
/// (so nothing can change the object); `Declared` when only TypeScript says its members are
/// read-only; `None` when it is merged with other declarations, the program writes one of its
/// members (casts can't hide that), or an importer is in an import cycle (so a read might run
/// before the object exists).
fn enum_trust(
    world: &World<'_>,
    links: &Links,
    module: &Module<'_>,
    symbol: SymbolId,
    exported: bool,
    cycles: &[bool],
    options: &AnalysisOptions,
) -> Option<Trust> {
    let scoping = module.semantic.scoping();
    if scoping
        .symbol_flags(symbol)
        .intersects(!(SymbolFlags::RegularEnum | SymbolFlags::ConstEnum))
    {
        return None;
    }
    let mut sealed = match uses(module, symbol) {
        Uses::MemberReads => true,
        Uses::Value => false,
        Uses::Written => return None,
    };
    if exported {
        sealed &= options.closed_world
            && !crate::interp::open_modules(world, options).contains(&module.id)
            && !exposed_modules(world, links).contains(&module.id);
        let target = module.symbol(symbol);
        for (import, resolved) in links.imports() {
            if resolved == Target::Symbol(target) {
                let importer = world.module(import.module);
                if cycles[importer.id.index()] {
                    return None;
                }
                match uses(importer, import.symbol) {
                    Uses::MemberReads => sealed &= !importer.info(import.symbol).exported,
                    Uses::Value => sealed = false,
                    Uses::Written => return None,
                }
            }
        }
    }
    Some(if sealed {
        Trust::Proven
    } else {
        Trust::Declared
    })
}

/// How a binding is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Uses {
    /// Only as the object of member reads.
    MemberReads,
    /// Also as a value (passed, stored, compared, …).
    Value,
    /// Some member of it is written, through casts and parentheses too.
    Written,
}

/// How `symbol` is used: the worst of its references.
fn uses(module: &Module<'_>, symbol: SymbolId) -> Uses {
    let scoping = module.semantic.scoping();
    let nodes = module.semantic.nodes();
    let mut worst = Uses::MemberReads;
    for reference in scoping.get_resolved_references(symbol) {
        if !reference.flags().is_value() {
            continue;
        }
        if reference.is_write() {
            return Uses::Written;
        }
        // Up through what doesn't change the value: `(E as any).A = 1` writes E's member.
        let mut node = reference.node_id();
        loop {
            let parent = nodes.parent_id(node);
            match nodes.kind(parent) {
                AstKind::ParenthesizedExpression(_)
                | AstKind::TSAsExpression(_)
                | AstKind::TSSatisfiesExpression(_)
                | AstKind::TSNonNullExpression(_)
                | AstKind::TSTypeAssertion(_) => node = parent,
                _ => break,
            }
        }
        let member = nodes.parent_id(node);
        let object_span = nodes.kind(node).span();
        let is_object = match nodes.kind(member) {
            AstKind::StaticMemberExpression(member) => member.object.span() == object_span,
            AstKind::ComputedMemberExpression(member) => member.object.span() == object_span,
            _ => false,
        };
        let this = if !is_object {
            Uses::Value
        } else if is_written(nodes, member) {
            Uses::Written
        } else if node == reference.node_id() {
            Uses::MemberReads
        } else {
            // A read through a cast: TypeScript can't vouch for what it reads.
            Uses::Value
        };
        worst = worst.max(this);
    }
    worst
}

/// Whether the member expression `member` is written: assigned, updated, deleted or a
/// destructuring or `for-in`/`of` target.
fn is_written(nodes: &oxc_semantic::AstNodes<'_>, member: NodeId) -> bool {
    let span = nodes.kind(member).span();
    match nodes.parent_kind(member) {
        AstKind::AssignmentExpression(assignment) => assignment.left.span() == span,
        AstKind::UpdateExpression(_)
        | AstKind::ArrayAssignmentTarget(_)
        | AstKind::ObjectAssignmentTarget(_)
        | AstKind::AssignmentTargetWithDefault(_)
        | AstKind::AssignmentTargetPropertyIdentifier(_)
        | AstKind::AssignmentTargetPropertyProperty(_)
        | AstKind::AssignmentTargetRest(_) => true,
        AstKind::UnaryExpression(unary) => unary.operator == UnaryOperator::Delete,
        AstKind::ForInStatement(statement) => statement.left.span() == span,
        AstKind::ForOfStatement(statement) => statement.left.span() == span,
        _ => false,
    }
}

/// Modules whose namespace object some code may hold: entries' and other open modules are
/// handled by the caller; these are the targets of `import * as`, `export * as`, dynamic
/// `import()`, and everything they re-export from. A dynamic `import()` of a computed
/// specifier exposes every module.
fn exposed_modules(world: &World<'_>, links: &Links) -> FxHashSet<ModuleId> {
    let mut pending: Vec<ModuleId> = Vec::new();
    for (_, target) in links.imports() {
        if let Target::Namespace(module) = target {
            pending.push(module);
        }
    }
    for module in &world.modules {
        for dynamic in &module.record.dynamic_imports {
            let span = dynamic.module_request;
            let text = span.source_text(module.program.source_text);
            let specifier = text
                .strip_prefix(['"', '\''])
                .and_then(|text| text.strip_suffix(['"', '\'']));
            match specifier.and_then(|specifier| module.imports.get(specifier).copied()) {
                Some(Some(target)) => pending.push(target),
                Some(None) => {}
                None => return world.modules.iter().map(|module| module.id).collect(),
            }
        }
        for entry in &module.record.indirect_export_entries {
            // `export * as ns from "x"`.
            if matches!(
                entry.import_name,
                oxc_syntax::module_record::ExportImportName::All
            ) && let Some(Some(target)) = entry
                .module_request
                .as_ref()
                .map(|request| module.imports.get(request.name.as_str()).copied().flatten())
            {
                pending.push(target);
            }
        }
    }
    let mut exposed = FxHashSet::default();
    while let Some(module) = pending.pop() {
        if module.index() >= world.modules.len() || !exposed.insert(module) {
            continue;
        }
        let info = world.module(module);
        for entry in info
            .record
            .star_export_entries
            .iter()
            .chain(&info.record.indirect_export_entries)
        {
            if let Some(target) = entry
                .module_request
                .as_ref()
                .and_then(|request| info.imports.get(request.name.as_str()).copied().flatten())
            {
                pending.push(target);
            }
        }
        for entry in &info.record.import_entries {
            if let Some(local) = info.root_binding(entry.local_name.name.as_str())
                && info.info(local).exported
                && let Some(Some(target)) = info.imports.get(entry.module_request.name.as_str())
            {
                pending.push(*target);
            }
        }
    }
    exposed
}

/// Local exports of modules the host can't see (not entries, not re-exported by one, no
/// namespace taken) that no import resolves to.
fn unused_exports(world: &World<'_>, links: &Links, options: &AnalysisOptions, facts: &mut Facts) {
    if options.entries.is_empty() {
        return;
    }
    let open = crate::interp::open_modules(world, options);
    let exposed = exposed_modules(world, links);
    let imported: FxHashSet<SymbolRef> = links
        .imports()
        .into_iter()
        .filter_map(|(_, target)| match target {
            Target::Symbol(symbol) => Some(symbol),
            _ => None,
        })
        .collect();
    let mut unused = FxHashMap::default();
    for module in &world.modules {
        if module.broken
            || open.contains(&module.id)
            || exposed.contains(&module.id)
            || module.called_from_generated_code()
        {
            continue;
        }
        for entry in &module.record.local_export_entries {
            if entry.is_type {
                continue;
            }
            let oxc_syntax::module_record::ExportLocalName::Name(local) = &entry.local_name else {
                continue;
            };
            let Some(symbol) = module.root_binding(local.name.as_str()) else {
                continue;
            };
            if module.info(symbol).import {
                continue;
            }
            let symbol = module.symbol(symbol);
            unused
                .entry(symbol)
                .and_modify(|unused: &mut bool| *unused &= !imported.contains(&symbol))
                .or_insert(!imported.contains(&symbol));
        }
    }
    for (symbol, unused) in unused {
        if unused {
            facts.set_export_unused(symbol);
        }
    }
}
