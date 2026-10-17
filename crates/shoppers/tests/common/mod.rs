//! Shopper keys kept in memory, for the tests.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Mutex;

use timada_shoppers::{ShopperKey, ShopperKeys};

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
