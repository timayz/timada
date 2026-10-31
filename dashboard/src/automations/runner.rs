//! Takes the runs through their steps: nothing in the event store fires
//! later, so every node looks for the runs due (`automations_runs`) on a
//! timer and moves each as far as it can now. Every move is written against
//! the version the run was loaded at: a node that loses the race writes
//! nothing and leaves the run to the winner.
//!
//! A step that acts outside the run (a mail) is claimed first and acts only
//! for the node whose claim is written; it is not tried again, since a
//! member mailed twice is worse off than one not mailed. A claim whose
//! outcome never comes (the node stopped) fails the step once its lease is
//! over.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use timada_automations::{
    AUTOMATION_ACTOR, ListenerState, RUN_HISTORY_SECS, RunState, Step, Transition,
    catalog::{self, CodeValue, DISCOUNT, EMAIL, NOTIFY, Subject, WEBHOOK},
    due_runs,
    interpreter::{self, Next, STUCK_SETTINGS, STUCK_UNKNOWN},
    left_checkouts, listener_of, normalize_email, opt_out_token, prune_checkouts, prune_runs,
    scenario_name,
};
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
use crate::cluster::Executor;
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
    read_pool: SqlitePool,
    /// The read-model database, writable: ended runs leave the list.
    write_pool: SqlitePool,
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
    definitions: Definitions,
}

impl<E: Store> Runner<E> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        read_pool: SqlitePool,
        write_pool: SqlitePool,
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
    ) -> anyhow::Result<Arc<Self>> {
        // rustls needs a process-wide crypto provider; installing twice is fine.
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
        // The webhooks' addresses are the merchants': no redirect, public
        // hosts only (the resolver checks the address dialled).
        let http = crate::net::public_client(allow_private_hosts)
            .timeout(webhook_timeout)
            .build()?;
        Ok(Arc::new(Self {
            read_pool,
            write_pool,
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
            definitions: Definitions::default(),
        }))
    }

    /// Sweeps every `every`, the first time right away.
    pub fn start(self: Arc<Self>, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                if let Err(err) = self.sweep(crate::auth::now()).await {
                    tracing::error!(error = %err, "automation runs not swept");
                }
            }
        })
    }

    /// Moves every run due by `now`. A run that fails to move is logged and
    /// the others still go.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<()> {
        prune_runs(&self.write_pool, now.saturating_sub(RUN_HISTORY_SECS)).await?;
        if let Err(err) = self.find_left_checkouts(now).await {
            tracing::error!(error = %err, "checkouts left unpaid not looked for");
        }
        let mut tenants: HashMap<String, Option<TenantState>> = HashMap::new();
        for (tenant_id, run_id) in due_runs(&self.read_pool, now, self.runs_per_look).await? {
            if !tenants.contains_key(&tenant_id) {
                let tenant = timada_tenancy::Command(self.store.clone())
                    .load_tenant(&tenant_id)
                    .await?;
                tenants.insert(tenant_id.clone(), tenant);
            }
            let Some(tenant) = tenants.get(&tenant_id).and_then(Option::as_ref) else {
                continue;
            };
            // A suspended shop's runs wait for it.
            if tenant.is_suspended() {
                continue;
            }
            if let Err(err) = self.advance(tenant, &run_id, now).await {
                tracing::error!(tenant = %tenant_id, run = %run_id, error = %err, "automation run not moved");
            }
        }
        Ok(())
    }

    /// Moves one run as far as it goes at `now`.
    pub async fn advance(
        &self,
        tenant: &TenantState,
        run_id: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        let automations = timada_automations::Command::new(self.store.clone(), &tenant.id);
        let Some(mut run) = automations.load_run(run_id).await? else {
            return Ok(());
        };
        for _ in 0..MOVES_PER_LOOK {
            if run.status.is_done() {
                return Ok(());
            }
            match listener_of(&self.read_pool, &tenant.id, &run.scenario_id)
                .await?
                .map(|listener| listener.state())
            {
                Some(ListenerState::Active) => {}
                Some(ListenerState::Archived) => {
                    let cancel = Transition::Cancel {
                        reason: "The scenario was deleted.".into(),
                    };
                    automations
                        .advance_run(AUTOMATION_ACTOR, &run, &[cancel])
                        .await?;
                    return Ok(());
                }
                // Held, or published a moment ago and not seen here yet.
                Some(ListenerState::Paused) | None => return Ok(()),
            }
            let Some(definition) = self
                .definitions
                .get(
                    &self.store,
                    &tenant.id,
                    &run.scenario_id,
                    run.scenario_version,
                )
                .await?
            else {
                return Ok(());
            };
            let moved = match interpreter::next(&definition, &run, now) {
                Next::Sleep(_) => return Ok(()),
                Next::Finish => {
                    automations
                        .advance_run(AUTOMATION_ACTOR, &run, &[Transition::Finish])
                        .await?
                }
                Next::Wait(step, until) => {
                    let wait = Transition::Wait {
                        step: step.id.clone(),
                        until,
                    };
                    automations
                        .advance_run(AUTOMATION_ACTOR, &run, &[wait])
                        .await?
                }
                Next::Branch(step) => {
                    let transition = self
                        .branch(tenant, &run, definition.subject(), step)
                        .await?;
                    automations
                        .advance_run(AUTOMATION_ACTOR, &run, &[transition])
                        .await?
                }
                Next::Act(step) => {
                    // A mail to the customer waits for what the shop must set
                    // up, then fails saying what is missing.
                    if step.kind == EMAIL
                        && self.mail_blocked(tenant).await?.is_some()
                        && now < run.updated_at + MAIL_HOLD_SECS
                    {
                        return Ok(());
                    }
                    let attempt = run.attempts(&step.id) + 1;
                    let claim = Transition::Claim {
                        step: step.id.clone(),
                        attempt,
                    };
                    if !automations
                        .advance_run(AUTOMATION_ACTOR, &run, &[claim])
                        .await?
                    {
                        return Ok(());
                    }
                    let Some(claimed) = automations.load_run(run_id).await? else {
                        return Ok(());
                    };
                    let outcome = match self.act(tenant, &claimed, definition.subject(), step).await
                    {
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
                    run = claimed;
                    automations
                        .advance_run(AUTOMATION_ACTOR, &run, &[outcome])
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
                        .advance_run(AUTOMATION_ACTOR, &run, &[lapsed])
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
                        .advance_run(AUTOMATION_ACTOR, &run, &[stuck])
                        .await?
                }
            };
            // Another node moved it meanwhile: it carries on from there.
            if !moved {
                return Ok(());
            }
            let Some(next) = automations.load_run(run_id).await? else {
                return Ok(());
            };
            run = next;
        }
        Ok(())
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
            &self.read_pool,
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
        let recipients = mailboxes(timada_identity::users_by_ids(&self.read_pool, &ids).await?);
        if recipients.is_empty() {
            return Ok(Acted::Skipped("No member to tell.".into()));
        }
        let scenario = scenario_name(&self.read_pool, &tenant.id, &run.scenario_id)
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
    /// is one; `None` once the address is gone (erased).
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
                "The customer's address is gone (erased).".into(),
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
        let scenario = scenario_name(&self.read_pool, &tenant.id, &run.scenario_id)
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
    /// Records the checkouts no order followed within the delay: each one
    /// starts the scenarios on that trigger, once whichever node finds it.
    async fn find_left_checkouts(&self, now: u64) -> anyhow::Result<()> {
        let to = now.saturating_sub(self.abandoned_after);
        let from = to.saturating_sub(LEFT_WINDOW_SECS);
        prune_checkouts(&self.write_pool, from).await?;
        for (tenant_id, cart_id, request_id) in
            left_checkouts(&self.read_pool, from, to, self.runs_per_look).await?
        {
            let orders = timada_orders::Command::new(self.store.clone(), &tenant_id);
            let Some(cart) = orders.load_cart(&cart_id).await? else {
                continue;
            };
            // Ordered meanwhile, or asked again since: not left, or not yet.
            let latest = cart
                .requests
                .last()
                .map(|request| request.request_id.as_str());
            if cart.order_id.is_some() || latest != Some(request_id.as_str()) {
                continue;
            }
            timada_automations::Command::new(self.store.clone(), &tenant_id)
                .detect_abandoned_checkout(AUTOMATION_ACTOR, &cart_id, &request_id)
                .await?;
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
    use evento::migrator::Migrate;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx_migrator::Info;
    use timada_automations::{
        Command, Definition, Lane, Outcome, RunStatus, Slot,
        catalog::{CONDITION, SUBSCRIBER_CONFIRMED, WAIT},
        read_model_subscription,
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
    async fn store() -> (Evento, SqlitePool) {
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
            .add_migrations(timada_identity::migrations())
            .unwrap();
        migrator
            .add_migrations(timada_orders::migrations())
            .unwrap();
        migrator
            .add_migrations(timada_newsletter::migrations())
            .unwrap();
        migrator
            .add_migrations(timada_automations::migrations())
            .unwrap();
        migrator
            .run(&mut *conn, &evento::migrator::Plan::apply_all())
            .await
            .unwrap();
        drop(conn);
        let sqlite: evento::Sqlite = pool.clone().into();
        (Evento::new(sqlite), pool)
    }

    /// The trigger subscription, caught up: the listeners, then the runs
    /// the events start.
    async fn follow(store: &Evento, pool: &SqlitePool) {
        super::super::triggers::subscription(pool.clone(), store.clone())
            .no_retry()
            .run_once(store)
            .await
            .unwrap();
    }

    /// The runs' read model, caught up.
    async fn catch_up(store: &Evento, pool: &SqlitePool) {
        read_model_subscription::<Evento>(pool.clone())
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

    fn runner(store: &Evento, pool: &SqlitePool, keys: &Arc<MemoryKeys>) -> Arc<Runner<Evento>> {
        Runner::new(
            pool.clone(),
            pool.clone(),
            store.clone(),
            Arc::new(AppContext::new()),
            "https://dashboard.example".into(),
            Arc::new(MemoryRing(keys.clone())),
            None,
            None,
            Duration::from_secs(5),
            true,
            Duration::from_secs(3600),
            10,
        )
        .unwrap()
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

    async fn the_run(store: &Evento, pool: &SqlitePool) -> Option<RunState> {
        catch_up(store, pool).await;
        let due = due_runs(pool, crate::auth::now() + 10_000_000, 10)
            .await
            .unwrap();
        let (_, id) = due.first()?;
        Command::new(store.clone(), TENANT)
            .load_run(id)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn an_event_starts_one_run_that_goes_through_its_steps() {
        let (store, pool) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        publish(&store, &welcome()).await;
        follow(&store, &pool).await;
        subscribe(&store, &keys, "ada@example.com").await;
        follow(&store, &pool).await;
        // Seen again (a redelivery, another node): still one run.
        follow(&store, &pool).await;
        catch_up(&store, &pool).await;
        let due = due_runs(&pool, crate::auth::now() + 1, 10).await.unwrap();
        assert_eq!(due.len(), 1);

        let run_id = &due[0].1;
        runner(&store, &pool, &keys)
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
        let (store, pool) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        subscribe(&store, &keys, "ada@example.com").await;
        publish(&store, &welcome()).await;
        follow(&store, &pool).await;
        assert!(the_run(&store, &pool).await.is_none());
    }

    #[tokio::test]
    async fn a_paused_scenario_holds_its_runs_until_resumed() {
        let (store, pool) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        let id = publish(&store, &welcome()).await;
        follow(&store, &pool).await;
        subscribe(&store, &keys, "ada@example.com").await;
        follow(&store, &pool).await;
        let automations = Command::new(store.clone(), TENANT);
        automations.pause_scenario(MEMBER, &id).await.unwrap();
        follow(&store, &pool).await;

        let run = the_run(&store, &pool).await.unwrap();
        let runner = runner(&store, &pool, &keys);
        runner
            .advance(&shop(), &run.id, crate::auth::now())
            .await
            .unwrap();
        let held = automations.load_run(&run.id).await.unwrap().unwrap();
        assert!(held.log.is_empty());
        assert_eq!(held.status, RunStatus::Running);

        automations.resume_scenario(MEMBER, &id).await.unwrap();
        follow(&store, &pool).await;
        runner
            .advance(&shop(), &run.id, crate::auth::now())
            .await
            .unwrap();
        let done = automations.load_run(&run.id).await.unwrap().unwrap();
        assert_eq!(done.status, RunStatus::Finished);
    }

    #[tokio::test]
    async fn a_wait_holds_the_run_until_its_time() {
        let (store, pool) = store().await;
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
        follow(&store, &pool).await;
        subscribe(&store, &keys, "ada@example.com").await;
        follow(&store, &pool).await;

        let run = the_run(&store, &pool).await.unwrap();
        let runner = runner(&store, &pool, &keys);
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
        let cart = orders.create_cart("storefront").await.unwrap();
        orders
            .add_to_cart(
                "storefront",
                &cart,
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
                        country: "FR".into(),
                        phone: None,
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
        let (store, pool) = store().await;
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
        follow(&store, &pool).await;
        let cart = checkout_left(&store).await;
        follow(&store, &pool).await;

        let runner = runner(&store, &pool, &keys);
        let now = crate::auth::now();
        // Within the hour: not left yet.
        runner.sweep(now).await.unwrap();
        follow(&store, &pool).await;
        assert!(the_run(&store, &pool).await.is_none());
        // The hour is over: found left once, whatever the looks.
        runner.sweep(now + 3601).await.unwrap();
        runner.sweep(now + 3602).await.unwrap();
        follow(&store, &pool).await;
        let run = the_run(&store, &pool).await.unwrap();
        assert_eq!(run.trigger, timada_automations::catalog::CHECKOUT_ABANDONED);
        assert_eq!(run.subject_id, cart);
        catch_up(&store, &pool).await;
        assert_eq!(
            due_runs(&pool, now + 10_000_000, 10).await.unwrap().len(),
            1
        );
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
