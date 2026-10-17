//! An in-memory overlay over the real file system, so tsrs sees the sources trioxide compiles
//! (which may differ from disk, e.g. unsaved Vite edits) and reads everything else
//! (`tsconfig.json`, `node_modules` declarations) from disk.

use std::{
    sync::{Arc, RwLock},
    time::SystemTime,
};

use rustc_hash::FxHashMap;
use tsrs_core::tspath;
use tsrs_vfs::{Entries, FS, FileInfo, FileMode, osvfs};

/// Files held in memory, by normalised absolute path. Writes from tsrs are refused: trioxide
/// never emits through the checker.
pub struct OverlayFs {
    files: RwLock<FxHashMap<String, Arc<str>>>,
    base: Option<&'static dyn FS>,
    case_sensitive: bool,
}

impl OverlayFs {
    /// An overlay over the OS file system.
    pub fn over_os() -> Self {
        let base = osvfs::fs();
        Self {
            files: RwLock::default(),
            case_sensitive: base.use_case_sensitive_file_names(),
            base: Some(base),
        }
    }

    /// Sets the in-memory contents of `path` (a normalised absolute path).
    pub fn set(&self, path: &str, source: Arc<str>) {
        let key = self.key(path);
        self.files
            .write()
            .expect("overlay lock")
            .insert(key, source);
    }

    fn key(&self, path: &str) -> String {
        let path = tspath::normalize_path(path);
        if self.case_sensitive {
            path
        } else {
            path.to_lowercase()
        }
    }

    fn get(&self, path: &str) -> Option<Arc<str>> {
        self.files
            .read()
            .expect("overlay lock")
            .get(&self.key(path))
            .cloned()
    }

    /// Whether some overlay file lives below `dir`.
    fn has_below(&self, dir: &str) -> bool {
        let mut prefix = self.key(dir);
        if !prefix.ends_with('/') {
            prefix.push('/');
        }
        self.files
            .read()
            .expect("overlay lock")
            .keys()
            .any(|key| key.starts_with(&prefix))
    }
}

impl FS for OverlayFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.case_sensitive
    }

    fn file_exists(&self, path: &str) -> bool {
        self.get(path).is_some() || self.base.is_some_and(|base| base.file_exists(path))
    }

    fn read_file(&self, path: &str) -> Option<String> {
        match self.get(path) {
            Some(source) => Some(source.to_string()),
            None => self.base?.read_file(path),
        }
    }

    fn write_file(&self, path: &str, _data: &str) -> Result<(), String> {
        Err(format!(
            "trioxide's type checker does not write files ({path})"
        ))
    }

    fn append_file(&self, path: &str, _data: &str) -> Result<(), String> {
        Err(format!(
            "trioxide's type checker does not write files ({path})"
        ))
    }

    fn remove(&self, path: &str) -> Result<(), String> {
        Err(format!(
            "trioxide's type checker does not remove files ({path})"
        ))
    }

    fn chtimes(&self, path: &str, _a: SystemTime, _m: SystemTime) -> Result<(), String> {
        Err(format!(
            "trioxide's type checker does not touch files ({path})"
        ))
    }

    fn directory_exists(&self, path: &str) -> bool {
        self.has_below(path) || self.base.is_some_and(|base| base.directory_exists(path))
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        let mut entries = self
            .base
            .map(|base| base.get_accessible_entries(path))
            .unwrap_or_default();
        let mut prefix = self.key(path);
        if !prefix.ends_with('/') {
            prefix.push('/');
        }
        let files = self.files.read().expect("overlay lock");
        let mut keys: Vec<&String> = files.keys().collect();
        keys.sort();
        for key in keys {
            let Some(rest) = key.strip_prefix(&prefix) else {
                continue;
            };
            let (name, list) = match rest.split_once('/') {
                Some((dir, _)) => (dir, &mut entries.directories),
                None => (rest, &mut entries.files),
            };
            if !list.iter().any(|existing| existing == name) {
                list.push(name.to_string());
            }
        }
        entries
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        if let Some(source) = self.get(path) {
            return Some(FileInfo {
                name: tspath::get_base_file_name(path),
                size: i64::try_from(source.len()).unwrap_or(i64::MAX),
                mode: FileMode::None,
                mod_time: None,
            });
        }
        if let Some(info) = self.base.and_then(|base| base.stat(path)) {
            return Some(info);
        }
        self.has_below(path).then(|| FileInfo {
            name: tspath::get_base_file_name(path),
            size: 0,
            mode: FileMode::Dir,
            mod_time: None,
        })
    }

    fn realpath(&self, path: &str) -> String {
        if self.get(path).is_some() || self.has_below(path) {
            return tspath::normalize_path(path);
        }
        match self.base {
            Some(base) => base.realpath(path),
            None => tspath::normalize_path(path),
        }
    }
}
