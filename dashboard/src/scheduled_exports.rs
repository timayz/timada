//! Sends the exports a shop's owner scheduled: its orders, its customers or
//! its newsletter's subscribers, each day, week or month, for the days
//! since the one before (`timada-orders`' `ExportSchedule`).
//!
//! Nothing in the event store fires later, so every node sweeps, on a
//! timer, the schedules the read model says are due, each node those of its
//! part of the shops ([`Share`]). The schedule's own stream decides: a run
//! is started on it at the version read (the claim), sent, then settled, so
//! two nodes that disagree for a moment on who is up send it once. A node
//! that stops between the two leaves the run open, and it is started again
//! a quarter of an hour later.
//!
//! Where a file goes is the owner's choice, and a file of people is not
//! sent lightly:
//!
//! - by mail, the file does not travel: the mail says it is ready and links
//!   to it in the back office, where it opens for a member signed in whose
//!   role reads it. A mailbox keeps no list of customers;
//! - to an address, the file is posted as the export page writes it, signed
//!   as a scenario's webhook is (`Timada-Signature`, HMAC-SHA256 of
//!   `"{t}.{body}"` under the schedule's own secret) with an
//!   `Idempotency-Key` that is the same on every try. The address has to be
//!   on the internet ([`crate::net`]) and is kept sealed.
//!
//! A posted file is held in memory to be signed, so it is refused past
//! [`MAX_POSTED_BYTES`]: a period's rows, not a shop's whole history.

use std::sync::Arc;
use std::time::Duration;

use jiff::{civil::Date, tz::TimeZone};
use timada_core::Share;
use timada_orders::{
    Command, ExportClock, ExportDestination, ExportKind, ExportOutcome, ExportRun,
    export_schedules_due,
};
use timada_tenancy::TenantState;
use tokio::task::JoinHandle;
use topcoat::context::{ContextMap, Cx};

use crate::automations::webhook::{self, Delivery};
use crate::cluster::Executor;
use crate::db::Db;
use crate::seal::WebhookKeys;

/// Who the runs say started and settled them.
pub const SENDER: &str = "timada-dashboard";

/// How often a node looks for exports that are due.
pub const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// How many due schedules one read takes.
const PAGE: i64 = 200;

/// The largest file posted to an address.
pub const MAX_POSTED_BYTES: usize = 32 * 1024 * 1024;

/// How long posting a file may take, connection and answer together.
const POST_TIMEOUT: Duration = Duration::from_secs(60);

/// What a sealed address is bound to beside its tenant and its kind: one
/// copied from a scenario's step does not open here.
const BINDING: &str = "scheduled-export";

/// The address of a schedule, sealed for its stream.
pub fn seal_address(
    keys: &WebhookKeys,
    tenant_id: &str,
    kind: ExportKind,
    url: &str,
) -> anyhow::Result<String> {
    webhook::seal_url(keys, tenant_id, BINDING, kind.key(), url)
}

/// The address a schedule holds, opened; `None` for one sealed under
/// another secret.
pub fn open_address(
    keys: &WebhookKeys,
    tenant_id: &str,
    kind: ExportKind,
    sealed: &str,
) -> Option<String> {
    webhook::open_url(keys, tenant_id, BINDING, kind.key(), sealed)
}

/// The secret a schedule's files are signed under, for the receiver to
/// check them with.
pub fn signing_secret(keys: &WebhookKeys, tenant_id: &str, kind: ExportKind) -> String {
    webhook::signing_secret(keys, tenant_id, BINDING, kind.key())
}

/// What the back office calls a kind of export.
pub fn label(kind: ExportKind) -> &'static str {
    match kind {
        ExportKind::Orders => "orders",
        ExportKind::Customers => "customers",
        ExportKind::Newsletter => "newsletter subscribers",
    }
}

/// The export's own page in the back office, for the days of a run: where
/// the mail's link leads.
pub fn export_link(
    base_url: &str,
    slug: &str,
    kind: ExportKind,
    first_day: Date,
    last_day: Date,
) -> String {
    format!(
        "{}/t/{slug}/{}/export?from={first_day}&to={last_day}",
        base_url.trim_end_matches('/'),
        kind.key(),
    )
}

/// The mail that says a file is ready.
pub struct Notice<'a> {
    pub shop: &'a TenantState,
    pub to: &'a str,
    pub run: &'a ExportRun,
    pub link: String,
}

/// A file on its way to an address.
pub struct Post<'a> {
    pub url: &'a str,
    pub body: Vec<u8>,
    pub signature: String,
    pub idempotency_key: String,
    pub file_name: String,
    pub run: &'a ExportRun,
}

/// What the exports go out through: the platform's mail and the internet,
/// or a test's.
#[async_trait::async_trait]
pub trait Deliver: Send + Sync {
    async fn mail(&self, notice: Notice<'_>) -> anyhow::Result<()>;
    async fn post(&self, post: Post<'_>) -> Delivery;
}

/// The platform's transport and sender, and an HTTP client that reaches
/// public hosts only (unless `allow_private_hosts`: development and tests).
pub struct Outbox {
    mail: Arc<ContextMap>,
    http: reqwest::Client,
    allow_private_hosts: bool,
}

impl Outbox {
    pub fn new(mail: Arc<ContextMap>, allow_private_hosts: bool) -> anyhow::Result<Self> {
        Ok(Self {
            mail,
            http: crate::net::public_client(allow_private_hosts)
                .timeout(POST_TIMEOUT)
                .build()?,
            allow_private_hosts,
        })
    }
}

#[async_trait::async_trait]
impl Deliver for Outbox {
    async fn mail(&self, notice: Notice<'_>) -> anyhow::Result<()> {
        let cx = Cx::new().with_shared(self.mail.clone());
        crate::mail::send_export_ready(
            &cx,
            notice.to,
            &notice.shop.name,
            label(notice.run.kind),
            &notice.run.first_day.to_string(),
            &notice.run.last_day.to_string(),
            &notice.link,
        )
        .await
        .map_err(|err| anyhow::anyhow!("{err}"))
    }

    async fn post(&self, post: Post<'_>) -> Delivery {
        if !self.allow_private_hosts
            && let Err(err) = crate::net::check_public_url(post.url).await
        {
            return Delivery::Refused(err.to_string());
        }
        let sent = self
            .http
            .post(post.url)
            .header("content-type", "text/csv; charset=utf-8")
            .header(
                "content-disposition",
                format!("attachment; filename=\"{}\"", post.file_name),
            )
            .header("timada-signature", post.signature)
            .header("idempotency-key", post.idempotency_key)
            .header("timada-export", post.run.kind.key())
            .header(
                "timada-export-days",
                format!("{}/{}", post.run.first_day, post.run.last_day),
            )
            .body(post.body)
            .send()
            .await;
        match sent {
            Ok(response) => webhook::status_delivery(response.status().as_u16()),
            // Without its address, which may be the receiver's only key.
            Err(err) => Delivery::Again(format!(
                "The address could not be reached: {}",
                err.without_url()
            )),
        }
    }
}

/// The sweep, with what it needs outside any request.
pub struct ScheduledExports<D: Deliver = Outbox> {
    /// The read models: the schedules due, and the rows of the files.
    db: Db,
    /// The store, every tenant's and the platform's, read from this node's
    /// file: a run is started at the version read, so a node a moment
    /// behind another's finds it taken.
    store: Executor,
    /// This node's place among the nodes up, as of now: which shops'
    /// exports it sends.
    place: Arc<dyn Fn() -> Share + Send + Sync>,
    deliver: D,
    /// What the addresses are sealed and the files signed with; `None`
    /// without the operator's secret: no address could be set either.
    keys: Option<WebhookKeys>,
    /// The dashboard's public address, for the mail's link.
    base_url: String,
}

impl<D: Deliver + 'static> ScheduledExports<D> {
    pub fn new(
        db: Db,
        store: Executor,
        place: Arc<dyn Fn() -> Share + Send + Sync>,
        deliver: D,
        keys: Option<WebhookKeys>,
        base_url: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            place,
            deliver,
            keys,
            base_url,
        })
    }

    /// Sweeps every `every`, the first time right away.
    pub fn start(self: Arc<Self>, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                match self.sweep(crate::auth::now()).await {
                    Ok(0) => {}
                    Ok(sent) => tracing::info!(sent, "scheduled exports sent"),
                    Err(err) => tracing::error!(error = %err, "scheduled exports not swept"),
                }
            }
        })
    }

    /// Sends the exports due at `now` (unix seconds) among the shops of
    /// this node's part; returns how many were delivered. A schedule that
    /// fails is logged and the others go on.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<usize> {
        let share = (self.place)();
        let mut delivered = 0;
        let mut after = None;
        loop {
            let due = export_schedules_due(&self.db, now as i64, after.as_ref(), PAGE).await?;
            for row in due.iter().filter(|row| share.owns(&row.tenant_id)) {
                let Some(kind) = ExportKind::parse(&row.what) else {
                    continue;
                };
                match self.run(&row.tenant_id, kind, now).await {
                    Ok(true) => delivered += 1,
                    Ok(false) => {}
                    Err(err) => {
                        tracing::error!(tenant = %row.tenant_id, what = kind.key(), error = %err, "a scheduled export not sent");
                    }
                }
            }
            if (due.len() as i64) < PAGE {
                return Ok(delivered);
            }
            after = due.last().cloned();
        }
    }

    /// Starts, sends and settles the export of `kind` of one shop, if it is
    /// due on the write model; whether it was delivered.
    async fn run(&self, tenant_id: &str, kind: ExportKind, now: u64) -> anyhow::Result<bool> {
        let Some(shop) = timada_tenancy::Command(self.store.clone())
            .load_tenant(tenant_id)
            .await?
        else {
            return Ok(false);
        };
        if shop.is_suspended() {
            return Ok(false);
        }
        let zone = TimeZone::get(shop.shop_time_zone()).unwrap_or(TimeZone::UTC);
        let clock = ExportClock::new(now, zone.clone());
        let orders = Command::new(self.store.clone(), tenant_id);
        // The write model decides; the read model only said where to look.
        let Some(run) = orders.start_export_run(SENDER, kind, &clock).await? else {
            return Ok(false);
        };
        let outcome = self.send(&shop, &zone, &run, now).await;
        match &outcome {
            ExportOutcome::Delivered => {
                tracing::info!(tenant = %tenant_id, what = kind.key(), first = %run.first_day, last = %run.last_day, "scheduled export sent");
            }
            ExportOutcome::Again(reason) | ExportOutcome::Refused(reason) => {
                tracing::warn!(tenant = %tenant_id, what = kind.key(), attempt = run.attempt, reason, "scheduled export not taken");
            }
        }
        let delivered = outcome == ExportOutcome::Delivered;
        orders
            .settle_export_run(SENDER, kind, run.attempt, outcome, &clock)
            .await?;
        Ok(delivered)
    }

    async fn send(
        &self,
        shop: &TenantState,
        zone: &TimeZone,
        run: &ExportRun,
        now: u64,
    ) -> ExportOutcome {
        match &run.destination {
            ExportDestination::Mail(to) => {
                let link = export_link(
                    &self.base_url,
                    &shop.slug,
                    run.kind,
                    run.first_day,
                    run.last_day,
                );
                match self
                    .deliver
                    .mail(Notice {
                        shop,
                        to,
                        run,
                        link,
                    })
                    .await
                {
                    Ok(()) => ExportOutcome::Delivered,
                    Err(err) => ExportOutcome::Again(format!("The mail did not go out: {err}")),
                }
            }
            ExportDestination::Url { sealed } => {
                let Some(keys) = &self.keys else {
                    return ExportOutcome::Refused(
                        "This installation no longer keeps addresses: its operator took its secret off."
                            .to_owned(),
                    );
                };
                let Some(url) = open_address(keys, &shop.id, run.kind, sealed) else {
                    return ExportOutcome::Refused(
                        "The address can no longer be read: save it again.".to_owned(),
                    );
                };
                let period =
                    crate::app::t::slug::sales::period_of_days(zone, run.first_day, run.last_day);
                let body = match crate::export::file(
                    self.db.clone(),
                    shop.id.clone(),
                    run.kind,
                    period,
                    MAX_POSTED_BYTES,
                )
                .await
                {
                    Ok(Some(body)) => body,
                    Ok(None) => {
                        return ExportOutcome::Refused(format!(
                            "The file is larger than {} MB: export it from its page.",
                            MAX_POSTED_BYTES / (1024 * 1024)
                        ));
                    }
                    Err(err) => {
                        tracing::error!(tenant = %shop.id, error = %err, "a scheduled export's file not written");
                        return ExportOutcome::Again("The file could not be written.".to_owned());
                    }
                };
                let secret = signing_secret(keys, &shop.id, run.kind);
                let signature = webhook::signature(&secret, &body, now);
                let delivery = self
                    .deliver
                    .post(Post {
                        url: &url,
                        body,
                        signature,
                        idempotency_key: format!(
                            "{}:{}:{}:{}",
                            shop.id,
                            run.kind.key(),
                            run.first_day,
                            run.last_day
                        ),
                        file_name: format!(
                            "{}-{}-{}-{}.csv",
                            run.kind.key(),
                            shop.slug,
                            run.first_day,
                            run.last_day
                        ),
                        run,
                    })
                    .await;
                match delivery {
                    Delivery::Taken(_) => ExportOutcome::Delivered,
                    Delivery::Again(reason) => ExportOutcome::Again(reason),
                    Delivery::Refused(reason) => ExportOutcome::Refused(reason),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use evento::Evento;
    use timada_orders::ExportEvery;
    use timada_tenancy::CreateTenant;

    use super::*;

    /// What went out, as the test's outbox saw it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Sent {
        Mail {
            to: String,
            link: String,
        },
        Post {
            url: String,
            body: Vec<u8>,
            signature: String,
            idempotency_key: String,
            file_name: String,
        },
    }

    /// Keeps what goes out, and answers a post what it is told to.
    struct Recording {
        sent: Arc<Mutex<Vec<Sent>>>,
        answer: Arc<Mutex<u16>>,
    }

    #[async_trait::async_trait]
    impl Deliver for Recording {
        async fn mail(&self, notice: Notice<'_>) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push(Sent::Mail {
                to: notice.to.to_owned(),
                link: notice.link,
            });
            Ok(())
        }

        async fn post(&self, post: Post<'_>) -> Delivery {
            self.sent.lock().unwrap().push(Sent::Post {
                url: post.url.to_owned(),
                body: post.body,
                signature: post.signature,
                idempotency_key: post.idempotency_key,
                file_name: post.file_name,
            });
            webhook::status_delivery(*self.answer.lock().unwrap())
        }
    }

    fn keys() -> WebhookKeys {
        WebhookKeys {
            sealer: crate::seal::Sealer::new(
                "automation-webhook",
                "a-secrets-key-of-thirty-two-chars!!",
            ),
            signing: [7; 32],
        }
    }

    /// Unix seconds of a time in UTC, where a shop lives until it says
    /// otherwise (`2026-11-18T12:00`).
    fn at(time: &str) -> u64 {
        time.parse::<jiff::civil::DateTime>()
            .unwrap()
            .to_zoned(TimeZone::UTC)
            .unwrap()
            .timestamp()
            .as_second() as u64
    }

    struct Rig {
        store: Evento,
        db: Db,
        shop: TenantState,
        sent: Arc<Mutex<Vec<Sent>>>,
        answer: Arc<Mutex<u16>>,
    }

    impl Rig {
        async fn new() -> Self {
            let turso = evento::Turso::in_memory_concurrent().await.unwrap();
            crate::db::migrate_store_read_models(&turso).await.unwrap();
            let db = Db::from(turso.clone());
            let store = Evento::new(turso);
            let shop = timada_tenancy::Command(store.clone())
                .create_tenant(CreateTenant {
                    name: "acme".to_owned(),
                    owner_user_id: "alice".to_owned(),
                })
                .await
                .unwrap();
            Self {
                store,
                db,
                shop,
                sent: Arc::default(),
                answer: Arc::new(Mutex::new(200)),
            }
        }

        fn orders(&self) -> Command<Evento> {
            Command::new(self.store.clone(), &self.shop.id)
        }

        fn node(&self, share: Share) -> Arc<ScheduledExports<Recording>> {
            ScheduledExports::new(
                self.db.clone(),
                self.store.clone(),
                Arc::new(move || share),
                Recording {
                    sent: self.sent.clone(),
                    answer: self.answer.clone(),
                },
                Some(keys()),
                "https://dashboard.example/".to_owned(),
            )
        }

        /// The read model the sweep reads, brought up to date.
        async fn catch_up(&self) {
            timada_orders::read_model_subscription::<Evento>(self.db.clone())
                .run_once(&self.store)
                .await
                .unwrap();
        }

        fn sent(&self) -> Vec<Sent> {
            self.sent.lock().unwrap().clone()
        }
    }

    /// A week's orders are told by mail once the week ended, with a link to
    /// the file and no file, however many nodes sweep at that moment.
    #[tokio::test]
    async fn a_mailed_export_is_a_link_sent_once_also_with_two_nodes() {
        let rig = Rig::new().await;
        rig.orders()
            .schedule_export(
                "alice",
                ExportKind::Orders,
                ExportEvery::Weekly,
                ExportDestination::Mail("books@example.org".into()),
                &ExportClock::new(at("2026-11-18T12:00"), TimeZone::UTC),
            )
            .await
            .unwrap();
        rig.catch_up().await;
        let (one, two) = (rig.node(Share::ALONE), rig.node(Share::ALONE));

        // The week still runs.
        assert_eq!(one.sweep(at("2026-11-22T23:00")).await.unwrap(), 0);
        assert!(rig.sent().is_empty());

        let due = at("2026-11-23T01:00");
        let (first, second) = tokio::join!(one.sweep(due), two.sweep(due));
        assert_eq!(first.unwrap() + second.unwrap(), 1);
        assert_eq!(
            rig.sent(),
            [Sent::Mail {
                to: "books@example.org".into(),
                link: format!(
                    "https://dashboard.example/t/{}/orders/export?from=2026-11-16&to=2026-11-22",
                    rig.shop.slug
                ),
            }]
        );

        // Swept again before the read model caught up, and after: nothing
        // until the next week ended.
        assert_eq!(two.sweep(due + 60).await.unwrap(), 0);
        rig.catch_up().await;
        assert_eq!(one.sweep(at("2026-11-29T12:00")).await.unwrap(), 0);
        assert_eq!(rig.sent().len(), 1);
        assert_eq!(one.sweep(at("2026-11-30T01:00")).await.unwrap(), 1);
        assert_eq!(rig.sent().len(), 2);
    }

    /// The file itself is posted to an address, signed under the
    /// schedule's secret; an address that fails is asked again with the
    /// same key, then given up.
    #[tokio::test]
    async fn a_posted_export_is_signed_and_tried_again_under_the_same_key() {
        let rig = Rig::new().await;
        let kind = ExportKind::Customers;
        let sealed = seal_address(
            &keys(),
            &rig.shop.id,
            kind,
            "https://books.example/in/s3cret",
        )
        .unwrap();
        // Sealed for this schedule: another kind's does not open.
        assert_eq!(
            open_address(&keys(), &rig.shop.id, ExportKind::Orders, &sealed),
            None
        );
        rig.orders()
            .schedule_export(
                "alice",
                kind,
                ExportEvery::Daily,
                ExportDestination::Url { sealed },
                &ExportClock::new(at("2026-11-18T12:00"), TimeZone::UTC),
            )
            .await
            .unwrap();
        rig.catch_up().await;
        let node = rig.node(Share::ALONE);

        // The address is down.
        *rig.answer.lock().unwrap() = 503;
        let mut now = at("2026-11-19T01:00");
        assert_eq!(node.sweep(now).await.unwrap(), 0);
        let key = format!("{}:customers:2026-11-18:2026-11-18", rig.shop.id);
        let [
            Sent::Post {
                url,
                body,
                signature,
                idempotency_key,
                file_name,
            },
        ] = &rig.sent()[..]
        else {
            panic!("one file posted: {:?}", rig.sent());
        };
        assert_eq!(url, "https://books.example/in/s3cret");
        assert_eq!(idempotency_key, &key);
        assert_eq!(
            file_name,
            &format!("customers-{}-2026-11-18-2026-11-18.csv", rig.shop.slug)
        );
        // Nobody signed in that day: the file is its columns.
        assert_eq!(
            String::from_utf8(body.clone()).unwrap(),
            "\u{feff}email,customer_since,orders,saved_addresses\r\n"
        );
        let secret = signing_secret(&keys(), &rig.shop.id, kind);
        assert!(crate::payments::signature::verify(
            body, signature, &secret, now as i64
        ));

        // Not asked again before its time; then with the same key.
        rig.catch_up().await;
        assert_eq!(node.sweep(now + 60).await.unwrap(), 0);
        assert_eq!(rig.sent().len(), 1);
        now += 5 * 60;
        *rig.answer.lock().unwrap() = 200;
        assert_eq!(node.sweep(now).await.unwrap(), 1);
        let sent = rig.sent();
        let Sent::Post {
            idempotency_key, ..
        } = &sent[1]
        else {
            panic!("a second post");
        };
        assert_eq!(idempotency_key, &key);
        let state = rig
            .orders()
            .load_export_schedule(kind)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.ended.unwrap().failure, None);

        // An address that says no is not asked twice: the day is given up,
        // and the next one goes out in its turn.
        rig.catch_up().await;
        *rig.answer.lock().unwrap() = 410;
        assert_eq!(node.sweep(at("2026-11-20T01:00")).await.unwrap(), 0);
        rig.catch_up().await;
        assert_eq!(node.sweep(at("2026-11-20T08:00")).await.unwrap(), 0);
        assert_eq!(rig.sent().len(), 3);
        let state = rig
            .orders()
            .load_export_schedule(kind)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            state.ended.unwrap().failure.as_deref(),
            Some("The endpoint answered 410.")
        );
    }

    /// A node sends the exports of its part of the shops.
    #[tokio::test]
    async fn a_node_leaves_the_other_nodes_shops_alone() {
        let rig = Rig::new().await;
        rig.orders()
            .schedule_export(
                "alice",
                ExportKind::Newsletter,
                ExportEvery::Daily,
                ExportDestination::Mail("books@example.org".into()),
                &ExportClock::new(at("2026-11-18T12:00"), TimeZone::UTC),
            )
            .await
            .unwrap();
        rig.catch_up().await;
        let now = at("2026-11-19T01:00");
        let mine = Share { index: 0, of: 2 };
        let (owner, other) = if mine.owns(&rig.shop.id) {
            (mine, Share { index: 1, of: 2 })
        } else {
            (Share { index: 1, of: 2 }, mine)
        };
        assert_eq!(rig.node(other).sweep(now).await.unwrap(), 0);
        assert!(rig.sent().is_empty());
        assert_eq!(rig.node(owner).sweep(now).await.unwrap(), 1);
        assert_eq!(rig.sent().len(), 1);
    }
}
