//! Takes the runs through their steps: nothing in the event store fires
//! later, so every node looks for the runs due (`automations_runs`) on a
//! timer and moves each as far as it can now. The nodes share the runs out:
//! each moves the ones whose id falls to it among the nodes up
//! ([`timada_core::Share`]), and any run its node has not moved a while
//! after it fell due ([`HELP_AFTER_SECS`]: that node stopped, or the nodes
//! disagreed for a moment on who is up). Any node can move any run: what a
//! move reads (the run, the scenario's state and its versions) follows
//! every region. Every move is written against the version the run was
//! loaded at and folded into the node's copy: a node that loses a race
//! writes nothing and leaves the run to the winner, which goes on without
//! reading the run back.
//!
//! A look moves `runs_per_check` runs at most, the most overdue first. A
//! run it leaves as it was (its shop suspended, a mail waiting for the shop
//! to set up what it needs, its version not seen here yet, a failure to
//! move it) does not count: its row on this node is put off
//! ([`LOOK_AGAIN_SECS`]), so the look goes on to the others and the next
//! ones do not read it again before then, and no number of held runs fills
//! a look. A paused scenario's runs are not due at all until it goes on.
//!
//! A step that acts outside the run (a mail) is claimed first and acts only
//! for the node whose claim is written; it is not tried again, since a
//! member mailed twice is worse off than one not mailed. A claim whose
//! outcome never comes (the node stopped) fails the step once its lease is
//! over.
//!
//! Each look also starts the runs an event made that fell to another node
//! and that it did not start (`automations_starts`), and finds the
//! checkouts left unpaid, each node its own part of them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::db::Db;
use timada_automations::{
    AUTOMATION_ACTOR, DueRun, RUN_HISTORY_SECS, RunState, ScenarioStatus, Step, Transition,
    catalog::{self, CodeValue, DISCORD, DISCOUNT, EMAIL, NOTIFY, Subject, WEBHOOK},
    due_runs,
    interpreter::{self, Next, STUCK_SETTINGS, STUCK_UNKNOWN},
    left_checkouts, normalize_email, opt_out_token, prune_checkouts, prune_runs, prune_starts,
    put_off_run, record_detected, run_known, scenario_name, scenario_status, settle_start,
    starts_owed,
};
use timada_core::Share;
use timada_core::keys::KeyRing;
use timada_identity::UserRow;
use timada_newsletter::{SubscriberStatus, subscriber_id};
use timada_orders::OrderState;
use timada_tenancy::{
    PERMISSION_CUSTOMERS_READ, PERMISSION_NEWSLETTER_READ, PERMISSION_ORDERS_READ,
    PERMISSION_REVIEWS_READ, PERMISSION_SUPPORT_READ, TenantState,
};
use tokio::task::JoinHandle;
use topcoat::context::{AppContext, Cx};

use super::definitions::Definitions;
use super::facts::facts_of;
use super::triggers::TRIGGER_WINDOW_SECS;
use crate::cluster::{Executor, ShareOf};
use crate::mail::{
    CampaignCopy, automation_vars, marketing_sender, render_automated, send_automated,
    send_automation_notice,
};
use crate::seal::{OptOutKey, WebhookKeys};

/// What the runner needs of a store.
pub trait Store: evento::Executor + Clone + Send + Sync + 'static {}

impl<E: evento::Executor + Clone + Send + Sync + 'static> Store for E {}

/// The most moves one run makes in one look: a scenario is at most 50
/// steps, and a run that keeps moving is looked at again next time.
const MOVES_PER_LOOK: usize = 60;
/// How long a run waits at a mail to the customer for the shop to set up
/// what it needs (a mail server picked for its marketing, its storefront's
/// address) before the step fails, in seconds.
const MAIL_HOLD_SECS: u64 = 3 * 24 * 60 * 60;
/// How long after its delay a checkout left unpaid may still be found:
/// older ones (a node away, the first look of a new scenario) start
/// nothing.
const LEFT_WINDOW_SECS: u64 = 3 * 24 * 60 * 60;
/// How long a node leaves another its part of the work, in seconds: a run
/// due, a run to start, a checkout to look at that much past its time is
/// taken by any node. A node that stops drops out of the nodes up within
/// [`crate::cluster::LIVE_WITHIN`], and its part goes to the others then;
/// this is for a node still heard from that does not do its part, and for
/// the moment the nodes disagree on who is up. At least [`HELP_LOOKS`]
/// looks of a node doing its part fit in it, so the others do not race it.
pub const HELP_AFTER_SECS: u64 = 2 * 60;
/// The looks of the runner a node is left for its part, at the least: the
/// wait is longer than [`HELP_AFTER_SECS`] when the looks are further apart.
const HELP_LOOKS: u64 = 4;
/// How long, in seconds, a run this node left as it was waits before the
/// node looks at it again: one held (its shop suspended, a mail waiting
/// for the shop to set up what it needs, its scenario or version not seen
/// here yet), ended or moved by another node since its row was written,
/// or not moved for a failure. Any move of the run, by any node, writes
/// its row again.
const LOOK_AGAIN_SECS: u64 = 5 * 60;
/// How many rows of the runs due one look reads at most for each run it
/// may move: the runs it leaves as they were do not count against
/// `runs_per_check`, but a look does not read them without end.
const ROWS_PER_MOVE: u32 = 4;
/// About how many rows of runs that ended 90 days ago one look drops, in a
/// short transaction: looking every 15 seconds (the default), over five
/// million a day, far more than runs end. A backlog (a node back after days
/// away) goes at that pace.
const PRUNED_PER_LOOK: u32 = 1_000;

/// What a look did with a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Looked {
    /// This node moved it on, a step or more (it may have ended).
    Moved,
    /// Nothing to do before then: it moved since its row was written (a
    /// wait began, another node acts on a step), which the row does not
    /// show yet.
    Asleep(u64),
    /// Left as it was: held (a mail waiting for the shop to set up what it
    /// needs, its scenario paused or its version not seen here yet), or
    /// nothing to do (ended, moved by another node first, not stored).
    Left,
}

/// What a step that acts did.
enum Acted {
    /// Its output, as JSON.
    Done(String),
    /// Nothing to do, and why.
    Skipped(String),
    /// Why it did not act.
    Failed(String),
    /// Why it did not act this time: asking again is safe (the step makes
    /// the same thing whatever the attempt), so it is tried again later.
    Retry(String),
}

/// When a step that may be tried again is, after its `attempt`th try; none
/// once it was tried enough.
fn retry_at(now: u64, attempt: u32) -> Option<u64> {
    const DELAYS: [u64; 4] = [60, 5 * 60, 30 * 60, 2 * 60 * 60];
    DELAYS
        .get(attempt.saturating_sub(1) as usize)
        .map(|delay| now + delay)
}

/// The sweep, with what it needs outside any request.
pub struct Runner<E = Executor> {
    /// The read models: the runs, the listeners, the checkouts, the members
    /// to tell.
    db: Db,
    /// The store, every tenant's.
    store: E,
    mail: Arc<AppContext>,
    /// The dashboard's public address, for the links the notices carry.
    base_url: String,
    keys: Arc<dyn KeyRing>,
    /// Signs the links that stop a shop's automated mails; none without the
    /// operator's secret, and then no mail goes to a customer.
    opt_out_key: Option<OptOutKey>,
    /// Opens the webhooks' addresses and signs their calls; none without
    /// the operator's secret, and then no webhook is called.
    webhooks: Option<WebhookKeys>,
    /// What the webhook calls go out through.
    http: reqwest::Client,
    /// Whether a webhook may call a host on this machine or network
    /// (development and tests only).
    allow_private_hosts: bool,
    /// How long, in seconds, a checkout waits for its order before it is
    /// found left.
    abandoned_after: u64,
    runs_per_look: u32,
    /// How often this node looks.
    every: Duration,
    /// How long, in seconds, the others leave a node its part of the work.
    help_after: u64,
    /// This node's place among the nodes up (in a region when asked for
    /// one): which runs it moves, which checkouts it looks at.
    place: ShareOf,
    definitions: Definitions,
}

impl<E: Store> Runner<E> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Db,
        store: E,
        mail: Arc<AppContext>,
        base_url: String,
        keys: Arc<dyn KeyRing>,
        opt_out_key: Option<OptOutKey>,
        webhooks: Option<WebhookKeys>,
        webhook_timeout: Duration,
        allow_private_hosts: bool,
        abandoned_after: Duration,
        runs_per_look: u32,
        every: Duration,
        place: ShareOf,
    ) -> anyhow::Result<Arc<Self>> {
        // rustls needs a process-wide crypto provider; installing twice is fine.
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
        // The webhooks' addresses are the merchants': no redirect, public
        // hosts only (the resolver checks the address dialled).
        let http = crate::net::public_client(allow_private_hosts)
            .timeout(webhook_timeout)
            .build()?;
        Ok(Arc::new(Self {
            db,
            store,
            mail,
            base_url,
            keys,
            opt_out_key,
            webhooks,
            http,
            allow_private_hosts,
            abandoned_after: abandoned_after.as_secs(),
            runs_per_look: runs_per_look.max(1),
            every,
            help_after: HELP_AFTER_SECS.max(HELP_LOOKS * every.as_secs()),
            place,
            definitions: Definitions::default(),
        }))
    }

    /// Sweeps every `every` given to [`new`](Self::new), the first time
    /// right away.
    pub fn start(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(self.every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                if let Err(err) = self.sweep(crate::auth::now()).await {
                    tracing::error!(error = %err, "automation runs not swept");
                }
            }
        })
    }

    /// Drops the rows of runs that ended long ago, starts the runs other
    /// nodes left, looks for the checkouts left unpaid, then moves the runs
    /// due by `now` that fall to this node.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<()> {
        let ended_before = now.saturating_sub(RUN_HISTORY_SECS);
        if let Err(err) = prune_runs(&self.db, ended_before, PRUNED_PER_LOOK).await {
            tracing::error!(error = %err, "runs ended long ago not dropped");
        }
        if let Err(err) = self.start_left_runs(now).await {
            tracing::error!(error = %err, "runs other nodes left not started");
        }
        if let Err(err) = self.find_left_checkouts(now).await {
            tracing::error!(error = %err, "checkouts left unpaid not looked for");
        }
        self.move_due_runs(now).await
    }

    /// Walks the runs due by `now` that fall to this node, the most overdue
    /// first, until `runs_per_check` of them moved, or it read
    /// [`ROWS_PER_MOVE`] rows per run it may move. A run it leaves as it
    /// was is put off on this node, to when it wakes if it sleeps, else
    /// [`LOOK_AGAIN_SECS`] later: the walk goes on past it, and the next
    /// looks do not read it before then. A run that fails to move is logged
    /// and put off the same.
    async fn move_due_runs(&self, now: u64) -> anyhow::Result<()> {
        let most_read = self.runs_per_look.saturating_mul(ROWS_PER_MOVE);
        let mut tenants: HashMap<String, Option<TenantState>> = HashMap::new();
        let (mut moved, mut read) = (0, 0);
        let mut after: Option<DueRun> = None;
        while moved < self.runs_per_look && read < most_read {
            let page = self.due(now, after.as_ref(), self.runs_per_look).await?;
            let last = page.len() < self.runs_per_look as usize;
            for due in &page {
                if moved == self.runs_per_look || read == most_read {
                    break;
                }
                read += 1;
                match self.look(&mut tenants, due, now).await {
                    Looked::Moved => moved += 1,
                    Looked::Asleep(until) => self.put_off(due, until).await,
                    Looked::Left => self.put_off(due, now + LOOK_AGAIN_SECS).await,
                }
            }
            if last {
                break;
            }
            after = page.into_iter().last();
        }
        Ok(())
    }

    /// Looks at one run of the walk: its shop, read once a look, then the
    /// run.
    async fn look(
        &self,
        tenants: &mut HashMap<String, Option<TenantState>>,
        due: &DueRun,
        now: u64,
    ) -> Looked {
        if !tenants.contains_key(&due.tenant_id) {
            let tenant = timada_tenancy::Command(self.store.clone())
                .load_tenant(&due.tenant_id)
                .await
                .unwrap_or_else(|err| {
                    tracing::error!(tenant = %due.tenant_id, error = %err, "shop not read: its automation runs wait");
                    None
                });
            tenants.insert(due.tenant_id.clone(), tenant);
        }
        match tenants.get(&due.tenant_id).and_then(Option::as_ref) {
            // A suspended shop's runs wait for it.
            Some(tenant) if !tenant.is_suspended() => {
                self.advance(tenant, &due.id, now)
                    .await
                    .unwrap_or_else(|err| {
                        tracing::error!(tenant = %due.tenant_id, run = %due.id, error = %err, "automation run not moved");
                        Looked::Left
                    })
            }
            _ => Looked::Left,
        }
    }

    /// Puts this node's next look at a run it left as it was off to
    /// `until`. One not put off (the write failed) is read again at the
    /// next look, nothing worse.
    async fn put_off(&self, due: &DueRun, until: u64) {
        if let Err(err) = put_off_run(&self.db, due, until).await {
            tracing::warn!(tenant = %due.tenant_id, run = %due.id, error = %format!("{err:#}"), "automation run not put off");
        }
    }

    /// The runs due by `now` that this node moves, `limit` at most after
    /// `after`: its own part of them, and any its node left for a while
    /// ([`HELP_AFTER_SECS`]).
    async fn due(
        &self,
        now: u64,
        after: Option<&DueRun>,
        limit: u32,
    ) -> anyhow::Result<Vec<DueRun>> {
        // The read model holds every region's runs, and any node moves any
        // run: the share is among all the nodes up.
        let share = (self.place)(None);
        let help_before = now.saturating_sub(self.help_after);
        due_runs(&self.db, now, share, help_before, after, limit).await
    }

    /// Moves one run as far as it goes at `now`, from one read of it, and
    /// says what came of it.
    pub async fn advance(
        &self,
        tenant: &TenantState,
        run_id: &str,
        now: u64,
    ) -> anyhow::Result<Looked> {
        let automations = timada_automations::Command::new(self.store.clone(), &tenant.id);
        let Some(mut run) = automations.load_run(run_id).await? else {
            return Ok(Looked::Left);
        };
        // What the look comes to where the run stops: moved, once it moved.
        let mut looked = Looked::Left;
        for _ in 0..MOVES_PER_LOOK {
            if run.status.is_done() {
                return Ok(looked);
            }
            // From the read model of the scenarios, which every node keeps
            // whatever its region: the listeners are the trigger
            // subscription's, which follows the node's region alone.
            match scenario_status(&self.db, &tenant.id, &run.scenario_id).await? {
                Some(ScenarioStatus::Active) => {}
                Some(ScenarioStatus::Archived) => {
                    let cancel = Transition::Cancel {
                        reason: "The scenario was deleted.".into(),
                    };
                    let cancelled = automations
                        .advance_run(AUTOMATION_ACTOR, &mut run, &[cancel])
                        .await?;
                    return Ok(if cancelled { Looked::Moved } else { looked });
                }
                // Held, or published a moment ago and not seen here yet.
                Some(ScenarioStatus::Paused | ScenarioStatus::Draft) | None => return Ok(looked),
            }
            let Some(definition) = self
                .definitions
                .get(&self.db, &tenant.id, &run.scenario_id, run.scenario_version)
                .await?
            else {
                return Ok(looked);
            };
            let moved = match interpreter::next(&definition, &run, now) {
                // Unless this look moved it, its row showed it due: it moved
                // since, and the row does not show it yet.
                Next::Sleep(_) if looked == Looked::Moved => return Ok(looked),
                Next::Sleep(until) => return Ok(Looked::Asleep(until)),
                Next::Finish => {
                    automations
                        .advance_run(AUTOMATION_ACTOR, &mut run, &[Transition::Finish])
                        .await?
                }
                Next::Wait(step, until) => {
                    let wait = Transition::Wait {
                        step: step.id.clone(),
                        until,
                    };
                    automations
                        .advance_run(AUTOMATION_ACTOR, &mut run, &[wait])
                        .await?
                }
                Next::Branch(step) => {
                    let transition = self
                        .branch(tenant, &run, definition.subject(), step)
                        .await?;
                    automations
                        .advance_run(AUTOMATION_ACTOR, &mut run, &[transition])
                        .await?
                }
                Next::Act(step) => {
                    // A mail to the customer waits for what the shop must set
                    // up, then fails saying what is missing.
                    if step.kind == EMAIL
                        && self.mail_blocked(tenant).await?.is_some()
                        && now < run.updated_at + MAIL_HOLD_SECS
                    {
                        return Ok(looked);
                    }
                    let attempt = run.attempts(&step.id) + 1;
                    let claim = Transition::Claim {
                        step: step.id.clone(),
                        attempt,
                    };
                    if !automations
                        .advance_run(AUTOMATION_ACTOR, &mut run, &[claim])
                        .await?
                    {
                        return Ok(looked);
                    }
                    // Claimed: it moved, whatever comes of the step.
                    looked = Looked::Moved;
                    let outcome = match self.act(tenant, &run, definition.subject(), step).await {
                        Acted::Done(output) => Transition::Succeed {
                            step: step.id.clone(),
                            output,
                        },
                        Acted::Skipped(reason) => Transition::Skip {
                            step: step.id.clone(),
                            reason,
                        },
                        Acted::Failed(reason) => Transition::Fail {
                            step: step.id.clone(),
                            attempt,
                            reason,
                            retry_at: None,
                        },
                        Acted::Retry(reason) => Transition::Fail {
                            step: step.id.clone(),
                            attempt,
                            reason,
                            retry_at: retry_at(now, attempt),
                        },
                    };
                    automations
                        .advance_run(AUTOMATION_ACTOR, &mut run, &[outcome])
                        .await?
                }
                Next::Lapsed(step, attempt) => {
                    let lapsed = Transition::Fail {
                        step: step.id.clone(),
                        attempt,
                        reason: "Its outcome is unknown: the node acting on it stopped.".into(),
                        retry_at: None,
                    };
                    automations
                        .advance_run(AUTOMATION_ACTOR, &mut run, &[lapsed])
                        .await?
                }
                Next::Stuck(step, reason) => {
                    let stuck = Transition::Fail {
                        step: step.id.clone(),
                        attempt: 0,
                        reason: reason.into(),
                        retry_at: None,
                    };
                    automations
                        .advance_run(AUTOMATION_ACTOR, &mut run, &[stuck])
                        .await?
                }
            };
            // Another node moved it meanwhile: it carries on from there.
            if !moved {
                return Ok(looked);
            }
            looked = Looked::Moved;
        }
        Ok(looked)
    }

    /// Checks a condition on the subject as it is now: the lane it sends
    /// the run down.
    async fn branch(
        &self,
        tenant: &TenantState,
        run: &RunState,
        subject: Option<Subject>,
        step: &Step,
    ) -> anyhow::Result<Transition> {
        let condition = catalog::parse_condition(subject, &step.settings);
        let (Some(subject), Ok(condition)) = (subject, condition) else {
            return Ok(Transition::Fail {
                step: step.id.clone(),
                attempt: 0,
                reason: STUCK_SETTINGS.into(),
                retry_at: None,
            });
        };
        let keys = self.keys.for_tenant(&tenant.id);
        let facts = facts_of(
            &self.store,
            &self.db,
            &*keys,
            &tenant.id,
            subject,
            &run.subject_id,
        )
        .await?;
        Ok(Transition::Branch {
            step: step.id.clone(),
            branch: if condition.holds(&facts) { 0 } else { 1 },
        })
    }

    /// Acts on a step claimed for this node. Whatever happens is an outcome:
    /// a claimed step never errors out of the run.
    async fn act(
        &self,
        tenant: &TenantState,
        run: &RunState,
        subject: Option<Subject>,
        step: &Step,
    ) -> Acted {
        let acted = match step.kind.as_str() {
            NOTIFY => self.notify(tenant, run, subject, step).await,
            EMAIL => self.email(tenant, run, subject, step).await,
            DISCOUNT => self.discount(tenant, run, step).await,
            WEBHOOK => self.webhook(tenant, run, step).await,
            DISCORD => self.discord(tenant, run, subject, step).await,
            _ => Ok(Acted::Failed(STUCK_UNKNOWN.into())),
        };
        acted.unwrap_or_else(|err| Acted::Failed(err.to_string()))
    }

    /// Mails the members who may see the run's subject: the merchant's note
    /// and a link to the subject in the dashboard, nothing a shopper wrote.
    async fn notify(
        &self,
        tenant: &TenantState,
        run: &RunState,
        subject: Option<Subject>,
        step: &Step,
    ) -> anyhow::Result<Acted> {
        let (Some(subject), Ok(settings)) = (subject, catalog::parse_notify(&step.settings)) else {
            return Ok(Acted::Failed(STUCK_SETTINGS.into()));
        };
        let permission = read_permission(subject);
        let ids: Vec<String> = tenant
            .members
            .keys()
            .filter(|user_id| tenant.can(user_id, permission))
            .cloned()
            .collect();
        let recipients = mailboxes(timada_identity::users_by_ids(&self.db, &ids).await?);
        if recipients.is_empty() {
            return Ok(Acted::Skipped("No member to tell.".into()));
        }
        let scenario = scenario_name(&self.db, &tenant.id, &run.scenario_id)
            .await?
            .unwrap_or_else(|| "A scenario".into());
        let link = subject_link(&self.base_url, &tenant.slug, subject, &run.subject_id);
        let cx = Cx::new(self.mail.clone());
        let mut mailed = 0;
        let mut refused = None;
        for to in &recipients {
            match send_automation_notice(
                &cx,
                to,
                &tenant.name,
                &scenario,
                &settings.subject,
                &settings.note,
                &link,
            )
            .await
            {
                Ok(()) => mailed += 1,
                Err(err) => refused = Some(err.to_string()),
            }
        }
        if mailed == 0 {
            let reason = refused.unwrap_or_default();
            return Ok(Acted::Failed(format!("The mail did not go out: {reason}")));
        }
        Ok(Acted::Done(
            serde_json::json!({ "mailed": mailed }).to_string(),
        ))
    }
}

impl<E: Store> Runner<E> {
    /// What keeps the shop from mailing its customers, if anything.
    async fn mail_blocked(&self, tenant: &TenantState) -> anyhow::Result<Option<&'static str>> {
        if self.opt_out_key.is_none() {
            return Ok(Some(
                "The dashboard has no secret to sign the opt-out links with.",
            ));
        }
        let storefront = timada_orders::Command::new(self.store.clone(), &tenant.id)
            .storefront_address()
            .await?;
        if storefront.is_none() {
            return Ok(Some("The shop has not said where its storefront lives."));
        }
        let cx = Cx::new(self.mail.clone());
        if marketing_sender(&cx, tenant).await?.is_none() {
            return Ok(Some("The shop picked no mail server for its marketing."));
        }
        Ok(None)
    }

    /// The customer's address, and the order the mail is about when there
    /// is one; `None` once the address is gone (erased, or on a cart dropped
    /// after lying idle).
    async fn recipient(
        &self,
        tenant: &TenantState,
        subject: Subject,
        subject_id: &str,
    ) -> anyhow::Result<Option<(String, Option<OrderState>)>> {
        let keys = self.keys.for_tenant(&tenant.id);
        let orders = timada_orders::Command::new(self.store.clone(), &tenant.id);
        Ok(match subject {
            Subject::Order => orders
                .load_order(subject_id)
                .await?
                .map(|order| (order.email.clone(), Some(order))),
            Subject::Review => {
                let reviews = timada_reviews::Command::new(self.store.clone(), &tenant.id);
                let Some(review) = reviews.load_review(subject_id).await? else {
                    return Ok(None);
                };
                orders
                    .load_order(&review.order_id)
                    .await?
                    .map(|order| (order.email.clone(), Some(order)))
            }
            Subject::Shopper => {
                let shoppers = timada_shoppers::Command::new(self.store.clone(), &tenant.id);
                match shoppers.load_shopper(subject_id).await? {
                    Some(state) => state
                        .open_with(&*keys)
                        .await?
                        .map(|shopper| (shopper.email, None)),
                    None => None,
                }
            }
            Subject::Subscriber => {
                let newsletter = timada_newsletter::Command::new(self.store.clone(), &tenant.id);
                match newsletter.load_subscriber(subject_id).await? {
                    Some(state) => state
                        .open_with(&*keys)
                        .await?
                        .map(|subscriber| (subscriber.email, None)),
                    None => None,
                }
            }
            Subject::Cart => {
                let orders = timada_orders::Command::new(self.store.clone(), &tenant.id);
                orders.load_cart(subject_id).await?.and_then(|cart| {
                    cart.requests
                        .last()
                        .map(|request| (request.email.clone(), None))
                })
            }
            Subject::Message => {
                let support = timada_support::Command::new(self.store.clone(), &tenant.id);
                match support.load_message(subject_id).await? {
                    Some(state) => state
                        .open_with(&*keys)
                        .await?
                        .map(|message| (message.email, None)),
                    None => None,
                }
            }
        })
    }

    /// Why an address must not get the shop's automated mails, if it must
    /// not: it stopped them, its account was erased, or it left (or was
    /// erased from) the newsletter.
    async fn refused_by(
        &self,
        tenant: &TenantState,
        address: &str,
    ) -> anyhow::Result<Option<&'static str>> {
        let automations = timada_automations::Command::new(self.store.clone(), &tenant.id);
        if automations.is_opted_out(address).await? {
            return Ok(Some("The customer stopped the shop's automated mails."));
        }
        let shoppers = timada_shoppers::Command::new(self.store.clone(), &tenant.id);
        if shoppers
            .load_shopper_by_email(address)
            .await?
            .is_some_and(|state| state.forgotten)
        {
            return Ok(Some("The customer's account was erased."));
        }
        let newsletter = timada_newsletter::Command::new(self.store.clone(), &tenant.id);
        if newsletter
            .load_subscriber(&subscriber_id(address))
            .await?
            .is_some_and(|state| state.forgotten || state.status == SubscriberStatus::Unsubscribed)
        {
            return Ok(Some("The customer left the shop's newsletter."));
        }
        Ok(None)
    }

    /// Mails the customer the run is about the merchant's message, through
    /// the server the shop picked for its marketing, with the link that
    /// stops these mails. At most one mail a day from a scenario to an
    /// address.
    async fn email(
        &self,
        tenant: &TenantState,
        run: &RunState,
        subject: Option<Subject>,
        step: &Step,
    ) -> anyhow::Result<Acted> {
        let (Some(subject), Ok(settings)) = (subject, catalog::parse_email(&step.settings)) else {
            return Ok(Acted::Failed(STUCK_SETTINGS.into()));
        };
        if let Some(reason) = self.mail_blocked(tenant).await? {
            return Ok(Acted::Failed(reason.into()));
        }
        let Some(key) = self.opt_out_key else {
            return Ok(Acted::Failed(STUCK_SETTINGS.into()));
        };
        let Some((address, order)) = self.recipient(tenant, subject, &run.subject_id).await? else {
            return Ok(Acted::Skipped(
                "The customer's address is gone (erased, or its cart dropped).".into(),
            ));
        };
        let Some(address) = normalize_email(&address) else {
            return Ok(Acted::Skipped(
                "The customer's address does not look like one.".into(),
            ));
        };
        if let Some(reason) = self.refused_by(tenant, &address).await? {
            return Ok(Acted::Skipped(reason.into()));
        }
        let automations = timada_automations::Command::new(self.store.clone(), &tenant.id);
        let day = crate::auth::now() / (24 * 60 * 60);
        if !automations
            .claim_mail(AUTOMATION_ACTOR, &run.scenario_id, &address, day)
            .await?
        {
            return Ok(Acted::Skipped(
                "This scenario mailed the customer today already.".into(),
            ));
        }
        let storefront = timada_orders::Command::new(self.store.clone(), &tenant.id)
            .storefront_address()
            .await?
            .unwrap_or_default();
        let storefront = storefront.trim_end_matches('/');
        let unsubscribe = format!(
            "{storefront}/emails/opt-out/{}",
            opt_out_token(&key.0, &tenant.id, &address)
        );
        let cx = Cx::new(self.mail.clone());
        let Some(sender) = marketing_sender(&cx, tenant).await? else {
            return Ok(Acted::Failed(
                "The shop picked no mail server for its marketing.".into(),
            ));
        };
        let about = order
            .as_ref()
            .map(|order| (order, Some(format!("{storefront}/orders/{}", order.id))));
        let vars = automation_vars(&cx, tenant, about, &unsubscribe, made_code(run));
        let copy = CampaignCopy {
            subject: &settings.subject,
            html: &settings.html,
            text: settings.text.as_deref(),
        };
        let rendered = match render_automated(&copy, &vars) {
            Ok(rendered) => rendered,
            Err(err) => return Ok(Acted::Failed(err.to_string())),
        };
        match send_automated(&cx, &sender, &address, rendered, &unsubscribe).await {
            Ok(()) => Ok(Acted::Done(serde_json::json!({ "mailed": 1 }).to_string())),
            Err(err) => Ok(Acted::Failed(format!("The mail did not go out: {err}"))),
        }
    }
}

impl<E: Store> Runner<E> {
    /// Makes the step's single-use code. Its id derives from the run and
    /// the step, so a retry (or another node) makes the same one.
    async fn discount(
        &self,
        tenant: &TenantState,
        run: &RunState,
        step: &Step,
    ) -> anyhow::Result<Acted> {
        let Ok(settings) = catalog::parse_discount(&step.settings) else {
            return Ok(Acted::Failed(STUCK_SETTINGS.into()));
        };
        let id = evento::hash_ids(vec!["automation-discount", &run.id, &step.id]);
        let suffix: String = id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_uppercase())
            .take(8)
            .collect();
        let code = format!("{}-{suffix}", settings.prefix);
        let ends_at = crate::auth::now() + u64::from(settings.days) * 24 * 60 * 60;
        let (value, worded) = match &settings.value {
            CodeValue::Percent(bps) => (
                timada_discounts::DiscountValue::Percent(*bps),
                format!("{} %", timada_core_percent(*bps)),
            ),
            CodeValue::Amount { cents, currency } => (
                timada_discounts::DiscountValue::Amount {
                    cents: *cents,
                    currency: currency.clone(),
                },
                timada_catalog::format_money(*cents, currency),
            ),
        };
        let discounts = timada_discounts::Command::new(self.store.clone(), &tenant.id);
        let made = discounts
            .create_discount_with_id(
                AUTOMATION_ACTOR,
                &id,
                timada_discounts::CreateDiscount {
                    code,
                    value,
                    starts_at: None,
                    ends_at: Some(ends_at),
                    max_uses: Some(1),
                    once_per_email: false,
                },
            )
            .await;
        match made {
            Ok(discount) => Ok(Acted::Done(
                serde_json::json!({
                    "code": discount.code,
                    "value": worded,
                    "ends": crate::ui::format_date(ends_at),
                })
                .to_string(),
            )),
            Err(err) if err.is_user_facing() => Ok(Acted::Failed(err.to_string())),
            Err(err) => Ok(Acted::Retry(err.to_string())),
        }
    }
}

impl<E: Store> Runner<E> {
    /// Posts what started the run to the step's address, signed. Asking
    /// again is safe (the same `Idempotency-Key` every try), so a call that
    /// got no answer is tried again later.
    async fn webhook(
        &self,
        tenant: &TenantState,
        run: &RunState,
        step: &Step,
    ) -> anyhow::Result<Acted> {
        let Ok(settings) = catalog::parse_webhook(&step.settings) else {
            return Ok(Acted::Failed(STUCK_SETTINGS.into()));
        };
        let Some(keys) = &self.webhooks else {
            return Ok(Acted::Failed(
                "The dashboard has no secret to open the webhook's address with.".into(),
            ));
        };
        let Some(url) = super::webhook::open_url(
            keys,
            &tenant.id,
            &run.scenario_id,
            &step.id,
            &settings.url_sealed,
        ) else {
            return Ok(Acted::Failed("The webhook's address does not open.".into()));
        };
        let scenario = scenario_name(&self.db, &tenant.id, &run.scenario_id)
            .await?
            .unwrap_or_default();
        let subject = catalog::trigger_kind(&run.trigger).map(|kind| kind.subject.as_str());
        let now = crate::auth::now();
        let id = format!("{}:{}", run.id, step.id);
        let body = serde_json::to_vec(&serde_json::json!({
            "id": id,
            "type": run.trigger,
            "shop": tenant.slug,
            "scenario": {
                "id": run.scenario_id,
                "name": scenario,
                "version": run.scenario_version,
            },
            "run": run.id,
            "subject": { "kind": subject, "id": run.subject_id },
            "sent_at": now,
        }))?;
        let secret = super::webhook::signing_secret(keys, &tenant.id, &run.scenario_id, &step.id);
        let signature = super::webhook::signature(&secret, &body, now);
        Ok(
            match super::webhook::deliver(
                &self.http,
                &url,
                body,
                signature,
                &id,
                self.allow_private_hosts,
            )
            .await
            {
                super::webhook::Delivery::Taken(status) => {
                    Acted::Done(serde_json::json!({ "status": status }).to_string())
                }
                super::webhook::Delivery::Again(reason) => Acted::Retry(reason),
                super::webhook::Delivery::Refused(reason) => Acted::Failed(reason),
            },
        )
    }
}

impl<E: Store> Runner<E> {
    /// Posts the merchant's message, its variables read from the subject as
    /// it is now, and a link to the subject to the step's Discord channel. A call that got no answer is tried again
    /// later, even if that may post it twice (see `discord.rs`).
    async fn discord(
        &self,
        tenant: &TenantState,
        run: &RunState,
        subject: Option<Subject>,
        step: &Step,
    ) -> anyhow::Result<Acted> {
        let (Some(subject), Ok(settings)) = (subject, catalog::parse_discord(&step.settings))
        else {
            return Ok(Acted::Failed(STUCK_SETTINGS.into()));
        };
        let Some(keys) = &self.webhooks else {
            return Ok(Acted::Failed(
                "The dashboard has no secret to open the channel's address with.".into(),
            ));
        };
        let Some(url) = super::webhook::open_url(
            keys,
            &tenant.id,
            &run.scenario_id,
            &step.id,
            &settings.url_sealed,
        ) else {
            return Ok(Acted::Failed("The channel's address does not open.".into()));
        };
        let link = subject_link(&self.base_url, &tenant.slug, subject, &run.subject_id);
        let Some(vars) = super::discord::vars_of(
            &self.store,
            &tenant.id,
            &tenant.name,
            subject,
            &run.subject_id,
        )
        .await?
        else {
            return Ok(Acted::Skipped("What started the run is gone.".into()));
        };
        let message = match timada_tenancy::render_message(&settings.message, &vars) {
            Ok(message) => message,
            Err(err) => return Ok(Acted::Failed(err.to_string())),
        };
        let content = super::discord::content(&message, &link);
        Ok(
            match super::discord::post(&self.http, &url, &content, self.allow_private_hosts).await {
                super::webhook::Delivery::Taken(status) => {
                    Acted::Done(serde_json::json!({ "status": status }).to_string())
                }
                super::webhook::Delivery::Again(reason) => Acted::Retry(reason),
                super::webhook::Delivery::Refused(reason) => Acted::Failed(reason),
            },
        )
    }
}

impl<E: Store> Runner<E> {
    /// Starts the runs an event made that fell to another node and that it
    /// did not start within a while ([`HELP_AFTER_SECS`]) of this node
    /// keeping them. A run the read model holds was started; one it does
    /// not is read from the store before it is written, as the read model
    /// may be behind. One kept longer ago than the trigger window (this
    /// node was away) starts nothing, as an event that old would not.
    async fn start_left_runs(&self, now: u64) -> anyhow::Result<()> {
        prune_starts(&self.db, now.saturating_sub(TRIGGER_WINDOW_SECS)).await?;
        let before = now.saturating_sub(self.help_after);
        for owed in starts_owed(&self.db, before, self.runs_per_look).await? {
            if !run_known(&self.db, &owed.tenant_id, &owed.run_id).await? {
                let automations =
                    timada_automations::Command::new(self.store.clone(), &owed.tenant_id);
                if automations.load_run(&owed.run_id).await?.is_none()
                    && automations
                        .start_run(AUTOMATION_ACTOR, &owed.start())
                        .await?
                {
                    tracing::info!(tenant = %owed.tenant_id, scenario = %owed.scenario_id, trigger = %owed.trigger, "automation run started for another node");
                }
            }
            settle_start(&self.db, &owed.tenant_id, &owed.run_id).await?;
        }
        Ok(())
    }

    /// Records the checkouts no order followed within the delay: each one
    /// starts the scenarios on that trigger. Only the nodes of the region a
    /// checkout was asked in follow it, and each looks at its own part of
    /// them, then at any another node left for a while
    /// ([`HELP_AFTER_SECS`]).
    async fn find_left_checkouts(&self, now: u64) -> anyhow::Result<()> {
        let to = now.saturating_sub(self.abandoned_after);
        let from = to.saturating_sub(LEFT_WINDOW_SECS);
        let help_before = to.saturating_sub(self.help_after);
        prune_checkouts(&self.db, from).await?;
        let mut shares: HashMap<Option<String>, Share> = HashMap::new();
        for left in left_checkouts(&self.db, from, to, self.runs_per_look).await? {
            let share = *shares
                .entry(left.region.clone())
                .or_insert_with(|| (self.place)(left.region.as_deref()));
            if !share.owns(&left.cart_id) && left.requested_at as u64 > help_before {
                continue;
            }
            let orders = timada_orders::Command::new(self.store.clone(), &left.tenant_id);
            let Some(cart) = orders.load_cart(&left.cart_id).await? else {
                continue;
            };
            // Ordered meanwhile, or asked again since: not left, or not yet.
            let latest = cart
                .requests
                .last()
                .map(|request| request.request_id.as_str());
            if cart.order_id.is_some() || latest != Some(left.request_id.as_str()) {
                continue;
            }
            // Written in the request's region, whose nodes follow the
            // checkout: they all see it found, whichever of them found it.
            let automations = timada_automations::Command::new(self.store.clone(), &left.tenant_id);
            let detected = match &left.region {
                Some(region) => {
                    automations
                        .detect_abandoned_checkout_with_routing(
                            AUTOMATION_ACTOR,
                            &left.cart_id,
                            &left.request_id,
                            region,
                        )
                        .await?
                }
                None => {
                    automations
                        .detect_abandoned_checkout(
                            AUTOMATION_ACTOR,
                            &left.cart_id,
                            &left.request_id,
                        )
                        .await?
                }
            };
            // Found already, by another node: this one stops looking at it
            // without waiting to see that node's event.
            if !detected {
                record_detected(&self.db, &left.tenant_id, &left.cart_id, &left.request_id).await?;
            }
        }
        Ok(())
    }
}

/// `1000` basis points as `10`, `1250` as `12.5`.
fn timada_core_percent(bps: u32) -> String {
    let whole = bps / 100;
    match bps % 100 {
        0 => whole.to_string(),
        cents if cents % 10 == 0 => format!("{whole}.{}", cents / 10),
        cents => format!("{whole}.{cents:02}"),
    }
}

/// The code a discount step made earlier in the run, for a mail to quote.
fn made_code(run: &RunState) -> Option<timada_tenancy::AutomationDiscountVars> {
    run.log
        .iter()
        .rev()
        .find_map(|record| match &record.outcome {
            timada_automations::Outcome::Succeeded { output } => {
                let made: serde_json::Value = serde_json::from_str(output).ok()?;
                Some(timada_tenancy::AutomationDiscountVars {
                    code: made.get("code")?.as_str()?.to_owned(),
                    value: made.get("value")?.as_str()?.to_owned(),
                    ends: made.get("ends")?.as_str()?.to_owned(),
                })
            }
            _ => None,
        })
}

/// What a member must be able to read to be told about a subject.
fn read_permission(subject: Subject) -> &'static str {
    match subject {
        Subject::Order => PERMISSION_ORDERS_READ,
        Subject::Shopper => PERMISSION_CUSTOMERS_READ,
        Subject::Subscriber => PERMISSION_NEWSLETTER_READ,
        Subject::Review => PERMISSION_REVIEWS_READ,
        Subject::Message => PERMISSION_SUPPORT_READ,
        Subject::Cart => PERMISSION_ORDERS_READ,
    }
}

/// Where a subject is seen in the dashboard.
pub fn subject_link(base_url: &str, slug: &str, subject: Subject, subject_id: &str) -> String {
    let base = base_url.trim_end_matches('/');
    match subject {
        Subject::Order => format!("{base}/t/{slug}/orders/{subject_id}"),
        Subject::Shopper => format!("{base}/t/{slug}/customers"),
        Subject::Subscriber => format!("{base}/t/{slug}/newsletter"),
        Subject::Review => format!("{base}/t/{slug}/reviews"),
        Subject::Message => format!("{base}/t/{slug}/messages"),
        Subject::Cart => format!("{base}/t/{slug}/carts"),
    }
}

/// The addresses to mail among these users: the living ones that have one.
fn mailboxes(users: Vec<UserRow>) -> Vec<String> {
    users
        .into_iter()
        .filter(|user| !user.deleted)
        .filter_map(|user| user.email)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex;

    use evento::Evento;
    use evento::ProjectionAggregate;
    use timada_automations::{
        Command, Definition, Lane, Outcome, RunStatus, Slot, StartRun,
        catalog::{CONDITION, ORDER_PLACED, SUBSCRIBER_CONFIRMED, WAIT},
        read_model_subscription, run_id,
    };
    use timada_core::{Key, Keys};
    use timada_newsletter::{RequestOutcome, RequestSubscription, SOURCE_FORM};

    use super::*;

    const TENANT: &str = "t1";
    const MEMBER: &str = "user-1";

    #[derive(Default)]
    struct MemoryKeys(Mutex<HashMap<String, Key>>);

    struct MemoryRing(Arc<MemoryKeys>);

    impl KeyRing for MemoryRing {
        fn for_tenant(&self, _tenant_id: &str) -> Arc<dyn Keys> {
            self.0.clone()
        }
    }

    #[async_trait::async_trait]
    impl Keys for MemoryKeys {
        async fn put(&self, key_id: &str, key: &Key) -> anyhow::Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(key_id.to_owned(), key.clone());
            Ok(())
        }

        async fn get(&self, key_id: &str) -> anyhow::Result<Option<Key>> {
            Ok(self.0.lock().unwrap().get(key_id).cloned())
        }

        async fn delete(&self, key_id: &str) -> anyhow::Result<()> {
            self.0.lock().unwrap().remove(key_id);
            Ok(())
        }
    }

    /// One in-memory database holding the store and the read models, the
    /// store wrapped as the cluster's is.
    async fn store() -> (Evento, Db) {
        let turso = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&turso).await.unwrap();
        let db = Db::from(turso.clone());
        (Evento::new(turso), db)
    }

    /// The trigger subscription, caught up: the listeners, then the runs
    /// the events start.
    async fn follow(store: &Evento, db: &Db) {
        follow_at(store, db, alone()).await;
    }

    /// The runs' read model, caught up.
    async fn catch_up(store: &Evento, db: &Db) {
        read_model_subscription::<Evento>(db.clone())
            .no_retry()
            .run_once(store)
            .await
            .unwrap();
    }

    async fn subscribe(store: &Evento, keys: &MemoryKeys, email: &str) {
        let newsletter = timada_newsletter::Command::new(store.clone(), TENANT);
        let RequestOutcome::ToConfirm { token, .. } = newsletter
            .request_subscription(
                keys,
                "storefront",
                RequestSubscription {
                    email: email.to_owned(),
                    language: None,
                    source: SOURCE_FORM.to_owned(),
                    time_zone: None,
                },
                crate::auth::now(),
            )
            .await
            .unwrap()
        else {
            panic!("nothing to confirm");
        };
        newsletter
            .confirm_subscription(keys, "storefront", &token, crate::auth::now())
            .await
            .unwrap();
    }

    fn runner(store: &Evento, db: &Db, keys: &Arc<MemoryKeys>) -> Arc<Runner<Evento>> {
        runner_at(store, db, keys, alone())
    }

    /// A runner on a node whose place among the nodes up is `place`.
    fn runner_at(
        store: &Evento,
        db: &Db,
        keys: &Arc<MemoryKeys>,
        place: ShareOf,
    ) -> Arc<Runner<Evento>> {
        runner_every(store, db, keys, place, Duration::from_secs(15))
    }

    /// The same, looking every `every`.
    fn runner_every(
        store: &Evento,
        db: &Db,
        keys: &Arc<MemoryKeys>,
        place: ShareOf,
        every: Duration,
    ) -> Arc<Runner<Evento>> {
        runner_moving(store, db, keys, place, every, 10)
    }

    /// The same, moving `runs_per_look` runs a look at most.
    fn runner_moving(
        store: &Evento,
        db: &Db,
        keys: &Arc<MemoryKeys>,
        place: ShareOf,
        every: Duration,
        runs_per_look: u32,
    ) -> Arc<Runner<Evento>> {
        Runner::new(
            db.clone(),
            store.clone(),
            Arc::new(AppContext::new()),
            "https://dashboard.example".into(),
            Arc::new(MemoryRing(keys.clone())),
            None,
            None,
            Duration::from_secs(5),
            true,
            Duration::from_secs(3600),
            runs_per_look,
            every,
            place,
        )
        .unwrap()
    }

    /// A node on its own: every run is its.
    fn alone() -> ShareOf {
        Arc::new(|_| Share::ALONE)
    }

    /// A node none of the work falls to: another node has it all.
    fn none_of_it() -> ShareOf {
        Arc::new(|_| Share { index: 1, of: 1 })
    }

    /// The trigger subscription on a node at `place`, caught up.
    async fn follow_at(store: &Evento, db: &Db, place: ShareOf) {
        super::super::triggers::subscription(db.clone(), store.clone(), place)
            .no_retry()
            .run_once(store)
            .await
            .unwrap();
    }

    /// The shop, with no member to tell.
    fn shop() -> TenantState {
        TenantState {
            id: TENANT.to_owned(),
            ..Default::default()
        }
    }

    fn settings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    /// "When someone joins the newsletter from the form, tell the team;
    /// otherwise wait a day".
    fn welcome() -> Definition {
        let mut definition = Definition::default();
        definition.set_trigger(SUBSCRIBER_CONFIRMED).unwrap();
        let condition = definition
            .insert(&Slot::new(Lane::Root, 0), CONDITION)
            .unwrap();
        definition
            .configure(
                &condition,
                settings(&[
                    ("rules.0.field", "subscriber.source"),
                    ("rules.0.op", "eq"),
                    ("rules.0.value", "form"),
                ]),
            )
            .unwrap();
        let yes = Lane::Branch {
            step: condition.clone(),
            branch: 0,
        };
        let notify = definition.insert(&Slot::new(yes, 0), NOTIFY).unwrap();
        definition
            .configure(&notify, settings(&[("subject", "New subscriber")]))
            .unwrap();
        let no = Lane::Branch {
            step: condition,
            branch: 1,
        };
        definition.insert(&Slot::new(no, 0), WAIT).unwrap();
        assert_eq!(definition.check(), vec![]);
        definition
    }

    async fn publish(store: &Evento, definition: &Definition) -> String {
        let automations = Command::new(store.clone(), TENANT);
        let id = automations
            .create_scenario(MEMBER, "Welcome")
            .await
            .unwrap();
        automations
            .publish_scenario(MEMBER, &id, 1, "Welcome", definition)
            .await
            .unwrap();
        id
    }

    async fn the_run(store: &Evento, db: &Db) -> Option<RunState> {
        catch_up(store, db).await;
        let due = due_runs(
            db,
            crate::auth::now() + 10_000_000,
            Share::ALONE,
            0,
            None,
            10,
        )
        .await
        .unwrap();
        Command::new(store.clone(), TENANT)
            .load_run(&due.first()?.id)
            .await
            .unwrap()
    }

    /// The ids of the runs a node at `runner` moves at `now`.
    async fn due_ids(runner: &Runner<Evento>, now: u64) -> Vec<String> {
        let due = runner.due(now, None, 100).await.unwrap();
        due.into_iter().map(|run| run.id).collect()
    }

    #[tokio::test]
    async fn an_event_starts_one_run_that_goes_through_its_steps() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        publish(&store, &welcome()).await;
        follow(&store, &db).await;
        subscribe(&store, &keys, "ada@example.com").await;
        follow(&store, &db).await;
        // Seen again (a redelivery, another node): still one run.
        follow(&store, &db).await;
        catch_up(&store, &db).await;
        let due = due_runs(&db, crate::auth::now() + 1, Share::ALONE, 0, None, 10)
            .await
            .unwrap();
        assert_eq!(due.len(), 1);

        let run_id = &due[0].id;
        runner(&store, &db, &keys)
            .advance(&shop(), run_id, crate::auth::now())
            .await
            .unwrap();
        let run = Command::new(store.clone(), TENANT)
            .load_run(run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.status, RunStatus::Finished);
        let outcomes: Vec<&Outcome> = run.log.iter().map(|record| &record.outcome).collect();
        // Joined from the form: the Yes lane. Nobody to tell in this shop:
        // the notice is skipped and the run goes on to its end.
        assert_eq!(outcomes[0], &Outcome::Branched(0));
        assert_eq!(outcomes[1], &Outcome::Claimed { attempt: 1 });
        assert!(matches!(outcomes[2], Outcome::Skipped { .. }));
        assert_eq!(outcomes.len(), 3);
    }

    #[tokio::test]
    async fn an_event_before_the_publication_starts_nothing() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        subscribe(&store, &keys, "ada@example.com").await;
        publish(&store, &welcome()).await;
        follow(&store, &db).await;
        assert!(the_run(&store, &db).await.is_none());
    }

    #[tokio::test]
    async fn a_paused_scenario_holds_its_runs_until_resumed() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        let id = publish(&store, &welcome()).await;
        follow(&store, &db).await;
        subscribe(&store, &keys, "ada@example.com").await;
        follow(&store, &db).await;
        let run = the_run(&store, &db).await.unwrap();
        let automations = Command::new(store.clone(), TENANT);
        automations.pause_scenario(MEMBER, &id).await.unwrap();
        follow(&store, &db).await;
        catch_up(&store, &db).await;

        // Not due: no look reads it.
        let runner = runner(&store, &db, &keys);
        assert!(
            due_ids(&runner, crate::auth::now() + 10_000_000)
                .await
                .is_empty()
        );
        // Moved all the same (a look that read it a moment before the
        // pause), it stays where it is.
        let looked = runner
            .advance(&shop(), &run.id, crate::auth::now())
            .await
            .unwrap();
        assert_eq!(looked, Looked::Left);
        let held = automations.load_run(&run.id).await.unwrap().unwrap();
        assert!(held.log.is_empty());
        assert_eq!(held.status, RunStatus::Running);

        automations.resume_scenario(MEMBER, &id).await.unwrap();
        // The runner reads the scenario's state from the read model.
        catch_up(&store, &db).await;
        assert_eq!(
            due_ids(&runner, crate::auth::now()).await,
            vec![run.id.clone()]
        );
        runner
            .advance(&shop(), &run.id, crate::auth::now())
            .await
            .unwrap();
        let done = automations.load_run(&run.id).await.unwrap().unwrap();
        assert_eq!(done.status, RunStatus::Finished);
    }

    #[tokio::test]
    async fn a_wait_holds_the_run_until_its_time() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        let mut definition = Definition::default();
        definition.set_trigger(SUBSCRIBER_CONFIRMED).unwrap();
        let wait = definition.insert(&Slot::new(Lane::Root, 0), WAIT).unwrap();
        definition
            .configure(&wait, settings(&[("amount", "1"), ("unit", "hours")]))
            .unwrap();
        let notify = definition
            .insert(&Slot::new(Lane::Root, 1), NOTIFY)
            .unwrap();
        definition
            .configure(&notify, settings(&[("subject", "An hour later")]))
            .unwrap();
        publish(&store, &definition).await;
        follow(&store, &db).await;
        subscribe(&store, &keys, "ada@example.com").await;
        follow(&store, &db).await;

        let run = the_run(&store, &db).await.unwrap();
        let runner = runner(&store, &db, &keys);
        let automations = Command::new(store.clone(), TENANT);
        let now = crate::auth::now();
        runner.advance(&shop(), &run.id, now).await.unwrap();
        let waiting = automations.load_run(&run.id).await.unwrap().unwrap();
        assert_eq!(waiting.status, RunStatus::Waiting);
        assert_eq!(waiting.next_at(), Some(now + 3600));
        // A minute later: still waiting.
        runner.advance(&shop(), &run.id, now + 60).await.unwrap();
        assert_eq!(
            automations
                .load_run(&run.id)
                .await
                .unwrap()
                .unwrap()
                .log
                .len(),
            1
        );
        // The hour is up: the notice, then the end.
        runner.advance(&shop(), &run.id, now + 3600).await.unwrap();
        let done = automations.load_run(&run.id).await.unwrap().unwrap();
        assert_eq!(done.status, RunStatus::Finished);
    }

    /// A cart of one published product whose shopper asked to pay.
    async fn checkout_left(store: &Evento) -> String {
        let catalog = timada_catalog::Command::new(store.clone(), TENANT);
        let product = catalog
            .create_product(
                MEMBER,
                timada_catalog::CreateProduct {
                    name: "Cat bed".into(),
                    slug: None,
                },
            )
            .await
            .unwrap();
        catalog
            .price_product(MEMBER, &product.id, 2500, "EUR")
            .await
            .unwrap();
        catalog.publish_product(MEMBER, &product.id).await.unwrap();
        let orders = timada_orders::Command::new(store.clone(), TENANT);
        let cart = orders
            .start_cart(
                "storefront",
                timada_orders::CartItem {
                    product_id: product.id.clone(),
                    variant_id: None,
                    quantity: 2,
                },
            )
            .await
            .unwrap();
        orders
            .request_checkout(
                "storefront",
                &cart,
                timada_orders::Checkout {
                    email: "ada@example.org".into(),
                    shipping: timada_orders::ShippingAddress {
                        name: "Ada Lovelace".into(),
                        line1: "1 rue de la Paix".into(),
                        line2: None,
                        postal_code: "75002".into(),
                        city: "Paris".into(),
                        region: None,
                        country: "FR".into(),
                        phone: Some("06 12 34 56 78".into()),
                    },
                    note: None,
                    expected_total_cents: None,
                    vat_number: None,
                    billing: None,
                    shipping_method: None,
                    language: None,
                    newsletter: false,
                    time_zone: None,
                },
            )
            .await
            .unwrap();
        cart
    }

    #[tokio::test]
    async fn a_checkout_left_unpaid_starts_a_run_once_its_delay_is_over() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        let mut definition = Definition::default();
        definition
            .set_trigger(timada_automations::catalog::CHECKOUT_ABANDONED)
            .unwrap();
        let notify = definition
            .insert(&Slot::new(Lane::Root, 0), NOTIFY)
            .unwrap();
        definition
            .configure(&notify, settings(&[("subject", "A cart was left")]))
            .unwrap();
        publish(&store, &definition).await;
        follow(&store, &db).await;
        let cart = checkout_left(&store).await;
        follow(&store, &db).await;

        let runner = runner(&store, &db, &keys);
        let now = crate::auth::now();
        // Within the hour: not left yet.
        runner.sweep(now).await.unwrap();
        follow(&store, &db).await;
        assert!(the_run(&store, &db).await.is_none());
        // The hour is over: found left once, whatever the looks.
        runner.sweep(now + 3601).await.unwrap();
        runner.sweep(now + 3602).await.unwrap();
        // The second look found it found already: it looks no more.
        assert!(
            left_checkouts(&db, 0, now + 2, 10)
                .await
                .unwrap()
                .is_empty()
        );
        follow(&store, &db).await;
        let run = the_run(&store, &db).await.unwrap();
        assert_eq!(run.trigger, timada_automations::catalog::CHECKOUT_ABANDONED);
        assert_eq!(run.subject_id, cart);
        catch_up(&store, &db).await;
        let due = due_runs(&db, now + 10_000_000, Share::ALONE, 0, None, 10)
            .await
            .unwrap();
        assert_eq!(due.len(), 1);
    }

    /// A shop of the store's own, owned by the member.
    async fn shop_in(store: &Evento, name: &str) -> TenantState {
        timada_tenancy::Command(store.clone())
            .create_tenant(timada_tenancy::CreateTenant {
                name: name.to_owned(),
                owner_user_id: MEMBER.to_owned(),
            })
            .await
            .unwrap()
    }

    /// Publishes `definition` in a shop: the scenario's id.
    async fn publish_in(store: &Evento, tenant_id: &str, definition: &Definition) -> String {
        let automations = Command::new(store.clone(), tenant_id);
        let id = automations
            .create_scenario(MEMBER, "Scenario")
            .await
            .unwrap();
        automations
            .publish_scenario(MEMBER, &id, 1, "Scenario", definition)
            .await
            .unwrap();
        id
    }

    /// Starts a run of version 1 of a shop's scenario, as an order placed
    /// would: the run's id.
    async fn start_in(store: &Evento, tenant_id: &str, scenario_id: &str, event: &str) -> String {
        Command::new(store.clone(), tenant_id)
            .start_run(
                MEMBER,
                &StartRun {
                    scenario_id: scenario_id.to_owned(),
                    scenario_version: 1,
                    trigger: ORDER_PLACED.to_owned(),
                    trigger_event_id: event.to_owned(),
                    subject_id: format!("order-{event}"),
                },
            )
            .await
            .unwrap();
        run_id(scenario_id, event)
    }

    /// "When an order is placed, `kind`", configured with `pairs`.
    fn on_orders(kind: &str, pairs: &[(&str, &str)]) -> Definition {
        let mut definition = Definition::default();
        definition.set_trigger(ORDER_PLACED).unwrap();
        let step = definition.insert(&Slot::new(Lane::Root, 0), kind).unwrap();
        definition.configure(&step, settings(pairs)).unwrap();
        assert_eq!(definition.check(), vec![]);
        definition
    }

    /// The runs a look leaves as they were (a suspended shop's, here) do
    /// not count against what it moves: it goes on past them to the other
    /// shops' runs, and puts them off on this node, so the next looks do
    /// not read them again before then.
    #[tokio::test]
    async fn held_runs_do_not_keep_a_look_from_the_others() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        let held = shop_in(&store, "Held Shop").await;
        timada_tenancy::Command(store.clone())
            .suspend_tenant(&held.id, "unpaid")
            .await
            .unwrap();
        let open = shop_in(&store, "Open Shop").await;
        let scenario = publish_in(
            &store,
            &open.id,
            &on_orders(NOTIFY, &[("subject", "An order")]),
        )
        .await;
        let mut waiting = Vec::new();
        for n in 0..3 {
            waiting.push(start_in(&store, &held.id, "sc-held", &format!("ev{n}")).await);
        }
        let run = start_in(&store, &open.id, &scenario, "ev1").await;
        catch_up(&store, &db).await;
        // The suspended shop's runs are the most overdue, and more than a
        // look moves.
        db.execute(
            "UPDATE automations_runs SET next_at = next_at - 60 WHERE tenant_id = ?1",
            (held.id.as_str(),),
        )
        .await
        .unwrap();

        let now = crate::auth::now();
        let runner = runner_moving(&store, &db, &keys, alone(), Duration::from_secs(15), 2);
        runner.sweep(now).await.unwrap();
        let moved = Command::new(store.clone(), &open.id)
            .load_run(&run)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(moved.status, RunStatus::Finished);

        catch_up(&store, &db).await;
        assert!(due_ids(&runner, now).await.is_empty(), "put off");
        let mut later = due_ids(&runner, now + LOOK_AGAIN_SECS).await;
        later.sort();
        waiting.sort();
        assert_eq!(later, waiting);
    }

    /// A run whose row shows it due though it moved since (another node
    /// began its wait, which the read model does not show yet) is put off
    /// to when it wakes.
    #[tokio::test]
    async fn a_run_moved_since_its_row_was_written_is_put_off_until_it_wakes() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        let shop = shop_in(&store, "Acme").await;
        let wait = on_orders(WAIT, &[("amount", "1"), ("unit", "hours")]);
        let scenario = publish_in(&store, &shop.id, &wait).await;
        let run = start_in(&store, &shop.id, &scenario, "ev1").await;
        catch_up(&store, &db).await;
        let now = crate::auth::now();
        let runner = runner(&store, &db, &keys);
        let looked = runner.advance(&shop, &run, now).await.unwrap();
        assert_eq!(looked, Looked::Moved);
        assert_eq!(due_ids(&runner, now).await, vec![run.clone()]);

        runner.sweep(now).await.unwrap();
        assert!(due_ids(&runner, now + 3599).await.is_empty());
        assert_eq!(due_ids(&runner, now + 3600).await, vec![run]);
    }

    /// A run at a mail to the customer waits for what the shop must set up
    /// (here the operator's secret, which signs the opt-out links): a look
    /// that finds it so leaves it and puts it off, and once the hold is
    /// over the step fails, saying what is missing.
    #[tokio::test]
    async fn a_run_held_at_a_mail_is_put_off_then_fails_once_the_hold_is_over() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        let shop = shop_in(&store, "Acme").await;
        let mail = on_orders(
            EMAIL,
            &[
                ("subject", "Thanks"),
                ("html", "<p>{{ unsubscribe_url }}</p>"),
            ],
        );
        let scenario = publish_in(&store, &shop.id, &mail).await;
        let run = start_in(&store, &shop.id, &scenario, "ev1").await;
        catch_up(&store, &db).await;

        let now = crate::auth::now();
        let runner = runner(&store, &db, &keys);
        runner.sweep(now).await.unwrap();
        let automations = Command::new(store.clone(), &shop.id);
        let held = automations.load_run(&run).await.unwrap().unwrap();
        assert!(held.log.is_empty());
        assert!(due_ids(&runner, now + LOOK_AGAIN_SECS - 1).await.is_empty());
        assert_eq!(
            due_ids(&runner, now + LOOK_AGAIN_SECS).await,
            vec![run.clone()]
        );

        runner.sweep(now + MAIL_HOLD_SECS).await.unwrap();
        let failed = automations.load_run(&run).await.unwrap().unwrap();
        assert_eq!(failed.status, RunStatus::Failed);
        assert!(failed.reason.unwrap().contains("secret"));
    }

    /// An event whose run falls to another node starts nothing here; this
    /// node starts it only once that node left it for a while.
    #[tokio::test]
    async fn a_run_another_node_is_to_start_is_started_here_only_if_it_does_not() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        publish(&store, &welcome()).await;
        follow_at(&store, &db, none_of_it()).await;
        subscribe(&store, &keys, "ada@example.com").await;
        follow_at(&store, &db, none_of_it()).await;
        assert!(the_run(&store, &db).await.is_none(), "not this node's");
        let now = crate::auth::now();
        let owed = starts_owed(&db, now, 10).await.unwrap();
        assert_eq!(owed.len(), 1);

        let runner = runner_at(&store, &db, &keys, none_of_it());
        // Its node has a while to start it.
        runner.sweep(now).await.unwrap();
        assert!(the_run(&store, &db).await.is_none());
        assert_eq!(starts_owed(&db, now, 10).await.unwrap().len(), 1);
        // It did not: this node does, once.
        runner.sweep(now + HELP_AFTER_SECS).await.unwrap();
        let run = the_run(&store, &db).await.unwrap();
        assert_eq!(run.id, owed[0].run_id);
        assert_eq!(run.subject_id, owed[0].subject_id);
        assert!(starts_owed(&db, now, 10).await.unwrap().is_empty());
    }

    /// A run its node started is only forgotten by the others: no second
    /// write.
    #[tokio::test]
    async fn a_run_its_node_started_is_left_alone() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        publish(&store, &welcome()).await;
        follow_at(&store, &db, none_of_it()).await;
        subscribe(&store, &keys, "ada@example.com").await;
        follow_at(&store, &db, none_of_it()).await;
        let now = crate::auth::now();
        let owed = starts_owed(&db, now, 10).await.unwrap();
        let automations = Command::new(store.clone(), TENANT);
        // Its node starts it.
        assert!(
            automations
                .start_run("node-2", &owed[0].start())
                .await
                .unwrap()
        );
        catch_up(&store, &db).await;

        runner_at(&store, &db, &keys, none_of_it())
            .start_left_runs(now + HELP_AFTER_SECS)
            .await
            .unwrap();
        assert!(starts_owed(&db, now, 10).await.unwrap().is_empty());
        let run = automations
            .load_run(&owed[0].run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.aggregate_version().unwrap(), 1, "started once");
    }

    /// A run due that falls to another node is moved by it; this node moves
    /// it only once that node left it for a while.
    #[tokio::test]
    async fn a_run_due_that_falls_to_another_node_waits_for_it_a_while() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        publish(&store, &welcome()).await;
        follow(&store, &db).await;
        subscribe(&store, &keys, "ada@example.com").await;
        follow(&store, &db).await;
        let run = the_run(&store, &db).await.unwrap();
        let now = crate::auth::now();
        let due = vec![run.id.clone()];

        assert_eq!(due_ids(&runner(&store, &db, &keys), now).await, due);
        let elsewhere = runner_at(&store, &db, &keys, none_of_it());
        assert!(
            due_ids(&elsewhere, now).await.is_empty(),
            "not this node's to move"
        );
        assert_eq!(due_ids(&elsewhere, now + HELP_AFTER_SECS).await, due);

        // Looks a minute apart: that node is left four of them.
        let slow = runner_every(&store, &db, &keys, none_of_it(), Duration::from_secs(60));
        assert!(due_ids(&slow, now + HELP_AFTER_SECS).await.is_empty());
        assert_eq!(due_ids(&slow, now + 4 * 60).await, due);
    }

    /// A node outside the scenario's region has no listener for it (the
    /// trigger subscription follows the node's region): it moves the run
    /// all the same, from the scenarios' read model.
    #[tokio::test]
    async fn a_run_moves_on_a_node_that_has_no_listener_for_it() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        publish(&store, &welcome()).await;
        follow(&store, &db).await;
        subscribe(&store, &keys, "ada@example.com").await;
        follow(&store, &db).await;
        let run = the_run(&store, &db).await.unwrap();
        db.execute("DELETE FROM automations_listeners", ())
            .await
            .unwrap();

        runner(&store, &db, &keys)
            .advance(&shop(), &run.id, crate::auth::now())
            .await
            .unwrap();
        let moved = Command::new(store.clone(), TENANT)
            .load_run(&run.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(moved.status, RunStatus::Finished);
    }

    #[test]
    fn a_step_is_tried_again_four_times_then_not() {
        assert_eq!(retry_at(100, 1), Some(160));
        assert_eq!(retry_at(100, 4), Some(100 + 7200));
        assert_eq!(retry_at(100, 5), None);
        assert_eq!(timada_core_percent(1000), "10");
        assert_eq!(timada_core_percent(1250), "12.5");
        assert_eq!(timada_core_percent(1234), "12.34");
    }

    #[test]
    fn a_subject_links_to_its_page() {
        assert_eq!(
            subject_link("https://d.example/", "acme", Subject::Order, "o1"),
            "https://d.example/t/acme/orders/o1"
        );
        assert_eq!(
            subject_link("", "acme", Subject::Message, "m1"),
            "/t/acme/messages"
        );
    }
}
