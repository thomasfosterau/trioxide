use std::path::{Path, PathBuf};

use trioxide::Options;

/// The nearest `trioxide.json` / `trioxide.jsonc` at or above `start`.
///
/// The search stops at the repository root (the first directory with a `.git` entry), so a
/// config in an unrelated parent directory is never picked up.
pub fn discover_config(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        if let Some(config) = Options::find_config(dir) {
            return Some(config);
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    None
}

/// Reads and validates the config file at `path`, returning it as written (without the
/// defaults filled in), so callers can merge it with options of their own.
pub fn read_config(path: &Path) -> trioxide::Result<serde_json::Value> {
    Options::from_file(path)?;
    let mut text = std::fs::read_to_string(path).map_err(|source| trioxide::Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let _ = json_strip_comments::strip(&mut text);
    serde_json::from_str(&text).map_err(|error| trioxide::Error::Config {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_walks_up_to_the_repository_root() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("repo/packages/app/src");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir(dir.path().join("repo/.git")).unwrap();
        assert_eq!(discover_config(&nested), None);

        std::fs::write(dir.path().join("trioxide.json"), "{}").unwrap();
        assert_eq!(discover_config(&nested), None, "outside the repository");

        let config = dir.path().join("repo/packages/trioxide.jsonc");
        std::fs::write(&config, "{ /* c */ \"check\": true, }").unwrap();
        assert_eq!(discover_config(&nested), Some(config.clone()));
        assert_eq!(
            read_config(&config).unwrap(),
            serde_json::json!({ "check": true })
        );
    }

    #[test]
    fn read_config_validates() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("trioxide.json");
        std::fs::write(&config, r#"{ "mdoe": "auto" }"#).unwrap();
        let error = read_config(&config).unwrap_err().to_string();
        assert!(error.contains("unknown field `mdoe`"), "{error}");
    }
}
