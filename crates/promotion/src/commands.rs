//! Create and disable discounts — admin-only commands.

use evento::{AggregateExt as _, ProjectionAggregate as _};
use timada_core::Executor;

use crate::aggregate::{DiscountCreated, DiscountDisabled, DiscountKind};
use crate::view::load_discount;

/// The aggregate id for a code: codes are case-insensitive natural keys.
pub fn discount_id(code: &str) -> String {
    evento::hash_ids(vec![&code.trim().to_uppercase(), "discount"])
}

/// Why a discount could not be created.
#[derive(Debug, thiserror::Error)]
pub enum CreateDiscountError {
    #[error("{0}")]
    Invalid(String),
    #[error("a discount with this code already exists")]
    CodeTaken,
    #[error(transparent)]
    Storage(#[from] anyhow::Error),
}

/// Create a discount and return its aggregate id. The code is normalized to
/// uppercase — customers type it however they like.
#[tracing::instrument(skip(executor))]
pub async fn create_discount(
    executor: &Executor,
    code: &str,
    kind: DiscountKind,
    starts_at: i64,
    ends_at: Option<i64>,
    usage_limit: Option<u32>,
) -> Result<String, CreateDiscountError> {
    let code = code.trim().to_uppercase();
    if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(CreateDiscountError::Invalid(
            "a code needs letters, digits or dashes".into(),
        ));
    }
    match kind {
        DiscountKind::Percentage { bps } if bps == 0 || bps > 10_000 => {
            return Err(CreateDiscountError::Invalid(
                "a percentage must be between 1 and 10000 basis points".into(),
            ));
        }
        DiscountKind::Fixed { amount } if amount.amount_cents <= 0 => {
            return Err(CreateDiscountError::Invalid(
                "a fixed discount must be a positive amount".into(),
            ));
        }
        _ => {}
    }
    if let Some(ends_at) = ends_at
        && ends_at <= starts_at
    {
        return Err(CreateDiscountError::Invalid(
            "the end of the validity window is before its start".into(),
        ));
    }
    if usage_limit == Some(0) {
        return Err(CreateDiscountError::Invalid(
            "a usage limit of zero would never redeem".into(),
        ));
    }

    let id = discount_id(&code);
    if executor.has_event::<DiscountCreated>(&id).await? {
        return Err(CreateDiscountError::CodeTaken);
    }

    evento::append(&id)
        .original_version(0)
        .event(&DiscountCreated {
            code: code.clone(),
            kind,
            starts_at,
            ends_at,
            usage_limit,
        })
        .commit(executor)
        .await
        .map_err(anyhow::Error::from)?;

    tracing::info!(%code, "discount created");
    Ok(id)
}

/// Switch a code off. Unknown or already-disabled codes are a no-op — a
/// double-clicked Disable button should not produce an error page.
#[tracing::instrument(skip(executor))]
pub async fn disable_discount(executor: &Executor, discount_id: &str) -> anyhow::Result<()> {
    let Some(discount) = load_discount(executor, discount_id).await? else {
        tracing::warn!(discount_id, "cannot disable an unknown discount");
        return Ok(());
    };
    if discount.disabled {
        tracing::info!(discount_id, "discount already disabled");
        return Ok(());
    }

    discount
        .write()?
        .event(&DiscountDisabled)
        .commit(executor)
        .await?;

    tracing::info!(discount_id, "discount disabled");
    Ok(())
}
