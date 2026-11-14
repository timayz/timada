//! The keys personal data is sealed under, kept outside the event store.
//!
//! The shoppers context records the address a shopper signs in with and the
//! addresses they ship to, the support context what a visitor wrote through
//! the contact form, and the newsletter the address a subscriber signed up
//! with, only sealed under a key of their own ([`timada_core::Key`]); this
//! module keeps those keys outside the event store, so destroying one when
//! the account is deleted or the message erased leaves what was recorded
//! unreadable for good.
//!
//! A key is one file, `<dir>/<tenant>/<first character of its id>/<key id>`,
//! holding the key sealed with the operator's `secrets.key`. A key id is
//! random, so one directory holds every context's keys, and the 64
//! directories below a tenant share them evenly: a shop with a million
//! accounts keeps some sixteen thousand files in each, not a million in one.
//! Like a picture ([`crate::media`]) a key is copied to a majority of the
//! cluster before the event naming it is recorded, over a route only the
//! nodes can use: unlike the pictures, nothing serves these files to the
//! outside. A node missing a key asks every peer for it at once, and takes
//! one that has not answered within [`FETCH_TIMEOUT`] for out of reach; each
//! node follows the streams that name keys ([`crate::shopper_keys`],
//! [`crate::message_keys`], [`crate::newsletter_keys`]) to fetch the keys of
//! new ones and destroy those of deleted ones. A node that joined by copying
//! the store copies every key from a peer first, page by page
//! ([`KeyVault::copy_from_peers`]), so its read models find them on its own
//! disk rather than ask for each in turn.
//!
//! Destroying a key leaves an empty file in its place, the tombstone, so a
//! copy or a fetch still under way cannot bring it back: a key takes its
//! name only where no file holds it yet (a hard link, which never replaces
//! one), while the tombstone replaces the key in one step (a rename). A
//! tombstone holds nothing, a key id being random, and takes the name the
//! key took. It stays for good: a peer that has not destroyed the key yet
//! would otherwise hand it back to whoever asks, the mail of a code asked
//! for before the account's deletion, say.
//!
//! Every key file, a tombstone too, is on the disk before it takes its name
//! ([`crate::disk`]), so a power cut never leaves an empty one, which would
//! read as a tombstone. The name is synced as well where something counts
//! on it: a key put, and a peer's copy of one, before the event naming the
//! key commits, and every tombstone, whose loss would bring the key back. A
//! key this node fetched, or copied as it joined, may lose its name to a
//! power cut, and is then asked for again.

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openidconnect::reqwest;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use timada_core::keys::KeyRing;
use timada_core::{Key, Keys};
use tokio::task::JoinSet;

use crate::config::DataKeysConfig;
use crate::disk::{self, Durability};
use crate::peers::{self, Peers, is_id};
use crate::seal::Sealer;

/// The route a node takes and hands out copies of a key on, below
/// `/cluster/keys`.
pub const PEER_ROUTE: &str = "/cluster/keys";

/// A sealed key is a nonce, 32 bytes and a tag; anything much longer is not
/// one.
pub const MAX_SEALED_LEN: usize = 256;

/// How long a node waits for a peer's answer about one key. A key is a few
/// dozen bytes: a peer slower than this is down or drowning, and what asked
/// (a read model's pass, say) is better failed and started again than held
/// for as long as a picture may take.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(2);

/// How many keys a page of a tenant's carries, as a node copies them from a
/// peer: some hundred kilobytes.
pub const PAGE_LEN: usize = 1000;

/// How many keys of a page a node writes at once as it copies them. Each is
/// synced to the disk before it takes its name, and a disk takes many syncs
/// at once in about the time of one: on a consumer NVMe, 64 at once copied a
/// shop's keys three times as fast as 16.
const WRITES_AT_ONCE: usize = 64;

/// What a key is sealed for: its tenant and its id, so a sealed file copied
/// under another tenant or name opens nothing.
fn owner(tenant_id: &str, key_id: &str) -> String {
    format!("{tenant_id}/{key_id}")
}

/// The tenants a node keeps keys for, as it lists them to a peer.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tenants {
    pub tenants: Vec<String>,
}

/// A page of one tenant's keys, as a node hands them to a peer that copies
/// them.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPage {
    /// In their ids' order.
    pub keys: Vec<PagedKey>,
    /// Where the next page starts (after this id), `None` on the last one.
    pub next: Option<String>,
}

/// One key of a [`KeyPage`].
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PagedKey {
    pub id: String,
    /// The sealed key, in URL-safe base64; `None` once it was destroyed.
    pub sealed: Option<String>,
}

/// What this node's disk holds for a key.
enum Held {
    /// The key, sealed with `secrets.key`.
    Sealed(Vec<u8>),
    /// Its tombstone: the key was destroyed.
    Destroyed,
    /// Nothing: the key never reached this node, or not yet.
    Missing,
}

pub struct KeyVault {
    dir: PathBuf,
    /// `None` without `secrets.key`: nothing sealed can be recorded.
    sealer: Option<Sealer>,
    /// The other nodes, when there are any.
    peers: Option<Arc<Peers>>,
    /// How long each peer has to answer for a key: [`FETCH_TIMEOUT`].
    fetch_timeout: Duration,
}

impl KeyVault {
    pub fn new(config: &DataKeysConfig, sealer: Option<Sealer>, peers: Option<Arc<Peers>>) -> Self {
        Self {
            dir: config.dir.clone(),
            sealer,
            peers,
            fetch_timeout: FETCH_TIMEOUT,
        }
    }

    /// The same vault, giving each peer `fetch_timeout` to answer for a key.
    #[cfg(test)]
    fn waiting(mut self, fetch_timeout: Duration) -> Self {
        self.fetch_timeout = fetch_timeout;
        self
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
            && self
                .open(tenant_id, key_id, sealed)
                .is_ok_and(|key| key.is_some());
        if !valid {
            return Ok(false);
        }
        self.store(tenant_id, key_id, sealed.to_vec(), Durability::Synced)
            .await
    }

    /// The sealed key, for a peer that lacks it: `None` when this node does
    /// not hold it (or destroyed it).
    pub async fn sealed(&self, tenant_id: &str, key_id: &str) -> anyhow::Result<Option<Vec<u8>>> {
        if !is_id(tenant_id) || !is_id(key_id) {
            return Ok(None);
        }
        Ok(match self.held(tenant_id, key_id).await? {
            Held::Sealed(sealed) => Some(sealed),
            Held::Destroyed | Held::Missing => None,
        })
    }

    /// Destroys a key for good: the tombstone takes its place, so nothing
    /// writes it again.
    pub async fn destroy(&self, tenant_id: &str, key_id: &str) -> anyhow::Result<()> {
        if !is_id(tenant_id) || !is_id(key_id) {
            return Ok(());
        }
        let bucket = self.bucket(tenant_id, key_id);
        let key_id = key_id.to_owned();
        on_disk(move || destroy_at(&bucket, &key_id, Durability::Synced)).await
    }

    /// Whether this node holds the key or destroyed it: either way there is
    /// nothing to fetch.
    pub async fn knows(&self, tenant_id: &str, key_id: &str) -> anyhow::Result<bool> {
        if !is_id(tenant_id) || !is_id(key_id) {
            return Ok(false);
        }
        Ok(tokio::fs::try_exists(self.path(tenant_id, key_id)).await?)
    }

    /// The tenants this node keeps keys for, for a peer that copies them.
    pub async fn tenants(&self) -> anyhow::Result<Tenants> {
        let dir = self.dir.clone();
        let tenants = on_disk(move || ids_in(&dir)).await?;
        Ok(Tenants { tenants })
    }

    /// Up to [`PAGE_LEN`] of a tenant's keys, destroyed ones included, in
    /// their ids' order after `after`, for a peer that copies them. The
    /// names must be ids ([`is_id`]).
    pub async fn page(&self, tenant_id: &str, after: Option<&str>) -> anyhow::Result<KeyPage> {
        anyhow::ensure!(
            is_id(tenant_id) && after.is_none_or(is_id),
            "not a tenant or key id"
        );
        let dir = self.dir.join(tenant_id);
        let after = after.map(str::to_owned);
        on_disk(move || page_of(&dir, after.as_deref(), PAGE_LEN)).await
    }

    /// Copies every key the peers keep, and every tombstone, onto this disk,
    /// page by page from the first peer that answers: what a node does once
    /// it joined by copying the store, before its read models catch up, so
    /// they find each key here instead of asking the peers for them one at a
    /// time (hours, for a shop with a million accounts). A peer that fails
    /// part of the way is left for the next, which carries on from the last
    /// key taken. A key held here already is kept; a tombstone replaces it.
    /// Returns how many keys and tombstones were handed over; an error when
    /// no peer handed over all of them, and the read models then ask the
    /// peers for each key still missing.
    pub async fn copy_from_peers(&self) -> anyhow::Result<usize> {
        let Some(peers) = &self.peers else {
            return Ok(0);
        };
        let mut copy = Progress::default();
        let mut failures = Vec::new();
        for url in peers.urls() {
            match self.copy_from(peers, url, &mut copy).await {
                Ok(()) => return Ok(copy.taken),
                Err(err) => {
                    tracing::warn!(peer = %url, error = %format!("{err:#}"), "cannot copy the keys from a peer");
                    failures.push(format!("{url}: {err:#}"));
                }
            }
        }
        anyhow::bail!(
            "no peer handed over its keys ({} taken; {})",
            copy.taken,
            failures.join("; ")
        )
    }

    /// Copies what is left of `copy` from the peer at `url`.
    async fn copy_from(&self, peers: &Peers, url: &str, copy: &mut Progress) -> anyhow::Result<()> {
        let ask = |path: String| {
            peers
                .client()
                .get(format!("{url}{PEER_ROUTE}{path}"))
                .header(reqwest::header::AUTHORIZATION, peers.authorization())
        };
        if copy.tenants.is_none() {
            let listed: Tenants = ask_json(ask(String::new())).await?;
            copy.tenants = Some(listed.tenants.into_iter().filter(|t| is_id(t)).collect());
        }
        while let Some(tenant_id) = copy.tenants.as_ref().and_then(VecDeque::front).cloned() {
            loop {
                // Ids need no escaping: letters, digits, `-` and `_`.
                let path = match &copy.after {
                    Some(after) => format!("/{tenant_id}?after={after}"),
                    None => format!("/{tenant_id}"),
                };
                let page: KeyPage = ask_json(ask(path)).await?;
                copy.taken += self.take_page(&tenant_id, &page.keys).await?;
                let Some(next) = page.next else {
                    break;
                };
                anyhow::ensure!(
                    is_id(&next) && copy.after.as_ref().is_none_or(|after| next > *after),
                    "the peer's pages of tenant {tenant_id} do not move on"
                );
                copy.after = Some(next);
            }
            copy.after = None;
            if let Some(tenants) = copy.tenants.as_mut() {
                tenants.pop_front();
            }
        }
        Ok(())
    }

    /// Stores a page of a tenant's keys a peer handed over, each checked to
    /// open as the key it is named after, as a copy a peer sends is
    /// ([`Self::accept`]). Returns how many there were.
    async fn take_page(&self, tenant_id: &str, keys: &[PagedKey]) -> anyhow::Result<usize> {
        let mut checked = Vec::with_capacity(keys.len());
        for key in keys {
            anyhow::ensure!(is_id(&key.id), "a peer handed over a key without an id");
            let sealed = match &key.sealed {
                None => None,
                Some(encoded) => {
                    let sealed = URL_SAFE_NO_PAD.decode(encoded)?;
                    anyhow::ensure!(
                        sealed.len() <= MAX_SEALED_LEN
                            && self.open(tenant_id, &key.id, &sealed)?.is_some(),
                        "a peer handed over {tenant_id}/{} that is not that key",
                        key.id
                    );
                    Some(sealed)
                }
            };
            checked.push((key.id.clone(), sealed));
        }
        // Several at once ([`WRITES_AT_ONCE`]), each lane a run of the page.
        let tenant_dir = self.dir.join(tenant_id);
        let mut lanes = JoinSet::new();
        for lane in checked.chunks(checked.len().div_ceil(WRITES_AT_ONCE).max(1)) {
            let (tenant_dir, lane) = (tenant_dir.clone(), lane.to_vec());
            lanes.spawn_blocking(move || take_lane(&tenant_dir, &lane));
        }
        while let Some(taken) = lanes.join_next().await {
            taken??;
        }
        Ok(checked.len())
    }

    /// The directory of a key's file. The names must be ids ([`is_id`]).
    fn bucket(&self, tenant_id: &str, key_id: &str) -> PathBuf {
        self.dir.join(tenant_id).join(bucket_name(key_id))
    }

    fn path(&self, tenant_id: &str, key_id: &str) -> PathBuf {
        self.bucket(tenant_id, key_id).join(key_id)
    }

    async fn held(&self, tenant_id: &str, key_id: &str) -> anyhow::Result<Held> {
        let path = self.path(tenant_id, key_id);
        on_disk(move || read_held(&path)).await
    }

    /// Writes a sealed key, unless it was destroyed: `false` then.
    async fn store(
        &self,
        tenant_id: &str,
        key_id: &str,
        sealed: Vec<u8>,
        durability: Durability,
    ) -> anyhow::Result<bool> {
        let bucket = self.bucket(tenant_id, key_id);
        let key_id = key_id.to_owned();
        on_disk(move || store_at(&bucket, &key_id, &sealed, durability)).await
    }

    /// The key a sealed file holds; `None` when it is not one. A key is
    /// sealed for its tenant and id ([`owner`]): a file copied under another
    /// name opens nothing.
    fn open(&self, tenant_id: &str, key_id: &str, sealed: &[u8]) -> anyhow::Result<Option<Key>> {
        let Some(sealer) = &self.sealer else {
            anyhow::bail!("keys cannot be read without secrets.key");
        };
        Ok(Key::from_bytes(
            &sealer.open_bytes_for(&owner(tenant_id, key_id), sealed)?,
        ))
    }

    /// Asks every peer at once for a key this node lacks, and keeps the first
    /// copy that opens as that key. `None` when every peer answers it holds
    /// none (destroyed, or never kept), or there are no peers; an error when
    /// none handed it over and some could not tell: out of reach, not
    /// answering within [`FETCH_TIMEOUT`], or handing over something else.
    /// What asked tries again later.
    async fn fetch(&self, tenant_id: &str, key_id: &str) -> anyhow::Result<Option<Key>> {
        let Some(peers) = &self.peers else {
            return Ok(None);
        };
        let mut asks = JoinSet::new();
        for url in peers.urls() {
            let request = peers
                .client()
                .get(format!("{url}{PEER_ROUTE}/{tenant_id}/{key_id}"))
                .header(reqwest::header::AUTHORIZATION, peers.authorization())
                .timeout(self.fetch_timeout);
            let url = url.clone();
            asks.spawn(async move { (url, ask_for_key(request).await) });
        }
        let mut unknown = 0;
        // The first copy wins; leaving drops the asks still under way.
        while let Some(asked) = asks.join_next().await {
            let Ok((url, answer)) = asked else {
                unknown += 1;
                continue;
            };
            let fetched = answer.and_then(|sealed| match sealed {
                None => Ok(None),
                Some(sealed) => {
                    let key = self
                        .open(tenant_id, key_id, &sealed)?
                        .ok_or_else(|| anyhow::anyhow!("not a key"))?;
                    Ok(Some((key, sealed)))
                }
            });
            match fetched {
                Ok(Some((key, sealed))) => {
                    if !self
                        .store(tenant_id, key_id, sealed, Durability::Cached)
                        .await?
                    {
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
            "key {tenant_id}/{key_id} is missing here and {unknown} peers could not tell"
        );
        Ok(None)
    }
}

/// How far a copy of the peers' keys got: the tenants left (listed by the
/// first peer that answered), the first of them under way, and the last key
/// of it taken.
#[derive(Default)]
struct Progress {
    tenants: Option<VecDeque<String>>,
    after: Option<String>,
    taken: usize,
}

/// A peer's copy of a key: `None` when it holds none.
async fn ask_for_key(request: reqwest::RequestBuilder) -> anyhow::Result<Option<Vec<u8>>> {
    let response = request.send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let sealed = response.error_for_status()?.bytes().await?;
    anyhow::ensure!(sealed.len() <= MAX_SEALED_LEN, "not a sealed key");
    Ok(Some(sealed.to_vec()))
}

/// A peer's answer, read as JSON.
async fn ask_json<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> anyhow::Result<T> {
    let response = request.send().await?;
    anyhow::ensure!(
        response.status().is_success(),
        "answered {}",
        response.status()
    );
    Ok(response.json().await?)
}

/// Runs a few file operations off the async workers, in one go.
async fn on_disk<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    Ok(tokio::task::spawn_blocking(work).await??)
}

/// The directory below a tenant's that holds a key: the first character of
/// its id. An id is never empty, and its characters are single bytes
/// ([`is_id`]).
fn bucket_name(key_id: &str) -> &str {
    &key_id[..1]
}

/// What the file at `path` holds.
fn read_held(path: &Path) -> io::Result<Held> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.is_empty() => Ok(Held::Destroyed),
        Ok(bytes) => Ok(Held::Sealed(bytes)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Held::Missing),
        Err(err) => Err(err),
    }
}

/// Writes a sealed key in `bucket` under its id, unless a file holds the
/// name already: the key itself, which stays, or its tombstone, which
/// nothing replaces. The bytes are on the disk before the name appears (a
/// hard link from a temporary file, which fails rather than replace), so
/// neither a reader nor a power cut ever finds half a key, or an empty file
/// that would read as its tombstone; with [`Durability::Synced`] the name is
/// too when this returns. `false` when the key was destroyed.
fn store_at(
    bucket: &Path,
    key_id: &str,
    sealed: &[u8],
    durability: Durability,
) -> io::Result<bool> {
    let path = bucket.join(key_id);
    let partial = disk::write_partial(&path, sealed)?;
    let linked = std::fs::hard_link(&partial, &path);
    let _ = std::fs::remove_file(&partial);
    let held = match linked {
        Ok(()) => true,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            matches!(read_held(&path)?, Held::Sealed(_))
        }
        Err(err) => return Err(err),
    };
    // A key held already may be a copy fetched, its name not synced yet.
    if held && durability == Durability::Synced {
        disk::sync_dir(bucket)?;
    }
    Ok(held)
}

/// Puts the tombstone in a key's place in one step: the key, if this node
/// held it, is gone the moment the tombstone is there. A tombstone must
/// survive a power cut, or the key comes back: with [`Durability::Cached`]
/// the caller syncs `bucket` itself, once for a batch.
fn destroy_at(bucket: &Path, key_id: &str, durability: Durability) -> io::Result<()> {
    let path = bucket.join(key_id);
    let partial = disk::write_partial(&path, b"")?;
    std::fs::rename(&partial, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&partial);
    })?;
    match durability {
        Durability::Synced => disk::sync_dir(bucket),
        Durability::Cached => Ok(()),
    }
}

/// Writes keys and tombstones a peer handed over, in their ids' order, below
/// a tenant's directory. A key's name is left to the kernel, the peers
/// holding the key; a tombstone's is synced, once per directory.
fn take_lane(tenant_dir: &Path, keys: &[(String, Option<Vec<u8>>)]) -> io::Result<()> {
    let mut destroyed: Vec<PathBuf> = Vec::new();
    for (key_id, sealed) in keys {
        let bucket = tenant_dir.join(bucket_name(key_id));
        match sealed {
            Some(sealed) => {
                store_at(&bucket, key_id, sealed, Durability::Cached)?;
            }
            None => {
                destroy_at(&bucket, key_id, Durability::Cached)?;
                if destroyed.last() != Some(&bucket) {
                    destroyed.push(bucket);
                }
            }
        }
    }
    destroyed
        .iter()
        .try_for_each(|bucket| disk::sync_dir(bucket))
}

/// The names in `dir` that are ids, in order; none when it does not exist.
fn ids_in(dir: &Path) -> io::Result<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut ids = Vec::new();
    for entry in entries {
        if let Ok(name) = entry?.file_name().into_string()
            && is_id(&name)
        {
            ids.push(name);
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

/// Up to `limit` of the keys in a tenant's directory, destroyed ones
/// included, in their ids' order after `after` (an id). A key's directory is
/// its first character, so the directories in order hold the keys in order.
fn page_of(tenant_dir: &Path, after: Option<&str>, limit: usize) -> io::Result<KeyPage> {
    let mut keys = Vec::new();
    for bucket in ids_in(tenant_dir)? {
        if bucket.len() != 1 || after.is_some_and(|after| bucket.as_str() < bucket_name(after)) {
            continue;
        }
        let dir = tenant_dir.join(&bucket);
        for id in ids_in(&dir)? {
            if !id.starts_with(bucket.as_str()) || after.is_some_and(|after| id.as_str() <= after) {
                continue;
            }
            let sealed = match read_held(&dir.join(&id))? {
                Held::Sealed(sealed) => Some(URL_SAFE_NO_PAD.encode(sealed)),
                Held::Destroyed => None,
                Held::Missing => continue,
            };
            keys.push(PagedKey { id, sealed });
            if keys.len() == limit {
                let next = keys.last().map(|key| key.id.clone());
                return Ok(KeyPage { keys, next });
            }
        }
    }
    Ok(KeyPage { keys, next: None })
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
        let sealed = sealer.seal_bytes_for(&owner(&self.tenant_id, key_id), key.as_bytes())?;
        anyhow::ensure!(
            vault
                .store(&self.tenant_id, key_id, sealed.clone(), Durability::Synced)
                .await?,
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
        match vault.held(&self.tenant_id, key_id).await? {
            Held::Sealed(sealed) => vault.open(&self.tenant_id, key_id, &sealed),
            Held::Destroyed => Ok(None),
            Held::Missing => vault.fetch(&self.tenant_id, key_id).await,
        }
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
    use std::time::Instant;

    use super::*;
    use crate::disk::test_support::Syncs;
    use crate::peers::test_support::{SECRET, answering, down, peer, silent};

    const OPERATOR_SECRET: &str = "a-test-secret-of-thirty-two-chars!!";

    fn sealer() -> Sealer {
        Sealer::new("shoppers", OPERATOR_SECRET)
    }

    fn fresh_dir() -> PathBuf {
        std::env::temp_dir().join(format!("timada-keys-{}", ulid::Ulid::generate()))
    }

    /// A vault in a fresh directory, with these peers.
    fn vault(peers: Option<Peers>) -> Arc<KeyVault> {
        vault_in(fresh_dir(), peers)
    }

    fn vault_in(dir: PathBuf, peers: Option<Peers>) -> Arc<KeyVault> {
        Arc::new(KeyVault::new(
            &DataKeysConfig { dir },
            Some(sealer()),
            peers.map(Arc::new),
        ))
    }

    /// `key` sealed as a node keeps it under `tenant_id`/`key_id`, leaked
    /// for a test peer's answers.
    fn sealed_as(tenant_id: &str, key_id: &str, key: &Key) -> &'static [u8] {
        sealer()
            .seal_bytes_for(&owner(tenant_id, key_id), key.as_bytes())
            .unwrap()
            .leak()
    }

    /// A peer answering as `source` would: the tenants it keeps, and the
    /// pages of each `page_len` keys long.
    async fn copying_peer(source: &Arc<KeyVault>, page_len: usize) -> String {
        let dir = source.dir.clone();
        answering(move |head, _| {
            let target = head.split(' ').nth(1).unwrap_or_default();
            let (path, query) = target.split_once('?').unwrap_or((target, ""));
            let after = query.strip_prefix("after=");
            let body = match path.strip_prefix(PEER_ROUTE).unwrap_or_default() {
                "" => serde_json::to_vec(&Tenants {
                    tenants: ids_in(&dir).unwrap(),
                }),
                tenant => {
                    serde_json::to_vec(&page_of(&dir.join(&tenant[1..]), after, page_len).unwrap())
                }
            };
            (200, body.unwrap())
        })
        .await
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
        let tombstone = || std::fs::read(vault.path("t1", "k1")).unwrap();
        assert!(tombstone().is_empty(), "the key is gone, its name kept");
        // Nothing brings it back: not a copy from a peer, not a put.
        assert!(!vault.accept("t1", "k1", &on_disk).await.unwrap());
        assert!(keys.put("k1", &key).await.is_err());
        assert!(tombstone().is_empty());
        assert_eq!(vault.sealed("t1", "k1").await.unwrap(), None);
        assert!(vault.knows("t1", "k1").await.unwrap());
        assert!(!vault.knows("t1", "k2").await.unwrap());
    }

    #[tokio::test]
    async fn keys_spread_over_a_directory_per_first_character() {
        let vault = vault(None);
        let keys = vault.for_tenant("t1");
        for key_id in ["ab", "a-", "_z", "9"] {
            keys.put(key_id, &Key::generate()).await.unwrap();
        }
        assert!(vault.dir.join("t1/a/ab").is_file());
        assert!(vault.dir.join("t1/a/a-").is_file());
        assert!(vault.dir.join("t1/_/_z").is_file());
        assert!(vault.dir.join("t1/9/9").is_file());
        // Nothing but the keys is left behind.
        assert_eq!(ids_in(&vault.dir.join("t1/a")).unwrap(), ["a-", "ab"]);
        assert_eq!(
            std::fs::read_dir(vault.dir.join("t1/a")).unwrap().count(),
            2
        );
    }

    #[tokio::test]
    async fn no_key_without_the_operator_secret() {
        let vault = Arc::new(KeyVault::new(
            &DataKeysConfig { dir: fresh_dir() },
            None,
            None,
        ));
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

    /// What something counts on is on the disk, name and all, by the time it
    /// is acknowledged: a key put, a peer's copy of one, a tombstone. A key
    /// fetched from a peer has its bytes there before its name, which a
    /// power cut may take back.
    #[tokio::test]
    async fn a_key_counted_on_is_synced_name_and_all() {
        let key = Key::generate();
        let vault = vault(Some(Peers::for_test(
            vec![peer(200, sealed_as("t1", "k2", &key)).await],
            3,
        )));
        let keys = vault.for_tenant("t1");
        // The directories a first key creates, out of the way.
        keys.put("k0", &Key::generate()).await.unwrap();
        let mut syncs = Syncs::below(&vault.dir);

        keys.put("k1", &Key::generate()).await.unwrap();
        assert_eq!(syncs.take(), ["t1/k/", "t1/k/k1"]);
        let copy = sealed_as("t1", "k3", &Key::generate());
        assert!(vault.accept("t1", "k3", copy).await.unwrap());
        assert_eq!(syncs.take(), ["t1/k/", "t1/k/k3"]);
        keys.delete("k1").await.unwrap();
        assert_eq!(syncs.take(), ["t1/k/", "t1/k/k1"]);

        assert_eq!(keys.get("k2").await.unwrap(), Some(key));
        assert_eq!(syncs.take(), ["t1/k/k2"], "fetched: the bytes alone");
    }

    #[tokio::test]
    async fn a_missing_key_is_fetched_from_a_peer() {
        let key = Key::generate();
        let sealed = sealed_as("t1", "k1", &key);
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
    async fn a_peer_that_does_not_answer_holds_nothing_up() {
        let key = Key::generate();
        let sealed = sealed_as("t1", "k1", &key);
        // Asked together: the one that holds it answers for both, long
        // before the silent one's minute is up. Keeping the key handed over
        // syncs it to the disk, which may take seconds on a busy one (a
        // build being written back, say), nowhere near a minute.
        let patience = Duration::from_secs(60);
        let peers = Peers::for_test(vec![silent().await, peer(200, sealed).await], 3);
        let vault = Arc::new(
            KeyVault::new(
                &DataKeysConfig { dir: fresh_dir() },
                Some(sealer()),
                Some(Arc::new(peers)),
            )
            .waiting(patience),
        );
        let started = Instant::now();
        assert_eq!(vault.for_tenant("t1").get("k1").await.unwrap(), Some(key));
        assert!(started.elapsed() < patience, "{:?}", started.elapsed());

        // Nobody hands it over: given up on within the bound. Nothing is
        // written, so the disk takes no part in the time.
        let unsure = super::tests::vault(Some(Peers::for_test(
            vec![silent().await, peer(404, b"").await],
            3,
        )));
        let started = Instant::now();
        assert!(unsure.for_tenant("t1").get("k1").await.is_err());
        let waited = started.elapsed();
        assert!(
            waited >= FETCH_TIMEOUT && waited < FETCH_TIMEOUT * 3,
            "{waited:?}"
        );
    }

    #[tokio::test]
    async fn a_copy_needs_the_secret_a_name_inside_the_directory_and_a_key() {
        let vault = vault(Some(Peers::for_test(vec![], 3)));
        assert!(vault.authorizes(Some(SECRET.as_bytes())));
        assert!(!vault.authorizes(None));
        assert!(!super::tests::vault(None).authorizes(Some(SECRET.as_bytes())));

        let sealed = sealer()
            .seal_bytes_for("t1/k1", Key::generate().as_bytes())
            .unwrap();
        for (tenant, key_id) in [
            ("..", "k1"),
            ("t1", "../k1"),
            ("t/1", "k1"),
            ("t1", "k1.part"),
        ] {
            assert!(
                !vault.accept(tenant, key_id, &sealed).await.unwrap(),
                "{tenant}/{key_id}"
            );
        }
        assert!(!vault.accept("t1", "k1", b"garbage").await.unwrap());
        let other = Sealer::new("mail", OPERATOR_SECRET)
            .seal_bytes_for("t1/k1", Key::generate().as_bytes())
            .unwrap();
        assert!(
            !vault.accept("t1", "k1", &other).await.unwrap(),
            "another key"
        );
        let elsewhere = sealer()
            .seal_bytes_for("t1/k2", Key::generate().as_bytes())
            .unwrap();
        assert!(
            !vault.accept("t1", "k1", &elsewhere).await.unwrap(),
            "sealed for another key id"
        );
        assert!(vault.accept("t1", "k1", &sealed).await.unwrap());
        // A second copy of a key held is taken, and changes nothing.
        let again = sealer()
            .seal_bytes_for("t1/k1", Key::generate().as_bytes())
            .unwrap();
        assert!(vault.accept("t1", "k1", &again).await.unwrap());
        assert_eq!(vault.sealed("t1", "k1").await.unwrap(), Some(sealed));
        assert_eq!(vault.sealed("..", "k1").await.unwrap(), None);
    }

    #[tokio::test]
    async fn pages_walk_a_tenants_keys_in_order() {
        let vault = vault(None);
        let keys = vault.for_tenant("t1");
        for key_id in ["b2", "a1", "b1", "Z9", "a2"] {
            keys.put(key_id, &Key::generate()).await.unwrap();
        }
        keys.delete("b1").await.unwrap();
        let tenant_dir = vault.dir.join("t1");
        let ids = |page: &KeyPage| -> Vec<String> {
            page.keys.iter().map(|key| key.id.clone()).collect()
        };

        let first = page_of(&tenant_dir, None, 2).unwrap();
        assert_eq!(ids(&first), ["Z9", "a1"]);
        assert_eq!(first.next.as_deref(), Some("a1"));
        let second = page_of(&tenant_dir, Some("a1"), 2).unwrap();
        assert_eq!(ids(&second), ["a2", "b1"]);
        assert_eq!(second.keys[1].sealed, None, "b1 was destroyed");
        let last = page_of(&tenant_dir, Some("b1"), 2).unwrap();
        assert_eq!(ids(&last), ["b2"]);
        assert_eq!(last.next, None);
        // A sealed key travels as it is kept.
        let sealed = URL_SAFE_NO_PAD
            .decode(last.keys[0].sealed.as_ref().unwrap())
            .unwrap();
        assert_eq!(vault.sealed("t1", "b2").await.unwrap(), Some(sealed));

        assert_eq!(vault.page("t1", None).await.unwrap().keys.len(), 5);
        assert_eq!(vault.page("t9", None).await.unwrap(), KeyPage::default());
        assert!(vault.page("..", None).await.is_err());
        assert!(vault.page("t1", Some("../x")).await.is_err());
        assert_eq!(vault.tenants().await.unwrap().tenants, ["t1"]);
    }

    #[tokio::test]
    async fn a_joining_node_copies_the_keys_page_by_page() {
        let source = vault(None);
        let mut kept = Vec::new();
        for (tenant, key_id) in [("t1", "a1"), ("t1", "b1"), ("t1", "b2"), ("t2", "c1")] {
            let key = Key::generate();
            source.for_tenant(tenant).put(key_id, &key).await.unwrap();
            kept.push((tenant, key_id, key));
        }
        source
            .for_tenant("t1")
            .put("gone", &Key::generate())
            .await
            .unwrap();
        source.destroy("t1", "gone").await.unwrap();

        // This node already holds a key the peer destroyed since, and one of
        // its own the peer lacks.
        let dir = fresh_dir();
        let before = vault_in(dir.clone(), None);
        before
            .for_tenant("t1")
            .put("gone", &Key::generate())
            .await
            .unwrap();
        let own = Key::generate();
        before.for_tenant("t1").put("own", &own).await.unwrap();

        // The first peer is down; the second hands its keys over two at a
        // time.
        let mut syncs = Syncs::below(&dir);
        let joined = vault_in(
            dir,
            Some(Peers::for_test(
                vec![down().await, copying_peer(&source, 2).await],
                3,
            )),
        );
        assert_eq!(joined.copy_from_peers().await.unwrap(), 5);
        // Every key's bytes; of the names, only the tombstone's, and each
        // directory created into the one above it.
        assert_eq!(
            syncs.take(),
            [
                "/",
                "t1/",
                "t1/",
                "t1/a/a1",
                "t1/b/b1",
                "t1/b/b2",
                "t1/g/",
                "t1/g/gone",
                "t2/",
                "t2/c/c1"
            ]
        );
        for (tenant, key_id, key) in kept {
            assert!(
                joined.knows(tenant, key_id).await.unwrap(),
                "{tenant}/{key_id}"
            );
            assert_eq!(
                joined.for_tenant(tenant).get(key_id).await.unwrap(),
                Some(key)
            );
        }
        assert_eq!(joined.sealed("t1", "gone").await.unwrap(), None);
        assert!(std::fs::read(joined.path("t1", "gone")).unwrap().is_empty());
        assert_eq!(joined.for_tenant("t1").get("own").await.unwrap(), Some(own));

        // Nobody to copy from.
        let stranded = super::tests::vault(Some(Peers::for_test(vec![down().await], 3)));
        assert!(stranded.copy_from_peers().await.is_err());
        assert_eq!(
            super::tests::vault(None).copy_from_peers().await.unwrap(),
            0
        );
    }

    /// How long a key put takes on this machine's disk (`PUTS` of them, 100
    /// unless set), alone: each is synced, name and all. Run by hand: `cargo
    /// test -p timada-dashboard --bin timada-dashboard probe -- --ignored
    /// --nocapture --test-threads=1`.
    #[tokio::test]
    #[ignore]
    async fn probe_a_key_put() {
        let count: u32 = std::env::var("PUTS")
            .ok()
            .and_then(|count| count.parse().ok())
            .unwrap_or(100);
        let vault = vault(None);
        let keys = vault.for_tenant("t1");
        // The 64 directories made first, as a shop's are.
        for first in "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-_".chars() {
            std::fs::create_dir_all(vault.bucket("t1", &first.to_string())).unwrap();
        }
        let started = Instant::now();
        for _ in 0..count {
            let key_id = timada_core::keys::new_key_id();
            keys.put(&key_id, &Key::generate()).await.unwrap();
        }
        let took = started.elapsed();
        println!("{count} keys put in {took:?}, {:?} each", took / count);
        let _ = std::fs::remove_dir_all(&vault.dir);
    }

    /// How long a joining node takes to copy a large shop's keys (`KEYS` of
    /// them, 100,000 unless set) from a peer on the same machine, in pages
    /// of [`PAGE_LEN`]. Run by hand, as [`probe_a_key_put`].
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn probe_a_large_shops_copy() {
        let count: usize = std::env::var("KEYS")
            .ok()
            .and_then(|count| count.parse().ok())
            .unwrap_or(100_000);
        let source = vault(None);
        // Written straight to the peer's disk, unsynced: what is measured
        // is the copy.
        let started = Instant::now();
        for _ in 0..count {
            let key_id = timada_core::keys::new_key_id();
            let sealed = sealer()
                .seal_bytes_for(&owner("t1", &key_id), Key::generate().as_bytes())
                .unwrap();
            let bucket = source.bucket("t1", &key_id);
            std::fs::create_dir_all(&bucket).unwrap();
            std::fs::write(bucket.join(&key_id), sealed).unwrap();
        }
        println!("{count} keys written in {:?}", started.elapsed());
        let joined = vault(Some(Peers::for_test(
            vec![copying_peer(&source, PAGE_LEN).await],
            3,
        )));
        let started = Instant::now();
        assert_eq!(joined.copy_from_peers().await.unwrap(), count);
        println!("{count} keys copied in {:?}", started.elapsed());
        let busiest = ids_in(&joined.dir.join("t1"))
            .unwrap()
            .iter()
            .map(|bucket| ids_in(&joined.dir.join("t1").join(bucket)).unwrap().len())
            .max()
            .unwrap_or_default();
        println!("at most {busiest} keys in one directory");
        let _ = std::fs::remove_dir_all(&source.dir);
        let _ = std::fs::remove_dir_all(&joined.dir);
    }

    #[tokio::test]
    async fn a_copy_refuses_what_is_not_a_key() {
        let source = vault(None);
        source
            .for_tenant("t1")
            .put("k1", &Key::generate())
            .await
            .unwrap();
        // Sealed for another key id, and renamed in the source's directory.
        std::fs::rename(source.path("t1", "k1"), source.dir.join("t1/k/k2")).unwrap();
        let joined = vault(Some(Peers::for_test(
            vec![copying_peer(&source, 10).await],
            3,
        )));
        assert!(joined.copy_from_peers().await.is_err());
        assert!(!joined.knows("t1", "k2").await.unwrap());
    }
}
