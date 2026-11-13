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
//! one after the other while a bounded number of shops go at once, each
//! shop's lane on its own, so a slow server holds its shop's lane and
//! nothing else; the first send a shop's server fails ends its lane until
//! the retry. Each send
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

use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash as _, Hasher as _};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use timada_core::db::FromRow;
use timada_tenancy::ShopMailKind;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use topcoat::context::AppContext;

use crate::cluster::Executor as Store;
use crate::db::Db;
use crate::key_vault::KeyVault;
use crate::payments::Payments;

mod schema;

pub use schema::{MIGRATIONS, TABLES};

/// How often the table is looked at without a subscription waking the
/// mailroom: a deferred retry falls due, a node that died mid-send lets its
/// lease lapse.
pub const CHECK_EVERY: Duration = Duration::from_secs(5);

/// The most mails one sweep takes, every shop together.
const MAILS_PER_SWEEP: i64 = 200;

/// How many shops send at once. Within a shop, one mail after the other.
const LANES: usize = 16;

/// The most mails one lane takes per beat: the rest wait for the next.
const MAILS_PER_LANE: usize = 50;

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
    /// A shop's mail to the shopper of an order (`Order`, then `OrderMail`):
    /// which one, by its template.
    OrderMail(ShopMailKind),
    /// The platform's notice to a shop's members that a message arrived
    /// (`Message`, then `MessageNotification`).
    MessageNotice,
}

impl Kind {
    /// The order mails, one per template the shop sends about an order.
    pub const ORDER_MAILS: [ShopMailKind; 4] = [
        ShopMailKind::OrderConfirmation,
        ShopMailKind::OrderPaid,
        ShopMailKind::OrderShipped,
        ShopMailKind::OrderCancelled,
    ];

    /// The column's value: an order mail's is its template's key.
    pub fn key(self) -> &'static str {
        match self {
            Self::PlatformCode => "platform_code",
            Self::ShopperCode => "shopper_code",
            Self::Invitation => "invitation",
            Self::NewsletterConfirmation => "newsletter_confirmation",
            Self::OrderMail(mail) => mail.key(),
            Self::MessageNotice => "message_notice",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "platform_code" => Self::PlatformCode,
            "shopper_code" => Self::ShopperCode,
            "invitation" => Self::Invitation,
            "newsletter_confirmation" => Self::NewsletterConfirmation,
            "message_notice" => Self::MessageNotice,
            _ => Self::OrderMail(*Self::ORDER_MAILS.iter().find(|mail| mail.key() == value)?),
        })
    }

    /// For a person.
    pub fn label(self) -> &'static str {
        match self {
            Self::PlatformCode => "sign-in code",
            Self::ShopperCode => "shopper sign-in code",
            Self::Invitation => "invitation",
            Self::NewsletterConfirmation => "newsletter confirmation",
            Self::OrderMail(ShopMailKind::OrderPaid) => "order payment mail",
            Self::OrderMail(ShopMailKind::OrderShipped) => "order shipping mail",
            Self::OrderMail(ShopMailKind::OrderCancelled) => "order cancellation mail",
            Self::OrderMail(_) => "order confirmation",
            Self::MessageNotice => "message notice",
        }
    }
}

/// What the stream said, for the table: where the mail stands, never by how
/// much it moved, so the same change written twice leaves the row as once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A new intent: owed from now, until `expires_at` (unix seconds), its
    /// `version` what the claim is checked against.
    Owed { version: u16, expires_at: u64 },
    /// A node took the send until `until`: nothing to look at before.
    Claimed { until: u64 },
    /// The send failed, `attempts` tries in all as the stream counts them;
    /// the next try is at `retry_at`.
    Deferred {
        retry_at: u64,
        attempts: u32,
        reason: String,
    },
    /// Given up on after `attempts` tries.
    Failed { reason: String, attempts: u32 },
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
    /// The read models: the members' addresses, and this node's table.
    pub(crate) db: Db,
    /// The store: the platform's streams (users, tenants, invitations) and
    /// the shops' (shoppers, subscribers) live in the one store.
    pub(crate) store: Store,
    /// The platform's transport and sender, the shops' own servers, and the
    /// keys the codes and links derive under.
    pub(crate) mail: Arc<AppContext>,
    /// The tenants' keys, to open the shoppers' addresses and the messages.
    pub(crate) keys: Arc<KeyVault>,
    /// The payment provider: the platform's server mails about the orders
    /// the checkout placed and no other (timada #431).
    pub(crate) payments: Arc<Payments>,
    /// The dashboard's public address, for the invitation links and the
    /// inbox's.
    pub(crate) base_url: String,
    /// This node, as the claims name it.
    pub(crate) node: String,
    /// Rung by the subscriptions once they wrote a row.
    wake: Notify,
    /// The lanes sending right now: a shop's mails are not taken again
    /// while its lane runs, and no more than [`LANES`] run at once.
    busy: Mutex<HashSet<Lane>>,
}

/// What a lane is keyed by: the shop (`""` for the platform) and, for the
/// platform's mails, which of [`PLATFORM_LANES`] a stream hashes to.
type Lane = (String, u64);

impl Mailroom {
    pub fn new(
        db: Db,
        store: Store,
        mail: Arc<AppContext>,
        keys: Arc<KeyVault>,
        payments: Arc<Payments>,
        base_url: String,
        node: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            mail,
            keys,
            payments,
            base_url,
            node,
            wake: Notify::new(),
            busy: Mutex::default(),
        })
    }

    /// Records what a stream of `kind` in `tenant_id` said, from its
    /// subscription. Safe to repeat: a subscription delivers at least once,
    /// and a replayed or redelivered event writes the same row; the tries
    /// are the stream's own count, set, never added to (#521).
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
            Change::Deferred {
                retry_at,
                attempts,
                reason,
            } => {
                self.db
                    .execute(
                        "UPDATE owed_mails SET due_at = ?4, attempts = ?5, reason = ?6,
                            updated_at = ?7
                         WHERE kind = ?1 AND tenant_id = ?2 AND stream_id = ?3 AND failed = 0",
                        (
                            kind,
                            tenant_id,
                            stream_id,
                            retry_at as i64,
                            i64::from(attempts),
                            reason.as_str(),
                            now,
                        ),
                    )
                    .await?;
            }
            Change::Failed { reason, attempts } => {
                self.db
                    .execute(
                        "UPDATE owed_mails SET failed = 1, attempts = ?4, reason = ?5,
                            updated_at = ?6
                         WHERE kind = ?1 AND tenant_id = ?2 AND stream_id = ?3",
                        (
                            kind,
                            tenant_id,
                            stream_id,
                            i64::from(attempts),
                            reason.as_str(),
                            now,
                        ),
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
    /// subscription wrote a row. The lanes run on their own: a shop whose
    /// server is slow holds its lane, never the next look at the table.
    pub fn start(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(CHECK_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = self.wake.notified() => {}
                }
                if let Err(err) = self.dispatch(crate::auth::now()).await {
                    tracing::error!(error = %format!("{err:#}"), "owed mails not swept");
                }
            }
        })
    }

    /// Sends the mails due at `now` (unix seconds) and waits for every
    /// lane; returns how many were handed to a server. What
    /// [`start`](Self::start) does on each beat, for a test.
    #[cfg(test)]
    pub(crate) async fn sweep(self: &Arc<Self>, now: u64) -> Result<usize> {
        let mut sent = 0;
        for lane in self.dispatch(now).await? {
            sent += lane.await.unwrap_or(0);
        }
        Ok(sent)
    }

    /// Starts a lane for each shop with mails due at `now` and no lane
    /// running, up to [`LANES`] running at once, then forgets the intents
    /// that lapsed and the failures shown long enough. A send that fails is
    /// recorded on its stream and is not an error here; a store that cannot
    /// be read ends the beat, logged by the caller.
    async fn dispatch(self: &Arc<Self>, now: u64) -> Result<Vec<JoinHandle<usize>>> {
        let due: Vec<OwedMail> = self
            .db
            .fetch_all(
                "SELECT kind, tenant_id, stream_id, version, due_at, expires_at, attempts, failed,
                        reason, updated_at
                 FROM owed_mails
                 WHERE failed = 0 AND due_at <= ?1 AND expires_at > ?1
                 ORDER BY due_at, tenant_id, stream_id LIMIT ?2",
                (now as i64, MAILS_PER_SWEEP),
            )
            .await?;
        let mut lanes = Vec::new();
        for (lane, mut mails) in lanes_of(due) {
            {
                let mut busy = self.busy.lock().unwrap();
                if busy.len() >= LANES || !busy.insert(lane.clone()) {
                    continue;
                }
            }
            mails.truncate(MAILS_PER_LANE);
            let room = self.clone();
            lanes.push(tokio::spawn(async move {
                let sent = room.lane(&lane, mails, now).await;
                room.busy.lock().unwrap().remove(&lane);
                sent
            }));
        }
        self.db
            .execute(
                "DELETE FROM owed_mails WHERE (failed = 0 AND expires_at <= ?1)
                    OR (failed = 1 AND updated_at <= ?2)",
                (now as i64, now.saturating_sub(FAILED_KEPT_SECS) as i64),
            )
            .await?;
        Ok(lanes)
    }

    /// One shop's mails, one after the other, from `now` on (each send is
    /// stamped with the time it starts: a lane may run for minutes); how
    /// many went out. The first send the server fails ends the lane: the
    /// server is down for the shop's other mails too, so they wait for the
    /// first retry rather than each spend a timeout proving it.
    async fn lane(self: &Arc<Self>, lane: &Lane, mails: Vec<OwedMail>, now: u64) -> usize {
        let started = std::time::Instant::now();
        let mut sent = 0;
        let mut mails = mails.iter();
        while let Some(owed) = mails.next() {
            let at = now + started.elapsed().as_secs();
            match self.send(owed, at).await {
                Ok(Outcome::Sent) => sent += 1,
                Ok(Outcome::Recorded) => {}
                Ok(Outcome::Left) => self.put_off(owed, at, at + LOOK_AGAIN_SECS).await,
                Ok(Outcome::Dropped) => self.forget(owed, at).await,
                Ok(Outcome::Deferred) => {
                    tracing::warn!(lane = %lane.0, "a shop's mail server failed a send: its other mails wait for the retry");
                    for rest in mails.by_ref() {
                        self.put_off(rest, at, at + timada_core::RETRY_DELAYS_SECS[0])
                            .await;
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

    /// Moves `owed`'s next look to `until`, unless the table moved it past
    /// `now` meanwhile (the subscription saw a claim or a deferral).
    async fn put_off(&self, owed: &OwedMail, now: u64, until: u64) {
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
                    until as i64,
                    now as i64,
                ),
            )
            .await
        {
            tracing::warn!(error = %format!("{err:#}"), "an owed mail could not be put off");
        }
    }

    /// Forgets `owed`: nothing is owed after all. Every node that owes it
    /// reaches the same conclusion from the same streams.
    async fn forget(&self, owed: &OwedMail, now: u64) {
        let Some(kind) = owed.kind() else {
            return;
        };
        if let Err(err) = self
            .record(kind, &owed.tenant_id, &owed.stream_id, Change::Settled, now)
            .await
        {
            tracing::warn!(error = %format!("{err:#}"), "an owed mail could not be dropped");
        }
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
            Some(Kind::OrderMail(mail)) => crate::order_mails::send(self, owed, mail, now).await,
            Some(Kind::MessageNotice) => crate::message_notifications::send(self, owed, now).await,
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
    /// Not sent, and the stream says why (refused for good, given up on):
    /// the subscription moves the row.
    Recorded,
    /// Not sent: the server did not answer or refused for now, the stream
    /// says when to try again, and the shop's other mails can wait for it.
    Deferred,
    /// Nothing claimed: another node is sending, the next try is not due,
    /// or the stream settled or moved on already.
    Left,
    /// Nothing to send after all, and nothing to record on the stream (the
    /// mail is not the shopper's to get): the row goes.
    Dropped,
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
    use crate::payments::fake::FakeGateway;
    use crate::seal::Sealer;

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
        room_paid_through(store, mail, keys, FakeGateway::new(), node)
    }

    /// A mailroom whose payment provider is `gateway`: what the order mails
    /// the platform's server would send are checked against.
    pub(crate) fn room_paid_through(
        store: &Turso,
        mail: Arc<AppContext>,
        keys: Arc<KeyVault>,
        gateway: Arc<FakeGateway>,
        node: &str,
    ) -> Arc<Mailroom> {
        let evento = Evento::new(store.clone());
        Mailroom::new(
            Db::from(store.clone()),
            evento.clone(),
            mail,
            keys,
            Arc::new(Payments::new(
                Some(Sealer::new("payments", "secret")),
                gateway,
            )),
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
    /// removes the row, a failure keeps it for the page, with the tries the
    /// stream counted however often the change came; a lapsed intent and an
    /// old failure are forgotten by the sweep.
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
        // Delivered twice, as a subscription may after a crash (#521).
        for _ in 0..2 {
            room.record(
                Kind::Invitation,
                "",
                "inv-1",
                Change::Deferred {
                    retry_at: now + 30,
                    attempts: 1,
                    reason: "connection refused".into(),
                },
                now + 2,
            )
            .await
            .unwrap();
        }
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
                attempts: 2,
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
                attempts: 1,
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
