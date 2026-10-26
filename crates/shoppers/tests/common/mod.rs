//! Shopper keys kept in memory, for the tests.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use timada_shoppers::{ShopperKey, ShopperKeyRing, ShopperKeys};

/// The tenant the tests act in.
pub const TENANT: &str = "acme";

#[derive(Default)]
pub struct MemoryKeys {
    keys: Mutex<HashMap<String, ShopperKey>>,
    /// Every `put` fails, as when no majority of nodes can be reached.
    pub refuse: bool,
}

impl MemoryKeys {
    pub fn refusing() -> Self {
        Self {
            refuse: true,
            ..Self::default()
        }
    }

    pub fn has(&self, key_id: &str) -> bool {
        self.keys.lock().unwrap().contains_key(key_id)
    }

    pub fn len(&self) -> usize {
        self.keys.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl ShopperKeys for MemoryKeys {
    async fn put(&self, key_id: &str, key: &ShopperKey) -> anyhow::Result<()> {
        anyhow::ensure!(!self.refuse, "no majority holds the key");
        self.keys
            .lock()
            .unwrap()
            .insert(key_id.to_owned(), key.clone());
        Ok(())
    }

    async fn get(&self, key_id: &str) -> anyhow::Result<Option<ShopperKey>> {
        Ok(self.keys.lock().unwrap().get(key_id).cloned())
    }

    async fn delete(&self, key_id: &str) -> anyhow::Result<()> {
        self.keys.lock().unwrap().remove(key_id);
        Ok(())
    }
}

/// The ring the subscriptions ask: every tenant shares one [`MemoryKeys`].
pub struct MemoryRing(pub Arc<MemoryKeys>);

impl ShopperKeyRing for MemoryRing {
    fn for_tenant(&self, _tenant_id: &str) -> Arc<dyn ShopperKeys> {
        self.0.clone()
    }
}

/// `keys` as the ring every tenant resolves to.
pub fn ring(keys: &Arc<MemoryKeys>) -> Arc<dyn ShopperKeyRing> {
    Arc::new(MemoryRing(keys.clone()))
}
