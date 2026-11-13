//! Asks the buyers of fulfilled orders, some days later, what they think of
//! what they bought: a mail under the shop's name with a link that opens the
//! order's review forms on the storefront, without the browser that placed
//! it. Nothing in the event store fires later, so every node sweeps on a
//! timer the requests the reviews read model says are due and not mailed
//! ([`review_requests_due`]), a page at a time from where the last one
//! stopped: an order that cannot be asked about (its shop has no storefront
//! yet, it was refunded, everything on it is reviewed already) stays listed
//! until its window closes, and holds up no other. The node that claims an
//! order's request first is the one that mails it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::db::Db;
use timada_orders::{OrderState, OrderStatus};
use timada_reviews::{DueReviewRequest, REQUEST_WINDOW_SECS, review_id, review_requests_due};
use tokio::task::JoinHandle;
use topcoat::context::{AppContext, Cx};

use crate::cluster::Executor;
use crate::mail::send_review_request;

/// Who the requests say mailed them.
pub const MAILER: &str = "timada-dashboard";

const DAY_SECS: u64 = 24 * 60 * 60;

/// The most due requests one statement reads, every shop together.
const REQUESTS_PER_PAGE: u32 = 200;

/// Whether an order fulfilled at `fulfilled_at` is asked about at `now`,
/// `days` after it left.
pub fn is_due(fulfilled_at: u64, days: u8, now: u64) -> bool {
    let due = fulfilled_at + u64::from(days) * DAY_SECS;
    due <= now && now < due + REQUEST_WINDOW_SECS
}

/// The sweep, with what it needs outside any request.
pub struct ReviewRequests {
    /// The read models: the requests due.
    db: Db,
    /// The store, every tenant's, read from this node's file: a request
    /// claimed a moment ago on another node, which this node has not read
    /// yet, finds its claim taken.
    store: Executor,
    mail: Arc<AppContext>,
    /// How many due requests one statement reads.
    page_len: u32,
}

impl ReviewRequests {
    pub fn new(db: Db, store: Executor, mail: Arc<AppContext>) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            mail,
            page_len: REQUESTS_PER_PAGE,
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

    /// Mails the buyers whose request is due at `now`, page by page and shop
    /// by shop within a page. A shop that fails is logged and the others
    /// still go. Returns how many were mailed.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<usize> {
        let mut mailed = 0;
        let mut after = None;
        loop {
            let page = review_requests_due(&self.db, now, after.as_ref(), self.page_len).await?;
            let mut shops: BTreeMap<&str, Vec<DueReviewRequest>> = BTreeMap::new();
            for request in &page {
                shops
                    .entry(request.tenant_id.as_str())
                    .or_default()
                    .push(request.clone());
            }
            for (tenant_id, due) in shops {
                match self.sweep_tenant(tenant_id, &due, now).await {
                    Ok(count) => mailed += count,
                    Err(err) => {
                        tracing::error!(tenant = %tenant_id, error = %err, "review requests not sent");
                    }
                }
            }
            if page.len() < self.page_len as usize {
                return Ok(mailed);
            }
            after = page.last().cloned();
        }
    }

    async fn sweep_tenant(
        &self,
        tenant_id: &str,
        due: &[DueReviewRequest],
        now: u64,
    ) -> anyhow::Result<usize> {
        let Some(tenant) = timada_tenancy::Command(self.store.clone())
            .load_tenant(tenant_id)
            .await?
        else {
            return Ok(0);
        };
        if tenant.is_suspended() {
            return Ok(0);
        }
        let cx = Cx::new(self.mail.clone());
        ask_buyers(&self.store, &cx, &tenant, due, now).await
    }
}

/// Mails the buyer of each of `due` (one tenant's requests) whose request is
/// due at `now` under the shop's policy as it stands: the read model only
/// said where to look. Returns how many were mailed.
pub async fn ask_buyers<E: evento::Executor + Clone>(
    store: &E,
    cx: &Cx,
    tenant: &timada_tenancy::TenantState,
    due: &[DueReviewRequest],
    now: u64,
) -> anyhow::Result<usize> {
    let reviews = timada_reviews::Command::new(store.clone(), &tenant.id);
    let Some(days) = reviews.review_requests().await?.delay_days() else {
        return Ok(0);
    };
    let due: Vec<&DueReviewRequest> = due
        .iter()
        .filter(|request| is_due(request.fulfilled_at as u64, days, now))
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
    for request in due {
        // A request mailed is not listed; one mailed a moment ago, which the
        // read model has not seen yet, its claim finds taken.
        let Some(order) = shop.load_order(&request.order_id).await? else {
            continue;
        };
        let products = unreviewed(&reviews, &order).await?;
        // Refunded in full: nothing left of the sale to ask about. Money
        // that came in outside timada took nothing through the provider.
        let refunded_in_full = order.settled_outside.is_none() && order.outstanding_cents() == 0;
        if order.status != OrderStatus::Fulfilled
            || !order.is_paid()
            || refunded_in_full
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
    use timada_orders::aggregator::{Fulfilled, Paid, Placed};
    use timada_orders::{OrderLine, Payment, ShippingAddress};
    use timada_reviews::SubmitReview;
    use topcoat::context::CxTestBuilder;
    use topcoat::mail::{MailConfig, MemoryTransport};

    use super::*;
    use crate::mail::MailSender;

    const SHOP: &str = "https://shop.example";

    async fn store() -> evento::Turso {
        evento::Turso::in_memory_concurrent().await.unwrap()
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

    /// The store's database with the reviews read model in it.
    async fn read_models(store: &evento::Turso) -> Db {
        let mut conn = store.connect().unwrap();
        timada_reviews::MIGRATIONS
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
        Db::from(store.clone())
    }

    /// The reviews read model, caught up with the store.
    async fn catch_up(store: &evento::Turso, db: &Db) {
        timada_reviews::read_model_subscription::<evento::Turso>(db.clone())
            .run_once(store)
            .await
            .unwrap();
    }

    /// A shop on the platform, with a storefront unless `storefront` says
    /// otherwise.
    async fn shop_named(store: &evento::Turso, name: &str, storefront: bool) -> String {
        let tenant = timada_tenancy::Command(store.clone())
            .create_tenant(timada_tenancy::CreateTenant {
                name: name.to_owned(),
                owner_user_id: "user-alice".to_owned(),
            })
            .await
            .unwrap();
        if storefront {
            with_storefront_in(store, &tenant.id).await;
        }
        tenant.id
    }

    /// The sweep over `store`, reading `page_len` requests at a time, and
    /// what it mails.
    fn sweeper(store: &evento::Turso, db: &Db, page_len: u32) -> (ReviewRequests, MemoryTransport) {
        let transport = MemoryTransport::new();
        let mut mail = AppContext::new();
        mail.insert(MailConfig::builder().transport(transport.clone()).build());
        mail.insert(MailSender("Timada <no-reply@example.com>".parse().unwrap()));
        mail.insert(crate::shop_mail::ShopMail::new(None, false));
        let sweep = ReviewRequests {
            db: db.clone(),
            store: evento::Evento::new(store.clone()),
            mail: Arc::new(mail),
            page_len,
        };
        (sweep, transport)
    }

    /// A paid order of a mug and some tea, fulfilled now. Returns its
    /// request, due a week later as the default policy says.
    async fn fulfilled(store: &evento::Turso, order_id: &str) -> DueReviewRequest {
        fulfilled_in(store, "tenant-1", order_id).await
    }

    /// The same, in `tenant_id`.
    async fn fulfilled_in(
        store: &evento::Turso,
        tenant_id: &str,
        order_id: &str,
    ) -> DueReviewRequest {
        let line = |product_id: &str, name: &str| OrderLine {
            product_id: product_id.to_owned(),
            name: name.to_owned(),
            unit_amount_cents: 1000,
            quantity: 1,
            ..Default::default()
        };
        evento::append(order_id)
            .tenant(tenant_id)
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
                supplier: None,
                placed_by: "storefront".to_owned(),
            })
            .event(&Paid {
                payment_id: String::new(),
                amount_cents: 3000,
                paid_by: "timada-dashboard".to_owned(),
            })
            .event(&Fulfilled {
                tracking: None,
                tracking_url: None,
                fulfilled_by: "user-alice".to_owned(),
            })
            .commit(store)
            .await
            .unwrap();
        let order = timada_orders::Command::new(store.clone(), tenant_id)
            .load_order(order_id)
            .await
            .unwrap()
            .unwrap();
        let fulfilled_at = order.fulfilled_at.unwrap();
        DueReviewRequest {
            tenant_id: tenant_id.to_owned(),
            order_id: order_id.to_owned(),
            fulfilled_at: fulfilled_at as i64,
            due_at: (fulfilled_at + 7 * DAY_SECS) as i64,
        }
    }

    async fn with_storefront(store: &evento::Turso) {
        with_storefront_in(store, "tenant-1").await;
    }

    async fn with_storefront_in(store: &evento::Turso, tenant_id: &str) {
        timada_orders::Command::new(store.clone(), tenant_id)
            .set_storefront_address("user-alice", Some(SHOP))
            .await
            .unwrap();
    }

    #[test]
    fn a_request_falls_due_days_after_and_not_forever() {
        let left = 1_000_000;
        assert!(!is_due(left, 7, left + 7 * DAY_SECS - 1));
        assert!(is_due(left, 7, left + 7 * DAY_SECS));
        assert!(is_due(
            left,
            7,
            left + 7 * DAY_SECS + REQUEST_WINDOW_SECS - 1
        ));
        assert!(!is_due(left, 7, left + 7 * DAY_SECS + REQUEST_WINDOW_SECS));
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
        let late = order.fulfilled_at as u64 + 7 * DAY_SECS + REQUEST_WINDOW_SECS;
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

    #[tokio::test]
    async fn a_sweep_mails_each_request_due_once_and_lists_it_no_more() {
        let store = store().await;
        let db = read_models(&store).await;
        let tenant = shop_named(&store, "Acme Tea", true).await;
        let order = fulfilled_in(&store, &tenant, "order-1").await;
        catch_up(&store, &db).await;
        let (sweep, transport) = sweeper(&store, &db, REQUESTS_PER_PAGE);
        let week = order.due_at as u64;

        assert_eq!(sweep.sweep(week - 1).await.unwrap(), 0, "not yet");
        assert_eq!(
            review_requests_due(&db, week, None, 10).await.unwrap(),
            std::slice::from_ref(&order)
        );
        assert_eq!(sweep.sweep(week).await.unwrap(), 1);
        catch_up(&store, &db).await;
        assert!(
            review_requests_due(&db, week, None, 10)
                .await
                .unwrap()
                .is_empty(),
            "mailed, so no longer listed"
        );
        assert_eq!(sweep.sweep(week + 60).await.unwrap(), 0);
        assert_eq!(transport.sent().len(), 1);
    }

    #[tokio::test]
    async fn a_shop_that_cannot_be_asked_holds_up_no_other() {
        let store = store().await;
        let db = read_models(&store).await;
        // Its orders fall due first, and stay listed: no storefront to link.
        let stuck = shop_named(&store, "No Storefront", false).await;
        for n in 0..3 {
            fulfilled_in(&store, &stuck, &format!("stuck-{n}")).await;
        }
        let open = shop_named(&store, "Acme Tea", true).await;
        let order = fulfilled_in(&store, &open, "order-1").await;
        catch_up(&store, &db).await;

        // One request read at a time: the walk carries on past the stuck
        // shop's three.
        let (sweep, transport) = sweeper(&store, &db, 1);
        let now = order.due_at as u64;
        assert_eq!(sweep.sweep(now).await.unwrap(), 1);
        assert_eq!(transport.sent().len(), 1);
        assert_eq!(
            review_requests_due(&db, now, None, 10).await.unwrap().len(),
            4,
            "the read model has not seen the claim yet"
        );
        catch_up(&store, &db).await;
        let left: Vec<String> = review_requests_due(&db, now, None, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|request| request.order_id)
            .collect();
        assert_eq!(left, ["stuck-0", "stuck-1", "stuck-2"]);
    }
}
