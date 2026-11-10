//! The mails the sagas owe, sent off the subscriptions' cursors: a lane per
//! shop, so one shop's mail server never sits in front of the others' mails
//! (#464).
//!
//! A subscription that follows a stream owing a mail (a sign-in code, an
//! invitation, a newsletter confirmation) sends nothing itself: it writes
//! what is owed into `owed_mails`, this node's table, and acknowledges the
//! event. A send inside a pass would hold the cursor for every tenant while
//! one shop's server takes its twenty seconds to not answer, and a send that
//! failed the pass would hold it until that server is back. The
//! [`Mailroom`] works the table instead: woken by the subscriptions and on a
//! timer, it takes the mails due, groups them by shop, and sends each shop's
//! one after the other while a bounded number of shops go at once. Each send
//! claims the mail on its stream first ([`timada_core::Sending`]), so a mail
//! owed to several nodes' tables goes out once; a send that fails for a
//! reason worth a retry is recorded as deferred, with when the next try may
//! be made ([`timada_core::RETRY_DELAYS_SECS`]), and given up on once the
//! retries run out, which `/admin/cluster` shows. The table follows the
//! stream through the same subscription: a claim moves the next look to the
//! lease's end, a deferral to the retry, an outcome removes the row.
//!
//! The table is a read model of the stream's `Sending` state, per node and
//! for the node's region (the sagas are sharded by routing key), kept
//! beside the other read models in the store's database. A node that never
//! saw an intent (down at the time) owes nothing for it; the nodes that did
//! carry it.

use std::collections::BTreeMap;
use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use timada_core::db::FromRow;
use tokio::sync::Notify;
use tokio::task::{JoinHandle, JoinSet};
use topcoat::context::AppContext;

use crate::cluster::Executor as Store;
use crate::db::Db;
use crate::key_vault::KeyVault;

mod schema;

pub use schema::{MIGRATIONS, TABLES};

/// How often the table is looked at without a subscription waking the
/// mailroom: a deferred retry falls due, a node that died mid-send lets its
/// lease lapse.
pub const CHECK_EVERY: Duration = Duration::from_secs(5);

/// The most mails one sweep takes, every shop together.
const MAILS_PER_SWEEP: i64 = 200;

/// How many shops send at once. Within a shop, one mail after the other.
const LANES: usize = 8;

/// The platform's own mails (codes, invitations) are not one shop's: they
/// go out over this many lanes, each stream always on the same one.
const PLATFORM_LANES: u64 = 4;

/// How long a send may take before it counts as not sent, over and above
/// the transports' own timeouts: a lane is never held for longer.
pub const SEND_DEADLINE: Duration = Duration::from_secs(60);

/// How long a mail nobody could claim (another node's claim still holds,
/// or the stream moved on and the row is about to go) waits before it is
/// looked at again.
const LOOK_AGAIN_SECS: u64 = 60;

/// How long a mail given up on stays on the cluster page.
const FAILED_KEPT_SECS: u64 = 7 * 24 * 60 * 60;

/// What a stream owes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A code proving an address to the platform (`EmailChallenge`).
    PlatformCode,
    /// A shop's sign-in code to a shopper (`ShopperChallenge`).
    ShopperCode,
    /// A link to join a tenant (`Invitation`).
    Invitation,
    /// A shop's double opt-in link (`Subscriber`).
    NewsletterConfirmation,
}

impl Kind {
    /// The column's value.
    pub fn key(self) -> &'static str {
        match self {
            Self::PlatformCode => "platform_code",
            Self::ShopperCode => "shopper_code",
            Self::Invitation => "invitation",
            Self::NewsletterConfirmation => "newsletter_confirmation",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "platform_code" => Self::PlatformCode,
            "shopper_code" => Self::ShopperCode,
            "invitation" => Self::Invitation,
            "newsletter_confirmation" => Self::NewsletterConfirmation,
            _ => return None,
        })
    }

    /// For a person.
    pub fn label(self) -> &'static str {
        match self {
            Self::PlatformCode => "sign-in code",
            Self::ShopperCode => "shopper sign-in code",
            Self::Invitation => "invitation",
            Self::NewsletterConfirmation => "newsletter confirmation",
        }
    }
}

/// What the stream said, for the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A new intent: owed from now, until `expires_at` (unix seconds), its
    /// `version` what the claim is checked against.
    Owed { version: u16, expires_at: u64 },
    /// A node took the send until `until`: nothing to look at before.
    Claimed { until: u64 },
    /// The send failed; the next try is at `retry_at`.
    Deferred { retry_at: u64, reason: String },
    /// Given up on.
    Failed { reason: String },
    /// Sent, refused for good, or the stream moved on: nothing owed.
    Settled,
}

/// One row of `owed_mails`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct OwedMail {
    pub kind: String,
    /// `""` for the platform's own mails.
    pub tenant_id: String,
    pub stream_id: String,
    pub version: i64,
    /// Unix seconds: when a node may next look at it.
    pub due_at: i64,
    /// Unix seconds: when the intent itself lapses.
    pub expires_at: i64,
    pub attempts: i64,
    pub failed: bool,
    /// The last failure.
    pub reason: String,
    pub updated_at: i64,
}

impl OwedMail {
    pub fn kind(&self) -> Option<Kind> {
        Kind::parse(&self.kind)
    }
}

/// How many mails are owed on this node, and how many were given up on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub owed: i64,
    pub failed: i64,
}

/// The counts for the cluster page.
pub async fn summary(db: &Db) -> Result<Summary> {
    let (owed, failed): (i64, i64) = db
        .fetch_one(
            "SELECT coalesce(sum(CASE WHEN failed = 0 THEN 1 ELSE 0 END), 0),
                    coalesce(sum(CASE WHEN failed = 1 THEN 1 ELSE 0 END), 0)
             FROM owed_mails",
            (),
        )
        .await?;
    Ok(Summary { owed, failed })
}

/// The mails given up on, latest first, at most `limit`.
pub async fn failed_mails(db: &Db, limit: i64) -> Result<Vec<OwedMail>> {
    db.fetch_all(
        "SELECT kind, tenant_id, stream_id, version, due_at, expires_at, attempts, failed, reason,
                updated_at
         FROM owed_mails WHERE failed = 1 ORDER BY updated_at DESC, kind, tenant_id, stream_id
         LIMIT ?1",
        (limit,),
    )
    .await
}

/// The sender, with what every kind of mail needs outside a request.
pub struct Mailroom {
    db: Db,
    /// The platform store, where the users, the tenants and the invitations
    /// live.
    pub(crate) platform: Store,
    /// The store, where the shops' shoppers and subscribers live.
    pub(crate) store: Store,
    /// The platform's transport and sender, the shops' own servers, and the
    /// keys the codes and links derive under.
    pub(crate) mail: Arc<AppContext>,
    /// The tenants' keys, to open the shoppers' addresses.
    pub(crate) keys: Arc<KeyVault>,
    /// The dashboard's public address, for the invitation links.
    pub(crate) base_url: String,
    /// This node, as the claims name it.
    pub(crate) node: String,
    /// Rung by the subscriptions once they wrote a row.
    wake: Notify,
}

impl Mailroom {
    pub fn new(
        db: Db,
        platform: Store,
        store: Store,
        mail: Arc<AppContext>,
        keys: Arc<KeyVault>,
        base_url: String,
        node: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            platform,
            store,
            mail,
            keys,
            base_url,
            node,
            wake: Notify::new(),
        })
    }

    /// Records what a stream of `kind` in `tenant_id` said, from its
    /// subscription. Safe to repeat: a replayed event writes the same row.
    pub async fn record(
        &self,
        kind: Kind,
        tenant_id: &str,
        stream_id: &str,
        change: Change,
        now: u64,
    ) -> Result<()> {
        let kind = kind.key();
        let now = now as i64;
        match change {
            Change::Owed {
                version,
                expires_at,
            } => {
                self.db
                    .execute(
                        "INSERT INTO owed_mails (kind, tenant_id, stream_id, version, due_at,
                            expires_at, attempts, failed, reason, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, '', ?5)
                         ON CONFLICT (kind, tenant_id, stream_id) DO UPDATE SET
                            version = excluded.version, due_at = excluded.due_at,
                            expires_at = excluded.expires_at, attempts = 0, failed = 0,
                            reason = '', updated_at = excluded.updated_at",
                        (
                            kind,
                            tenant_id,
                            stream_id,
                            i64::from(version),
                            now,
                            expires_at as i64,
                        ),
                    )
                    .await?;
                self.wake.notify_one();
            }
            Change::Claimed { until } => {
                self.db
                    .execute(
                        "UPDATE owed_mails SET due_at = ?4, updated_at = ?5
                         WHERE kind = ?1 AND tenant_id = ?2 AND stream_id = ?3 AND failed = 0",
                        (kind, tenant_id, stream_id, until as i64, now),
                    )
                    .await?;
            }
            Change::Deferred { retry_at, reason } => {
                self.db
                    .execute(
                        "UPDATE owed_mails SET due_at = ?4, attempts = attempts + 1, reason = ?5,
                            updated_at = ?6
                         WHERE kind = ?1 AND tenant_id = ?2 AND stream_id = ?3 AND failed = 0",
                        (
                            kind,
                            tenant_id,
                            stream_id,
                            retry_at as i64,
                            reason.as_str(),
                            now,
                        ),
                    )
                    .await?;
            }
            Change::Failed { reason } => {
                self.db
                    .execute(
                        "UPDATE owed_mails SET failed = 1, attempts = attempts + 1, reason = ?4,
                            updated_at = ?5
                         WHERE kind = ?1 AND tenant_id = ?2 AND stream_id = ?3",
                        (kind, tenant_id, stream_id, reason.as_str(), now),
                    )
                    .await?;
            }
            Change::Settled => {
                self.db
                    .execute(
                        "DELETE FROM owed_mails
                         WHERE kind = ?1 AND tenant_id = ?2 AND stream_id = ?3",
                        (kind, tenant_id, stream_id),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// Sends what is due every [`CHECK_EVERY`], and as soon as a
    /// subscription wrote a row.
    pub fn start(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(CHECK_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = self.wake.notified() => {}
                }
                if let Err(err) = self.sweep(crate::auth::now()).await {
                    tracing::error!(error = %format!("{err:#}"), "owed mails not swept");
                }
            }
        })
    }

    /// Sends the mails due at `now` (unix seconds), a lane per shop,
    /// [`LANES`] shops at once; returns how many were handed to a server.
    /// Then forgets the intents that lapsed and the failures shown long
    /// enough. A send that fails is recorded on its stream and is not an
    /// error here; a store that cannot be read or written ends the shop's
    /// lane for this sweep, logged, and the rest go on.
    pub async fn sweep(self: &Arc<Self>, now: u64) -> Result<usize> {
        let due: Vec<OwedMail> = self
            .db
            .fetch_all(
                "SELECT kind, tenant_id, stream_id, version, due_at, expires_at, attempts, failed,
                        reason, updated_at
                 FROM owed_mails
                 WHERE failed = 0 AND due_at <= ?1 AND expires_at > ?1
                 ORDER BY tenant_id, due_at, stream_id LIMIT ?2",
                (now as i64, MAILS_PER_SWEEP),
            )
            .await?;
        let mut sent = 0;
        let mut lanes = JoinSet::new();
        for (_, mails) in lanes_of(due) {
            if lanes.len() >= LANES
                && let Some(done) = lanes.join_next().await
            {
                sent += done.unwrap_or(0);
            }
            let room = self.clone();
            lanes.spawn(async move { room.lane(mails, now).await });
        }
        while let Some(done) = lanes.join_next().await {
            sent += done.unwrap_or(0);
        }
        self.db
            .execute(
                "DELETE FROM owed_mails WHERE (failed = 0 AND expires_at <= ?1)
                    OR (failed = 1 AND updated_at <= ?2)",
                (now as i64, now.saturating_sub(FAILED_KEPT_SECS) as i64),
            )
            .await?;
        Ok(sent)
    }

    /// One shop's mails, one after the other; how many went out.
    async fn lane(self: Arc<Self>, mails: Vec<OwedMail>, now: u64) -> usize {
        let mut sent = 0;
        for owed in &mails {
            match self.send(owed, now).await {
                Ok(Outcome::Sent) => sent += 1,
                Ok(Outcome::Recorded) => {}
                Ok(Outcome::Left) => {
                    // Another node's claim holds, or the stream moved on and
                    // this row is about to go: not before a while.
                    if let Err(err) = self
                        .db
                        .execute(
                            "UPDATE owed_mails SET due_at = ?4
                             WHERE kind = ?1 AND tenant_id = ?2 AND stream_id = ?3
                               AND failed = 0 AND due_at <= ?5",
                            (
                                owed.kind.as_str(),
                                owed.tenant_id.as_str(),
                                owed.stream_id.as_str(),
                                (now + LOOK_AGAIN_SECS) as i64,
                                now as i64,
                            ),
                        )
                        .await
                    {
                        tracing::warn!(error = %format!("{err:#}"), "an owed mail could not be put off");
                    }
                }
                Err(err) => {
                    tracing::error!(
                        tenant = %owed.tenant_id,
                        kind = %owed.kind,
                        stream = %owed.stream_id,
                        error = %format!("{err:#}"),
                        "an owed mail could not be sent: the store did not answer; the shop's lane stops for now"
                    );
                    break;
                }
            }
        }
        sent
    }

    /// Sends one owed mail, by its kind. An error is the store's (a load,
    /// a claim or a record that failed), never the mail server's.
    async fn send(&self, owed: &OwedMail, now: u64) -> Result<Outcome> {
        match owed.kind() {
            Some(Kind::PlatformCode) => crate::code_mails::send_platform(self, owed, now).await,
            Some(Kind::ShopperCode) => crate::code_mails::send_shopper(self, owed, now).await,
            Some(Kind::Invitation) => crate::invitation_mails::send(self, owed, now).await,
            Some(Kind::NewsletterConfirmation) => {
                crate::newsletter_confirmations::send(self, owed, now).await
            }
            None => {
                tracing::error!(kind = %owed.kind, "not a kind of owed mail; left alone");
                Ok(Outcome::Left)
            }
        }
    }
}

/// How a send of an owed mail went, for the lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Handed to the server, and recorded.
    Sent,
    /// Not sent, and the stream says why (refused for good, deferred, given
    /// up on): the subscription moves the row.
    Recorded,
    /// Nothing claimed: another node is sending, the next try is not due,
    /// or the stream settled or moved on already.
    Left,
}

/// The mails due, grouped into lanes: one per shop, the platform's over
/// [`PLATFORM_LANES`] by stream. Each lane keeps the order they fell due in.
fn lanes_of(due: Vec<OwedMail>) -> BTreeMap<(String, u64), Vec<OwedMail>> {
    let mut lanes: BTreeMap<(String, u64), Vec<OwedMail>> = BTreeMap::new();
    for owed in due {
        let slot = if owed.tenant_id.is_empty() {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            owed.stream_id.hash(&mut hasher);
            hasher.finish() % PLATFORM_LANES
        } else {
            0
        };
        lanes
            .entry((owed.tenant_id.clone(), slot))
            .or_default()
            .push(owed);
    }
    lanes
}

#[cfg(test)]
pub(crate) mod tests {
    use evento::{Evento, Turso};
    use topcoat::mail::Transport;

    use super::*;
    use crate::code_mails::tests::{mail_context, vault};

    pub(crate) const BASE_URL: &str = "https://timada.example/";

    /// An in-memory store with the read models' tables, the mailroom's
    /// among them.
    pub(crate) async fn store() -> Turso {
        let store = Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&store).await.unwrap();
        store
    }

    /// A mailroom over `store` for `node`, mailing through `transport`.
    pub(crate) fn room(
        store: &Turso,
        transport: impl Transport + 'static,
        node: &str,
    ) -> Arc<Mailroom> {
        room_with(store, mail_context(transport), vault(), node)
    }

    pub(crate) fn room_with(
        store: &Turso,
        mail: Arc<AppContext>,
        keys: Arc<KeyVault>,
        node: &str,
    ) -> Arc<Mailroom> {
        Mailroom::new(
            Db::from(store.clone()),
            Evento::new(store.clone()),
            Evento::new(store.clone()),
            mail,
            keys,
            BASE_URL.to_owned(),
            node.to_owned(),
        )
    }

    /// Every row of the table, in key order.
    pub(crate) async fn rows(store: &Turso) -> Vec<OwedMail> {
        Db::from(store.clone())
            .fetch_all(
                "SELECT kind, tenant_id, stream_id, version, due_at, expires_at, attempts, failed,
                        reason, updated_at
                 FROM owed_mails ORDER BY kind, tenant_id, stream_id",
                (),
            )
            .await
            .unwrap()
    }

    fn owed(tenant: &str, stream: &str, due_at: i64) -> OwedMail {
        OwedMail {
            kind: Kind::ShopperCode.key().to_owned(),
            tenant_id: tenant.to_owned(),
            stream_id: stream.to_owned(),
            version: 1,
            due_at,
            expires_at: due_at + 600,
            attempts: 0,
            failed: false,
            reason: String::new(),
            updated_at: due_at,
        }
    }

    /// Each shop's mails are one lane, in the order they fell due; the
    /// platform's are spread over a few, a stream always on the same one.
    #[test]
    fn mails_are_grouped_by_shop_and_the_platforms_spread_over_lanes() {
        let lanes = lanes_of(vec![
            owed("acme", "a-2", 20),
            owed("acme", "a-1", 10),
            owed("bob", "b-1", 5),
            owed("", "p-1", 1),
            owed("", "p-2", 2),
            owed("", "p-3", 3),
            owed("", "p-4", 4),
            owed("", "p-5", 5),
            owed("", "p-6", 6),
        ]);
        let acme = &lanes[&("acme".to_owned(), 0)];
        assert_eq!(
            acme.iter()
                .map(|m| m.stream_id.as_str())
                .collect::<Vec<_>>(),
            ["a-2", "a-1"],
            "the order the query gave"
        );
        assert_eq!(lanes[&("bob".to_owned(), 0)].len(), 1);
        let platform: Vec<_> = lanes.keys().filter(|(t, _)| t.is_empty()).collect();
        assert!(platform.len() > 1, "{platform:?}");
        assert!(platform.iter().all(|(_, slot)| *slot < PLATFORM_LANES));
        let again = lanes_of(vec![owed("", "p-1", 1)]);
        let (slot, _) = again.iter().next().map(|(k, v)| (k.1, v)).unwrap();
        assert!(
            lanes[&(String::new(), slot)]
                .iter()
                .any(|m| m.stream_id == "p-1")
        );
    }

    /// The table follows the stream: an intent owes, a claim puts the next
    /// look off to the lease's end, a deferral to the retry, an outcome
    /// removes the row, a failure keeps it for the page; a lapsed intent
    /// and an old failure are forgotten by the sweep.
    #[tokio::test]
    async fn the_table_follows_what_the_stream_says() {
        let store = store().await;
        let room = room(&store, topcoat::mail::MemoryTransport::new(), "node-a");
        let now = 1_000;
        let owe = |stream: &'static str| {
            room.record(
                Kind::Invitation,
                "",
                stream,
                Change::Owed {
                    version: 1,
                    expires_at: now + 100,
                },
                now,
            )
        };
        owe("inv-1").await.unwrap();
        owe("inv-2").await.unwrap();
        room.record(
            Kind::Invitation,
            "",
            "inv-1",
            Change::Claimed { until: now + 300 },
            now + 1,
        )
        .await
        .unwrap();
        let rows_now = rows(&store).await;
        assert_eq!(rows_now.len(), 2);
        assert_eq!(rows_now[0].due_at, now as i64 + 300);
        assert_eq!(rows_now[1].due_at, now as i64);
        room.record(
            Kind::Invitation,
            "",
            "inv-1",
            Change::Deferred {
                retry_at: now + 30,
                reason: "connection refused".into(),
            },
            now + 2,
        )
        .await
        .unwrap();
        let deferred = &rows(&store).await[0];
        assert_eq!((deferred.due_at, deferred.attempts), (now as i64 + 30, 1));
        assert_eq!(deferred.reason, "connection refused");
        room.record(Kind::Invitation, "", "inv-2", Change::Settled, now + 3)
            .await
            .unwrap();
        assert_eq!(rows(&store).await.len(), 1);
        room.record(
            Kind::Invitation,
            "",
            "inv-1",
            Change::Failed {
                reason: "still refused".into(),
            },
            now + 4,
        )
        .await
        .unwrap();
        let failed = &rows(&store).await[0];
        assert!(failed.failed);
        assert_eq!(failed.attempts, 2);
        assert_eq!(
            summary(&Db::from(store.clone())).await.unwrap(),
            Summary { owed: 0, failed: 1 }
        );
        assert_eq!(
            failed_mails(&Db::from(store.clone()), 10).await.unwrap()[0].stream_id,
            "inv-1"
        );
        // A new intent on the stream starts afresh.
        owe("inv-1").await.unwrap();
        let fresh = &rows(&store).await[0];
        assert!(!fresh.failed);
        assert_eq!((fresh.attempts, fresh.due_at), (0, now as i64));
        // Lapsed: nothing to send, and the sweep forgets it.
        assert_eq!(room.sweep(now + 101).await.unwrap(), 0);
        assert!(rows(&store).await.is_empty());
        // An old failure goes too.
        owe("inv-3").await.unwrap();
        room.record(
            Kind::Invitation,
            "",
            "inv-3",
            Change::Failed {
                reason: "gone".into(),
            },
            now,
        )
        .await
        .unwrap();
        room.sweep(now + FAILED_KEPT_SECS - 1).await.unwrap();
        assert_eq!(rows(&store).await.len(), 1);
        room.sweep(now + FAILED_KEPT_SECS).await.unwrap();
        assert!(rows(&store).await.is_empty());
    }
}
