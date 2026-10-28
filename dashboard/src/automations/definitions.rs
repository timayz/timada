//! The definitions runs are pinned to: a version, once published, never
//! changes, so this node reads a scenario's versions from its stream once
//! and keeps them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use timada_automations::{Command, Definition};

type Key = (String, String, u32);

#[derive(Default)]
pub struct Definitions {
    cache: Mutex<HashMap<Key, Arc<Definition>>>,
}

impl Definitions {
    /// Version `version` of a tenant's scenario; `None` for one this store
    /// does not have (yet).
    pub async fn get<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant_id: &str,
        scenario_id: &str,
        version: u32,
    ) -> anyhow::Result<Option<Arc<Definition>>> {
        let key = (tenant_id.to_owned(), scenario_id.to_owned(), version);
        if let Some(found) = self.lock().get(&key) {
            return Ok(Some(found.clone()));
        }
        let Some(state) = Command::new(store.clone(), tenant_id)
            .load_scenario(scenario_id)
            .await?
        else {
            return Ok(None);
        };
        let mut cache = self.lock();
        for (number, json) in &state.versions {
            let definition = Arc::new(Definition::from_json(json)?);
            cache.insert(
                (tenant_id.to_owned(), scenario_id.to_owned(), *number),
                definition,
            );
        }
        Ok(cache.get(&key).cloned())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Arc<Definition>>> {
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
