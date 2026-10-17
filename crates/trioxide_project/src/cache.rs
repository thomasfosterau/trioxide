use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use trioxide_common::ModuleId;

/// A 64-bit xxh3 hash of a module's source text (and anything else mixed in with
/// [`ContentHash::combine`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(pub u64);

impl ContentHash {
    /// Hashes `bytes`.
    pub fn of(bytes: impl AsRef<[u8]>) -> Self {
        Self(xxhash_rust::xxh3::xxh3_64(bytes.as_ref()))
    }

    /// Mixes another hash in, e.g. the options hash into a module's source hash.
    #[must_use]
    pub fn combine(self, other: Self) -> Self {
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&self.0.to_le_bytes());
        bytes[8..].copy_from_slice(&other.0.to_le_bytes());
        Self::of(bytes)
    }
}

/// Per-module results keyed by content hash.
///
/// An entry is returned only if it was stored under the same hash, so callers compute the key
/// from everything the result depends on (source hash combined with an options hash) and never
/// need explicit invalidation. Stale entries are overwritten on the next insert.
#[derive(Debug, Clone)]
pub struct ContentCache<V> {
    entries: FxHashMap<ModuleId, (ContentHash, V)>,
}

impl<V> Default for ContentCache<V> {
    fn default() -> Self {
        Self {
            entries: FxHashMap::default(),
        }
    }
}

impl<V> ContentCache<V> {
    /// The cached value for `module` if it was stored under `hash`.
    pub fn get(&self, module: ModuleId, hash: ContentHash) -> Option<&V> {
        self.entries
            .get(&module)
            .filter(|(stored, _)| *stored == hash)
            .map(|(_, value)| value)
    }

    /// Stores `value` for `module` under `hash`, replacing any previous entry.
    pub fn insert(&mut self, module: ModuleId, hash: ContentHash, value: V) {
        self.entries.insert(module, (hash, value));
    }

    /// Drops the entry for `module`.
    pub fn remove(&mut self, module: ModuleId) {
        self.entries.remove(&module);
    }

    /// Drops every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// The number of entries, stale or not.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_only_on_same_hash() {
        let mut cache = ContentCache::default();
        let module = ModuleId::new(0);
        let hash = ContentHash::of("let a = 1;");
        cache.insert(module, hash, "out");
        assert_eq!(cache.get(module, hash), Some(&"out"));
        assert_eq!(cache.get(module, ContentHash::of("let a = 2;")), None);
        assert_eq!(
            cache.get(module, hash.combine(ContentHash::of("opts"))),
            None
        );
    }
}
