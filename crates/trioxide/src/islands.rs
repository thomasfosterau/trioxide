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
