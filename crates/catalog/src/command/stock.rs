//! Stock: how many of a thing the shop holds, the checkouts holding some of
//! it and the orders that took some.
//!
//! A checkout reserves what it sells before the shopper is asked to pay (an
//! optimistic write on the item's own stream, so two checkouts never take
//! the last one twice); the paid order consumes the hold, a refused or
//! abandoned checkout releases it (or lets it lapse: its expiry is read,
//! never scheduled) and a cancelled order puts its quantity back.
//!
//! # Generations
//!
//! Every checkout writes an event or two to the stream of each thing it
//! sells, and a stream has 65 535 versions: a thing that sells well would
//! stop selling after some thirty thousand checkouts. So a thing's stock is
//! written in generations of at most [`STOCK_CHUNK`] events. `StockPointer`
//! names the current one; the writer that finds it full opens the next
//! generation with [`Opened`], carrying what is on hand, the live holds and
//! the recent orders, then rolls the pointer, and tries its write again.
//! Nothing is ever appended to a full generation (every writer checks the
//! version it read against the cap before writing with it), so every writer
//! that opens the next one carries the same state, and the first to write
//! it wins.
//!
//! A generation forgets the orders that took from it, bar the last
//! [`RECENT_ORDERS`]; what makes an order take, and give back, its share
//! exactly once for good is its own `StockTake` stream.

use std::collections::{BTreeMap, VecDeque};

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::stock::{
    Consumed, OnHandSet, Opened, Released, Reserved, Restored, Rolled, StockItem, StockPointer,
    Tracked, Untracked,
};
use crate::aggregator::stock_take::{Restored as TakeRestored, StockTake, Taken};
use crate::error::CatalogError;
use crate::value_object::{
    CarriedHold, CarriedTake, MAX_STOCK, RECENT_ORDERS, STOCK_CHUNK, TakenLine, stock_item_id,
    stock_pointer_id, stock_take_id,
};

/// How often a write that lost a race with another one is tried again.
const WRITE_ATTEMPTS: usize = 5;

/// How many events a generation holds: [`STOCK_CHUNK`], or three in the
/// unit tests so a roll is cheap to reach.
const fn chunk_size() -> u16 {
    if cfg!(test) { 3 } else { STOCK_CHUNK }
}

/// How many orders a generation remembers: [`RECENT_ORDERS`], or two in the
/// unit tests so forgetting one is cheap to reach.
const fn recent_orders() -> usize {
    if cfg!(test) { 2 } else { RECENT_ORDERS }
}

fn is_conflict(err: &CatalogError) -> bool {
    matches!(err, CatalogError::Write(WriteError::InvalidOriginalVersion))
}

use timada_core::unix_now;

/// Some of a thing a checkout or an order names: a product without variants
/// (`variant_id` `None`) or one variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockLine {
    pub product_id: String,
    pub variant_id: Option<String>,
    pub quantity: u32,
}

impl From<&StockLine> for TakenLine {
    fn from(line: &StockLine) -> Self {
        Self {
            product_id: line.product_id.clone(),
            variant_id: line.variant_id.clone(),
            quantity: line.quantity,
        }
    }
}

/// The lines of a checkout, one per thing: the quote may split a line in two
/// (an offer prices one unit), and a thing is held once, for the sum.
fn merge(lines: &[StockLine]) -> Vec<StockLine> {
    let mut merged: BTreeMap<(String, Option<String>), u32> = BTreeMap::new();
    for line in lines {
        let quantity = merged
            .entry((line.product_id.clone(), line.variant_id.clone()))
            .or_default();
        *quantity = quantity.saturating_add(line.quantity);
    }
    merged
        .into_iter()
        .filter(|(_, quantity)| *quantity > 0)
        .map(|((product_id, variant_id), quantity)| StockLine {
            product_id,
            variant_id,
            quantity,
        })
        .collect()
}

/// What a checkout holds, until `expires_at` (unix seconds, excluded).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StockHold {
    pub quantity: u32,
    pub expires_at: u64,
}

/// One of the last orders a generation remembers: the one made from
/// `cart_id` (unknown when only its restoration is remembered), and whether
/// what it took is back on the shelf.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecentTake {
    pub order_id: String,
    pub cart_id: Option<String>,
    pub restored: bool,
}

/// The stock of one thing, as its current generation says it.
#[evento::projection(id = id, name = "timada-catalog/StockState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct StockState {
    pub id: String,
    pub product_id: String,
    pub variant_id: Option<String>,
    /// Untracked sells without limit.
    pub tracked: bool,
    /// On the shelf: what the merchant counted, less what orders took.
    pub on_hand: u32,
    /// Cart id → what its checkout holds.
    pub holds: BTreeMap<String, StockHold>,
    /// The last [`RECENT_ORDERS`] orders that took some of it, or gave it
    /// back, oldest first.
    pub recent: VecDeque<RecentTake>,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl StockState {
    /// What checkouts other than `cart_id` hold at `now`.
    pub fn held_at(&self, now: u64, except_cart: Option<&str>) -> u32 {
        self.holds
            .iter()
            .filter(|(cart_id, hold)| {
                now < hold.expires_at && Some(cart_id.as_str()) != except_cart
            })
            .map(|(_, hold)| hold.quantity)
            .fold(0u32, u32::saturating_add)
    }

    /// What a new checkout may still take at `now`: `None` when untracked
    /// (no limit).
    pub fn available_at(&self, now: u64) -> Option<u32> {
        self.available_for(None, now)
    }

    /// What the checkout of `cart_id` may take at `now`, its own hold aside:
    /// `None` when untracked.
    pub fn available_for(&self, cart_id: Option<&str>, now: u64) -> Option<u32> {
        self.tracked
            .then(|| self.on_hand.saturating_sub(self.held_at(now, cart_id)))
    }

    /// Whether the order made from `cart_id` took its share, as far as the
    /// generation remembers.
    pub fn consumed_by_cart(&self, cart_id: &str) -> bool {
        self.recent
            .iter()
            .any(|take| take.cart_id.as_deref() == Some(cart_id))
    }

    /// What the generation remembers of `order_id`.
    pub fn recent_take(&self, order_id: &str) -> Option<&RecentTake> {
        self.recent.iter().find(|take| take.order_id == order_id)
    }

    /// The holds not lapsed at `now`.
    fn live_holds(&self, now: u64) -> Vec<CarriedHold> {
        self.holds
            .iter()
            .filter(|(_, hold)| now < hold.expires_at)
            .map(|(cart_id, hold)| CarriedHold {
                cart_id: cart_id.clone(),
                quantity: hold.quantity,
                expires_at: hold.expires_at,
            })
            .collect()
    }

    /// Remembers `take`, forgetting the oldest beyond the recent ones.
    fn remember(&mut self, take: RecentTake) {
        self.recent.retain(|known| known.order_id != take.order_id);
        self.recent.push_back(take);
        while self.recent.len() > recent_orders() {
            self.recent.pop_front();
        }
    }
}

pub fn stock_projection<E: Executor>() -> Projection<E, StockState> {
    Projection::new::<StockItem>()
        .handler(on_opened())
        .handler(on_tracked())
        .handler(on_untracked())
        .handler(on_on_hand_set())
        .handler(on_reserved())
        .handler(on_released())
        .handler(on_consumed())
        .handler(on_restored())
        .strict()
}

#[evento::handler]
async fn on_opened(event: Event<Opened>, stock: &mut StockState) -> anyhow::Result<()> {
    stock.id = event.aggregate_id.to_owned();
    stock.product_id = event.data.product_id.to_owned();
    stock.variant_id = event.data.variant_id.to_owned();
    stock.tracked = event.data.tracked;
    stock.on_hand = event.data.on_hand;
    stock.holds = event
        .data
        .holds
        .iter()
        .map(|hold| {
            (
                hold.cart_id.clone(),
                StockHold {
                    quantity: hold.quantity,
                    expires_at: hold.expires_at,
                },
            )
        })
        .collect();
    stock.recent = event
        .data
        .recent
        .iter()
        .map(|take| RecentTake {
            order_id: take.order_id.clone(),
            cart_id: take.cart_id.clone(),
            restored: take.restored,
        })
        .collect();
    stock.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_tracked(event: Event<Tracked>, stock: &mut StockState) -> anyhow::Result<()> {
    stock.id = event.aggregate_id.to_owned();
    stock.product_id = event.data.product_id.to_owned();
    stock.variant_id = event.data.variant_id.to_owned();
    stock.tracked = true;
    stock.on_hand = event.data.on_hand;
    stock.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_untracked(event: Event<Untracked>, stock: &mut StockState) -> anyhow::Result<()> {
    stock.tracked = false;
    stock.holds.clear();
    stock.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_on_hand_set(event: Event<OnHandSet>, stock: &mut StockState) -> anyhow::Result<()> {
    stock.on_hand = event.data.on_hand;
    stock.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_reserved(event: Event<Reserved>, stock: &mut StockState) -> anyhow::Result<()> {
    stock.holds.insert(
        event.data.cart_id.to_owned(),
        StockHold {
            quantity: event.data.quantity,
            expires_at: event.data.expires_at,
        },
    );
    stock.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_released(event: Event<Released>, stock: &mut StockState) -> anyhow::Result<()> {
    stock.holds.remove(event.data.cart_id.as_str());
    stock.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_consumed(event: Event<Consumed>, stock: &mut StockState) -> anyhow::Result<()> {
    stock.holds.remove(event.data.cart_id.as_str());
    stock.on_hand = stock.on_hand.saturating_sub(event.data.quantity);
    stock.remember(RecentTake {
        order_id: event.data.order_id.to_owned(),
        cart_id: Some(event.data.cart_id.to_owned()),
        restored: false,
    });
    stock.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_restored(event: Event<Restored>, stock: &mut StockState) -> anyhow::Result<()> {
    stock.on_hand = stock.on_hand.saturating_add(event.data.quantity);
    let cart_id = stock
        .recent_take(&event.data.order_id)
        .and_then(|take| take.cart_id.clone());
    stock.remember(RecentTake {
        order_id: event.data.order_id.to_owned(),
        cart_id,
        restored: true,
    });
    stock.updated_at = event.timestamp;
    Ok(())
}

/// Which generation of a thing's stock is current.
#[evento::projection(id = id, name = "timada-catalog/StockPointerState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct StockPointerState {
    pub id: String,
    pub generation: u32,
}

pub fn stock_pointer_projection<E: Executor>() -> Projection<E, StockPointerState> {
    Projection::new::<StockPointer>()
        .handler(on_rolled())
        .strict()
}

#[evento::handler]
async fn on_rolled(event: Event<Rolled>, pointer: &mut StockPointerState) -> anyhow::Result<()> {
    pointer.id = event.aggregate_id.to_owned();
    pointer.generation = pointer.generation.max(event.data.generation);
    Ok(())
}

/// What one paid order took off the shelves, and whether it is back.
#[evento::projection(id = id, name = "timada-catalog/StockTakeState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct StockTakeState {
    pub id: String,
    pub cart_id: String,
    pub lines: Vec<TakenLine>,
    pub restored: bool,
}

pub fn stock_take_projection<E: Executor>() -> Projection<E, StockTakeState> {
    Projection::new::<StockTake>()
        .handler(on_taken())
        .handler(on_take_restored())
        .strict()
}

#[evento::handler]
async fn on_taken(event: Event<Taken>, take: &mut StockTakeState) -> anyhow::Result<()> {
    take.id = event.aggregate_id.to_owned();
    take.cart_id = event.data.cart_id.to_owned();
    take.lines = event.data.lines.to_owned();
    Ok(())
}

#[evento::handler]
async fn on_take_restored(
    _event: Event<TakeRestored>,
    take: &mut StockTakeState,
) -> anyhow::Result<()> {
    take.restored = true;
    Ok(())
}

/// The stock of a product without variants (`variant_id` `None`) or of one
/// variant, as its current generation says it; `None` when it was never
/// tracked. `tenant` is the one the streams are in.
pub async fn load_stock<E: Executor>(
    executor: &E,
    tenant: &str,
    product_id: &str,
    variant_id: Option<&str>,
) -> anyhow::Result<Option<StockState>> {
    Ok(read_generation(executor, tenant, product_id, variant_id)
        .await?
        .state)
}

/// A thing's current generation, as read for one attempt.
struct Generation {
    /// The pointer, `None` while the first generation is current.
    pointer: Option<StockPointerState>,
    /// The generation the pointer names (0 without one).
    number: u32,
    /// The stream of that generation.
    id: String,
    /// Its state, `None` when nothing was ever written to it.
    state: Option<StockState>,
}

impl Generation {
    /// Whether nothing more may be written to it: the next write opens the
    /// generation after it.
    fn full(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|stock| stock.aggregate_version >= chunk_size())
    }
}

async fn read_generation<E: Executor>(
    executor: &E,
    tenant: &str,
    product_id: &str,
    variant_id: Option<&str>,
) -> anyhow::Result<Generation> {
    let pointer = stock_pointer_projection()
        .load(stock_pointer_id(product_id, variant_id))
        .tenant(tenant)
        .execute(executor)
        .await?;
    let number = pointer.as_ref().map_or(0, |pointer| pointer.generation);
    let id = stock_item_id(product_id, variant_id, number);
    let state = stock_projection()
        .load(&id)
        .tenant(tenant)
        .execute(executor)
        .await?;
    Ok(Generation {
        pointer,
        number,
        id,
        state,
    })
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Counts a product without variants (`variant_id` `None`) or one of its
    /// variants: `on_hand` are on the shelf now. Tracks it when it was not.
    pub async fn set_stock(
        &self,
        actor: &str,
        product_id: &str,
        variant_id: Option<&str>,
        on_hand: u32,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        if on_hand > MAX_STOCK {
            return Err(CatalogError::InvalidStock);
        }
        let product = self.require_editable(product_id).await?;
        match variant_id {
            Some(variant_id) => {
                product
                    .variant(variant_id)
                    .ok_or(CatalogError::VariantNotFound)?;
            }
            None if !product.variants.is_empty() => return Err(CatalogError::VariantNotFound),
            None => {}
        }
        for attempt in 1..=WRITE_ATTEMPTS {
            let current = self
                .current_generation(actor, product_id, variant_id, &routing_key)
                .await?;
            let written = match current.state {
                Some(stock) if stock.tracked && stock.on_hand == on_hand => return Ok(()),
                Some(stock) if stock.tracked => stock
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&OnHandSet {
                        on_hand,
                        set_by: actor.to_owned(),
                    })
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
                    .map_err(CatalogError::from),
                Some(stock) => stock
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&tracked(actor, product_id, variant_id, on_hand))
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
                    .map_err(CatalogError::from),
                None => evento::append(&current.id)
                    .original_version(0)
                    .routing_key_opt(routing_key.clone())
                    .event(&tracked(actor, product_id, variant_id, on_hand))
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
                    .map_err(CatalogError::from),
            };
            match written {
                Ok(_) => return Ok(()),
                Err(err) if is_conflict(&err) && attempt < WRITE_ATTEMPTS => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    /// Lets a thing sell without limit again; untracked already is a no-op.
    pub async fn untrack_stock(
        &self,
        actor: &str,
        product_id: &str,
        variant_id: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        self.require_editable(product_id).await?;
        for attempt in 1..=WRITE_ATTEMPTS {
            let Some(stock) = self
                .current_generation(actor, product_id, variant_id, &routing_key)
                .await?
                .state
                .filter(|stock| stock.tracked)
            else {
                return Ok(());
            };
            let written = stock
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Untracked {
                    untracked_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await
                .map_err(CatalogError::from);
            match written {
                Ok(_) => return Ok(()),
                Err(err) if is_conflict(&err) && attempt < WRITE_ATTEMPTS => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    /// Holds what the checkout of `cart_id` sells for `ttl_secs`, thing by
    /// thing: `OutOfStock` (and nothing held) when one of them has less left
    /// than it asks. Reserving again for the same cart moves the holds;
    /// untracked things hold nothing.
    pub async fn reserve_stock(
        &self,
        actor: &str,
        cart_id: &str,
        lines: &[StockLine],
        ttl_secs: u64,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let lines = merge(lines);
        let mut held: Vec<StockLine> = Vec::new();
        for line in &lines {
            match self
                .reserve_line(actor, cart_id, line, ttl_secs, &routing_key)
                .await
            {
                Ok(true) => held.push(line.clone()),
                Ok(false) => {}
                Err(err) => {
                    if let Err(release) = self
                        .release_lines(actor, cart_id, &held, &routing_key)
                        .await
                    {
                        tracing::warn!(%cart_id, %release, "cannot give back the stock a checkout held");
                    }
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    /// Gives back what the checkout of `cart_id` holds, when no order took it.
    pub async fn release_stock(
        &self,
        actor: &str,
        cart_id: &str,
        lines: &[StockLine],
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        self.release_lines(actor, cart_id, &merge(lines), &routing_key)
            .await
    }

    /// Takes what the paid order `order_id` (made from `cart_id`) sold off
    /// the shelf, and records the take in the order's own stream. Delivery
    /// may repeat: an order takes its share once.
    pub async fn consume_stock(
        &self,
        actor: &str,
        cart_id: &str,
        order_id: &str,
        lines: &[StockLine],
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        if self.load_take(order_id).await?.is_some() {
            return Ok(());
        }
        let mut taken: Vec<TakenLine> = Vec::new();
        for line in merge(lines) {
            for attempt in 1..=WRITE_ATTEMPTS {
                let Some(stock) = self
                    .current_generation(
                        actor,
                        &line.product_id,
                        line.variant_id.as_deref(),
                        &routing_key,
                    )
                    .await?
                    .state
                    .filter(|stock| stock.tracked)
                else {
                    break;
                };
                if stock.recent_take(order_id).is_some() {
                    taken.push(TakenLine::from(&line));
                    break;
                }
                let written = stock
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&Consumed {
                        cart_id: cart_id.to_owned(),
                        order_id: order_id.to_owned(),
                        quantity: line.quantity,
                        consumed_by: actor.to_owned(),
                    })
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
                    .map_err(CatalogError::from);
                match written {
                    Ok(_) => {
                        taken.push(TakenLine::from(&line));
                        break;
                    }
                    Err(err) if is_conflict(&err) && attempt < WRITE_ATTEMPTS => {}
                    Err(err) => return Err(err),
                }
            }
        }
        if taken.is_empty() {
            return Ok(());
        }
        let written = evento::append(stock_take_id(order_id))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Taken {
                cart_id: cart_id.to_owned(),
                lines: taken,
                taken_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match written {
            // Another delivery of the order recorded it meanwhile.
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// Puts back what the cancelled order `order_id` took, as its own stream
    /// recorded it. Delivery may repeat: an order's quantity comes back once.
    pub async fn restore_stock(
        &self,
        actor: &str,
        order_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let Some(take) = self
            .load_take(order_id)
            .await?
            .filter(|take| !take.restored)
        else {
            return Ok(());
        };
        for line in &take.lines {
            for attempt in 1..=WRITE_ATTEMPTS {
                let Some(stock) = self
                    .current_generation(
                        actor,
                        &line.product_id,
                        line.variant_id.as_deref(),
                        &routing_key,
                    )
                    .await?
                    .state
                    .filter(|stock| stock.tracked)
                else {
                    break;
                };
                if stock
                    .recent_take(order_id)
                    .is_some_and(|recent| recent.restored)
                {
                    break;
                }
                let written = stock
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&Restored {
                        order_id: order_id.to_owned(),
                        quantity: line.quantity,
                        restored_by: actor.to_owned(),
                    })
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
                    .map_err(CatalogError::from);
                match written {
                    Ok(_) => break,
                    Err(err) if is_conflict(&err) && attempt < WRITE_ATTEMPTS => {}
                    Err(err) => return Err(err),
                }
            }
        }
        let written = take
            .write()?
            .routing_key_opt(routing_key)
            .event(&TakeRestored {
                restored_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match written {
            // Another delivery of the cancellation recorded it meanwhile.
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

fn tracked(actor: &str, product_id: &str, variant_id: Option<&str>, on_hand: u32) -> Tracked {
    Tracked {
        product_id: product_id.to_owned(),
        variant_id: variant_id.map(str::to_owned),
        on_hand,
        tracked_by: actor.to_owned(),
    }
}

impl<E: Executor> super::Command<E> {
    /// The stock of a product without variants (`variant_id` `None`) or of
    /// one variant; `None` when it was never tracked.
    pub async fn load_stock(
        &self,
        product_id: &str,
        variant_id: Option<&str>,
    ) -> anyhow::Result<Option<StockState>> {
        load_stock(&self.executor, &self.tenant, product_id, variant_id).await
    }

    /// What the paid order `order_id` took off the shelves; `None` when it
    /// took nothing counted (or is not placed yet).
    pub async fn load_take(&self, order_id: &str) -> anyhow::Result<Option<StockTakeState>> {
        stock_take_projection()
            .load(stock_take_id(order_id))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The generation of a thing's stock the next write goes to: the current
    /// one, or the one opened after it when it is full.
    async fn current_generation(
        &self,
        actor: &str,
        product_id: &str,
        variant_id: Option<&str>,
        routing_key: &Option<String>,
    ) -> Result<Generation, CatalogError> {
        loop {
            let current =
                read_generation(&self.executor, &self.tenant, product_id, variant_id).await?;
            if !current.full() {
                return Ok(current);
            }
            self.roll(actor, product_id, variant_id, current, routing_key)
                .await?;
        }
    }

    /// Opens the generation after the full one `current`, from what it ended
    /// with, then points at it. Another writer doing the same at the same
    /// moment carries the same state, so either write losing its race is
    /// the work done.
    async fn roll(
        &self,
        actor: &str,
        product_id: &str,
        variant_id: Option<&str>,
        current: Generation,
        routing_key: &Option<String>,
    ) -> Result<(), CatalogError> {
        let Some(full) = current.state else {
            return Ok(());
        };
        let next = current.number + 1;
        let now = unix_now();
        let opened = evento::append(stock_item_id(product_id, variant_id, next))
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&Opened {
                product_id: full.product_id.clone(),
                variant_id: full.variant_id.clone(),
                tracked: full.tracked,
                on_hand: full.on_hand,
                holds: full.live_holds(now),
                recent: full
                    .recent
                    .iter()
                    .map(|take| CarriedTake {
                        order_id: take.order_id.clone(),
                        cart_id: take.cart_id.clone(),
                        restored: take.restored,
                    })
                    .collect(),
                opened_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match opened {
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => {}
            Err(err) => return Err(err.into()),
        }
        let rolled = Rolled {
            generation: next,
            rolled_by: actor.to_owned(),
        };
        let pointed = match current.pointer {
            Some(pointer) => {
                pointer
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&rolled)
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
            }
            None => {
                evento::append(stock_pointer_id(product_id, variant_id))
                    .original_version(0)
                    .routing_key_opt(routing_key.clone())
                    .event(&rolled)
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
            }
        };
        match pointed {
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// Holds one thing; `false` when there was nothing to hold (untracked,
    /// or the cart's order already took it).
    async fn reserve_line(
        &self,
        actor: &str,
        cart_id: &str,
        line: &StockLine,
        ttl_secs: u64,
        routing_key: &Option<String>,
    ) -> Result<bool, CatalogError> {
        for attempt in 1..=WRITE_ATTEMPTS {
            let now = unix_now();
            let Some(stock) = self
                .current_generation(
                    actor,
                    &line.product_id,
                    line.variant_id.as_deref(),
                    routing_key,
                )
                .await?
                .state
                .filter(|stock| stock.tracked)
            else {
                return Ok(false);
            };
            if stock.consumed_by_cart(cart_id) {
                return Ok(false);
            }
            let available = stock.available_for(Some(cart_id), now).unwrap_or(u32::MAX);
            if available < line.quantity {
                return Err(CatalogError::OutOfStock {
                    product_id: line.product_id.clone(),
                    variant_id: line.variant_id.clone(),
                    available,
                });
            }
            let written = stock
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Reserved {
                    cart_id: cart_id.to_owned(),
                    quantity: line.quantity,
                    expires_at: now + ttl_secs,
                    reserved_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await
                .map_err(CatalogError::from);
            match written {
                Ok(_) => return Ok(true),
                Err(err) if is_conflict(&err) && attempt < WRITE_ATTEMPTS => {}
                Err(err) => return Err(err),
            }
        }
        Err(CatalogError::OutOfStock {
            product_id: line.product_id.clone(),
            variant_id: line.variant_id.clone(),
            available: 0,
        })
    }

    async fn release_lines(
        &self,
        actor: &str,
        cart_id: &str,
        lines: &[StockLine],
        routing_key: &Option<String>,
    ) -> Result<(), CatalogError> {
        for line in lines {
            for attempt in 1..=WRITE_ATTEMPTS {
                let Some(stock) = self
                    .current_generation(
                        actor,
                        &line.product_id,
                        line.variant_id.as_deref(),
                        routing_key,
                    )
                    .await?
                    .state
                    .filter(|stock| stock.holds.contains_key(cart_id))
                else {
                    break;
                };
                let written = stock
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&Released {
                        cart_id: cart_id.to_owned(),
                        released_by: actor.to_owned(),
                    })
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
                    .map_err(CatalogError::from);
                match written {
                    Ok(_) => break,
                    Err(err) if is_conflict(&err) && attempt < WRITE_ATTEMPTS => {}
                    Err(err) => return Err(err),
                }
            }
        }
        Ok(())
    }
}

/// Whether a product can be bought at all, as a listing says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockSummary {
    /// Nothing of it is counted: it sells without limit.
    Untracked,
    /// Something of it is left (or not counted).
    InStock,
    /// Everything of it is counted and none is left.
    SoldOut,
}

impl StockSummary {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Untracked => "untracked",
            Self::InStock => "in_stock",
            Self::SoldOut => "sold_out",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "in_stock" => Self::InStock,
            "sold_out" => Self::SoldOut,
            _ => Self::Untracked,
        }
    }

    /// The summary of `product` from what is on the shelf of each thing it
    /// sells: `on_hand` gives the count of a tracked thing (`variant_id`
    /// `None` for a product without variants), `None` for an untracked one.
    /// Streams of variants since removed are ignored.
    pub fn of(
        product: &super::ProductState,
        on_hand: impl Fn(Option<&str>) -> Option<u32>,
    ) -> Self {
        let counts: Vec<Option<u32>> = if product.variants.is_empty() {
            vec![on_hand(None)]
        } else {
            product
                .variants
                .iter()
                .map(|variant| on_hand(Some(&variant.id)))
                .collect()
        };
        if counts.iter().all(Option::is_none) {
            Self::Untracked
        } else if counts.iter().any(|count| count.is_none_or(|left| left > 0)) {
            Self::InStock
        } else {
            Self::SoldOut
        }
    }
}

#[cfg(test)]
mod tests;
