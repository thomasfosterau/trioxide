//! The per-module pipeline: parse → semantic → passes → TS strip → codegen.

use std::path::Path;

use oxc_allocator::Allocator;
use oxc_diagnostics::OxcDiagnostic;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use trioxide_common::{Diagnostic, ModuleId, log::ModuleLog};
use trioxide_js::{EmitOptions, FactSource, PassManager, PassOptions, emit_with};

use crate::{ModuleOutput, SourceMap, islands::Rewrite};

/// One module to compile.
#[derive(Clone, Copy)]
pub(crate) struct ModuleSource<'s> {
    pub id: ModuleId,
    pub path: &'s Path,
    pub source: &'s str,
    pub source_type: SourceType,
}

/// Compiles single modules with fixed settings.
pub(crate) struct ModuleCompiler {
    pub emit: EmitOptions,
    pub passes: PassOptions,
}

impl ModuleCompiler {
    /// Compiles one module. With `optimise` off, only strips types (the differential suite's
    /// baseline). `rewrite` points calls of WASM island exports at their glue.
    pub fn compile(
        &self,
        input: &ModuleSource<'_>,
        facts: &mut dyn FactSource,
        log: &mut ModuleLog,
        optimise: bool,
        rewrite: Option<&Rewrite<'_>>,
    ) -> ModuleOutput {
        let ModuleSource {
            id,
            path,
            source,
            source_type,
        } = *input;
        let file = Some(path.to_path_buf());
        let to_diagnostics = |errors: &[OxcDiagnostic]| -> Vec<Diagnostic> {
            errors
                .iter()
                .map(|error| Diagnostic::from_oxc(error, file.clone()))
                .collect()
        };
        let failed = |diagnostics| ModuleOutput {
            path: path.to_path_buf(),
            code: String::new(),
            map: None,
            islands: Vec::new(),
            diagnostics,
        };

        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, source_type).parse();
        let mut diagnostics = to_diagnostics(&parsed.diagnostics);
        if parsed.fatal_error || diagnostics.iter().any(Diagnostic::is_error) {
            return failed(diagnostics);
        }
        let mut program = parsed.program;

        let semantic = SemanticBuilder::new()
            .with_enum_eval(true)
            .with_check_syntax_error(true)
            .build(&program);
        diagnostics.extend(to_diagnostics(&semantic.diagnostics));
        if diagnostics.iter().any(Diagnostic::is_error) {
            return failed(diagnostics);
        }
        let mut scoping = semantic.semantic.into_scoping();

        if optimise {
            let mut passes = PassManager::from_options(self.passes.clone());
            if !passes.is_empty() {
                scoping = passes.run(&allocator, &mut program, scoping, id, facts, log);
            }
        }

        let mut islands = Vec::new();
        let out = emit_with(
            &allocator,
            &mut program,
            scoping,
            path,
            &self.emit,
            &mut |allocator, program, scoping| {
                if let Some(rewrite) = rewrite {
                    islands = crate::islands::rewrite_calls(allocator, program, scoping, rewrite);
                }
            },
        );
        diagnostics.extend(out.diagnostics);
        ModuleOutput {
            path: path.to_path_buf(),
            code: out.code,
            map: out.map.map(SourceMap::from_json),
            islands,
            diagnostics,
        }
    }
}
