//! Reserving, releasing and spending the uses of a discount.
//!
//! Each stream is written with the version it was read at, so two checkouts
//! after the last use of a code cannot both take it: the second write fails,
//! is read again, and finds the code exhausted.

use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::buyer::{
    Consumed as BuyerConsumed, Released as BuyerReleased, Reserved as BuyerReserved,
};
use crate::aggregator::usage::{Consumed, Released, Reserved};
use crate::error::DiscountError;
use crate::value_object::{discount_buyer_id, discount_usage_id, unix_now};

use super::{BuyerState, DiscountState, UsageState};

/// How many times a write is tried again when another checkout wrote the
/// same stream at the same moment.
const WRITE_ATTEMPTS: usize = 5;

/// Whether a write lost the race for the stream's version.
fn is_conflict(err: &DiscountError) -> bool {
    matches!(
        err,
        DiscountError::Write(WriteError::InvalidOriginalVersion)
    )
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
                .reserve_usage(
                    actor,
                    &discount,
                    max_uses,
                    cart_id,
                    email,
                    ttl_secs,
                    &routing_key,
                )
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
                let Some(usage) = self.load_usage(discount_id).await? else {
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
                    .commit(&self.0)
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
                let usage = self.load_usage(discount_id).await?;
                if usage
                    .as_ref()
                    .is_some_and(|usage| usage.consumed.contains_key(cart_id))
                {
                    break;
                }
                if !usage
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
                let written = match usage {
                    Some(usage) => {
                        usage
                            .write()?
                            .routing_key_opt(routing_key.clone())
                            .event(&consumed)
                            .commit(&self.0)
                            .await
                    }
                    None => {
                        evento::append(discount_usage_id(discount_id))
                            .original_version(0)
                            .routing_key_opt(routing_key.clone())
                            .event(&consumed)
                            .commit(&self.0)
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
                            .commit(&self.0)
                            .await
                    }
                    None => {
                        evento::append(discount_buyer_id(discount_id, email))
                            .original_version(0)
                            .routing_key_opt(routing_key.clone())
                            .event(&consumed)
                            .commit(&self.0)
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
                        .commit(&self.0)
                        .await
                }
                None => {
                    evento::append(discount_buyer_id(&discount.id, email))
                        .original_version(0)
                        .routing_key_opt(routing_key.clone())
                        .event(&reserved)
                        .commit(&self.0)
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
                .commit(&self.0)
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

    #[allow(clippy::too_many_arguments)]
    async fn reserve_usage(
        &self,
        actor: &str,
        discount: &DiscountState,
        max_uses: u32,
        cart_id: &str,
        email: &str,
        ttl_secs: u64,
        routing_key: &Option<String>,
    ) -> Result<(), DiscountError> {
        for attempt in 1..=WRITE_ATTEMPTS {
            let now = unix_now();
            let usage: Option<UsageState> = self.load_usage(&discount.id).await?;
            if let Some(usage) = &usage {
                if usage.consumed.contains_key(cart_id) {
                    return Ok(());
                }
                let mine = usize::from(usage.holds(cart_id, now));
                if usage.taken_at(now) - mine >= max_uses as usize {
                    return Err(DiscountError::Exhausted);
                }
            }
            let reserved = Reserved {
                cart_id: cart_id.to_owned(),
                email: email.to_owned(),
                expires_at: now + ttl_secs,
                reserved_by: actor.to_owned(),
            };
            let written = match usage {
                Some(usage) => {
                    usage
                        .write()?
                        .routing_key_opt(routing_key.clone())
                        .event(&reserved)
                        .commit(&self.0)
                        .await
                }
                None => {
                    evento::append(discount_usage_id(&discount.id))
                        .original_version(0)
                        .routing_key_opt(routing_key.clone())
                        .event(&reserved)
                        .commit(&self.0)
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
