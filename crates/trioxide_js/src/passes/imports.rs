//! The import guard: keeps every module the original program runs.
//!
//! The TypeScript strip elides an import declaration whose bindings are never used as values.
//! When the passes remove the last such use (by inlining an imported constant or deleting dead
//! code), the declaration would be elided and its module might not run at all. So
//! [`RecordImports`] notes, before any rewrite, which declarations the strip would keep, and
//! [`KeepImports`] adds a side-effect `import "…"` after each of those that lost its last
//! value use, so it runs in the same order as before.

use std::{cell::RefCell, rc::Rc};

use oxc_allocator::CloneIn;
use oxc_ast::ast::{
    ImportDeclaration, ImportDeclarationSpecifier, ImportOrExportKind, Program, Statement,
};
use oxc_semantic::Scoping;
use oxc_span::Span;
use rustc_hash::FxHashSet;

use crate::{Pass, PassContext};

/// What [`RecordImports`] saw, for [`KeepImports`].
#[derive(Debug, Default)]
pub struct Guard {
    /// Spans of the import declarations with a value use before the passes ran.
    used: FxHashSet<Span>,
    /// Those already given a side-effect import.
    kept: FxHashSet<Span>,
}

/// Records which imports have value uses (`"imports"`, first in the foundation group).
pub struct RecordImports(Rc<RefCell<Guard>>);

impl RecordImports {
    /// A recorder writing into `guard`.
    pub fn new(guard: Rc<RefCell<Guard>>) -> Self {
        Self(guard)
    }
}

/// Keeps the modules of imports whose value uses the passes removed (`"imports"`, last in the
/// cleanup group).
pub struct KeepImports(Rc<RefCell<Guard>>);

impl KeepImports {
    /// A guard reading what `guard` recorded.
    pub fn new(guard: Rc<RefCell<Guard>>) -> Self {
        Self(guard)
    }
}

/// Whether some binding of `declaration` has a value use in `scoping`.
fn has_value_use(declaration: &ImportDeclaration<'_>, scoping: &Scoping) -> bool {
    let Some(specifiers) = &declaration.specifiers else {
        return false;
    };
    specifiers.iter().any(|specifier| {
        if let ImportDeclarationSpecifier::ImportSpecifier(specifier) = specifier
            && specifier.import_kind == ImportOrExportKind::Type
        {
            return false;
        }
        scoping
            .get_resolved_references(specifier.local().symbol_id())
            .any(|reference| !reference.is_type())
    })
}

fn value_imports<'p, 'a>(
    program: &'p Program<'a>,
) -> impl Iterator<Item = &'p ImportDeclaration<'a>> {
    program.body.iter().filter_map(|statement| match statement {
        Statement::ImportDeclaration(declaration)
            if declaration.import_kind != ImportOrExportKind::Type =>
        {
            Some(&**declaration)
        }
        _ => None,
    })
}

impl Pass for RecordImports {
    fn name(&self) -> &'static str {
        "imports"
    }

    fn reads_facts(&self) -> bool {
        false
    }

    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
        let mut guard = self.0.borrow_mut();
        if guard.used.is_empty() {
            guard.used = value_imports(program)
                .filter(|declaration| has_value_use(declaration, ctx.scoping()))
                .map(|declaration| declaration.span)
                .collect();
        }
        false
    }
}

impl Pass for KeepImports {
    fn name(&self) -> &'static str {
        "imports"
    }

    fn reads_facts(&self) -> bool {
        false
    }

    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
        let mut guard = self.0.borrow_mut();
        if guard.used.is_empty() {
            return false;
        }
        // The manager rebuilds the scoping between iterations, but not within this one.
        let fresh = crate::rebuild_scoping(program);
        let mut lost: Vec<(
            usize,
            Span,
            oxc_allocator::ArenaBox<'a, ImportDeclaration<'a>>,
        )> = Vec::new();
        for (index, statement) in program.body.iter().enumerate() {
            let Statement::ImportDeclaration(declaration) = statement else {
                continue;
            };
            let span = declaration.span;
            if guard.used.contains(&span)
                && !guard.kept.contains(&span)
                && !has_value_use(declaration, &fresh)
            {
                let mut side_effect = declaration.clone_in(ctx.allocator);
                side_effect.specifiers = None;
                lost.push((index, span, side_effect));
            }
        }
        if lost.is_empty() {
            return false;
        }
        for (index, span, side_effect) in lost.into_iter().rev() {
            guard.kept.insert(span);
            program
                .body
                .insert(index + 1, Statement::ImportDeclaration(side_effect));
            ctx.log.applied(self.name(), Some(span));
        }
        true
    }
}
