//! Stock: how many of a thing the shop holds, the checkouts holding some of
//! it and the orders that took some.
//!
//! A checkout reserves what it sells before the shopper is asked to pay (an
//! optimistic write on the item's own stream, so two checkouts never take
//! the last one twice); the paid order consumes the hold, a refused or
//! abandoned checkout releases it (or lets it lapse: its expiry is read,
//! never scheduled) and a cancelled order puts its quantity back.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::stock::{
    Consumed, OnHandSet, Released, Reserved, Restored, Tracked, Untracked,
};
use crate::error::CatalogError;
use crate::value_object::{MAX_STOCK, stock_item_id};

/// How often a write that lost a race with another one is tried again.
const WRITE_ATTEMPTS: usize = 5;

fn is_conflict(err: &CatalogError) -> bool {
    matches!(err, CatalogError::Write(WriteError::InvalidOriginalVersion))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Some of a thing a checkout or an order names: a product without variants
/// (`variant_id` `None`) or one variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockLine {
    pub product_id: String,
    pub variant_id: Option<String>,
    pub quantity: u32,
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

/// What an order took.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StockConsumption {
    pub cart_id: String,
    pub quantity: u32,
}

/// The stock of one thing.
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
    /// Order id → what it took.
    pub consumed: BTreeMap<String, StockConsumption>,
    /// The orders whose quantity came back.
    pub restored: BTreeSet<String>,
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

    /// Whether the order made from `cart_id` already took its share.
    pub fn consumed_by_cart(&self, cart_id: &str) -> bool {
        self.consumed
            .values()
            .any(|consumption| consumption.cart_id == cart_id)
    }
}

pub fn stock_projection<E: Executor>() -> Projection<E, StockState> {
    Projection::new::<crate::aggregator::stock::StockItem>()
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
    stock.consumed.insert(
        event.data.order_id.to_owned(),
        StockConsumption {
            cart_id: event.data.cart_id.to_owned(),
            quantity: event.data.quantity,
        },
    );
    stock.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_restored(event: Event<Restored>, stock: &mut StockState) -> anyhow::Result<()> {
    stock.on_hand = stock.on_hand.saturating_add(event.data.quantity);
    stock.restored.insert(event.data.order_id.to_owned());
    stock.updated_at = event.timestamp;
    Ok(())
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
            let stock = self.load_stock(product_id, variant_id).await?;
            let written = match stock {
                Some(stock) if stock.tracked && stock.on_hand == on_hand => return Ok(()),
                Some(stock) if stock.tracked => stock
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&OnHandSet {
                        on_hand,
                        set_by: actor.to_owned(),
                    })
                    .commit(&self.0)
                    .await
                    .map_err(CatalogError::from),
                Some(stock) => stock
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&tracked(actor, product_id, variant_id, on_hand))
                    .commit(&self.0)
                    .await
                    .map_err(CatalogError::from),
                None => evento::append(stock_item_id(product_id, variant_id))
                    .original_version(0)
                    .routing_key_opt(routing_key.clone())
                    .event(&tracked(actor, product_id, variant_id, on_hand))
                    .commit(&self.0)
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
                .load_stock(product_id, variant_id)
                .await?
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
                .commit(&self.0)
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
    /// the shelf. Delivery may repeat: an order takes its share once.
    pub async fn consume_stock(
        &self,
        actor: &str,
        cart_id: &str,
        order_id: &str,
        lines: &[StockLine],
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        for line in merge(lines) {
            for attempt in 1..=WRITE_ATTEMPTS {
                let Some(stock) = self
                    .load_stock(&line.product_id, line.variant_id.as_deref())
                    .await?
                    .filter(|stock| stock.tracked)
                else {
                    break;
                };
                if stock.consumed.contains_key(order_id) {
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
                    .commit(&self.0)
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

    /// Puts back what the cancelled order `order_id` took. Delivery may
    /// repeat: an order's quantity comes back once.
    pub async fn restore_stock(
        &self,
        actor: &str,
        order_id: &str,
        lines: &[StockLine],
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        for line in merge(lines) {
            for attempt in 1..=WRITE_ATTEMPTS {
                let Some(stock) = self
                    .load_stock(&line.product_id, line.variant_id.as_deref())
                    .await?
                    .filter(|stock| stock.tracked)
                else {
                    break;
                };
                if stock.restored.contains(order_id) {
                    break;
                }
                let Some(quantity) = stock.consumed.get(order_id).map(|taken| taken.quantity)
                else {
                    break;
                };
                let written = stock
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&Restored {
                        order_id: order_id.to_owned(),
                        quantity,
                        restored_by: actor.to_owned(),
                    })
                    .commit(&self.0)
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
        stock_projection()
            .load(stock_item_id(product_id, variant_id))
            .execute(&self.0)
            .await
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
                .load_stock(&line.product_id, line.variant_id.as_deref())
                .await?
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
                .commit(&self.0)
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
                    .load_stock(&line.product_id, line.variant_id.as_deref())
                    .await?
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
                    .commit(&self.0)
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
