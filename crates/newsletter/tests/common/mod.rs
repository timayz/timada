#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_core::keys::KeyRing;
use timada_core::{Key, Keys};
use timada_newsletter::{Command, RequestOutcome, RequestSubscription, SOURCE_FORM};

pub type Newsletter = Command<evento::Sqlite>;

/// A fresh in-memory event store with the read-model schema applied.
pub async fn executor_with_pool() -> (evento::Sqlite, sqlx::SqlitePool) {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let mut migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
    migrator
        .add_migrations(timada_newsletter::migrations())
        .unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    (pool.clone().into(), pool)
}

pub async fn command() -> Newsletter {
    Command::new(executor_with_pool().await.0, TENANT)
}

/// The tenant the tests act in.
pub const TENANT: &str = "acme";

pub const ALICE: &str = "user-alice";
pub const VISITOR: &str = "storefront";
pub const EMAIL: &str = "ada@example.com";

/// Now, in unix seconds: the store stamps events with the real clock.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

pub fn ask(email: &str) -> RequestSubscription {
    RequestSubscription {
        email: email.to_owned(),
        language: Some("fr-FR,fr;q=0.9,en;q=0.8".to_owned()),
        source: SOURCE_FORM.to_owned(),
    }
}

/// Asks for `email` and returns the mailed link's token.
pub async fn request(newsletter: &Newsletter, keys: &MemoryKeys, email: &str) -> (String, String) {
    match newsletter
        .request_subscription(keys, VISITOR, ask(email), now())
        .await
        .unwrap()
    {
        RequestOutcome::ToConfirm {
            subscriber_id,
            token,
            ..
        } => (subscriber_id, token),
        other => panic!("nothing to mail: {other:?}"),
    }
}

/// Subscriber keys kept in memory.
#[derive(Default)]
pub struct MemoryKeys {
    keys: Mutex<HashMap<String, Key>>,
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

    /// Forgets every key without a trace, as a node that never got them.
    pub fn lose_all(&self) {
        self.keys.lock().unwrap().clear();
    }
}

#[async_trait::async_trait]
impl Keys for MemoryKeys {
    async fn put(&self, key_id: &str, key: &Key) -> anyhow::Result<()> {
        anyhow::ensure!(!self.refuse, "no majority holds the key");
        self.keys
            .lock()
            .unwrap()
            .insert(key_id.to_owned(), key.clone());
        Ok(())
    }

    async fn get(&self, key_id: &str) -> anyhow::Result<Option<Key>> {
        Ok(self.keys.lock().unwrap().get(key_id).cloned())
    }

    async fn delete(&self, key_id: &str) -> anyhow::Result<()> {
        self.keys.lock().unwrap().remove(key_id);
        Ok(())
    }
}

/// The ring the subscriptions ask: every tenant shares one [`MemoryKeys`].
pub struct MemoryRing(pub Arc<MemoryKeys>);

impl KeyRing for MemoryRing {
    fn for_tenant(&self, _tenant_id: &str) -> Arc<dyn Keys> {
        self.0.clone()
    }
}

/// `keys` as the ring every tenant resolves to.
pub fn ring(keys: &Arc<MemoryKeys>) -> Arc<dyn KeyRing> {
    Arc::new(MemoryRing(keys.clone()))
}
