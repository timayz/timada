//! Reminds a shop's merchants, before the hold on a shopper's card lapses,
//! of an order AliExpress may have and whose money is still only held: the
//! money is taken once AliExpress accepted the order, so an answer lost (a
//! timeout, a node stopping) leaves the order waiting on the merchant. Were
//! the hold to lapse unnoticed, the shop would have paid AliExpress and
//! the shopper never been charged ([`Released::KeptOpen`]).
//!
//! Nothing in the event store fires later, so every node sweeps, on a
//! timer, the orders the read model says are due a reminder
//! ([`hold_reminder_due_at`](OrderState::hold_reminder_due_at)), checks each
//! on its write model, and records the reminder on the order before mailing
//! (claim, then send, as the order mails do): the node that claims it first
//! is the one that mails, so the merchants are reminded once per order. The
//! mail goes to every member whose role lets them manage orders. Taking the
//! payment, or saying the order is not on AliExpress, ends the reminder.
//!
//! [`Released::KeptOpen`]: timada_orders::Released::KeptOpen

use std::sync::Arc;
use std::time::Duration;

use timada_identity::users_by_ids;
use timada_orders::{HoldReminderDue, OrderState, hold_reminders_due};
use timada_tenancy::{PERMISSION_ORDERS_MANAGE, TenantState};
use tokio::task::JoinHandle;
use topcoat::context::{AppContext, Cx};

use crate::cluster::Executor;
use crate::db::Db;
use crate::mail::send_hold_lapse_reminder;
use crate::message_notifications::{mailboxes, members_who_may};

/// Who the reminders say sent them.
pub const MAILER: &str = "timada-dashboard";

/// The most orders one sweep looks at, every shop together.
const ORDERS_PER_SWEEP: i64 = 200;

/// A reminder claimed and about to go out.
pub struct Reminder<'a> {
    pub shop: &'a TenantState,
    pub order: &'a OrderState,
    /// The addresses of the members who manage the shop's orders.
    pub recipients: &'a [String],
    /// The order's page in the dashboard.
    pub link: String,
}

/// What a reminder goes out through: the platform's mail, or a test's.
#[async_trait::async_trait]
pub trait Remind: Send + Sync {
    /// Mails the reminder to each recipient. An address that fails is the
    /// implementation's to log: the reminder is claimed already, so nothing
    /// is tried again.
    async fn remind(&self, reminder: Reminder<'_>) -> anyhow::Result<()>;
}

/// The platform's transport and sender.
pub struct PlatformMail(pub Arc<AppContext>);

#[async_trait::async_trait]
impl Remind for PlatformMail {
    async fn remind(&self, reminder: Reminder<'_>) -> anyhow::Result<()> {
        let cx = Cx::new(self.0.clone());
        for to in reminder.recipients {
            if let Err(err) = send_hold_lapse_reminder(
                &cx,
                to,
                &reminder.shop.name,
                &reminder.order.reference,
                reminder.order.hold_lapses_at(),
                &reminder.link,
            )
            .await
            {
                tracing::error!(tenant = %reminder.shop.id, order = %reminder.order.id, error = %err, "hold reminder not delivered");
            }
        }
        Ok(())
    }
}

/// The sweep, with what it needs outside any request.
pub struct HoldReminders<R: Remind = PlatformMail> {
    /// The read models: the orders due a reminder, the members' addresses.
    db: Db,
    /// The store, every tenant's and the platform's.
    store: Executor,
    remind: R,
    /// The dashboard's public address, for the link.
    base_url: String,
}

impl<R: Remind + 'static> HoldReminders<R> {
    pub fn new(db: Db, store: Executor, remind: R, base_url: String) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            remind,
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
                if let Err(err) = self.sweep(crate::auth::now()).await {
                    tracing::error!(error = %err, "hold reminders not swept");
                }
            }
        })
    }

    /// Reminds the merchants of the orders due at `now` (unix seconds), shop
    /// by shop; returns how many orders were mailed about. A shop that fails
    /// is logged and the others go on.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<usize> {
        let due = hold_reminders_due(&self.db, now as i64, ORDERS_PER_SWEEP).await?;
        let mut mailed = 0;
        for orders in due.chunk_by(|a, b| a.tenant_id == b.tenant_id) {
            let tenant_id = orders[0].tenant_id.as_str();
            match self.sweep_tenant(tenant_id, orders, now).await {
                Ok(count) => mailed += count,
                Err(err) => {
                    tracing::error!(%tenant_id, error = %err, "a shop's hold reminders not sent");
                }
            }
        }
        Ok(mailed)
    }

    async fn sweep_tenant(
        &self,
        tenant_id: &str,
        due: &[HoldReminderDue],
        now: u64,
    ) -> anyhow::Result<usize> {
        let Some(shop) = timada_tenancy::Command(self.store.clone())
            .load_tenant(tenant_id)
            .await?
        else {
            return Ok(0);
        };
        if shop.is_suspended() {
            return Ok(0);
        }
        // Nobody to tell: nothing is claimed, so a member given an address
        // later is still reminded while the hold lives.
        let recipients = mailboxes(
            users_by_ids(&self.db, &members_who_may(&shop, PERMISSION_ORDERS_MANAGE)).await?,
        );
        if recipients.is_empty() {
            tracing::debug!(%tenant_id, "no member to remind of a lapsing hold");
            return Ok(0);
        }
        let orders = timada_orders::Command::new(self.store.clone(), tenant_id);
        let mut mailed = 0;
        for row in due {
            // The write model decides; the read model only said where to look.
            if !orders.claim_hold_reminder(MAILER, &row.id, now).await? {
                continue;
            }
            let Some(order) = orders.load_order(&row.id).await? else {
                continue;
            };
            // Claimed, then sent: a failure here is not retried, since
            // merchants reminded twice are worse off than ones who read the
            // order's banner.
            let link = format!(
                "{}/t/{}/orders/{}",
                self.base_url.trim_end_matches('/'),
                shop.slug,
                order.id
            );
            match self
                .remind
                .remind(Reminder {
                    shop: &shop,
                    order: &order,
                    recipients: &recipients,
                    link,
                })
                .await
            {
                Ok(()) => {
                    mailed += 1;
                    tracing::info!(%tenant_id, order = %order.id, recipients = recipients.len(), "hold reminder sent");
                }
                Err(err) => {
                    tracing::error!(%tenant_id, order = %order.id, error = %err, "hold reminder not sent");
                }
            }
        }
        Ok(mailed)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use evento::Evento;
    use timada_orders::{HOLD_REMINDER_BEFORE_SECS, PROVIDER_STRIPE, Payment, ShippingAddress};
    use timada_tenancy::CreateTenant;

    use super::*;

    const ORDER: &str = "order-1";

    /// A reminder as the test's mail saw it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Sent {
        order_id: String,
        recipients: Vec<String>,
        link: String,
    }

    struct Recording(Arc<Mutex<Vec<Sent>>>);

    #[async_trait::async_trait]
    impl Remind for Recording {
        async fn remind(&self, reminder: Reminder<'_>) -> anyhow::Result<()> {
            self.0.lock().unwrap().push(Sent {
                order_id: reminder.order.id.clone(),
                recipients: reminder.recipients.to_vec(),
                link: reminder.link,
            });
            Ok(())
        }
    }

    /// A shop whose owner has an address, with one order AliExpress may
    /// have and whose hold the provider said lapses at `lapses_at`; the
    /// store, the read models, and the shop.
    async fn shop_with_order(lapses_at: u64) -> (Evento, Db, TenantState) {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&store).await.unwrap();
        let db = Db::from(store.clone());
        let executor = Evento::new(store);
        evento::append("alice")
            .original_version(0)
            .event(&timada_identity::aggregator::EmailRegistered {
                email: "alice@example.org".to_owned(),
                label: "alice@example.org".to_owned(),
                password_hash: String::new(),
            })
            .commit(&executor)
            .await
            .unwrap();
        let shop = timada_tenancy::Command(executor.clone())
            .create_tenant(CreateTenant {
                name: "acme".to_owned(),
                owner_user_id: "alice".to_owned(),
            })
            .await
            .unwrap();
        evento::append(ORDER)
            .tenant(&shop.id)
            .original_version(0)
            .event(&timada_orders::aggregator::Placed {
                reference: "K7Q4-MZP2".to_owned(),
                cart_id: "cart-1".to_owned(),
                email: "ada@example.com".to_owned(),
                shipping: ShippingAddress::default(),
                note: None,
                lines: Vec::new(),
                currency: "EUR".to_owned(),
                total_cents: 1000,
                payment: Payment {
                    provider: PROVIDER_STRIPE.to_owned(),
                    payment_id: "pi_1".to_owned(),
                    amount_cents: 1000,
                    currency: "EUR".to_owned(),
                    capture_before: Some(lapses_at),
                },
                supplier: Some("aliexpress".to_owned()),
                placed_by: "storefront".to_owned(),
            })
            .event(&timada_orders::aggregator::SupplierOrderStarted {
                supplier: "aliexpress".to_owned(),
                started_by: "timada-aliexpress".to_owned(),
            })
            .commit(&executor)
            .await
            .unwrap();
        catch_up(&db, &executor).await;
        (executor, db, shop)
    }

    /// The read models the sweep reads, brought up to date.
    async fn catch_up(db: &Db, executor: &Evento) {
        timada_identity::read_model_subscription::<Evento>(db.clone())
            .run_once(executor)
            .await
            .unwrap();
        timada_orders::read_model_subscription::<Evento>(db.clone())
            .run_once(executor)
            .await
            .unwrap();
    }

    /// Two days before the hold lapses, the owner is mailed once, however
    /// many nodes sweep at that moment, and never again.
    #[tokio::test]
    async fn the_merchant_is_reminded_once_also_with_two_nodes() {
        let now = crate::auth::now();
        let lapses_at = now + 7 * 86_400;
        let due = lapses_at - HOLD_REMINDER_BEFORE_SECS;
        let (executor, db, shop) = shop_with_order(lapses_at).await;
        let sent = Arc::new(Mutex::new(Vec::new()));
        let node = || {
            HoldReminders::new(
                db.clone(),
                executor.clone(),
                Recording(sent.clone()),
                "https://dashboard.example/".to_owned(),
            )
        };
        let (one, two) = (node(), node());

        // Not due yet.
        assert_eq!(one.sweep(due - 1).await.unwrap(), 0);
        assert!(sent.lock().unwrap().is_empty());

        // Due: both nodes sweep at once, one of them mails.
        let (first, second) = tokio::join!(one.sweep(due), two.sweep(due));
        assert_eq!(first.unwrap() + second.unwrap(), 1);
        assert_eq!(
            sent.lock().unwrap().clone(),
            vec![Sent {
                order_id: ORDER.to_owned(),
                recipients: vec!["alice@example.org".to_owned()],
                link: format!("https://dashboard.example/t/{}/orders/{ORDER}", shop.slug),
            }]
        );
        let order = timada_orders::Command::new(executor.clone(), &shop.id)
            .load_order(ORDER)
            .await
            .unwrap()
            .unwrap();
        assert!(order.hold_reminded_at.is_some());

        // Swept again before the read model caught up, and after: nothing.
        assert_eq!(two.sweep(due + 60).await.unwrap(), 0);
        catch_up(&db, &executor).await;
        assert_eq!(one.sweep(lapses_at).await.unwrap(), 0);
        assert_eq!(sent.lock().unwrap().len(), 1);
    }

    /// A shop with nobody to tell claims nothing: a member given an address
    /// later is still reminded while the hold lives. The money taken ends it.
    #[tokio::test]
    async fn nobody_to_tell_leaves_the_reminder_for_later() {
        let now = crate::auth::now();
        let lapses_at = now + 7 * 86_400;
        let (executor, db, shop) = shop_with_order(lapses_at).await;
        // Alice gave her address up.
        evento::append("alice")
            .original_version(1)
            .event(&timada_identity::aggregator::Deleted)
            .commit(&executor)
            .await
            .unwrap();
        catch_up(&db, &executor).await;
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sweep = HoldReminders::new(
            db.clone(),
            executor.clone(),
            Recording(sent.clone()),
            "https://dashboard.example".to_owned(),
        );
        assert_eq!(sweep.sweep(lapses_at).await.unwrap(), 0);
        let orders = timada_orders::Command::new(executor.clone(), &shop.id);
        let order = orders.load_order(ORDER).await.unwrap().unwrap();
        assert_eq!(order.hold_reminded_at, None, "not claimed");

        // Taken meanwhile: no longer at risk, whoever could be told.
        orders
            .request_capture("timada-dashboard", ORDER)
            .await
            .unwrap();
        orders
            .record_payment("timada-dashboard", ORDER, "pi_1", 1000)
            .await
            .unwrap();
        catch_up(&db, &executor).await;
        assert_eq!(
            timada_orders::hold_reminders_due(&db, lapses_at as i64, 10)
                .await
                .unwrap(),
            Vec::new()
        );
    }
}
