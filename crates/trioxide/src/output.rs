use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use trioxide_common::{Diagnostic, Severity};
use trioxide_wasm::{IslandArtifact, IslandRef};

use crate::Report;

/// A sourcemap (v3), kept as its JSON text.
///
/// The JSON is what every consumer wants (Vite's `transform` hook, files next to the output),
/// and it keeps `oxc_sourcemap`'s types out of the public API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceMap(String);

impl SourceMap {
    /// Wraps sourcemap v3 JSON.
    pub fn from_json(json: String) -> Self {
        Self(json)
    }

    /// The sourcemap as JSON.
    pub fn as_json(&self) -> &str {
        &self.0
    }

    /// The sourcemap as JSON, by value.
    pub fn into_json(self) -> String {
        self.0
    }
}

/// The compiled form of one module (PLAN §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleOutput {
    /// The module's source path.
    pub path: PathBuf,
    /// The JavaScript. Empty if the module had errors that prevented compiling it.
    pub code: String,
    /// Its sourcemap, back to the original TypeScript, if sourcemaps are on.
    pub map: Option<SourceMap>,
    /// Islands this module's code imports (their glue modules).
    pub islands: Vec<IslandRef>,
    /// Errors and warnings for this module. Check [`ModuleOutput::has_errors`].
    pub diagnostics: Vec<Diagnostic>,
}

impl ModuleOutput {
    /// Whether any diagnostic is an error; if so `code` may be empty or incomplete.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

/// The result of a whole-program build (PLAN §7).
#[derive(Debug, Clone)]
pub struct BuildOutput {
    /// Every module in the graph, in discovery order (entries first).
    pub modules: Vec<ModuleOutput>,
    /// Built WASM islands with their glue. Islands that could not be built are missing here
    /// and reported in [`Report::diagnostics`]; their functions stay JS.
    pub islands: Vec<IslandArtifact>,
    /// The decision log.
    pub report: Report,
}

impl BuildOutput {
    /// Whether any module or the build itself reported an error.
    pub fn has_errors(&self) -> bool {
        self.modules.iter().any(ModuleOutput::has_errors)
            || self.report.diagnostics.iter().any(Diagnostic::is_error)
    }
}

/// What [`Project::update`](crate::Project::update) affected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invalidation {
    /// The module whose source changed, if it is part of the graph.
    pub changed: Option<PathBuf>,
    /// Modules that import it, directly or transitively. Their outputs may change too (e.g.
    /// cross-module constant inlining), so a dev server should invalidate them.
    pub dependants: Vec<PathBuf>,
}
