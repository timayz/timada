//! Two nodes doing the same thing at once: one of them answers a request to
//! pay, and an order placed by both keeps one reference.

mod common;

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use evento::cursor::{Args, ReadResult, Value};
use evento::{EventFilter, Executor, RoutingKey, SubscriberStatus, TenantFilter, WriteError};
use ulid::Ulid;

use common::*;
use timada_orders::aggregator::Claimed;
use timada_orders::{CheckoutClaim, Command, Payment, order_reference_id};

#[tokio::test]
async fn one_node_answers_a_request_to_pay() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    let request_id = orders
        .request_checkout(SHOPPER, &cart_id, checkout())
        .await
        .unwrap();

    // Two nodes take it on at the same moment: one of them does.
    let (first, second) = tokio::join!(
        orders.claim_checkout(DASHBOARD, &cart_id, &request_id, "node-1"),
        orders.claim_checkout(DASHBOARD, &cart_id, &request_id, "node-2"),
    );
    let mut outcomes = vec![first.unwrap(), second.unwrap()];
    outcomes.sort_by_key(|outcome| *outcome != CheckoutClaim::Taken);
    assert_eq!(outcomes, [CheckoutClaim::Taken, CheckoutClaim::Held]);
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    let claim = cart.request(&request_id).unwrap().claim.clone().unwrap();
    let other = if claim.claimant == "node-1" {
        "node-2"
    } else {
        "node-1"
    };

    // Told the request again (a retry, a restart), the claimant goes on;
    // the other node still leaves it alone.
    assert_eq!(
        orders
            .claim_checkout(DASHBOARD, &cart_id, &request_id, &claim.claimant)
            .await
            .unwrap(),
        CheckoutClaim::Taken
    );
    assert_eq!(
        orders
            .claim_checkout(DASHBOARD, &cart_id, &request_id, other)
            .await
            .unwrap(),
        CheckoutClaim::Held
    );
    let again = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(
        again.request(&request_id).unwrap().claim,
        Some(claim.clone())
    );

    // Answered, it is nobody's to answer.
    assert!(
        orders
            .refuse_payment_request(DASHBOARD, &cart_id, &request_id, "price_changed")
            .await
            .unwrap(),
        "the refusal is the answer"
    );
    assert!(
        !orders
            .refuse_payment_request(DASHBOARD, &cart_id, &request_id, "other")
            .await
            .unwrap(),
        "a second answer is not"
    );
    for node in [claim.claimant.as_str(), other] {
        assert_eq!(
            orders
                .claim_checkout(DASHBOARD, &cart_id, &request_id, node)
                .await
                .unwrap(),
            CheckoutClaim::Answered
        );
    }
    assert_eq!(
        orders
            .claim_checkout(DASHBOARD, &cart_id, "no-such-request", "node-1")
            .await
            .unwrap(),
        CheckoutClaim::Answered
    );
}

#[tokio::test]
async fn a_claim_nobody_answers_lapses() {
    // The store stamps its writes from a clock set twenty minutes back,
    // longer than a claim holds.
    let offset_ms = Arc::new(AtomicI64::new(-20 * 60 * 1000));
    let clock = offset_ms.clone();
    let store = evento::Turso::in_memory_concurrent()
        .await
        .unwrap()
        .with_clock(move || {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            (now + clock.load(Ordering::SeqCst)) as u64
        });
    let orders: Orders = Command::new(store.clone(), TENANT);
    let catalog: Catalog = timada_catalog::Command::new(store, TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    let request_id = orders
        .request_checkout(SHOPPER, &cart_id, checkout())
        .await
        .unwrap();
    assert_eq!(
        orders
            .claim_checkout(DASHBOARD, &cart_id, &request_id, "node-1")
            .await
            .unwrap(),
        CheckoutClaim::Taken
    );

    // node-1 stopped for good: once its claim lapsed, another node takes
    // the request on, and node-1, back, leaves it to that one.
    offset_ms.store(0, Ordering::SeqCst);
    assert_eq!(
        orders
            .claim_checkout(DASHBOARD, &cart_id, &request_id, "node-2")
            .await
            .unwrap(),
        CheckoutClaim::Taken
    );
    assert_eq!(
        orders
            .claim_checkout(DASHBOARD, &cart_id, &request_id, "node-1")
            .await
            .unwrap(),
        CheckoutClaim::Held
    );
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(
        cart.request(&request_id)
            .unwrap()
            .claim
            .as_ref()
            .map(|claim| claim.claimant.as_str()),
        Some("node-2")
    );
}

#[tokio::test]
async fn a_reference_names_only_the_order_that_carries_it() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    let placed = pay(&orders, &cart_id, checkout()).await;

    // A reference claimed for the order, which was placed under another.
    evento::append(order_reference_id("ZZZZ-ZZZZ"))
        .original_version(0)
        .event(&Claimed {
            order_id: placed.order_id.clone(),
        })
        .tenant(TENANT)
        .commit(orders.executor())
        .await
        .unwrap();
    assert!(
        orders
            .load_order_by_reference("ZZZZ-ZZZZ")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        orders
            .load_order_by_reference(&placed.reference)
            .await
            .unwrap()
            .map(|order| order.id),
        Some(placed.order_id)
    );
}

#[tokio::test]
async fn an_order_placed_at_once_elsewhere_gives_its_reference_back() {
    let (executor, _) = executor_with_pool().await;
    let elsewhere = PlacedElsewhere::new(executor.clone());
    let orders = Command::new(elsewhere.clone(), TENANT);
    let catalog: Catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = orders.create_cart(SHOPPER).await.unwrap();
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&mug, None, 1))
        .await
        .unwrap();
    let (_, payment) = open_payment_on(&orders, &cart_id).await;

    // Another node places the order between this one reading that there is
    // none and writing it: that order stands, and this node's reference is
    // given back.
    elsewhere.arm(&cart_id, payment.clone());
    let placed = orders
        .place_order(DASHBOARD, &cart_id, payment)
        .await
        .unwrap();
    let order = orders.load_order(&placed.order_id).await.unwrap().unwrap();
    assert_eq!(placed.reference, order.reference);

    let references = reference_streams(&executor).await;
    assert_eq!(references.len(), 2, "each node claimed one: {references:?}");
    for reference in references {
        let index = timada_orders::order_reference_projection::<evento::Turso>()
            .load(&reference)
            .tenant(TENANT)
            .execute(&executor)
            .await
            .unwrap()
            .unwrap();
        let carried = reference == order_reference_id(&order.reference);
        assert_eq!(index.released, !carried, "{index:?}");
        assert_eq!(index.order_id, order.id);
    }
}

/// Opens a payment for the cart as the dashboard would, through `orders`.
async fn open_payment_on(orders: &Command<PlacedElsewhere>, cart_id: &str) -> (String, Payment) {
    let request_id = orders
        .request_checkout(SHOPPER, cart_id, checkout())
        .await
        .unwrap();
    let pending = orders
        .pending_payment(cart_id, &request_id)
        .await
        .unwrap()
        .unwrap();
    let payment = Payment {
        provider: timada_orders::PROVIDER_STRIPE.into(),
        payment_id: format!("pi_{request_id}"),
        amount_cents: pending.total_cents,
        currency: pending.currency.clone(),
        capture_before: None,
    };
    orders
        .record_payment_request(
            DASHBOARD,
            cart_id,
            pending,
            timada_orders::OpenedPayment {
                provider: timada_orders::PROVIDER_STRIPE.into(),
                payment_id: payment.payment_id.clone(),
                client_secret: format!("{}_secret", payment.payment_id),
                publishable_key: "pk_test".into(),
            },
        )
        .await
        .unwrap();
    (request_id, payment)
}

/// The order-reference streams of the tenant.
async fn reference_streams(executor: &evento::Turso) -> Vec<String> {
    let filter = EventFilter::by_type::<timada_orders::aggregator::OrderReference>();
    let read = executor
        .read(
            Some(Arc::from([filter])),
            None,
            TenantFilter::Only(TENANT.to_owned()),
            Args::forward(100, None),
            None,
        )
        .await
        .unwrap();
    let streams: std::collections::BTreeSet<String> = read
        .edges
        .into_iter()
        .map(|edge| edge.node.aggregate_id)
        .collect();
    streams.into_iter().collect()
}

/// The store of a node that another node beats to placing an order: armed
/// with a cart and its payment, the first time this node writes `Placed`,
/// the other places that order first.
#[derive(Clone)]
struct PlacedElsewhere {
    store: evento::Turso,
    armed: Arc<Mutex<Option<(String, Payment)>>>,
}

impl PlacedElsewhere {
    fn new(store: evento::Turso) -> Self {
        Self {
            store,
            armed: Arc::default(),
        }
    }

    fn arm(&self, cart_id: &str, payment: Payment) {
        *self.armed.lock().unwrap() = Some((cart_id.to_owned(), payment));
    }
}

#[async_trait::async_trait]
impl Executor for PlacedElsewhere {
    async fn write(&self, events: Vec<evento::Event>) -> Result<(), WriteError> {
        if events.iter().any(|event| event.name == "Placed") {
            let armed = self.armed.lock().unwrap().take();
            if let Some((cart_id, payment)) = armed {
                Command::new(self.store.clone(), TENANT)
                    .place_order(DASHBOARD, &cart_id, payment)
                    .await
                    .unwrap();
            }
        }
        self.store.write(events).await
    }

    async fn read(
        &self,
        aggregators: Option<Arc<[EventFilter]>>,
        routing_key: Option<RoutingKey>,
        tenant: TenantFilter,
        args: Args,
        to_micros: Option<u64>,
    ) -> anyhow::Result<ReadResult<evento::Event>> {
        self.store
            .read(aggregators, routing_key, tenant, args, to_micros)
            .await
    }

    async fn latest_timestamp(
        &self,
        aggregators: Option<Arc<[EventFilter]>>,
        routing_key: Option<RoutingKey>,
        tenant: TenantFilter,
    ) -> anyhow::Result<u64> {
        self.store
            .latest_timestamp(aggregators, routing_key, tenant)
            .await
    }

    async fn get_subscriber_cursor(&self, key: String) -> anyhow::Result<Option<Value>> {
        self.store.get_subscriber_cursor(key).await
    }

    async fn is_subscriber_running(&self, key: String, worker_id: Ulid) -> anyhow::Result<bool> {
        self.store.is_subscriber_running(key, worker_id).await
    }

    async fn subscriber_status(
        &self,
        key: String,
        worker_id: Ulid,
    ) -> anyhow::Result<SubscriberStatus> {
        self.store.subscriber_status(key, worker_id).await
    }

    async fn latest_version(
        &self,
        tenant: String,
        aggregate_type: String,
        aggregate_id: String,
    ) -> anyhow::Result<u16> {
        self.store
            .latest_version(tenant, aggregate_type, aggregate_id)
            .await
    }

    async fn stream_routing_key(
        &self,
        tenant: String,
        aggregate_type: String,
        aggregate_id: String,
    ) -> anyhow::Result<Option<Option<String>>> {
        self.store
            .stream_routing_key(tenant, aggregate_type, aggregate_id)
            .await
    }

    async fn upsert_subscriber(&self, key: String, worker_id: Ulid) -> anyhow::Result<()> {
        self.store.upsert_subscriber(key, worker_id).await
    }

    async fn acknowledge(
        &self,
        key: String,
        worker_id: Ulid,
        cursor: Value,
        lag: u64,
    ) -> anyhow::Result<bool> {
        self.store.acknowledge(key, worker_id, cursor, lag).await
    }

    async fn get_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        aggregate_revision: String,
        id: String,
    ) -> anyhow::Result<Option<(Vec<u8>, Value)>> {
        self.store
            .get_snapshot(tenant, aggregate_type, projection, aggregate_revision, id)
            .await
    }

    async fn save_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        aggregate_revision: String,
        id: String,
        data: Vec<u8>,
        cursor: Value,
    ) -> anyhow::Result<()> {
        self.store
            .save_snapshot(
                tenant,
                aggregate_type,
                projection,
                aggregate_revision,
                id,
                data,
                cursor,
            )
            .await
    }

    async fn delete_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        id: String,
    ) -> anyhow::Result<()> {
        self.store
            .delete_snapshot(tenant, aggregate_type, projection, id)
            .await
    }
}
