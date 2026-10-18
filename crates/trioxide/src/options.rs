use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Names of the config files trioxide looks for, in order of preference.
pub const CONFIG_FILE_NAMES: [&str; 2] = ["trioxide.json", "trioxide.jsonc"];

/// Everything that configures a trioxide build.
///
/// Read from `trioxide.json` / `trioxide.jsonc` (see [`Options::from_file`]), merged with Vite
/// plugin options or CLI flags. Source directives (`"use wasm"` / `"use js"`,
/// `/** @trioxide wasm|js */`) override all of it. Field names are camelCase in JSON. The JSON
/// schema in `schema/trioxide.schema.json` is generated from this type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Options {
    /// The JSON schema reference, allowed so editors can validate config files. Ignored.
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    /// How aggressively functions move to WebAssembly.
    pub mode: Mode,

    /// Whether the whole program is visible (`app`) or exports may be called from unknown code
    /// (`library`). Unset: `app` when building from entries (Vite builds, `trioxide build`),
    /// `library` for single-file transforms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub world: Option<World>,

    /// Treat TypeScript types that rest on annotations, casts or `any` as proof. Unsound if
    /// the types lie; off by default.
    pub trust_types: bool,

    /// Report TypeScript type errors as diagnostics.
    pub check: bool,

    /// The `tsconfig.json` to use. Unset: the nearest one above the project root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tsconfig: Option<PathBuf>,

    /// Override the tsconfig's `target` (e.g. `"es2022"`, `"esnext"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,

    /// Generate sourcemaps.
    pub sourcemap: bool,

    /// Turn individual optimisation passes on or off by name, e.g. `{ "fold": false }`. Passes
    /// not listed use their defaults. `"specialize": true` also lets passes fold facts that
    /// hold only because every caller passes the same values into function bodies (off by
    /// default: it makes output harder to read).
    pub passes: BTreeMap<String, bool>,

    /// WebAssembly output.
    pub wasm: WasmOptions,

    /// Settings that apply only to the Vite dev server.
    pub dev: DevOptions,

    /// Where to cache compiled islands. Unset: `node_modules/.trioxide` under the root, or the
    /// system cache directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<PathBuf>,

    /// Per-file overrides, applied in order (later wins).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<Override>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            schema: None,
            mode: Mode::default(),
            world: None,
            trust_types: false,
            check: false,
            tsconfig: None,
            target: None,
            sourcemap: true,
            passes: BTreeMap::new(),
            wasm: WasmOptions::default(),
            dev: DevOptions::default(),
            cache_dir: None,
            overrides: Vec::new(),
        }
    }
}

/// How aggressively functions move to WebAssembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Never create WASM islands; only optimise JS.
    JsOnly,
    /// Create islands the cost model says are profitable.
    #[default]
    Auto,
    /// Prefer WASM for every eligible function unless the cost model strongly objects.
    WasmPreferred,
}

/// The world assumption: what code outside the module graph may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum World {
    /// Closed world: the whole module graph is visible and exports are only used internally.
    App,
    /// Open world: exported functions may be called with anything their types allow.
    Library,
}

/// WebAssembly settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct WasmOptions {
    /// Produce WASM islands at all. `--no-wasm` turns this off.
    pub enabled: bool,
    /// Fail the build if the Rust toolchain is missing instead of falling back to JS.
    pub require: bool,
    /// Run `wasm-opt` on islands when it is on PATH.
    pub wasm_opt: bool,
    /// How island glue waits for instantiation.
    pub glue: GlueMode,
}

impl Default for WasmOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            require: false,
            wasm_opt: true,
            glue: GlueMode::default(),
        }
    }
}

/// How island glue modules wait for their `.wasm` to instantiate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum GlueMode {
    /// Top-level `await` in the glue module: islands run from the first call, but modules
    /// importing them finish evaluating later (after other pending tasks).
    Tla,
    /// No top-level `await`: functions run their JS versions until the island is
    /// instantiated, and the glue exports a `ready()` promise. Module evaluation order is
    /// unchanged.
    #[default]
    Ready,
}

/// Vite dev-server settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct DevOptions {
    /// Build WASM islands in dev too (off by default for fast reloads).
    pub wasm: bool,
}

/// Forces files matching `files` to stay JS or to become WASM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Override {
    /// Glob patterns, relative to the project root.
    pub files: Vec<String>,
    /// What to force.
    pub target: OverrideTarget,
}

/// The target an [`Override`] forces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OverrideTarget {
    /// Like `"use js"` on every function.
    Js,
    /// Like `"use wasm"` on every function; ineligible functions are errors.
    Wasm,
}

impl Options {
    /// Parses options from JSON. Comments and trailing commas are accepted (JSONC).
    pub fn from_json(text: &str) -> serde_json::Result<Self> {
        let mut text = text.to_string();
        // Strips comments and trailing commas in place; only fails on unterminated comments,
        // which serde_json then reports.
        let _ = json_strip_comments::strip(&mut text);
        serde_json::from_str(&text)
    }

    /// Reads options from a `trioxide.json` / `trioxide.jsonc` file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json(&text).map_err(|error| Error::Config {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
    }

    /// The config file in `dir`, if there is one.
    pub fn find_config(dir: &Path) -> Option<PathBuf> {
        CONFIG_FILE_NAMES
            .iter()
            .map(|name| dir.join(name))
            .find(|path| path.is_file())
    }

    /// The world assumption in effect, applying the default described on [`Options::world`].
    pub fn world_or(&self, building_from_entries: bool) -> World {
        self.world.unwrap_or(if building_from_entries {
            World::App
        } else {
            World::Library
        })
    }

    /// The JSON schema for config files.
    pub fn json_schema() -> serde_json::Value {
        let mut schema = schemars::schema_for!(Options).to_value();
        if let Some(object) = schema.as_object_mut() {
            object.insert("title".into(), "trioxide.json".into());
        }
        schema
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonc() {
        let options = Options::from_json(
            r#"{
                // comments are fine
                "$schema": "./node_modules/trioxide/schema.json",
                "mode": "wasm-preferred",
                "world": "library",
                "passes": { "fold": false },
                "wasm": { "glue": "ready" },
                "overrides": [{ "files": ["src/dom/**"], "target": "js" }],
            }"#,
        )
        .unwrap();
        assert_eq!(options.mode, Mode::WasmPreferred);
        assert_eq!(options.world, Some(World::Library));
        assert_eq!(options.passes.get("fold"), Some(&false));
        assert_eq!(
            options.wasm,
            WasmOptions {
                glue: GlueMode::Ready,
                ..WasmOptions::default()
            }
        );
        assert!(options.sourcemap);
        assert_eq!(options.overrides[0].target, OverrideTarget::Js);
    }

    #[test]
    fn rejects_unknown_keys() {
        let error = Options::from_json(r#"{ "mdoe": "auto" }"#).unwrap_err();
        assert!(
            error.to_string().contains("unknown field `mdoe`"),
            "{error}"
        );
    }

    #[test]
    fn default_world_depends_on_entries() {
        let options = Options::default();
        assert_eq!(options.world_or(true), World::App);
        assert_eq!(options.world_or(false), World::Library);
    }

    /// `schema/trioxide.schema.json` must match [`Options`]. Regenerate it with
    /// `TRIOXIDE_UPDATE_SCHEMA=1 cargo test -p trioxide schema`.
    #[test]
    fn schema_is_up_to_date() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/trioxide.schema.json");
        let expected = serde_json::to_string_pretty(&Options::json_schema()).unwrap() + "\n";
        if std::env::var_os("TRIOXIDE_UPDATE_SCHEMA").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &expected).unwrap();
        }
        let actual = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            actual == expected,
            "schema/trioxide.schema.json is stale; rerun with TRIOXIDE_UPDATE_SCHEMA=1"
        );
    }
}
