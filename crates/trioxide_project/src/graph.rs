use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
};

use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_resolver::{
    ResolveOptions, Resolver, TsconfigDiscovery, TsconfigOptions, TsconfigReferences,
};
use oxc_span::{SourceType, Span};
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::{Diagnostic, ModuleId};

use crate::{ContentHash, ProjectError};

/// Extensions of files trioxide compiles. Declaration files (`.d.ts`) are excluded separately.
const MODULE_EXTENSIONS: [&str; 8] = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// Whether `path` is a source file trioxide compiles: TS or JS, not a declaration file, and
/// not inside `node_modules`.
pub fn is_module_path(path: &Path) -> bool {
    let has_module_extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| MODULE_EXTENSIONS.contains(&ext));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    // `x.d.ts`, `x.d.mts`, and arbitrary-extension declarations such as `x.d.css.ts`.
    let is_declaration = name.contains(".d.")
        && Path::new(name).extension().is_some_and(|ext| {
            ["ts", "mts", "cts"]
                .iter()
                .any(|ts| ext.eq_ignore_ascii_case(ts))
        });
    has_module_extension
        && !is_declaration
        && !path.components().any(|c| c.as_os_str() == "node_modules")
}

/// How the module graph resolves imports.
#[derive(Debug, Clone, Default)]
pub struct GraphOptions {
    /// The project root. Relative entry paths are resolved against it.
    pub root: PathBuf,
    /// The `tsconfig.json` whose `paths` / `baseUrl` apply, if any.
    pub tsconfig: Option<PathBuf>,
}

/// How a module refers to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImportKind {
    /// `import … from`, `import "…"`, or `export … from`.
    Static,
    /// `import("…")` with a string literal.
    Dynamic,
}

/// What an import resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportTarget {
    /// A module in this graph.
    Module(ModuleId),
    /// Something trioxide doesn't compile: a package in `node_modules`, a JSON or CSS file, or
    /// a bare specifier that didn't resolve (left to the bundler). `path` is the resolved file
    /// if there is one.
    External {
        /// The resolved file, if resolution succeeded.
        path: Option<PathBuf>,
    },
    /// A relative or absolute specifier that points at nothing.
    Unresolved,
}

/// One import of a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// The specifier as written.
    pub specifier: String,
    /// The specifier string literal's span.
    pub span: Span,
    /// Static or dynamic.
    pub kind: ImportKind,
    /// What it resolved to.
    pub target: ImportTarget,
}

/// One source file in the graph.
#[derive(Debug, Clone)]
pub struct Module {
    /// The module's id.
    pub id: ModuleId,
    /// Absolute path (canonical when the file exists on disk).
    pub path: PathBuf,
    /// The source text trioxide compiles: an in-memory override if set, else the file.
    pub source: Arc<str>,
    /// Hash of `source`.
    pub hash: ContentHash,
    /// Language and module kind, from the extension.
    pub source_type: SourceType,
    /// Value imports in source order. Type-only imports are erased and not followed.
    pub imports: Vec<Import>,
    /// Problems found while loading (unreadable file, unresolved imports).
    pub diagnostics: Vec<Diagnostic>,
}

/// The module graph of a project (PLAN §2 step 1).
///
/// Modules are discovered from entry points by following value imports. Ids are assigned in
/// discovery order and stay stable across [`set_source`](Self::set_source) updates.
pub struct ModuleGraph {
    options: GraphOptions,
    resolver: Resolver,
    modules: Vec<Module>,
    by_path: FxHashMap<PathBuf, ModuleId>,
    overrides: FxHashMap<PathBuf, Arc<str>>,
    entries: Vec<ModuleId>,
}

impl std::fmt::Debug for ModuleGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleGraph")
            .field("root", &self.options.root)
            .field("modules", &self.modules.len())
            .finish_non_exhaustive()
    }
}

impl ModuleGraph {
    /// An empty graph.
    pub fn new(options: GraphOptions) -> Self {
        let tsconfig = options.tsconfig.clone().map(|config_file| {
            TsconfigDiscovery::Manual(TsconfigOptions {
                config_file,
                references: TsconfigReferences::Auto,
            })
        });
        let resolver = Resolver::new(ResolveOptions {
            tsconfig,
            extensions: [
                ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".json",
            ]
            .map(String::from)
            .to_vec(),
            // TypeScript's ESM convention: `./util.js` names `./util.ts`.
            extension_alias: vec![
                (
                    ".js".into(),
                    vec![".ts".into(), ".tsx".into(), ".js".into()],
                ),
                (".jsx".into(), vec![".tsx".into(), ".jsx".into()]),
                (".mjs".into(), vec![".mts".into(), ".mjs".into()]),
                (".cjs".into(), vec![".cts".into(), ".cjs".into()]),
            ],
            condition_names: vec!["import".into(), "module".into(), "default".into()],
            main_fields: vec!["module".into(), "main".into()],
            ..ResolveOptions::default()
        });
        Self {
            options,
            resolver,
            modules: Vec::new(),
            by_path: FxHashMap::default(),
            overrides: FxHashMap::default(),
            entries: Vec::new(),
        }
    }

    /// The options the graph was created with.
    pub fn options(&self) -> &GraphOptions {
        &self.options
    }

    /// Adds entry points and loads everything they reach. Returns the entries' ids.
    ///
    /// Fails only if an entry itself can't be read; problems further down are recorded as
    /// diagnostics on the importing module.
    pub fn add_entries(&mut self, entries: &[PathBuf]) -> Result<Vec<ModuleId>, ProjectError> {
        let mut ids = Vec::with_capacity(entries.len());
        for entry in entries {
            let path = self.normalize(entry);
            if !self.overrides.contains_key(&path)
                && let Err(source) = std::fs::metadata(&path)
            {
                return Err(ProjectError::Entry { path, source });
            }
            let id = self.load(&path);
            if !self.entries.contains(&id) {
                self.entries.push(id);
            }
            ids.push(id);
        }
        Ok(ids)
    }

    /// Sets (`Some`) or clears (`None`) the in-memory source of `path`, which need not exist on
    /// disk. If the module is in the graph its source and imports are refreshed, and newly
    /// imported modules are loaded. Returns the module's id if it is in the graph.
    pub fn set_source(&mut self, path: &Path, source: Option<String>) -> Option<ModuleId> {
        let path = self.normalize(path);
        match source {
            Some(source) => {
                self.overrides.insert(path.clone(), source.into());
            }
            None => {
                self.overrides.remove(&path);
            }
        }
        let id = *self.by_path.get(&path)?;
        self.load(&path);
        Some(id)
    }

    /// The entry modules, in the order they were added.
    pub fn entries(&self) -> &[ModuleId] {
        &self.entries
    }

    /// Every module, indexed by id.
    pub fn modules(&self) -> &[Module] {
        &self.modules
    }

    /// The module with `id`.
    ///
    /// # Panics
    /// If `id` is not from this graph.
    pub fn module(&self, id: ModuleId) -> &Module {
        &self.modules[id.index()]
    }

    /// The id of the module at `path`, if it is in the graph.
    pub fn get(&self, path: &Path) -> Option<ModuleId> {
        self.by_path.get(&self.normalize(path)).copied()
    }

    /// Modules that import `id` directly.
    pub fn importers(&self, id: ModuleId) -> Vec<ModuleId> {
        self.modules
            .iter()
            .filter(|module| {
                module
                    .imports
                    .iter()
                    .any(|import| import.target == ImportTarget::Module(id))
            })
            .map(|module| module.id)
            .collect()
    }

    /// Modules that import `id` directly or transitively, excluding `id` itself unless it is
    /// part of a cycle. Sorted by id.
    pub fn dependants(&self, id: ModuleId) -> Vec<ModuleId> {
        let mut seen = FxHashSet::default();
        let mut queue = VecDeque::from([id]);
        while let Some(next) = queue.pop_front() {
            for importer in self.importers(next) {
                if seen.insert(importer) {
                    queue.push_back(importer);
                }
            }
        }
        let mut dependants: Vec<_> = seen.into_iter().collect();
        dependants.sort_unstable();
        dependants
    }

    /// Makes `path` absolute (against the root) and canonical if it exists.
    fn normalize(&self, path: &Path) -> PathBuf {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.options.root.join(path)
        };
        std::fs::canonicalize(&absolute).unwrap_or(absolute)
    }

    /// Loads (or reloads) `path` and everything it reaches that isn't loaded yet.
    fn load(&mut self, path: &Path) -> ModuleId {
        let first = self.load_one(path);
        let mut queue = VecDeque::from([first]);
        while let Some(id) = queue.pop_front() {
            let pending: Vec<PathBuf> = self.modules[id.index()]
                .imports
                .iter()
                .filter_map(|import| match &import.target {
                    ImportTarget::External { path: Some(path) } if is_module_path(path) => {
                        Some(path.clone())
                    }
                    _ => None,
                })
                .collect();
            for path in pending {
                let new = !self.by_path.contains_key(&path);
                let target = if new {
                    self.load_one(&path)
                } else {
                    self.by_path[&path]
                };
                for import in &mut self.modules[id.index()].imports {
                    if import.target
                        == (ImportTarget::External {
                            path: Some(path.clone()),
                        })
                    {
                        import.target = ImportTarget::Module(target);
                    }
                }
                if new {
                    queue.push_back(target);
                }
            }
        }
        first
    }

    /// Reads and scans one module, without following its imports. Imports of compilable files
    /// are left as `External { path: Some(..) }` for `load` to link up.
    fn load_one(&mut self, path: &Path) -> ModuleId {
        let mut diagnostics = Vec::new();
        let source: Arc<str> = match self.overrides.get(path) {
            Some(source) => Arc::clone(source),
            None => match std::fs::read_to_string(path) {
                Ok(source) => source.into(),
                Err(error) => {
                    diagnostics.push(
                        Diagnostic::error(format!("cannot read {}: {error}", path.display()))
                            .with_code("trioxide(read)")
                            .with_file(path),
                    );
                    "".into()
                }
            },
        };
        let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::ts());
        let imports = self.scan_imports(path, &source, source_type, &mut diagnostics);
        let hash = ContentHash::of(source.as_bytes());

        let id = if let Some(&id) = self.by_path.get(path) {
            id
        } else {
            let id =
                ModuleId::new(u32::try_from(self.modules.len()).expect("fewer than 2^32 modules"));
            self.by_path.insert(path.to_path_buf(), id);
            self.modules.push(Module {
                id,
                path: path.to_path_buf(),
                source: Arc::clone(&source),
                hash,
                source_type,
                imports: Vec::new(),
                diagnostics: Vec::new(),
            });
            id
        };
        let module = &mut self.modules[id.index()];
        module.source = source;
        module.hash = hash;
        module.source_type = source_type;
        module.imports = imports;
        module.diagnostics = diagnostics;
        id
    }

    fn scan_imports(
        &self,
        path: &Path,
        source: &str,
        source_type: SourceType,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Vec<Import> {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, source_type).parse();
        let record = &parsed.module_record;

        let mut requests: Vec<(Span, String, ImportKind)> = record
            .requested_modules
            .iter()
            .flat_map(|(specifier, requests)| {
                requests
                    .iter()
                    .filter(|request| !request.is_type)
                    .map(move |request| (request.span, specifier.to_string(), ImportKind::Static))
            })
            .collect();
        for dynamic in &record.dynamic_imports {
            let text = dynamic.module_request.source_text(source);
            let unquoted = text
                .strip_prefix(['"', '\''])
                .and_then(|rest| rest.strip_suffix(['"', '\'']))
                .filter(|inner| !inner.contains(['"', '\'', '\\']));
            if let Some(specifier) = unquoted {
                requests.push((
                    dynamic.module_request,
                    specifier.to_string(),
                    ImportKind::Dynamic,
                ));
            }
        }
        requests.sort_by_key(|(span, ..)| span.start);
        requests.dedup_by_key(|(span, ..)| *span);

        requests
            .into_iter()
            .map(|(span, specifier, kind)| {
                let target = match self.resolver.resolve_file(path, &specifier) {
                    Ok(resolution) => ImportTarget::External {
                        path: Some(resolution.into_path_buf()),
                    },
                    Err(_) if specifier.starts_with('.') || specifier.starts_with('/') => {
                        diagnostics.push(
                            Diagnostic::warning(format!("cannot resolve import \"{specifier}\""))
                                .with_code("trioxide(unresolved-import)")
                                .with_file(path)
                                .with_label(span, None),
                        );
                        ImportTarget::Unresolved
                    }
                    Err(_) => ImportTarget::External { path: None },
                };
                Import {
                    specifier,
                    span,
                    kind,
                    target,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_paths() {
        assert!(is_module_path(Path::new("/a/b.ts")));
        assert!(is_module_path(Path::new("/a/b.tsx")));
        assert!(is_module_path(Path::new("/a/b.mjs")));
        assert!(!is_module_path(Path::new("/a/b.d.ts")));
        assert!(!is_module_path(Path::new("/a/b.json")));
        assert!(!is_module_path(Path::new("/a/node_modules/x/index.js")));
    }
}
