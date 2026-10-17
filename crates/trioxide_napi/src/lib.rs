//! Node-API binding for trioxide (napi-rs v3).
//!
//! Exposes [`transform`] for one-off transforms and the [`Project`] class for the
//! whole-program API the Vite plugin uses. Options are passed as plain JS objects in the
//! `trioxide.json` shape and validated by serde; results are plain objects. The typed
//! JavaScript API lives in `packages/trioxide`.

#![expect(
    clippy::needless_pass_by_value,
    reason = "napi-rs passes arguments by value"
)]

use std::path::{Path, PathBuf};

use napi::bindgen_prelude::{Buffer, Error, Result};
use napi_derive::napi;
use serde_json::Value;

/// One source range of a diagnostic.
#[napi(object)]
pub struct Label {
    /// Byte offset where the range starts.
    pub start: u32,
    /// Byte offset where the range ends.
    pub end: u32,
    /// Text shown next to the range.
    pub message: Option<String>,
}

/// An error, warning or note.
#[napi(object)]
pub struct Diagnostic {
    /// `"error"`, `"warning"` or `"advice"`.
    pub severity: String,
    /// A stable code such as `trioxide(unresolved-import)`.
    pub code: Option<String>,
    /// The headline.
    pub message: String,
    /// The file the labels point into.
    pub file: Option<String>,
    /// Source ranges, primary first.
    pub labels: Vec<Label>,
    /// How to fix it.
    pub help: Option<String>,
}

/// A WASM island imported by a module.
#[napi(object)]
pub struct IslandRef {
    /// The island's id.
    pub id: String,
    /// The specifier the module imports its glue by.
    pub specifier: String,
}

/// The compiled form of one module.
#[napi(object)]
pub struct ModuleOutput {
    /// The source path.
    pub path: String,
    /// The JavaScript.
    pub code: String,
    /// Sourcemap v3 JSON.
    pub map: Option<String>,
    /// Islands this module imports.
    pub islands: Vec<IslandRef>,
    /// Errors and warnings.
    pub diagnostics: Vec<Diagnostic>,
}

/// A built WASM island.
#[napi(object)]
pub struct IslandArtifact {
    /// The island's id.
    pub id: String,
    /// The compiled module.
    pub wasm: Buffer,
    /// The ESM glue module.
    pub glue: String,
}

/// The result of a whole-program build.
#[napi(object)]
pub struct BuildOutput {
    /// Every module in the graph.
    pub modules: Vec<ModuleOutput>,
    /// Built islands.
    pub islands: Vec<IslandArtifact>,
    /// The decision log, as JSON-compatible data.
    pub report: Value,
}

/// What an update affected.
#[napi(object)]
pub struct Invalidation {
    /// The module whose source changed, if it is in the graph.
    pub changed: Option<String>,
    /// Modules that import it, directly or transitively.
    pub dependants: Vec<String>,
}

/// Transforms one module given its source. `filename` locates the module (imports and the
/// tsconfig are resolved relative to it); it need not exist on disk.
#[napi]
pub fn transform(code: String, filename: String, options: Option<Value>) -> Result<ModuleOutput> {
    let options = parse_options(options)?;
    let path = absolute(Path::new(&filename))?;
    let root = path
        .parent()
        .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
    let mut project = trioxide::Project::new(root, options).map_err(to_napi)?;
    project.update(&path, Some(code)).map_err(to_napi)?;
    project.transform(&path).map(module_output).map_err(to_napi)
}

/// The JSON schema of the options object (`trioxide.json`).
#[napi]
pub fn options_schema() -> Value {
    trioxide::Options::json_schema()
}

/// Whether this build embeds the tsrs type checker.
#[napi]
pub fn has_tsrs() -> bool {
    trioxide::HAS_TSRS
}

/// A trioxide project: the module graph plus caches, kept alive across calls.
#[napi]
pub struct Project {
    inner: trioxide::Project,
}

#[napi]
impl Project {
    /// Creates a project rooted at `root` with `options` (the `trioxide.json` shape).
    #[napi(constructor)]
    pub fn new(root: String, options: Option<Value>) -> Result<Self> {
        let options = parse_options(options)?;
        let inner =
            trioxide::Project::new(absolute(Path::new(&root))?, options).map_err(to_napi)?;
        Ok(Self { inner })
    }

    /// The canonical project root.
    #[napi(getter)]
    pub fn root(&self) -> String {
        self.inner.root().display().to_string()
    }

    /// Compiles the whole program reachable from `entries`.
    #[napi]
    pub fn build(&mut self, entries: Vec<String>) -> Result<BuildOutput> {
        let entries: Vec<PathBuf> = entries.into_iter().map(PathBuf::from).collect();
        let output = self.inner.build(&entries).map_err(to_napi)?;
        Ok(BuildOutput {
            modules: output.modules.into_iter().map(module_output).collect(),
            islands: output
                .islands
                .into_iter()
                .map(|island| IslandArtifact {
                    id: island.spec.id.0,
                    wasm: island.wasm.into(),
                    glue: island.glue,
                })
                .collect(),
            report: serde_json::to_value(&output.report).map_err(to_napi)?,
        })
    }

    /// Records new contents for `path` (or re-reads it from disk when `source` is omitted).
    #[napi]
    pub fn update(&mut self, path: String, source: Option<String>) -> Result<Invalidation> {
        let invalidation = self
            .inner
            .update(Path::new(&path), source)
            .map_err(to_napi)?;
        Ok(Invalidation {
            changed: invalidation.changed.map(|path| path.display().to_string()),
            dependants: invalidation
                .dependants
                .into_iter()
                .map(|path| path.display().to_string())
                .collect(),
        })
    }

    /// The compiled form of one module.
    #[napi]
    pub fn transform(&mut self, path: String) -> Result<ModuleOutput> {
        self.inner
            .transform(Path::new(&path))
            .map(module_output)
            .map_err(to_napi)
    }

    /// The decision log for `target` (`file` or `file:function`; everything if omitted).
    #[napi]
    pub fn explain(&self, target: Option<String>) -> Result<Value> {
        let query: trioxide::ExplainQuery = target
            .as_deref()
            .unwrap_or_default()
            .parse()
            .unwrap_or_default();
        serde_json::to_value(self.inner.explain(&query)).map_err(to_napi)
    }
}

fn parse_options(options: Option<Value>) -> Result<trioxide::Options> {
    match options {
        None | Some(Value::Null) => Ok(trioxide::Options::default()),
        Some(value) => serde_json::from_value(value)
            .map_err(|error| Error::from_reason(format!("invalid trioxide options: {error}"))),
    }
}

fn absolute(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir().map_err(to_napi)?.join(path))
}

fn to_napi(error: impl std::fmt::Display) -> Error {
    Error::from_reason(error.to_string())
}

fn module_output(output: trioxide::ModuleOutput) -> ModuleOutput {
    ModuleOutput {
        path: output.path.display().to_string(),
        code: output.code,
        map: output.map.map(trioxide::SourceMap::into_json),
        islands: output
            .islands
            .into_iter()
            .map(|island| IslandRef {
                id: island.id.0,
                specifier: island.specifier,
            })
            .collect(),
        diagnostics: output.diagnostics.into_iter().map(diagnostic).collect(),
    }
}

fn diagnostic(diagnostic: trioxide::Diagnostic) -> Diagnostic {
    Diagnostic {
        severity: match diagnostic.severity {
            trioxide::Severity::Error => "error",
            trioxide::Severity::Warning => "warning",
            trioxide::Severity::Advice => "advice",
        }
        .to_string(),
        code: diagnostic.code,
        message: diagnostic.message,
        file: diagnostic.file.map(|file| file.display().to_string()),
        labels: diagnostic
            .labels
            .into_iter()
            .map(|label| Label {
                start: label.span.start,
                end: label.span.end,
                message: label.message,
            })
            .collect(),
        help: diagnostic.help,
    }
}
