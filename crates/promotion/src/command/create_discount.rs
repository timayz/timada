use evento::Executor;

use crate::{
    aggregator::DiscountCreated,
    error::PromotionError,
    value_object::{DiscountKind, normalize_code},
};

use super::discount_id;

#[derive(Debug, Clone)]
pub struct CreateDiscount {
    pub code: String,
    pub kind: DiscountKind,
    pub max_redemptions: Option<u32>,
    pub valid_until: Option<u64>,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Creates a promo code. The id is derived from the code, so a duplicate
    /// is rejected atomically by the store.
    pub async fn create_discount(
        &self,
        cmd: CreateDiscount,
        routing_key: Option<String>,
    ) -> Result<String, PromotionError> {
        let code = normalize_code(&cmd.code);
        if code.is_empty() {
            return Err(PromotionError::Required("code"));
        }
        match &cmd.kind {
            DiscountKind::Percent { bp } if !(1..=10_000).contains(bp) => {
                return Err(PromotionError::InvalidKind);
            }
            DiscountKind::FixedAmount { amount } if !amount.is_positive() => {
                return Err(PromotionError::InvalidKind);
            }
            _ => {}
        }

        let id = discount_id(&code);
        let result = evento::append(&id)
            .routing_key_opt(routing_key)
            .event(&DiscountCreated {
                code: code.clone(),
                kind: cmd.kind,
                max_redemptions: cmd.max_redemptions,
                valid_until: cmd.valid_until,
            })
            .commit(self.executor)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(discount_id = %id, %code, "discount created");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => {
                Err(PromotionError::CodeAlreadyExists(code))
            }
            Err(err) => Err(err.into()),
        }
    }
}
