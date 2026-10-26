//! The keys personal data is sealed under, kept outside the event store.
//!
//! The shoppers context records the address a shopper signs in with and the
//! addresses they ship to, and the support context what a visitor wrote
//! through the contact form, only sealed under a key of their own
//! ([`timada_core::Key`]); this module keeps those keys outside the event
//! store, so destroying one when the account is deleted or the message
//! erased leaves what was recorded unreadable for good.
//!
//! A key is one file, `<dir>/<tenant>/<key id>`, holding the key sealed with
//! the operator's `secrets.key`. Like a picture ([`crate::media`]) it is
//! copied to a majority of the cluster before the event naming it is
//! recorded, over a route only the nodes can use: unlike the pictures,
//! nothing serves these files to the outside. A node missing a key fetches
//! it from a peer when it needs it, and each node follows the streams that
//! name keys ([`crate::shopper_keys`], [`crate::message_keys`]) to fetch
//! the keys of new ones and destroy those of deleted ones. A key id is
//! random, so one directory holds every context's keys.
//!
//! Destroying leaves a tombstone, `<key id>.gone`, so a copy or a fetch still
//! under way cannot bring a destroyed key back. A tombstone holds nothing:
//! a key id is random.

use std::path::PathBuf;
use std::sync::Arc;

use openidconnect::reqwest;
use timada_core::keys::KeyRing;
use timada_core::{Key, Keys};

use crate::config::DataKeysConfig;
use crate::peers::{self, Peers, is_id};
use crate::seal::Sealer;

/// The route a node takes and hands out copies of a key on, below
/// `/cluster/keys`.
pub const PEER_ROUTE: &str = "/cluster/keys";

/// A sealed key is a nonce, 32 bytes and a tag; anything much longer is not
/// one.
pub const MAX_SEALED_LEN: usize = 256;

/// What a destroyed key's file is followed by.
const TOMBSTONE: &str = ".gone";

pub struct KeyVault {
    dir: PathBuf,
    /// `None` without `secrets.key`: nothing sealed can be recorded.
    sealer: Option<Sealer>,
    /// The other nodes, when there are any.
    peers: Option<Arc<Peers>>,
}

impl KeyVault {
    pub fn new(config: &DataKeysConfig, sealer: Option<Sealer>, peers: Option<Arc<Peers>>) -> Self {
        Self {
            dir: config.dir.clone(),
            sealer,
            peers,
        }
    }

    /// The keys of every tenant, as the subscriptions ask for them.
    pub fn ring(self: &Arc<Self>) -> Arc<dyn KeyRing> {
        Arc::new(VaultRing(self.clone()))
    }

    /// The keys of one tenant, as the contexts ask for them.
    pub fn for_tenant(self: &Arc<Self>, tenant_id: &str) -> Arc<dyn Keys> {
        Arc::new(TenantKeys {
            vault: self.clone(),
            tenant_id: tenant_id.to_owned(),
        })
    }

    /// Whether a request carries the cluster's secret. Without peers no
    /// request does: a single node shares no keys.
    pub fn authorizes(&self, header: Option<&[u8]>) -> bool {
        peers::authorizes(self.peers.as_deref(), header)
    }

    /// Takes a copy a peer sends: checks the names cannot leave the
    /// directory and that the bytes open to a key, then stores it. `false`
    /// when it is refused, a destroyed key included.
    pub async fn accept(
        &self,
        tenant_id: &str,
        key_id: &str,
        sealed: &[u8],
    ) -> anyhow::Result<bool> {
        let valid = is_id(tenant_id)
            && is_id(key_id)
            && sealed.len() <= MAX_SEALED_LEN
            && self.open(sealed).is_ok_and(|key| key.is_some());
        if !valid {
            return Ok(false);
        }
        self.store(tenant_id, key_id, sealed).await
    }

    /// The sealed key, for a peer that lacks it: `None` when this node does
    /// not hold it (or destroyed it).
    pub async fn sealed(&self, tenant_id: &str, key_id: &str) -> anyhow::Result<Option<Vec<u8>>> {
        if !is_id(tenant_id) || !is_id(key_id) || self.is_destroyed(tenant_id, key_id).await? {
            return Ok(None);
        }
        match tokio::fs::read(self.path(tenant_id, key_id)).await {
            Ok(sealed) => Ok(Some(sealed)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Destroys a key for good: the tombstone first, so nothing writes it
    /// again, then the file.
    pub async fn destroy(&self, tenant_id: &str, key_id: &str) -> anyhow::Result<()> {
        if !is_id(tenant_id) || !is_id(key_id) {
            return Ok(());
        }
        let dir = self.dir.join(tenant_id);
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(self.tombstone(tenant_id, key_id), b"").await?;
        match tokio::fs::remove_file(self.path(tenant_id, key_id)).await {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    }

    fn path(&self, tenant_id: &str, key_id: &str) -> PathBuf {
        self.dir.join(tenant_id).join(key_id)
    }

    fn tombstone(&self, tenant_id: &str, key_id: &str) -> PathBuf {
        self.dir
            .join(tenant_id)
            .join(format!("{key_id}{TOMBSTONE}"))
    }

    async fn is_destroyed(&self, tenant_id: &str, key_id: &str) -> anyhow::Result<bool> {
        Ok(tokio::fs::try_exists(self.tombstone(tenant_id, key_id)).await?)
    }

    /// The key a sealed file holds; `None` when it is not one.
    fn open(&self, sealed: &[u8]) -> anyhow::Result<Option<Key>> {
        let Some(sealer) = &self.sealer else {
            anyhow::bail!("keys cannot be read without secrets.key");
        };
        Ok(Key::from_bytes(&sealer.open_bytes(sealed)?))
    }

    /// Writes a sealed key, unless it was destroyed. The bytes land in a
    /// temporary file first, so a reader never sees a half-written one; a
    /// tombstone that appeared meanwhile wins. `false` when destroyed.
    async fn store(&self, tenant_id: &str, key_id: &str, sealed: &[u8]) -> anyhow::Result<bool> {
        if self.is_destroyed(tenant_id, key_id).await? {
            return Ok(false);
        }
        let dir = self.dir.join(tenant_id);
        tokio::fs::create_dir_all(&dir).await?;
        let partial = dir.join(format!("{key_id}.{}.part", ulid::Ulid::generate()));
        tokio::fs::write(&partial, sealed).await?;
        let path = self.path(tenant_id, key_id);
        tokio::fs::rename(&partial, &path).await?;
        if self.is_destroyed(tenant_id, key_id).await? {
            let _ = tokio::fs::remove_file(&path).await;
            return Ok(false);
        }
        Ok(true)
    }

    /// Asks the peers for a key this node lacks and keeps it. `None` when
    /// every peer answers it has none (or there are no peers); an error when
    /// some could not tell.
    async fn fetch(&self, tenant_id: &str, key_id: &str) -> anyhow::Result<Option<Key>> {
        let Some(peers) = &self.peers else {
            return Ok(None);
        };
        let mut unknown = 0;
        for url in peers.urls() {
            let fetched = async {
                let response = peers
                    .client()
                    .get(format!("{url}{PEER_ROUTE}/{tenant_id}/{key_id}"))
                    .header(reqwest::header::AUTHORIZATION, peers.authorization())
                    .send()
                    .await?;
                if response.status() == reqwest::StatusCode::NOT_FOUND {
                    return Ok(None);
                }
                let sealed = response.error_for_status()?.bytes().await?;
                anyhow::ensure!(sealed.len() <= MAX_SEALED_LEN, "not a sealed key");
                let key = self
                    .open(&sealed)?
                    .ok_or_else(|| anyhow::anyhow!("not a key"))?;
                Ok(Some((key, sealed)))
            }
            .await;
            match fetched {
                Ok(Some((key, sealed))) => {
                    if !self.store(tenant_id, key_id, &sealed).await? {
                        return Ok(None);
                    }
                    tracing::info!(peer = %url, tenant = %tenant_id, "fetched a key from a peer");
                    return Ok(Some(key));
                }
                Ok(None) => {}
                Err(err) => {
                    unknown += 1;
                    tracing::debug!(peer = %url, tenant = %tenant_id, %err, "peer cannot hand a key");
                }
            }
        }
        anyhow::ensure!(
            unknown == 0,
            "key {tenant_id}/{key_id} is missing here and {unknown} peers could not be asked"
        );
        Ok(None)
    }
}

/// The keys of one tenant.
struct TenantKeys {
    vault: Arc<KeyVault>,
    tenant_id: String,
}

#[async_trait::async_trait]
impl Keys for TenantKeys {
    async fn put(&self, key_id: &str, key: &Key) -> anyhow::Result<()> {
        let vault = &self.vault;
        let Some(sealer) = &vault.sealer else {
            anyhow::bail!("recording personal data needs secrets.key");
        };
        anyhow::ensure!(is_id(&self.tenant_id) && is_id(key_id), "not a key id");
        let sealed = sealer.seal_bytes(key.as_bytes())?;
        anyhow::ensure!(
            vault.store(&self.tenant_id, key_id, &sealed).await?,
            "key {key_id} was destroyed"
        );
        let Some(peers) = &vault.peers else {
            return Ok(());
        };
        peers
            .put_to_quorum(
                &format!("{PEER_ROUTE}/{}/{key_id}", self.tenant_id),
                sealed.into(),
                "key",
            )
            .await
    }

    async fn get(&self, key_id: &str) -> anyhow::Result<Option<Key>> {
        let vault = &self.vault;
        if !is_id(&self.tenant_id) || !is_id(key_id) {
            return Ok(None);
        }
        if let Some(sealed) = vault.sealed(&self.tenant_id, key_id).await? {
            return vault.open(&sealed);
        }
        if vault.is_destroyed(&self.tenant_id, key_id).await? {
            return Ok(None);
        }
        vault.fetch(&self.tenant_id, key_id).await
    }

    async fn delete(&self, key_id: &str) -> anyhow::Result<()> {
        self.vault.destroy(&self.tenant_id, key_id).await
    }
}

/// The vault as the subscriptions following every tenant see it: the keys
/// of whichever tenant an event names.
struct VaultRing(Arc<KeyVault>);

impl KeyRing for VaultRing {
    fn for_tenant(&self, tenant_id: &str) -> Arc<dyn Keys> {
        self.0.for_tenant(tenant_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peers::test_support::{SECRET, down, peer};

    const OPERATOR_SECRET: &str = "a-test-secret-of-thirty-two-chars!!";

    fn sealer() -> Sealer {
        Sealer::new("shoppers", OPERATOR_SECRET)
    }

    /// A vault in a fresh directory, with these peers.
    fn vault(peers: Option<Peers>) -> Arc<KeyVault> {
        let dir = std::env::temp_dir().join(format!("timada-keys-{}", ulid::Ulid::generate()));
        Arc::new(KeyVault::new(
            &DataKeysConfig { dir },
            Some(sealer()),
            peers.map(Arc::new),
        ))
    }

    #[tokio::test]
    async fn a_key_is_kept_sealed_and_destroyed_for_good() {
        let vault = vault(None);
        let keys = vault.for_tenant("t1");
        let key = Key::generate();
        keys.put("k1", &key).await.unwrap();
        assert_eq!(keys.get("k1").await.unwrap(), Some(key.clone()));

        let on_disk = tokio::fs::read(vault.path("t1", "k1")).await.unwrap();
        assert!(!on_disk.windows(32).any(|w| w == key.as_bytes()));
        assert_eq!(keys.get("unknown").await.unwrap(), None);
        assert_eq!(vault.for_tenant("t2").get("k1").await.unwrap(), None);

        keys.delete("k1").await.unwrap();
        keys.delete("k1").await.unwrap();
        assert_eq!(keys.get("k1").await.unwrap(), None);
        assert!(!vault.path("t1", "k1").exists());
        // Nothing brings it back: not a copy from a peer, not a put.
        assert!(!vault.accept("t1", "k1", &on_disk).await.unwrap());
        assert!(keys.put("k1", &key).await.is_err());
        assert!(!vault.path("t1", "k1").exists());
        assert_eq!(vault.sealed("t1", "k1").await.unwrap(), None);
    }

    #[tokio::test]
    async fn no_key_without_the_operator_secret() {
        let dir = std::env::temp_dir().join(format!("timada-keys-{}", ulid::Ulid::generate()));
        let vault = Arc::new(KeyVault::new(&DataKeysConfig { dir }, None, None));
        let keys = vault.for_tenant("t1");
        assert!(keys.put("k1", &Key::generate()).await.is_err());
        assert!(!vault.path("t1", "k1").exists());
    }

    #[tokio::test]
    async fn a_new_key_needs_a_majority_of_the_cluster() {
        let three = vault(Some(Peers::for_test(
            vec![peer(204, b"").await, down().await],
            3,
        )));
        let keys = three.for_tenant("t1");
        keys.put("k1", &Key::generate()).await.unwrap();

        let alone = vault(Some(Peers::for_test(
            vec![peer(401, b"").await, down().await],
            3,
        )));
        assert!(
            alone
                .for_tenant("t1")
                .put("k1", &Key::generate())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_missing_key_is_fetched_from_a_peer() {
        let key = Key::generate();
        let sealed: &'static [u8] = sealer().seal_bytes(key.as_bytes()).unwrap().leak();
        let vault = vault(Some(Peers::for_test(
            vec![peer(404, b"").await, peer(200, sealed).await],
            3,
        )));
        let keys = vault.for_tenant("t1");
        assert_eq!(keys.get("k1").await.unwrap(), Some(key));
        assert!(vault.path("t1", "k1").is_file(), "kept here now");

        // Every peer has none: destroyed (or never kept) everywhere.
        let nowhere = super::tests::vault(Some(Peers::for_test(
            vec![peer(404, b"").await, peer(404, b"").await],
            3,
        )));
        assert_eq!(nowhere.for_tenant("t1").get("k1").await.unwrap(), None);
        // A peer that cannot be asked might have it.
        let unsure = super::tests::vault(Some(Peers::for_test(
            vec![peer(404, b"").await, down().await],
            3,
        )));
        assert!(unsure.for_tenant("t1").get("k1").await.is_err());
        // Bytes that are no key are not kept.
        let garbage = super::tests::vault(Some(Peers::for_test(
            vec![peer(200, b"not a sealed key").await],
            3,
        )));
        assert!(garbage.for_tenant("t1").get("k1").await.is_err());
        assert!(!garbage.path("t1", "k1").exists());
    }

    #[tokio::test]
    async fn a_copy_needs_the_secret_a_name_inside_the_directory_and_a_key() {
        let vault = vault(Some(Peers::for_test(vec![], 3)));
        assert!(vault.authorizes(Some(SECRET.as_bytes())));
        assert!(!vault.authorizes(None));
        assert!(!super::tests::vault(None).authorizes(Some(SECRET.as_bytes())));

        let sealed = sealer().seal_bytes(Key::generate().as_bytes()).unwrap();
        for (tenant, key_id) in [
            ("..", "k1"),
            ("t1", "../k1"),
            ("t/1", "k1"),
            ("t1", "k1.gone"),
        ] {
            assert!(
                !vault.accept(tenant, key_id, &sealed).await.unwrap(),
                "{tenant}/{key_id}"
            );
        }
        assert!(!vault.accept("t1", "k1", b"garbage").await.unwrap());
        let other = Sealer::new("mail", OPERATOR_SECRET)
            .seal_bytes(Key::generate().as_bytes())
            .unwrap();
        assert!(
            !vault.accept("t1", "k1", &other).await.unwrap(),
            "another key"
        );
        assert!(vault.accept("t1", "k1", &sealed).await.unwrap());
        assert_eq!(vault.sealed("t1", "k1").await.unwrap(), Some(sealed));
        assert_eq!(vault.sealed("..", "k1").await.unwrap(), None);
    }
}
