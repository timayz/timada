//! Asks the buyers of fulfilled orders, some days later, what they think of
//! what they bought: a mail under the shop's name with a link that opens the
//! order's review forms on the storefront, without the browser that placed
//! it. Nothing in the event store fires later, so every node sweeps the
//! fulfilled orders it knows of on a timer; the node that claims an order's
//! request first is the one that mails it.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use timada_orders::{FulfilledOrder, OrderState, OrderStatus, fulfilled_between};
use timada_reviews::{MAX_REQUEST_DELAY_DAYS, review_id};
use tokio::task::JoinHandle;
use topcoat::context::{AppContext, Cx};

use crate::cluster::Executor;
use crate::mail::send_review_request;

/// Who the requests say mailed them.
pub const MAILER: &str = "timada-dashboard";

const DAY_SECS: u64 = 24 * 60 * 60;

/// How late a request may still go out after it fell due: enough for a node
/// to have been down, while a shop that just turned the requests on (or a
/// node that just joined) does not mail its whole history.
pub const WINDOW_SECS: u64 = 3 * DAY_SECS;

/// Whether an order fulfilled at `fulfilled_at` is asked about at `now`,
/// `days` after it left.
pub fn is_due(fulfilled_at: u64, days: u8, now: u64) -> bool {
    let due = fulfilled_at + u64::from(days) * DAY_SECS;
    due <= now && now < due + WINDOW_SECS
}

/// The sweep, with what it needs outside any request.
pub struct ReviewRequests {
    read_pool: SqlitePool,
    /// The store, every tenant's.
    store: Executor,
    mail: Arc<AppContext>,
}

impl ReviewRequests {
    pub fn new(read_pool: SqlitePool, store: Executor, mail: Arc<AppContext>) -> Arc<Self> {
        Arc::new(Self {
            read_pool,
            store,
            mail,
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
                    tracing::error!(error = %err, "review requests not swept");
                }
            }
        })
    }

    /// Mails the buyers whose request is due at `now`, tenant by tenant. A
    /// tenant that fails is logged and the others still go.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<()> {
        let from = now.saturating_sub(u64::from(MAX_REQUEST_DELAY_DAYS) * DAY_SECS + WINDOW_SECS);
        let orders = fulfilled_between(&self.read_pool, from as i64, now as i64 + 1).await?;
        for tenant in orders.chunk_by(|a, b| a.tenant_id == b.tenant_id) {
            let tenant_id = &tenant[0].tenant_id;
            if let Err(err) = self.sweep_tenant(tenant_id, tenant, now).await {
                tracing::error!(tenant = %tenant_id, error = %err, "review requests not sent");
            }
        }
        Ok(())
    }

    async fn sweep_tenant(
        &self,
        tenant_id: &str,
        orders: &[FulfilledOrder],
        now: u64,
    ) -> anyhow::Result<()> {
        let Some(tenant) = timada_tenancy::Command(self.store.clone())
            .load_tenant(tenant_id)
            .await?
        else {
            return Ok(());
        };
        if tenant.is_suspended() {
            return Ok(());
        }
        let cx = Cx::new(self.mail.clone());
        ask_buyers(&self.store, &cx, &tenant, orders, now).await?;
        Ok(())
    }
}

/// Mails the buyer of each of `orders` (one tenant's) whose request is due
/// at `now`. Returns how many were mailed.
pub async fn ask_buyers<E: evento::Executor + Clone>(
    store: &E,
    cx: &Cx,
    tenant: &timada_tenancy::TenantState,
    orders: &[FulfilledOrder],
    now: u64,
) -> anyhow::Result<usize> {
    let reviews = timada_reviews::Command::new(store.clone(), &tenant.id);
    let Some(days) = reviews.review_requests().await?.delay_days() else {
        return Ok(0);
    };
    let due: Vec<&FulfilledOrder> = orders
        .iter()
        .filter(|order| is_due(order.fulfilled_at as u64, days, now))
        .collect();
    if due.is_empty() {
        return Ok(0);
    }
    // Without the storefront there is no link to send. Nothing is claimed,
    // so an address set within the window still gets its buyers mailed.
    let shop = timada_orders::Command::new(store.clone(), &tenant.id);
    let Some(address) = shop.storefront_address().await? else {
        return Ok(0);
    };
    let mut mailed = 0;
    for order in due {
        if reviews.load_review_request(&order.id).await?.is_some() {
            continue;
        }
        let Some(order) = shop.load_order(&order.id).await? else {
            continue;
        };
        let products = unreviewed(&reviews, &order).await?;
        if order.status != OrderStatus::Fulfilled
            || order.refunded_cents >= order.total_cents
            || products.is_empty()
        {
            continue;
        }
        if !reviews
            .claim_review_request(&order.id, &order.email, MAILER)
            .await?
        {
            continue;
        }
        // Claimed, then sent: a failure here is not retried, since a buyer
        // asked twice is worse off than one never asked.
        let token = reviews.issue_review_link(&order.id, MAILER, now).await?;
        let link = format!("{address}/reviews/{token}");
        match send_review_request(cx, tenant, &order, &products, &link).await {
            Ok(()) => {
                mailed += 1;
                tracing::info!(order = %order.id, "review request mailed");
            }
            Err(err) => {
                tracing::error!(order = %order.id, error = %err, "review request not delivered");
            }
        }
    }
    Ok(mailed)
}

/// The names of the products of `order` nobody reviewed yet, once each, in
/// the order's line order.
async fn unreviewed<E: evento::Executor>(
    reviews: &timada_reviews::Command<E>,
    order: &OrderState,
) -> anyhow::Result<Vec<String>> {
    let mut seen = Vec::new();
    let mut names = Vec::new();
    for line in &order.lines {
        if seen.contains(&line.product_id) {
            continue;
        }
        seen.push(line.product_id.clone());
        if reviews
            .load_review(&review_id(&order.id, &line.product_id))
            .await?
            .is_none()
        {
            names.push(line.name.clone());
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use evento::migrator::Migrate;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use timada_orders::aggregator::{Fulfilled, Placed};
    use timada_orders::{OrderLine, Payment, ShippingAddress};
    use timada_reviews::SubmitReview;
    use topcoat::context::CxTestBuilder;
    use topcoat::mail::{MailConfig, MemoryTransport};

    use super::*;
    use crate::mail::MailSender;

    const SHOP: &str = "https://shop.example";

    async fn store() -> evento::Sqlite {
        let options = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        evento::sql_migrator::new::<sqlx::Sqlite>()
            .unwrap()
            .run(&mut *conn, &evento::migrator::Plan::apply_all())
            .await
            .unwrap();
        drop(conn);
        pool.into()
    }

    fn shop(name: &str) -> timada_tenancy::TenantState {
        timada_tenancy::TenantState {
            id: "tenant-1".into(),
            name: name.into(),
            ..Default::default()
        }
    }

    fn mail() -> (Cx, MemoryTransport) {
        let transport = MemoryTransport::new();
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(transport.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
            .app_context(crate::shop_mail::ShopMail::new(None, false))
            .build();
        (cx, transport)
    }

    /// A paid order of a mug and some tea, fulfilled now. Returns its row.
    async fn fulfilled(store: &evento::Sqlite, order_id: &str) -> FulfilledOrder {
        let line = |product_id: &str, name: &str| OrderLine {
            product_id: product_id.to_owned(),
            name: name.to_owned(),
            unit_amount_cents: 1000,
            quantity: 1,
            ..Default::default()
        };
        evento::append(order_id)
            .tenant("tenant-1")
            .original_version(0)
            .event(&Placed {
                reference: format!("REF-{order_id}"),
                cart_id: format!("cart-{order_id}"),
                email: "ada@example.com".to_owned(),
                shipping: ShippingAddress {
                    name: "Ada Lovelace".to_owned(),
                    ..Default::default()
                },
                note: None,
                lines: vec![line("mug", "Mug"), line("tea", "Tea"), line("mug", "Mug")],
                currency: "EUR".to_owned(),
                total_cents: 3000,
                payment: Payment::default(),
                placed_by: "storefront".to_owned(),
            })
            .event(&Fulfilled {
                tracking: None,
                fulfilled_by: "user-alice".to_owned(),
            })
            .commit(store)
            .await
            .unwrap();
        let order = timada_orders::Command::new(store.clone(), "tenant-1")
            .load_order(order_id)
            .await
            .unwrap()
            .unwrap();
        FulfilledOrder {
            tenant_id: "tenant-1".to_owned(),
            id: order_id.to_owned(),
            fulfilled_at: order.fulfilled_at.unwrap() as i64,
        }
    }

    async fn with_storefront(store: &evento::Sqlite) {
        timada_orders::Command::new(store.clone(), "tenant-1")
            .set_storefront_address("user-alice", Some(SHOP))
            .await
            .unwrap();
    }

    #[test]
    fn a_request_falls_due_days_after_and_not_forever() {
        let left = 1_000_000;
        assert!(!is_due(left, 7, left + 7 * DAY_SECS - 1));
        assert!(is_due(left, 7, left + 7 * DAY_SECS));
        assert!(is_due(left, 7, left + 7 * DAY_SECS + WINDOW_SECS - 1));
        assert!(!is_due(left, 7, left + 7 * DAY_SECS + WINDOW_SECS));
        assert!(is_due(left, 0, left));
    }

    #[tokio::test]
    async fn a_buyer_is_asked_once_a_week_after_with_a_link() {
        let store = store().await;
        with_storefront(&store).await;
        let order = fulfilled(&store, "order-1").await;
        let left = order.fulfilled_at as u64;
        let (cx, transport) = mail();
        let orders = [order];

        // Too early.
        let early = left + 6 * DAY_SECS;
        assert_eq!(
            ask_buyers(&store, &cx, &shop("Acme Tea"), &orders, early)
                .await
                .unwrap(),
            0
        );
        let week = left + 7 * DAY_SECS;
        assert_eq!(
            ask_buyers(&store, &cx, &shop("Acme Tea"), &orders, week)
                .await
                .unwrap(),
            1
        );
        // Another sweep, or another node: already asked.
        assert_eq!(
            ask_buyers(&store, &cx, &shop("Acme Tea"), &orders, week)
                .await
                .unwrap(),
            0
        );

        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap()).unwrap();
        // Unfold the quoted-printable soft line breaks.
        let wire = wire.replace("=\r\n", "").replace("=\n", "");
        let start = wire.find("https://shop.example/reviews/").unwrap();
        let token: String = wire[start + "https://shop.example/reviews/".len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let reviews = timada_reviews::Command::new(store.clone(), "tenant-1");
        assert_eq!(
            reviews
                .resolve_review_link(&token, week)
                .await
                .unwrap()
                .as_deref(),
            Some("order-1")
        );
        assert!(
            reviews
                .load_review_request("order-1")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn an_order_left_long_ago_is_not_asked_about() {
        let store = store().await;
        with_storefront(&store).await;
        let order = fulfilled(&store, "order-1").await;
        let late = order.fulfilled_at as u64 + 7 * DAY_SECS + WINDOW_SECS;
        let (cx, transport) = mail();
        assert_eq!(
            ask_buyers(&store, &cx, &shop("Acme"), &[order], late)
                .await
                .unwrap(),
            0
        );
        assert!(transport.sent().is_empty());
    }

    #[tokio::test]
    async fn no_storefront_no_request_until_there_is_one() {
        let store = store().await;
        let order = fulfilled(&store, "order-1").await;
        let week = order.fulfilled_at as u64 + 7 * DAY_SECS;
        let orders = [order];
        let (cx, _) = mail();
        assert_eq!(
            ask_buyers(&store, &cx, &shop("Acme"), &orders, week)
                .await
                .unwrap(),
            0
        );
        let reviews = timada_reviews::Command::new(store.clone(), "tenant-1");
        assert!(
            reviews
                .load_review_request("order-1")
                .await
                .unwrap()
                .is_none()
        );

        with_storefront(&store).await;
        assert_eq!(
            ask_buyers(&store, &cx, &shop("Acme"), &orders, week + 1)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn the_merchant_decides_when_or_whether() {
        let store = store().await;
        with_storefront(&store).await;
        let order = fulfilled(&store, "order-1").await;
        let left = order.fulfilled_at as u64;
        let orders = [order];
        let (cx, _) = mail();
        let reviews = timada_reviews::Command::new(store.clone(), "tenant-1");

        reviews
            .set_review_requests("user-alice", None)
            .await
            .unwrap();
        assert_eq!(
            ask_buyers(&store, &cx, &shop("Acme"), &orders, left + 7 * DAY_SECS)
                .await
                .unwrap(),
            0
        );
        reviews
            .set_review_requests("user-alice", Some(0))
            .await
            .unwrap();
        assert_eq!(
            ask_buyers(&store, &cx, &shop("Acme"), &orders, left)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn only_what_is_not_reviewed_yet_is_asked_about() {
        let store = store().await;
        with_storefront(&store).await;
        let order = fulfilled(&store, "order-1").await;
        let week = order.fulfilled_at as u64 + 7 * DAY_SECS;
        let reviews = timada_reviews::Command::new(store.clone(), "tenant-1");
        let review = |product_id: &str| SubmitReview {
            order_id: "order-1".to_owned(),
            product_id: product_id.to_owned(),
            rating: 5,
            title: None,
            body: "Lovely, just lovely.".to_owned(),
            photos: vec![],
        };
        reviews
            .submit_review("storefront", review("mug"))
            .await
            .unwrap();
        let (cx, transport) = mail();
        let orders = [order];
        assert_eq!(
            ask_buyers(&store, &cx, &shop("Acme"), &orders, week)
                .await
                .unwrap(),
            1
        );
        let wire = String::from_utf8(transport.sent()[0].clone().formatted(&cx).unwrap()).unwrap();
        assert!(wire.contains("Tea"), "{wire}");
        assert!(!wire.contains("Mug"), "{wire}");

        // Everything reviewed: nothing to ask.
        let store = self::store().await;
        with_storefront(&store).await;
        let order = fulfilled(&store, "order-1").await;
        let reviews = timada_reviews::Command::new(store.clone(), "tenant-1");
        reviews
            .submit_review("storefront", review("mug"))
            .await
            .unwrap();
        reviews
            .submit_review("storefront", review("tea"))
            .await
            .unwrap();
        let week = order.fulfilled_at as u64 + 7 * DAY_SECS;
        assert_eq!(
            ask_buyers(&store, &cx, &shop("Acme"), &[order], week)
                .await
                .unwrap(),
            0
        );
    }
}
