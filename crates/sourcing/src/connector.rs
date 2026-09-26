//! Where a supplier's answers come from is a port, [`SupplierConnector`]: a
//! marketplace's API behind an adapter of the host's, or nobody — then the
//! supplier is worked by hand ([`ManualConnector`]) and an operator types
//! what it costs and buys on its site themselves.
//!
//! A connector says what it is able to do ([`SupplierConnector::does`]) and
//! how hard it may be pushed ([`ConnectorLimits`]); the workers ask nothing
//! of it that it has not agreed to, and call it at most once a pass per
//! supplier.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use timada_core::{Address, Money};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectorError {
    /// The supplier will keep saying no: an address it does not serve, an
    /// order it will not take. Asking again is pointless.
    #[error("refused: {0}")]
    Refused(String),
    /// It could not be reached, or failed on its own side. Asking again may
    /// work.
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// Too many calls. Nothing of this supplier is asked for `retry_after`
    /// seconds — and it costs the waiting work no attempt: being throttled
    /// is not a failure of the thing being asked for.
    #[error("rate limited, retry in {retry_after}s")]
    RateLimited { retry_after: u64 },
    /// The item is gone from the supplier's catalogue.
    #[error("unknown item {0}")]
    UnknownItem(String),
}

/// What a connector is able to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorTask {
    /// Say what an item costs and how many it holds.
    Offers,
    /// Place an order.
    Placing,
    /// Say where an order it took has got to.
    Tracking,
}

impl ConnectorTask {
    /// How a refusal words it: "connector `x` does not place orders".
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Offers => "quote items",
            Self::Placing => "place orders",
            Self::Tracking => "track orders",
        }
    }
}

/// How hard a connector may be pushed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectorLimits {
    /// Items one [`SupplierConnector::offers`] call may ask about.
    pub batch: u32,
    /// The shortest gap between two calls to this supplier.
    pub min_interval: Duration,
}

impl Default for ConnectorLimits {
    fn default() -> Self {
        Self {
            batch: 20,
            min_interval: Duration::from_secs(1),
        }
    }
}

/// What the shop knows an item by on the supplier's side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupplierItemRef {
    pub external_item_id: String,
    pub external_sku: Option<String>,
}

impl SupplierItemRef {
    pub fn new(external_item_id: impl Into<String>, external_sku: Option<String>) -> Self {
        Self {
            external_item_id: external_item_id.into(),
            external_sku,
        }
    }

    /// Whether the supplier's answer is about this item.
    pub fn matches(&self, other: &Self) -> bool {
        self.external_item_id == other.external_item_id && self.external_sku == other.external_sku
    }
}

/// One item, as the supplier describes it now. Costs are in the supplier's
/// own currency and exclude tax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupplierOffer {
    pub item: SupplierItemRef,
    pub cost: Money,
    /// What the supplier charges to ship one unit; zero when it is free.
    pub shipping: Money,
    pub available: u32,
    /// What the supplier calls it — shown when an operator links the item.
    pub title: Option<String>,
    pub url: Option<String>,
}

/// One line of what the shop wants to buy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseLine {
    pub item: SupplierItemRef,
    pub quantity: u32,
    /// What the supplier quoted for one unit, in its own currency.
    pub unit_cost: Money,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceOrder<'a> {
    /// The shop's own id for the purchase: the connector's idempotency key,
    /// so a worker that died after the supplier answered buys nothing twice.
    pub reference: &'a str,
    pub lines: &'a [PurchaseLine],
    /// Where the supplier sends the parcel: the customer's address.
    pub ship_to: &'a Address,
    pub note: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedOrder {
    pub external_order_id: String,
    /// What the supplier actually charged, which is not always what it quoted.
    pub cost: Money,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupplierOrderStanding {
    Pending,
    Shipped {
        carrier: String,
        tracking_number: String,
    },
    Cancelled {
        reason: String,
    },
}

/// The result of a connector call, boxed so connectors can be `dyn`.
pub type ConnectorFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ConnectorError>> + Send + 'a>>;

pub trait SupplierConnector: Send + Sync {
    /// Matches `SupplierRegistered.connector`.
    fn key(&self) -> &str;

    fn does(&self, task: ConnectorTask) -> bool;

    fn limits(&self) -> ConnectorLimits {
        ConnectorLimits::default()
    }

    /// What the supplier says about a batch of items. Answers may come back
    /// in any order, and an item the supplier no longer lists may be left
    /// out rather than refused.
    fn offers<'a>(
        &'a self,
        items: &'a [SupplierItemRef],
    ) -> ConnectorFuture<'a, Vec<SupplierOffer>>;

    fn place<'a>(&'a self, order: &'a PlaceOrder<'a>) -> ConnectorFuture<'a, PlacedOrder>;

    fn standing<'a>(
        &'a self,
        external_order_id: &'a str,
    ) -> ConnectorFuture<'a, SupplierOrderStanding>;

    /// Best effort: a supplier that cannot be told refuses.
    fn cancel<'a>(&'a self, external_order_id: &'a str) -> ConnectorFuture<'a, ()>;
}

/// The connectors a host plugged in, keyed by [`SupplierConnector::key`].
/// Several from day one: a shop buying from two marketplaces is the ordinary
/// case, not an extension.
#[derive(Clone, Default)]
pub struct SupplierConnectors(Vec<Arc<dyn SupplierConnector>>);

impl SupplierConnectors {
    /// Adds a connector, replacing one registered under the same key.
    pub fn with(mut self, connector: impl SupplierConnector + 'static) -> Self {
        let connector: Arc<dyn SupplierConnector> = Arc::new(connector);
        self.0.retain(|known| known.key() != connector.key());
        self.0.push(connector);
        self
    }

    pub fn of(&self, key: &str) -> Option<&Arc<dyn SupplierConnector>> {
        self.0.iter().find(|connector| connector.key() == key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|connector| connector.key())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for SupplierConnectors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.keys()).finish()
    }
}

const WORKED_BY_HAND: &str = "this supplier is worked by hand";

/// A supplier with no API: the operator reads its site, types what it costs
/// and how many it holds, and buys there themselves. It says no to
/// everything, and because it does, the workers never call it.
#[derive(Debug, Clone, Copy, Default)]
pub struct ManualConnector;

impl ManualConnector {
    pub const KEY: &'static str = "manual";
}

impl SupplierConnector for ManualConnector {
    fn key(&self) -> &str {
        Self::KEY
    }

    fn does(&self, _task: ConnectorTask) -> bool {
        false
    }

    fn offers<'a>(
        &'a self,
        _items: &'a [SupplierItemRef],
    ) -> ConnectorFuture<'a, Vec<SupplierOffer>> {
        Box::pin(async { Err(ConnectorError::Refused(WORKED_BY_HAND.into())) })
    }

    fn place<'a>(&'a self, _order: &'a PlaceOrder<'a>) -> ConnectorFuture<'a, PlacedOrder> {
        Box::pin(async { Err(ConnectorError::Refused(WORKED_BY_HAND.into())) })
    }

    fn standing<'a>(&'a self, _id: &'a str) -> ConnectorFuture<'a, SupplierOrderStanding> {
        Box::pin(async { Err(ConnectorError::Refused(WORKED_BY_HAND.into())) })
    }

    fn cancel<'a>(&'a self, _id: &'a str) -> ConnectorFuture<'a, ()> {
        Box::pin(async { Err(ConnectorError::Refused(WORKED_BY_HAND.into())) })
    }
}

#[derive(Default)]
struct FakeState {
    offers: Vec<SupplierOffer>,
    orders: Vec<(String, SupplierOrderStanding)>,
    next_offers: Vec<Result<Vec<SupplierOffer>, ConnectorError>>,
    next_place: Vec<Result<PlacedOrder, ConnectorError>>,
    asked: Vec<Vec<SupplierItemRef>>,
    placed: Vec<(String, Money)>,
    cancelled: Vec<String>,
}

/// A connector for tests and demos: it holds a catalogue you stock, takes
/// every order, and lets you script what the next call answers.
///
/// Public API rather than a test fixture, like `FakeProvider` in
/// `timada-payment`: a host wiring the admin up before it has an API key
/// wants a supplier that behaves.
pub struct FakeConnector {
    key: String,
    limits: ConnectorLimits,
    state: Mutex<FakeState>,
}

impl Default for FakeConnector {
    fn default() -> Self {
        Self::new("fake")
    }
}

impl FakeConnector {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            limits: ConnectorLimits::default(),
            state: Mutex::new(FakeState::default()),
        }
    }

    pub fn with_limits(mut self, limits: ConnectorLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Puts an item in the supplier's catalogue, replacing what was there.
    pub fn stock(&self, offer: SupplierOffer) -> &Self {
        if let Ok(mut state) = self.state.lock() {
            state
                .offers
                .retain(|known| !known.item.matches(&offer.item));
            state.offers.push(offer);
        }
        self
    }

    /// Takes the item off the supplier's catalogue: it answers about it no
    /// more, the way a delisted item behaves.
    pub fn delist(&self, item: &SupplierItemRef) -> &Self {
        if let Ok(mut state) = self.state.lock() {
            state.offers.retain(|known| !known.item.matches(item));
        }
        self
    }

    /// What the next `offers` call answers, instead of the catalogue.
    pub fn answer_offers(&self, answer: Result<Vec<SupplierOffer>, ConnectorError>) -> &Self {
        if let Ok(mut state) = self.state.lock() {
            state.next_offers.push(answer);
        }
        self
    }

    /// What the next `place` call answers, instead of taking the order.
    pub fn answer_place(&self, answer: Result<PlacedOrder, ConnectorError>) -> &Self {
        if let Ok(mut state) = self.state.lock() {
            state.next_place.push(answer);
        }
        self
    }

    pub fn mark_shipped(&self, external_order_id: &str, carrier: &str, tracking: &str) -> &Self {
        self.set_standing(
            external_order_id,
            SupplierOrderStanding::Shipped {
                carrier: carrier.to_owned(),
                tracking_number: tracking.to_owned(),
            },
        )
    }

    pub fn mark_cancelled(&self, external_order_id: &str, reason: &str) -> &Self {
        self.set_standing(
            external_order_id,
            SupplierOrderStanding::Cancelled {
                reason: reason.to_owned(),
            },
        )
    }

    fn set_standing(&self, external_order_id: &str, standing: SupplierOrderStanding) -> &Self {
        if let Ok(mut state) = self.state.lock() {
            state.orders.retain(|(id, _)| id != external_order_id);
            state.orders.push((external_order_id.to_owned(), standing));
        }
        self
    }

    /// The batches it was asked about, in order.
    pub fn asked(&self) -> Vec<Vec<SupplierItemRef>> {
        self.state
            .lock()
            .map(|state| state.asked.clone())
            .unwrap_or_default()
    }

    /// The purchases it took, as `(reference, cost)`.
    pub fn placed(&self) -> Vec<(String, Money)> {
        self.state
            .lock()
            .map(|state| state.placed.clone())
            .unwrap_or_default()
    }

    pub fn cancelled(&self) -> Vec<String> {
        self.state
            .lock()
            .map(|state| state.cancelled.clone())
            .unwrap_or_default()
    }
}

impl SupplierConnector for FakeConnector {
    fn key(&self) -> &str {
        &self.key
    }

    fn does(&self, _task: ConnectorTask) -> bool {
        true
    }

    fn limits(&self) -> ConnectorLimits {
        self.limits
    }

    fn offers<'a>(
        &'a self,
        items: &'a [SupplierItemRef],
    ) -> ConnectorFuture<'a, Vec<SupplierOffer>> {
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .map_err(|_| ConnectorError::Unavailable("fake connector poisoned".into()))?;
            state.asked.push(items.to_vec());
            if !state.next_offers.is_empty() {
                return state.next_offers.remove(0);
            }
            Ok(state
                .offers
                .iter()
                .filter(|offer| items.iter().any(|item| offer.item.matches(item)))
                .cloned()
                .collect())
        })
    }

    fn place<'a>(&'a self, order: &'a PlaceOrder<'a>) -> ConnectorFuture<'a, PlacedOrder> {
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .map_err(|_| ConnectorError::Unavailable("fake connector poisoned".into()))?;
            let placed = if state.next_place.is_empty() {
                let currency = order
                    .lines
                    .first()
                    .map(|line| line.unit_cost.currency.clone())
                    .unwrap_or_else(|| Money::EUR.to_owned());
                let mut cost = Money::zero(currency);
                for line in order.lines {
                    cost = cost
                        .checked_add(
                            &line
                                .unit_cost
                                .checked_mul(line.quantity)
                                .map_err(|err| ConnectorError::Refused(err.to_string()))?,
                        )
                        .map_err(|err| ConnectorError::Refused(err.to_string()))?;
                }
                PlacedOrder {
                    external_order_id: format!("fake-{}", order.reference),
                    cost,
                }
            } else {
                state.next_place.remove(0)?
            };
            state
                .placed
                .push((order.reference.to_owned(), placed.cost.clone()));
            state.orders.push((
                placed.external_order_id.clone(),
                SupplierOrderStanding::Pending,
            ));
            Ok(placed)
        })
    }

    fn standing<'a>(
        &'a self,
        external_order_id: &'a str,
    ) -> ConnectorFuture<'a, SupplierOrderStanding> {
        Box::pin(async move {
            let state = self
                .state
                .lock()
                .map_err(|_| ConnectorError::Unavailable("fake connector poisoned".into()))?;
            state
                .orders
                .iter()
                .rev()
                .find(|(id, _)| id == external_order_id)
                .map(|(_, standing)| standing.clone())
                .ok_or_else(|| ConnectorError::UnknownItem(external_order_id.to_owned()))
        })
    }

    fn cancel<'a>(&'a self, external_order_id: &'a str) -> ConnectorFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self
                .state
                .lock()
                .map_err(|_| ConnectorError::Unavailable("fake connector poisoned".into()))?;
            state.cancelled.push(external_order_id.to_owned());
            state.orders.retain(|(id, _)| id != external_order_id);
            state.orders.push((
                external_order_id.to_owned(),
                SupplierOrderStanding::Cancelled {
                    reason: "cancelled by the shop".into(),
                },
            ));
            Ok(())
        })
    }
}
