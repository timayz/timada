//! The definitions runs are pinned to: a version, once published, never
//! changes, so this node reads it once from the read model
//! (`automations_versions`) and keeps the ones asked for last, within
//! [`CACHE_BYTES`] of JSON.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use timada_automations::{Definition, published_definition};

use crate::db::Db;

/// How much JSON the definitions kept were read from: a few hundred
/// scenarios of ordinary size, a few dozen at the largest a definition may
/// be.
const CACHE_BYTES: usize = 16 * 1024 * 1024;

/// A tenant's scenario and the version.
type Key = (String, String, u32);

struct Cached {
    key: Key,
    definition: Arc<Definition>,
    /// The size of the JSON it was read from.
    bytes: usize,
}

pub struct Definitions {
    /// The least recently asked for first.
    entries: Mutex<VecDeque<Cached>>,
    max_bytes: usize,
}

impl Default for Definitions {
    fn default() -> Self {
        Self::new(CACHE_BYTES)
    }
}

impl Definitions {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            entries: Mutex::new(VecDeque::new()),
            max_bytes,
        }
    }

    /// Version `version` of a tenant's scenario; `None` for one this node
    /// has not seen published (yet).
    pub async fn get(
        &self,
        db: &Db,
        tenant_id: &str,
        scenario_id: &str,
        version: u32,
    ) -> anyhow::Result<Option<Arc<Definition>>> {
        let key = (tenant_id.to_owned(), scenario_id.to_owned(), version);
        if let Some(found) = self.cached(&key) {
            return Ok(Some(found));
        }
        let Some(json) = published_definition(db, tenant_id, scenario_id, version).await? else {
            return Ok(None);
        };
        let definition = Arc::new(Definition::from_json(&json)?);
        self.keep(key, definition.clone(), json.len());
        Ok(Some(definition))
    }

    /// A definition kept, now the most recently asked for.
    fn cached(&self, key: &Key) -> Option<Arc<Definition>> {
        let mut entries = self.lock();
        let at = entries.iter().position(|cached| cached.key == *key)?;
        let cached = entries.remove(at)?;
        let definition = cached.definition.clone();
        entries.push_back(cached);
        Some(definition)
    }

    /// Keeps a definition, dropping the least recently asked for past the
    /// size. One larger than the whole cache is not kept.
    fn keep(&self, key: Key, definition: Arc<Definition>, bytes: usize) {
        if bytes > self.max_bytes {
            return;
        }
        let mut entries = self.lock();
        entries.retain(|cached| cached.key != key);
        entries.push_back(Cached {
            key,
            definition,
            bytes,
        });
        let mut total: usize = entries.iter().map(|cached| cached.bytes).sum();
        while total > self.max_bytes {
            let Some(dropped) = entries.pop_front() else {
                break;
            };
            total -= dropped.bytes;
        }
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<Cached>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(version: u32) -> Key {
        ("t1".to_owned(), "sc1".to_owned(), version)
    }

    #[test]
    fn the_cache_keeps_the_last_asked_within_its_size() {
        let cache = Definitions::new(10);
        let definition = Arc::new(Definition::default());
        cache.keep(key(1), definition.clone(), 4);
        cache.keep(key(2), definition.clone(), 4);
        // Asked again: the most recent now.
        assert!(cache.cached(&key(1)).is_some());
        cache.keep(key(3), definition.clone(), 4);
        assert!(cache.cached(&key(2)).is_none(), "dropped past the size");
        assert!(cache.cached(&key(1)).is_some());
        assert!(cache.cached(&key(3)).is_some());
        // Larger than the whole cache: not kept, nothing dropped for it.
        cache.keep(key(4), definition, 11);
        assert!(cache.cached(&key(4)).is_none());
        assert!(cache.cached(&key(1)).is_some());
    }
}
