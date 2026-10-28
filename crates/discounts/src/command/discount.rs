use evento::{Executor, ProjectionAggregate, WriteError};
use timada_catalog::MAX_AMOUNT_CENTS;
use ulid::Ulid;

use crate::aggregator::{Archived, Created, Deactivated, Reactivated};
use crate::error::DiscountError;
use crate::value_object::{
    DiscountValue, MAX_PERCENT_BPS, MAX_USES, normalize_code, normalize_currency,
};

use super::DiscountState;

/// What a merchant gives to create a discount.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateDiscount {
    /// Normalized here: case does not matter.
    pub code: String,
    pub value: DiscountValue,
    /// Unix seconds; `None` is from now on.
    pub starts_at: Option<u64>,
    /// Unix seconds, excluded; `None` is no end.
    pub ends_at: Option<u64>,
    /// `None` is no limit.
    pub max_uses: Option<u32>,
    pub once_per_email: bool,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Creates an active discount: claims its code, then appends `Created`.
    /// A discount stream that cannot be created after the claim gives the
    /// code back (best effort: a failure there only leaves it reserved).
    pub async fn create_discount(
        &self,
        actor: &str,
        cmd: CreateDiscount,
        routing_key: Option<String>,
    ) -> Result<DiscountState, DiscountError> {
        self.create_discount_as(actor, &Ulid::generate().to_string(), cmd, routing_key)
            .await
    }

    /// Creates a discount under an id of the caller's (one derived from
    /// what it is for), so asking again gives the same discount: created
    /// already, it is returned as it is.
    pub async fn create_discount_with_id(
        &self,
        actor: &str,
        discount_id: &str,
        cmd: CreateDiscount,
        routing_key: Option<String>,
    ) -> Result<DiscountState, DiscountError> {
        if let Some(existing) = self.load_discount(discount_id).await? {
            return Ok(existing);
        }
        self.create_discount_as(actor, discount_id, cmd, routing_key)
            .await
    }

    /// Stops shoppers from using it; already inactive is a no-op. A checkout
    /// that already holds a use keeps it.
    pub async fn deactivate_discount(
        &self,
        actor: &str,
        discount_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), DiscountError> {
        let discount = self.require_editable(discount_id).await?;
        if discount.status == super::DiscountStatus::Inactive {
            return Ok(());
        }
        discount
            .write()?
            .routing_key_opt(routing_key)
            .event(&Deactivated {
                deactivated_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Lets shoppers use it again; already active is a no-op.
    pub async fn reactivate_discount(
        &self,
        actor: &str,
        discount_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), DiscountError> {
        let discount = self.require_editable(discount_id).await?;
        if discount.status == super::DiscountStatus::Active {
            return Ok(());
        }
        discount
            .write()?
            .routing_key_opt(routing_key)
            .event(&Reactivated {
                reactivated_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Retires the discount for good and frees its code. Already archived is
    /// a no-op.
    pub async fn archive_discount(
        &self,
        actor: &str,
        discount_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), DiscountError> {
        let discount = match self.require_editable(discount_id).await {
            Ok(discount) => discount,
            Err(DiscountError::Archived) => return Ok(()),
            Err(err) => return Err(err),
        };
        discount
            .write()?
            .routing_key_opt(routing_key.clone())
            .event(&Archived {
                archived_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        self.release_code(&discount.code, discount_id, routing_key)
            .await
    }
}

impl<E: Executor> super::Command<E> {
    /// Claims the code for `discount_id`, then appends `Created`; a
    /// discount created at the same moment under the same id is returned.
    async fn create_discount_as(
        &self,
        actor: &str,
        discount_id: &str,
        cmd: CreateDiscount,
        routing_key: Option<String>,
    ) -> Result<DiscountState, DiscountError> {
        let code = normalize_code(&cmd.code)?;
        let (percent_bps, amount_cents, currency) = match cmd.value {
            DiscountValue::Percent(bps) if (1..=MAX_PERCENT_BPS).contains(&bps) => {
                (Some(bps), None, None)
            }
            DiscountValue::Percent(_) => return Err(DiscountError::InvalidPercent),
            DiscountValue::Amount { cents, currency } => {
                if cents == 0 || cents > MAX_AMOUNT_CENTS {
                    return Err(DiscountError::InvalidAmount);
                }
                (None, Some(cents), Some(normalize_currency(&currency)?))
            }
        };
        if let (Some(start), Some(end)) = (cmd.starts_at, cmd.ends_at)
            && end <= start
        {
            return Err(DiscountError::InvalidWindow);
        }
        if cmd.max_uses.is_some_and(|max| max == 0 || max > MAX_USES) {
            return Err(DiscountError::InvalidMaxUses);
        }

        self.claim_code(&code, discount_id, routing_key.clone())
            .await?;
        let created = evento::append(discount_id)
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&Created {
                code: code.clone(),
                percent_bps,
                amount_cents,
                currency,
                starts_at: cmd.starts_at,
                ends_at: cmd.ends_at,
                max_uses: cmd.max_uses,
                once_per_email: cmd.once_per_email,
                created_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        if let Err(WriteError::InvalidOriginalVersion) = created
            && let Some(existing) = self.load_discount(discount_id).await?
        {
            return Ok(existing);
        }
        if let Err(err) = created {
            if let Err(release) = self.release_code(&code, discount_id, routing_key).await {
                tracing::warn!(%code, %release, "cannot give the code back after a failed creation");
            }
            return Err(err.into());
        }

        self.load_discount(discount_id)
            .await?
            .ok_or(DiscountError::NotFound)
    }
}
