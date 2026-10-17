use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use trioxide_common::{Diagnostic, log::ModuleLog};

/// The decision log of a build: per module, what each function became and which passes fired
/// (PLAN §2 step 8). `trioxide explain` and `--emit=report` print it; it serialises to JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// The type oracle in use (`"noop"` until the tsrs adapter lands).
    pub type_oracle: String,
    /// Per-module logs, in graph order.
    pub modules: Vec<ModuleReport>,
    /// Problems not tied to one module (configuration, toolchain).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

/// The decision log of one module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleReport {
    /// The module's path.
    pub path: PathBuf,
    /// What was decided.
    #[serde(flatten)]
    pub log: ModuleLog,
}

impl Report {
    /// The part of the report `query` asks about.
    #[must_use]
    pub fn filter(&self, query: &ExplainQuery) -> Self {
        let modules = self
            .modules
            .iter()
            .filter(|module| {
                query
                    .file
                    .as_ref()
                    .is_none_or(|file| path_matches(&module.path, file))
            })
            .map(|module| {
                let mut module = module.clone();
                if let Some(function) = &query.function {
                    module
                        .log
                        .functions
                        .retain(|decision| decision.name.as_deref() == Some(function.as_str()));
                }
                module
            })
            .collect();
        Self {
            type_oracle: self.type_oracle.clone(),
            modules,
            diagnostics: self.diagnostics.clone(),
        }
    }
}

/// Matches an absolute module path against a user-supplied (possibly relative) path.
fn path_matches(module: &Path, query: &Path) -> bool {
    if query.is_absolute() {
        return module == query;
    }
    std::fs::canonicalize(query)
        .map_or_else(|_| module.ends_with(query), |canonical| canonical == module)
}

/// What `trioxide explain [file[:fn]]` asks about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExplainQuery {
    /// Only this module (absolute, or relative to the working directory).
    pub file: Option<PathBuf>,
    /// Only functions with this name.
    pub function: Option<String>,
}

impl FromStr for ExplainQuery {
    type Err = std::convert::Infallible;

    /// Parses `file`, `file:function` or the empty string (everything).
    fn from_str(target: &str) -> Result<Self, Self::Err> {
        if target.is_empty() {
            return Ok(Self::default());
        }
        // The function part is an identifier, which keeps `C:\path` intact on Windows.
        if let Some((file, function)) = target.rsplit_once(':')
            && !function.is_empty()
            && function
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
            && !file.is_empty()
        {
            return Ok(Self {
                file: Some(file.into()),
                function: Some(function.into()),
            });
        }
        Ok(Self {
            file: Some(target.into()),
            function: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_explain_targets() {
        assert_eq!("".parse::<ExplainQuery>().unwrap(), ExplainQuery::default());
        assert_eq!(
            "src/a.ts:blur".parse::<ExplainQuery>().unwrap(),
            ExplainQuery {
                file: Some("src/a.ts".into()),
                function: Some("blur".into())
            }
        );
        assert_eq!(
            "src/a.ts".parse::<ExplainQuery>().unwrap(),
            ExplainQuery {
                file: Some("src/a.ts".into()),
                function: None
            }
        );
        assert_eq!(
            r"C:\src\a.ts".parse::<ExplainQuery>().unwrap(),
            ExplainQuery {
                file: Some(r"C:\src\a.ts".into()),
                function: None
            }
        );
    }
}
