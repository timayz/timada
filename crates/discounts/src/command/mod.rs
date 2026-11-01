//! Write side: the command gateway and the strict write models it loads.
//!
//! Every write model folds *all* events of its aggregate (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never goes
//! stale. None of them is snapshotted: a discount is a handful of events, a
//! usage generation at most [`USAGE_CHUNK`](crate::USAGE_CHUNK) events, the
//! offers stream a few per offer.

mod code_index;
mod discount;
mod offer;
mod usage;

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::ops::Deref;

pub use discount::*;
pub use offer::*;
pub use usage::load_usage;

use evento::{Executor, Projection, metadata::Event};

use crate::aggregator::buyer::{
    Consumed as BuyerConsumed, Released as BuyerReleased, Reserved as BuyerReserved,
};
use crate::aggregator::usage::{
    Consumed, DiscountUsagePointer, Opened, Released as UsageReleased, Reserved, Rolled,
};
use crate::aggregator::{Archived, Claimed, Created, Deactivated, Reactivated, Released};
use crate::error::DiscountError;
use crate::value_object::{
    DiscountValue, MIN_CHARGE_CENTS, RECENT_USES, discount_buyer_id, discount_code_id,
};

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor> {
    executor: E,
    tenant: String,
}

impl<E: Executor> Command<E> {
    /// The gateway of one tenant: every stream it writes, loads or reads is
    /// that tenant's. `tenant` must not be empty (the default tenant holds
    /// the platform's own streams).
    pub fn new(executor: E, tenant: impl Into<String>) -> Self {
        let tenant = tenant.into();
        debug_assert!(!tenant.is_empty(), "a tenant gateway needs a tenant");
        Self { executor, tenant }
    }

    /// A gateway naming no tenant: for a store that is one tenant's already
    /// (a storefront's connection, which the store API serves in its key's
    /// tenant whatever its requests name). Every call then acts in the
    /// default tenant, which that store maps to its own.
    pub fn unscoped(executor: E) -> Self {
        Self {
            executor,
            tenant: String::new(),
        }
    }

    /// The tenant every call acts in.
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// The store the tenant's streams live in.
    pub fn executor(&self) -> &E {
        &self.executor
    }
}

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.executor
    }
}

// =============================================================================
// Discount
// =============================================================================

/// Whether shoppers may use a discount: an archived one is gone for good.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiscountStatus {
    #[default]
    Active,
    Inactive,
    Archived,
}

impl DiscountStatus {
    /// The value stored in the read models.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "inactive" => Some(Self::Inactive),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

impl fmt::Display for DiscountStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[evento::projection(id = id, name = "timada-discounts/DiscountState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct DiscountState {
    pub id: String,
    pub code: String,
    pub value: DiscountValue,
    /// Unix seconds; `None` is from the start.
    pub starts_at: Option<u64>,
    /// Unix seconds, excluded; `None` is no end.
    pub ends_at: Option<u64>,
    pub max_uses: Option<u32>,
    pub once_per_email: bool,
    pub status: DiscountStatus,
    pub created_by: String,
    /// Unix seconds of `Created`.
    pub created_at: u64,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl DiscountState {
    pub fn is_archived(&self) -> bool {
        self.status == DiscountStatus::Archived
    }

    /// Whether a shopper may use it at `now` (unix seconds): active, and
    /// inside its window.
    pub fn is_usable_at(&self, now: u64) -> bool {
        self.status == DiscountStatus::Active
            && self.starts_at.is_none_or(|start| start <= now)
            && self.ends_at.is_none_or(|end| now < end)
    }

    /// What it takes off an items subtotal in `currency` at `now`: refused
    /// when it cannot be used now, or is an amount in another currency.
    /// Never more than leaves [`MIN_CHARGE_CENTS`] to pay.
    pub fn amount_off(
        &self,
        subtotal_cents: u64,
        currency: &str,
        now: u64,
    ) -> Result<u64, DiscountError> {
        if !self.is_usable_at(now) {
            return Err(DiscountError::Inactive);
        }
        let off = self
            .value
            .off(subtotal_cents, currency)
            .ok_or(DiscountError::Currency)?;
        Ok(off.min(subtotal_cents.saturating_sub(MIN_CHARGE_CENTS)))
    }
}

pub fn discount_projection<E: Executor>() -> Projection<E, DiscountState> {
    Projection::new::<crate::aggregator::Discount>()
        .handler(on_created())
        .handler(on_deactivated())
        .handler(on_reactivated())
        .handler(on_archived())
        .strict()
}

#[evento::handler]
async fn on_created(event: Event<Created>, discount: &mut DiscountState) -> anyhow::Result<()> {
    let data = &event.data;
    discount.id = event.aggregate_id.to_owned();
    discount.code = data.code.to_owned();
    discount.value = match (data.percent_bps, data.amount_cents, &data.currency) {
        (Some(bps), _, _) => DiscountValue::Percent(bps),
        (None, Some(cents), Some(currency)) => DiscountValue::Amount {
            cents,
            currency: currency.to_owned(),
        },
        _ => anyhow::bail!("a discount takes a percentage or an amount off"),
    };
    discount.starts_at = data.starts_at;
    discount.ends_at = data.ends_at;
    discount.max_uses = data.max_uses;
    discount.once_per_email = data.once_per_email;
    discount.status = DiscountStatus::Active;
    discount.created_by = data.created_by.to_owned();
    discount.created_at = event.timestamp;
    discount.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_deactivated(
    event: Event<Deactivated>,
    discount: &mut DiscountState,
) -> anyhow::Result<()> {
    discount.status = DiscountStatus::Inactive;
    discount.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_reactivated(
    event: Event<Reactivated>,
    discount: &mut DiscountState,
) -> anyhow::Result<()> {
    discount.status = DiscountStatus::Active;
    discount.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_archived(event: Event<Archived>, discount: &mut DiscountState) -> anyhow::Result<()> {
    discount.status = DiscountStatus::Archived;
    discount.updated_at = event.timestamp;
    Ok(())
}

// =============================================================================
// DiscountCode (index)
// =============================================================================

#[evento::projection(id = id, name = "timada-discounts/DiscountCodeState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct DiscountCodeState {
    pub id: String,
    pub discount_id: String,
    pub released: bool,
}

pub fn discount_code_projection<E: Executor>() -> Projection<E, DiscountCodeState> {
    Projection::new::<crate::aggregator::DiscountCode>()
        .handler(on_code_claimed())
        .handler(on_code_released())
        .strict()
}

#[evento::handler]
async fn on_code_claimed(
    event: Event<Claimed>,
    index: &mut DiscountCodeState,
) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.discount_id = event.data.discount_id.to_owned();
    index.released = false;
    Ok(())
}

#[evento::handler]
async fn on_code_released(
    _event: Event<Released>,
    index: &mut DiscountCodeState,
) -> anyhow::Result<()> {
    index.released = true;
    Ok(())
}

// =============================================================================
// DiscountUsage / DiscountBuyer
// =============================================================================

/// A use held for a checkout until `expires_at` (unix seconds, excluded).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reservation {
    pub expires_at: u64,
}

/// A use an order spent, as the generation remembers it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecentUse {
    pub cart_id: String,
    pub order_id: String,
}

/// How many spent uses a generation remembers: [`RECENT_USES`], or two in
/// the unit tests so forgetting one is cheap to reach.
const fn recent_uses() -> usize {
    if cfg!(test) { 2 } else { RECENT_USES }
}

/// The uses of a capped code, as its current generation says them: the ones
/// held by checkouts, by cart id, and how many orders spent one.
#[evento::projection(id = id, name = "timada-discounts/UsageState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct UsageState {
    pub id: String,
    pub reserved: BTreeMap<String, Reservation>,
    /// How many orders spent a use, over every generation.
    pub consumed_count: u32,
    /// The last [`RECENT_USES`] orders that spent one, oldest first.
    pub recent: VecDeque<RecentUse>,
}

impl UsageState {
    /// The uses that count against the cap at `now`: the spent ones and the
    /// reservations that have not lapsed.
    pub fn taken_at(&self, now: u64) -> usize {
        self.consumed_count as usize
            + self
                .reserved
                .values()
                .filter(|held| now < held.expires_at)
                .count()
    }

    /// Whether `cart_id` holds a live reservation at `now`.
    pub fn holds(&self, cart_id: &str, now: u64) -> bool {
        self.reserved
            .get(cart_id)
            .is_some_and(|held| now < held.expires_at)
    }

    /// The order that spent the use of `cart_id`, as far as the generation
    /// remembers.
    pub fn consumed_by_cart(&self, cart_id: &str) -> Option<&str> {
        self.recent
            .iter()
            .find(|spent| spent.cart_id == cart_id)
            .map(|spent| spent.order_id.as_str())
    }

    /// The reservations not lapsed at `now`.
    pub(crate) fn live_reservations(
        &self,
        now: u64,
    ) -> Vec<crate::value_object::CarriedReservation> {
        self.reserved
            .iter()
            .filter(|(_, held)| now < held.expires_at)
            .map(|(cart_id, held)| crate::value_object::CarriedReservation {
                cart_id: cart_id.clone(),
                expires_at: held.expires_at,
            })
            .collect()
    }

    /// Remembers `spent`, forgetting the oldest beyond the recent ones.
    fn remember(&mut self, spent: RecentUse) {
        self.recent.retain(|known| known.cart_id != spent.cart_id);
        self.recent.push_back(spent);
        while self.recent.len() > recent_uses() {
            self.recent.pop_front();
        }
    }
}

pub fn usage_projection<E: Executor>() -> Projection<E, UsageState> {
    Projection::new::<crate::aggregator::usage::DiscountUsage>()
        .handler(on_usage_opened())
        .handler(on_usage_reserved())
        .handler(on_usage_released())
        .handler(on_usage_consumed())
        .strict()
}

#[evento::handler]
async fn on_usage_opened(event: Event<Opened>, usage: &mut UsageState) -> anyhow::Result<()> {
    usage.id = event.aggregate_id.to_owned();
    usage.reserved = event
        .data
        .reserved
        .iter()
        .map(|held| {
            (
                held.cart_id.clone(),
                Reservation {
                    expires_at: held.expires_at,
                },
            )
        })
        .collect();
    usage.consumed_count = event.data.consumed_count;
    usage.recent = event
        .data
        .recent
        .iter()
        .map(|spent| RecentUse {
            cart_id: spent.cart_id.clone(),
            order_id: spent.order_id.clone(),
        })
        .collect();
    Ok(())
}

/// Which generation of a code's uses is current.
#[evento::projection(id = id, name = "timada-discounts/UsagePointerState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct UsagePointerState {
    pub id: String,
    pub generation: u32,
}

pub fn usage_pointer_projection<E: Executor>() -> Projection<E, UsagePointerState> {
    Projection::new::<DiscountUsagePointer>()
        .handler(on_usage_rolled())
        .strict()
}

#[evento::handler]
async fn on_usage_rolled(
    event: Event<Rolled>,
    pointer: &mut UsagePointerState,
) -> anyhow::Result<()> {
    pointer.id = event.aggregate_id.to_owned();
    pointer.generation = pointer.generation.max(event.data.generation);
    Ok(())
}

#[evento::handler]
async fn on_usage_reserved(event: Event<Reserved>, usage: &mut UsageState) -> anyhow::Result<()> {
    usage.id = event.aggregate_id.to_owned();
    usage.reserved.insert(
        event.data.cart_id.to_owned(),
        Reservation {
            expires_at: event.data.expires_at,
        },
    );
    Ok(())
}

#[evento::handler]
async fn on_usage_released(
    event: Event<UsageReleased>,
    usage: &mut UsageState,
) -> anyhow::Result<()> {
    usage.reserved.remove(&event.data.cart_id);
    Ok(())
}

#[evento::handler]
async fn on_usage_consumed(event: Event<Consumed>, usage: &mut UsageState) -> anyhow::Result<()> {
    usage.id = event.aggregate_id.to_owned();
    usage.reserved.remove(&event.data.cart_id);
    usage.consumed_count = usage.consumed_count.saturating_add(1);
    usage.remember(RecentUse {
        cart_id: event.data.cart_id.to_owned(),
        order_id: event.data.order_id.to_owned(),
    });
    Ok(())
}

/// One shopper's use of a code good once per e-mail.
#[evento::projection(id = id, name = "timada-discounts/BuyerState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct BuyerState {
    pub id: String,
    /// The cart holding or having spent the use; `None` once released.
    pub cart_id: Option<String>,
    pub expires_at: u64,
    /// The order that spent it.
    pub order_id: Option<String>,
}

impl BuyerState {
    /// The cart the use belongs to at `now`, if any: the one that spent it,
    /// or the one whose reservation has not lapsed.
    pub fn holder_at(&self, now: u64) -> Option<&str> {
        let cart_id = self.cart_id.as_deref()?;
        (self.order_id.is_some() || now < self.expires_at).then_some(cart_id)
    }
}

pub fn buyer_projection<E: Executor>() -> Projection<E, BuyerState> {
    Projection::new::<crate::aggregator::buyer::DiscountBuyer>()
        .handler(on_buyer_reserved())
        .handler(on_buyer_released())
        .handler(on_buyer_consumed())
        .strict()
}

#[evento::handler]
async fn on_buyer_reserved(
    event: Event<BuyerReserved>,
    buyer: &mut BuyerState,
) -> anyhow::Result<()> {
    buyer.id = event.aggregate_id.to_owned();
    buyer.cart_id = Some(event.data.cart_id.to_owned());
    buyer.expires_at = event.data.expires_at;
    Ok(())
}

#[evento::handler]
async fn on_buyer_released(
    event: Event<BuyerReleased>,
    buyer: &mut BuyerState,
) -> anyhow::Result<()> {
    if buyer.cart_id.as_deref() == Some(event.data.cart_id.as_str()) {
        buyer.cart_id = None;
    }
    Ok(())
}

#[evento::handler]
async fn on_buyer_consumed(
    event: Event<BuyerConsumed>,
    buyer: &mut BuyerState,
) -> anyhow::Result<()> {
    buyer.id = event.aggregate_id.to_owned();
    buyer.cart_id = Some(event.data.cart_id.to_owned());
    buyer.order_id = Some(event.data.order_id.to_owned());
    Ok(())
}

// =============================================================================
// Loaders
// =============================================================================

impl<E: Executor> Command<E> {
    pub async fn load_discount(&self, discount_id: &str) -> anyhow::Result<Option<DiscountState>> {
        discount_projection()
            .load(discount_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The live discount a code names, if any (the code is normalized here,
    /// so a shopper's typing is fine).
    pub async fn load_discount_by_code(&self, code: &str) -> anyhow::Result<Option<DiscountState>> {
        let Ok(code) = crate::value_object::normalize_code(code) else {
            return Ok(None);
        };
        let Some(index) = self.load_code_index(&code).await? else {
            return Ok(None);
        };
        Ok(self
            .load_discount(&index.discount_id)
            .await?
            .filter(|discount| !discount.is_archived()))
    }

    /// The live index entry of a code (released ones are `None`).
    pub async fn load_code_index(&self, code: &str) -> anyhow::Result<Option<DiscountCodeState>> {
        Ok(self
            .load_code_index_any(code)
            .await?
            .filter(|index| !index.released))
    }

    /// The index entry including released ones (needed to re-claim).
    pub(crate) async fn load_code_index_any(
        &self,
        code: &str,
    ) -> anyhow::Result<Option<DiscountCodeState>> {
        discount_code_projection()
            .load(discount_code_id(code))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The uses of a capped discount (`None` before the first one).
    pub async fn load_usage(&self, discount_id: &str) -> anyhow::Result<Option<UsageState>> {
        load_usage(&self.executor, &self.tenant, discount_id).await
    }

    /// One shopper's use of a discount good once per e-mail.
    pub async fn load_buyer(
        &self,
        discount_id: &str,
        email: &str,
    ) -> anyhow::Result<Option<BuyerState>> {
        buyer_projection()
            .load(discount_buyer_id(discount_id, email))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The discount, or `NotFound`; `Archived` once it is: every edit starts
    /// here.
    pub(crate) async fn require_editable(
        &self,
        discount_id: &str,
    ) -> Result<DiscountState, DiscountError> {
        let discount = self
            .load_discount(discount_id)
            .await?
            .ok_or(DiscountError::NotFound)?;
        if discount.is_archived() {
            return Err(DiscountError::Archived);
        }
        Ok(discount)
    }
}
