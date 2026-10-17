use std::path::{Path, PathBuf};

use oxc_resolver::{ResolveOptions, Resolver};
use serde::{Deserialize, Serialize};

use crate::ProjectError;

/// How JSX is compiled (`compilerOptions.jsx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JsxMode {
    /// Leave JSX in the output (`preserve`, `react-native`).
    Preserve,
    /// `React.createElement`-style calls to `jsxFactory` (`react`).
    Classic,
    /// The automatic runtime, `jsx()` from `<jsxImportSource>/jsx-runtime` (`react-jsx`).
    #[default]
    Automatic,
    /// The automatic development runtime (`react-jsxdev`).
    AutomaticDev,
}

/// The `tsconfig.json` compiler options that change trioxide's output (PLAN D11).
///
/// Everything else in `tsconfig.json` either doesn't affect emit or is read by the tsrs
/// adapter directly. Module resolution (`paths`, `baseUrl`) is handled by the resolver.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompilerSettings {
    /// The tsconfig these came from, if any.
    pub path: Option<PathBuf>,
    /// `target`, lowercased (e.g. `"es2022"`, `"esnext"`). `None` means no syntax lowering.
    pub target: Option<String>,
    /// `jsx`.
    pub jsx: JsxMode,
    /// `jsxFactory`, for [`JsxMode::Classic`].
    pub jsx_factory: Option<String>,
    /// `jsxFragmentFactory`, for [`JsxMode::Classic`].
    pub jsx_fragment_factory: Option<String>,
    /// `jsxImportSource`, for the automatic runtimes.
    pub jsx_import_source: Option<String>,
    /// `useDefineForClassFields`, resolved against `target` as TypeScript does.
    pub use_define_for_class_fields: bool,
    /// `experimentalDecorators`.
    pub experimental_decorators: bool,
    /// `emitDecoratorMetadata`.
    pub emit_decorator_metadata: bool,
    /// `verbatimModuleSyntax`: only `import type` is elided.
    pub verbatim_module_syntax: bool,
    /// `rewriteRelativeImportExtensions`: `./a.ts` imports are emitted as `./a.js`.
    pub rewrite_relative_import_extensions: bool,
    /// `strictNullChecks` (or `strict`), which changes emitted decorator metadata. Defaults to
    /// true, as in TypeScript 7.
    pub strict_null_checks: bool,
}

impl CompilerSettings {
    /// Loads the settings from `tsconfig` (following `extends`).
    pub fn load(tsconfig: &Path) -> Result<Self, ProjectError> {
        let resolver = Resolver::new(ResolveOptions::default());
        let config =
            resolver
                .resolve_tsconfig(tsconfig)
                .map_err(|error| ProjectError::Tsconfig {
                    path: tsconfig.to_path_buf(),
                    message: error.to_string(),
                })?;
        let options = &config.compiler_options;
        let target = options
            .target
            .as_ref()
            .map(|target| target.to_ascii_lowercase());
        let use_define_for_class_fields = options
            .use_define_for_class_fields
            .unwrap_or_else(|| target_defaults_to_define(target.as_deref()));
        let jsx = match options
            .jsx
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("preserve" | "react-native") => JsxMode::Preserve,
            Some("react") => JsxMode::Classic,
            Some("react-jsxdev") => JsxMode::AutomaticDev,
            _ => JsxMode::Automatic,
        };
        Ok(Self {
            path: Some(tsconfig.to_path_buf()),
            target,
            jsx,
            jsx_factory: options.jsx_factory.clone(),
            jsx_fragment_factory: options.jsx_fragment_factory.clone(),
            jsx_import_source: options.jsx_import_source.clone(),
            use_define_for_class_fields,
            experimental_decorators: options.experimental_decorators.unwrap_or(false),
            emit_decorator_metadata: options.emit_decorator_metadata.unwrap_or(false),
            verbatim_module_syntax: options.verbatim_module_syntax.unwrap_or(false),
            rewrite_relative_import_extensions: options
                .rewrite_relative_import_extensions
                .unwrap_or(false),
            strict_null_checks: options
                .strict_null_checks
                .or(options.strict)
                .unwrap_or(true),
        })
    }

    /// The settings used when there is no tsconfig: no lowering, automatic JSX runtime.
    pub fn without_tsconfig() -> Self {
        Self {
            use_define_for_class_fields: true,
            strict_null_checks: true,
            ..Self::default()
        }
    }
}

/// TypeScript defaults `useDefineForClassFields` to true for ES2022 and later.
fn target_defaults_to_define(target: Option<&str>) -> bool {
    match target {
        None | Some("esnext") => true,
        Some(target) => target
            .strip_prefix("es")
            .and_then(|year| year.parse::<u32>().ok())
            .is_some_and(|year| year >= 2022),
    }
}

/// The nearest `tsconfig.json` in `dir` or one of its ancestors, as `tsc` finds it.
pub fn find_tsconfig(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .map(|dir| dir.join("tsconfig.json"))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn define_semantics_follow_target() {
        assert!(target_defaults_to_define(None));
        assert!(target_defaults_to_define(Some("esnext")));
        assert!(target_defaults_to_define(Some("es2022")));
        assert!(!target_defaults_to_define(Some("es2021")));
        assert!(!target_defaults_to_define(Some("es5")));
    }
}
