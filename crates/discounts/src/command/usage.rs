//! Reserving, releasing and spending the uses of a discount.
//!
//! Each stream is written with the version it was read at, so two checkouts
//! after the last use of a code cannot both take it: the second write fails,
//! is read again, and finds the code exhausted.
//!
//! # Generations
//!
//! Every checkout writes an event or two to the usage stream of the code it
//! carries, and a stream has 65 535 versions: a popular code would stop
//! working after some thirty thousand checkouts. So the uses are written in
//! generations of at most [`USAGE_CHUNK`] events. `DiscountUsagePointer`
//! names the current one; the writer that finds it full opens the next
//! generation with [`Opened`], carrying the live reservations, the count of
//! spent uses and the recent orders, then rolls the pointer, and tries its
//! write again. Nothing is ever appended to a full generation (every writer
//! checks the version it read against the cap before writing with it), so
//! every writer that opens the next one carries the same state, and the
//! first to write it wins.

use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::buyer::{
    Consumed as BuyerConsumed, Released as BuyerReleased, Reserved as BuyerReserved,
};
use crate::aggregator::usage::{Consumed, Opened, Released, Reserved, Rolled};
use crate::error::DiscountError;
use crate::value_object::{
    CarriedUse, USAGE_CHUNK, discount_buyer_id, discount_usage_id, discount_usage_pointer_id,
    unix_now,
};

use super::{
    BuyerState, DiscountState, UsagePointerState, UsageState, usage_pointer_projection,
    usage_projection,
};

/// How many times a write is tried again when another checkout wrote the
/// same stream at the same moment.
const WRITE_ATTEMPTS: usize = 5;

/// How many events a generation holds: [`USAGE_CHUNK`], or three in the
/// unit tests so a roll is cheap to reach.
const fn chunk_size() -> u16 {
    if cfg!(test) { 3 } else { USAGE_CHUNK }
}

/// Whether a write lost the race for the stream's version.
fn is_conflict(err: &DiscountError) -> bool {
    matches!(
        err,
        DiscountError::Write(WriteError::InvalidOriginalVersion)
    )
}

/// The uses of a capped discount, as its current generation says them;
/// `None` before the first one. `tenant` is the one the streams are in.
pub async fn load_usage<E: Executor>(
    executor: &E,
    tenant: &str,
    discount_id: &str,
) -> anyhow::Result<Option<UsageState>> {
    Ok(read_generation(executor, tenant, discount_id).await?.state)
}

/// A code's current usage generation, as read for one attempt.
struct Generation {
    /// The pointer, `None` while the first generation is current.
    pointer: Option<UsagePointerState>,
    /// The generation the pointer names (0 without one).
    number: u32,
    /// The stream of that generation.
    id: String,
    /// Its state, `None` when nothing was ever written to it.
    state: Option<UsageState>,
}

impl Generation {
    /// Whether nothing more may be written to it: the next write opens the
    /// generation after it.
    fn full(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|usage| usage.aggregate_version >= chunk_size())
    }
}

async fn read_generation<E: Executor>(
    executor: &E,
    tenant: &str,
    discount_id: &str,
) -> anyhow::Result<Generation> {
    let pointer = usage_pointer_projection()
        .load(discount_usage_pointer_id(discount_id))
        .tenant(tenant)
        .execute(executor)
        .await?;
    let number = pointer.as_ref().map_or(0, |pointer| pointer.generation);
    let id = discount_usage_id(discount_id, number);
    let state = usage_projection()
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
    /// Holds a use of the discount for the checkout of `cart_id` by `email`
    /// (normalized) for `ttl_secs`: `Exhausted` when a capped code has no use
    /// left, `AlreadyUsed` when the e-mail used a code good once per e-mail
    /// (or another of its checkouts holds it). Reserving again for the same
    /// cart moves the expiry; a discount with neither limit holds nothing.
    pub async fn reserve_use(
        &self,
        actor: &str,
        discount_id: &str,
        cart_id: &str,
        email: &str,
        ttl_secs: u64,
        routing_key: Option<String>,
    ) -> Result<(), DiscountError> {
        let discount = self
            .load_discount(discount_id)
            .await?
            .ok_or(DiscountError::NotFound)?;
        if discount.once_per_email {
            self.reserve_buyer(actor, &discount, cart_id, email, ttl_secs, &routing_key)
                .await?;
        }
        if let Some(max_uses) = discount.max_uses {
            let reserved = self
                .reserve_usage(actor, &discount, max_uses, cart_id, ttl_secs, &routing_key)
                .await;
            if let Err(err) = reserved {
                if discount.once_per_email
                    && let Err(release) = self
                        .release_buyer(actor, &discount, cart_id, email, &routing_key)
                        .await
                {
                    tracing::warn!(%discount_id, %cart_id, %release, "cannot give a buyer's use back");
                }
                return Err(err);
            }
        }
        Ok(())
    }

    /// Gives back the use `cart_id` holds, if it holds one that no order has
    /// spent: the payment it was held for did not go through.
    pub async fn release_use(
        &self,
        actor: &str,
        discount_id: &str,
        cart_id: &str,
        email: &str,
        routing_key: Option<String>,
    ) -> Result<(), DiscountError> {
        let Some(discount) = self.load_discount(discount_id).await? else {
            return Ok(());
        };
        if discount.max_uses.is_some() {
            for attempt in 1..=WRITE_ATTEMPTS {
                let Some(usage) = self
                    .current_generation(actor, discount_id, &routing_key)
                    .await?
                    .state
                else {
                    break;
                };
                if !usage.reserved.contains_key(cart_id) {
                    break;
                }
                let written = usage
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&Released {
                        cart_id: cart_id.to_owned(),
                        released_by: actor.to_owned(),
                    })
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
                    .map_err(DiscountError::from);
                match written {
                    Ok(_) => break,
                    Err(err) if is_conflict(&err) && attempt < WRITE_ATTEMPTS => {}
                    Err(err) => return Err(err),
                }
            }
        }
        if discount.once_per_email {
            self.release_buyer(actor, &discount, cart_id, email, &routing_key)
                .await?;
        }
        Ok(())
    }

    /// Spends the use of the checkout of `cart_id` for `order_id`. Told
    /// twice, nothing more happens. An order whose reservation lapsed before
    /// the shopper paid is still recorded: they paid the price they were
    /// shown, so the code may serve one order more than its cap says.
    pub async fn consume_use(
        &self,
        actor: &str,
        discount_id: &str,
        cart_id: &str,
        email: &str,
        order_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), DiscountError> {
        let discount = self
            .load_discount(discount_id)
            .await?
            .ok_or(DiscountError::NotFound)?;
        if discount.max_uses.is_some() {
            for attempt in 1..=WRITE_ATTEMPTS {
                let current = self
                    .current_generation(actor, discount_id, &routing_key)
                    .await?;
                if current
                    .state
                    .as_ref()
                    .is_some_and(|usage| usage.consumed_by_cart(cart_id).is_some())
                {
                    break;
                }
                if !current
                    .state
                    .as_ref()
                    .is_some_and(|usage| usage.holds(cart_id, unix_now()))
                {
                    tracing::warn!(%discount_id, %cart_id, %order_id, "an order spends a use its checkout no longer held");
                }
                let consumed = Consumed {
                    cart_id: cart_id.to_owned(),
                    order_id: order_id.to_owned(),
                    consumed_by: actor.to_owned(),
                };
                let written = match current.state {
                    Some(usage) => {
                        usage
                            .write()?
                            .routing_key_opt(routing_key.clone())
                            .event(&consumed)
                            .tenant(&self.tenant)
                            .commit(&self.executor)
                            .await
                    }
                    None => {
                        evento::append(&current.id)
                            .original_version(0)
                            .routing_key_opt(routing_key.clone())
                            .event(&consumed)
                            .tenant(&self.tenant)
                            .commit(&self.executor)
                            .await
                    }
                }
                .map_err(DiscountError::from);
                match written {
                    Ok(_) => break,
                    Err(err) if is_conflict(&err) && attempt < WRITE_ATTEMPTS => {}
                    Err(err) => return Err(err),
                }
            }
        }
        if discount.once_per_email {
            for attempt in 1..=WRITE_ATTEMPTS {
                let buyer = self.load_buyer(discount_id, email).await?;
                if let Some(spent) = buyer.as_ref().and_then(|buyer| buyer.order_id.as_deref()) {
                    if spent != order_id {
                        tracing::warn!(%discount_id, %order_id, %spent, "an e-mail used a code good once a second time");
                    }
                    break;
                }
                let consumed = BuyerConsumed {
                    cart_id: cart_id.to_owned(),
                    order_id: order_id.to_owned(),
                    consumed_by: actor.to_owned(),
                };
                let written = match buyer {
                    Some(buyer) => {
                        buyer
                            .write()?
                            .routing_key_opt(routing_key.clone())
                            .event(&consumed)
                            .tenant(&self.tenant)
                            .commit(&self.executor)
                            .await
                    }
                    None => {
                        evento::append(discount_buyer_id(discount_id, email))
                            .original_version(0)
                            .routing_key_opt(routing_key.clone())
                            .event(&consumed)
                            .tenant(&self.tenant)
                            .commit(&self.executor)
                            .await
                    }
                }
                .map_err(DiscountError::from);
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

impl<E: Executor> super::Command<E> {
    /// The generation of a code's uses the next write goes to: the current
    /// one, or the one opened after it when it is full.
    async fn current_generation(
        &self,
        actor: &str,
        discount_id: &str,
        routing_key: &Option<String>,
    ) -> Result<Generation, DiscountError> {
        loop {
            let current = read_generation(&self.executor, &self.tenant, discount_id).await?;
            if !current.full() {
                return Ok(current);
            }
            self.roll(actor, discount_id, current, routing_key).await?;
        }
    }

    /// Opens the generation after the full one `current`, from what it ended
    /// with, then points at it. Another writer doing the same at the same
    /// moment carries the same state, so either write losing its race is
    /// the work done.
    async fn roll(
        &self,
        actor: &str,
        discount_id: &str,
        current: Generation,
        routing_key: &Option<String>,
    ) -> Result<(), DiscountError> {
        let Some(full) = current.state else {
            return Ok(());
        };
        let next = current.number + 1;
        let opened = evento::append(discount_usage_id(discount_id, next))
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&Opened {
                reserved: full.live_reservations(unix_now()),
                consumed_count: full.consumed_count,
                recent: full
                    .recent
                    .iter()
                    .map(|spent| CarriedUse {
                        cart_id: spent.cart_id.clone(),
                        order_id: spent.order_id.clone(),
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
                evento::append(discount_usage_pointer_id(discount_id))
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

    async fn reserve_buyer(
        &self,
        actor: &str,
        discount: &DiscountState,
        cart_id: &str,
        email: &str,
        ttl_secs: u64,
        routing_key: &Option<String>,
    ) -> Result<(), DiscountError> {
        for attempt in 1..=WRITE_ATTEMPTS {
            let now = unix_now();
            let buyer: Option<BuyerState> = self.load_buyer(&discount.id, email).await?;
            if let Some(buyer) = &buyer {
                if buyer.order_id.is_some() {
                    return if buyer.cart_id.as_deref() == Some(cart_id) {
                        Ok(())
                    } else {
                        Err(DiscountError::AlreadyUsed)
                    };
                }
                if buyer.holder_at(now).is_some_and(|holder| holder != cart_id) {
                    return Err(DiscountError::AlreadyUsed);
                }
            }
            let reserved = BuyerReserved {
                cart_id: cart_id.to_owned(),
                expires_at: now + ttl_secs,
                reserved_by: actor.to_owned(),
            };
            let written = match buyer {
                Some(buyer) => {
                    buyer
                        .write()?
                        .routing_key_opt(routing_key.clone())
                        .event(&reserved)
                        .tenant(&self.tenant)
                        .commit(&self.executor)
                        .await
                }
                None => {
                    evento::append(discount_buyer_id(&discount.id, email))
                        .original_version(0)
                        .routing_key_opt(routing_key.clone())
                        .event(&reserved)
                        .tenant(&self.tenant)
                        .commit(&self.executor)
                        .await
                }
            }
            .map_err(DiscountError::from);
            match written {
                Ok(_) => return Ok(()),
                Err(err) if is_conflict(&err) && attempt < WRITE_ATTEMPTS => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    async fn release_buyer(
        &self,
        actor: &str,
        discount: &DiscountState,
        cart_id: &str,
        email: &str,
        routing_key: &Option<String>,
    ) -> Result<(), DiscountError> {
        for attempt in 1..=WRITE_ATTEMPTS {
            let Some(buyer) = self.load_buyer(&discount.id, email).await? else {
                return Ok(());
            };
            if buyer.order_id.is_some() || buyer.cart_id.as_deref() != Some(cart_id) {
                return Ok(());
            }
            let written = buyer
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&BuyerReleased {
                    cart_id: cart_id.to_owned(),
                    released_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await
                .map_err(DiscountError::from);
            match written {
                Ok(_) => return Ok(()),
                Err(err) if is_conflict(&err) && attempt < WRITE_ATTEMPTS => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    async fn reserve_usage(
        &self,
        actor: &str,
        discount: &DiscountState,
        max_uses: u32,
        cart_id: &str,
        ttl_secs: u64,
        routing_key: &Option<String>,
    ) -> Result<(), DiscountError> {
        for attempt in 1..=WRITE_ATTEMPTS {
            let now = unix_now();
            let current = self
                .current_generation(actor, &discount.id, routing_key)
                .await?;
            if let Some(usage) = &current.state {
                if usage.consumed_by_cart(cart_id).is_some() {
                    return Ok(());
                }
                let mine = usize::from(usage.holds(cart_id, now));
                if usage.taken_at(now) - mine >= max_uses as usize {
                    return Err(DiscountError::Exhausted);
                }
            }
            let reserved = Reserved {
                cart_id: cart_id.to_owned(),
                expires_at: now + ttl_secs,
                reserved_by: actor.to_owned(),
            };
            let written = match current.state {
                Some(usage) => {
                    usage
                        .write()?
                        .routing_key_opt(routing_key.clone())
                        .event(&reserved)
                        .tenant(&self.tenant)
                        .commit(&self.executor)
                        .await
                }
                None => {
                    evento::append(&current.id)
                        .original_version(0)
                        .routing_key_opt(routing_key.clone())
                        .event(&reserved)
                        .tenant(&self.tenant)
                        .commit(&self.executor)
                        .await
                }
            }
            .map_err(DiscountError::from);
            match written {
                Ok(_) => return Ok(()),
                Err(err) if is_conflict(&err) && attempt < WRITE_ATTEMPTS => {}
                Err(err) => return Err(err),
            }
        }
        Err(DiscountError::Exhausted)
    }
}

#[cfg(test)]
mod tests;
