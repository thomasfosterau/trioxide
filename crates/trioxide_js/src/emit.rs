use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_semantic::Scoping;
use oxc_transformer::{JsxRuntime, RewriteExtensionsMode, TransformOptions, Transformer};
use trioxide_common::Diagnostic;
use trioxide_project::{CompilerSettings, JsxMode};

/// How to strip and print a module.
#[derive(Debug, Clone, Default)]
pub struct EmitOptions {
    /// The `oxc_transformer` configuration, usually from [`transform_options`].
    pub transform: TransformOptions,
    /// Whether to produce a sourcemap.
    pub source_map: bool,
}

/// A printed module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitOutput {
    /// The JavaScript (or JSX, with `jsx: preserve`).
    pub code: String,
    /// Sourcemap v3 JSON, if requested.
    pub map: Option<String>,
    /// Transformer errors and warnings.
    pub diagnostics: Vec<Diagnostic>,
}

/// Maps tsconfig settings onto `oxc_transformer` options, the way `tsc` would emit.
///
/// Returns warnings for settings that can't be honoured exactly (e.g. a `target` below
/// ES2015, which the transformer doesn't support and is raised to ES2015).
pub fn transform_options(settings: &CompilerSettings) -> (TransformOptions, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let mut options = match settings.target.as_deref() {
        None | Some("esnext") => TransformOptions::default(),
        Some(target @ ("es3" | "es5")) => {
            diagnostics.push(
                Diagnostic::warning(format!(
                    "tsconfig target \"{target}\" is not supported; using es2015"
                ))
                .with_code("trioxide(target)"),
            );
            TransformOptions::from_target("es2015").expect("es2015 is a valid target")
        }
        Some(target) => TransformOptions::from_target(target).unwrap_or_else(|error| {
            diagnostics.push(
                Diagnostic::warning(format!(
                    "unrecognised tsconfig target \"{target}\" ({error}); not lowering syntax"
                ))
                .with_code("trioxide(target)"),
            );
            TransformOptions::default()
        }),
    };

    options.typescript.only_remove_type_imports = settings.verbatim_module_syntax;
    if settings.rewrite_relative_import_extensions {
        options.typescript.rewrite_import_extensions = Some(RewriteExtensionsMode::Rewrite);
    }
    if !settings.use_define_for_class_fields {
        options.typescript.remove_class_fields_without_initializer = true;
        options.assumptions.set_public_class_fields = true;
    }
    options.decorator.legacy = settings.experimental_decorators;
    options.decorator.emit_decorator_metadata = settings.emit_decorator_metadata;
    options.decorator.strict_null_checks = settings.strict_null_checks;

    let jsx = &mut options.jsx;
    match settings.jsx {
        JsxMode::Preserve => jsx.jsx_plugin = false,
        JsxMode::Classic => {
            jsx.runtime = JsxRuntime::Classic;
            jsx.pragma.clone_from(&settings.jsx_factory);
            jsx.pragma_frag.clone_from(&settings.jsx_fragment_factory);
        }
        JsxMode::Automatic | JsxMode::AutomaticDev => {
            jsx.runtime = JsxRuntime::Automatic;
            jsx.development = settings.jsx == JsxMode::AutomaticDev;
            jsx.import_source.clone_from(&settings.jsx_import_source);
        }
    }
    if let Some(factory) = &settings.jsx_factory {
        options.typescript.jsx_pragma = factory.clone().into();
    }
    if let Some(fragment) = &settings.jsx_fragment_factory {
        options.typescript.jsx_pragma_frag = fragment.clone().into();
    }
    (options, diagnostics)
}

/// Strips TypeScript from `program` and prints it (PLAN §2 step 6).
///
/// `scoping` must match `program` (from [`rebuild_scoping`](crate::rebuild_scoping) or the
/// pass manager). `path` names the module in the sourcemap and decides JSX handling.
pub fn emit<'a>(
    allocator: &'a Allocator,
    program: &mut Program<'a>,
    scoping: Scoping,
    path: &Path,
    options: &EmitOptions,
) -> EmitOutput {
    emit_with(allocator, program, scoping, path, options, &mut |_, _, _| {})
}

/// [`emit`], running `after_strip` on the JavaScript AST between stripping TypeScript and
/// printing. The facade uses it to point call sites at WASM island glue; nodes it adds need
/// no semantic ids (printing falls back to their names).
pub fn emit_with<'a>(
    allocator: &'a Allocator,
    program: &mut Program<'a>,
    scoping: Scoping,
    path: &Path,
    options: &EmitOptions,
    after_strip: &mut dyn FnMut(&'a Allocator, &mut Program<'a>, &Scoping),
) -> EmitOutput {
    let transformed =
        Transformer::new(allocator, path, &options.transform).build_with_scoping(scoping, program);
    after_strip(allocator, program, &transformed.scoping);
    let diagnostics = transformed
        .diagnostics
        .iter()
        .map(|diagnostic| Diagnostic::from_oxc(diagnostic, Some(path.to_path_buf())))
        .collect();

    let codegen_options = CodegenOptions {
        source_map_path: options.source_map.then(|| path.to_path_buf()),
        ..CodegenOptions::default()
    };
    let printed = Codegen::new()
        .with_options(codegen_options)
        .with_source_text(program.source_text)
        .with_scoping(Some(transformed.scoping))
        .build(program);
    EmitOutput {
        code: printed.code,
        map: printed.map.map(|map| map.to_json_string()),
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use oxc_parser::Parser;
    use oxc_span::SourceType;

    use super::*;
    use crate::rebuild_scoping;

    fn strip(source: &str, path: &str, settings: &CompilerSettings) -> EmitOutput {
        let allocator = Allocator::default();
        let path = Path::new(path);
        let mut program = Parser::new(&allocator, source, SourceType::from_path(path).unwrap())
            .parse()
            .program;
        let scoping = rebuild_scoping(&program);
        let (transform, warnings) = transform_options(settings);
        assert_eq!(warnings, Vec::new());
        emit(
            &allocator,
            &mut program,
            scoping,
            path,
            &EmitOptions {
                transform,
                source_map: true,
            },
        )
    }

    #[test]
    fn strips_types_and_maps() {
        let out = strip(
            "const x: number = 1;\nexport type T = string;\n",
            "/a.ts",
            &CompilerSettings::without_tsconfig(),
        );
        assert_eq!(out.code, "const x = 1;\nexport {};\n");
        let map = out.map.expect("sourcemap requested");
        assert!(map.contains("\"mappings\""), "{map}");
    }

    #[test]
    fn classic_jsx_uses_factory() {
        let settings = CompilerSettings {
            jsx: JsxMode::Classic,
            jsx_factory: Some("h".into()),
            jsx_fragment_factory: Some("Fragment".into()),
            ..CompilerSettings::without_tsconfig()
        };
        let out = strip(
            "export const a = <><b x=\"1\" /></>;\n",
            "/a.tsx",
            &settings,
        );
        assert_eq!(
            out.code,
            "export const a = /* @__PURE__ */ h(Fragment, null, /* @__PURE__ */ h(\"b\", { x: \"1\" }));\n"
        );
    }

    #[test]
    fn define_semantics_off_drops_uninitialised_fields() {
        let settings = CompilerSettings {
            use_define_for_class_fields: false,
            ..CompilerSettings::without_tsconfig()
        };
        let out = strip("class A { x: number; }\n", "/a.ts", &settings);
        assert_eq!(out.code, "class A {}\n");
    }
}
