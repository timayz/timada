use evento::Executor;
use timada_core::Money;

use crate::{
    error::PromotionError,
    value_object::{CodeKind, CodeRedemption, normalize_code},
};

impl<E: Executor> super::Command<'_, E> {
    /// Redeems whatever the "code promo ou bon d'achat" box holds: a promo
    /// code takes its rule off `subtotal`, a voucher spends its balance; both
    /// stop at `max`. Idempotent per order, like the commands underneath.
    pub async fn redeem_code(
        &self,
        code: &str,
        order_id: &str,
        subtotal: &Money,
        max: &Money,
    ) -> Result<CodeRedemption, PromotionError> {
        let (kind, amount) = match self.redeem_discount(code, order_id, subtotal, max).await {
            Ok(amount) => (CodeKind::Discount, amount),
            Err(PromotionError::UnknownCode) => (
                CodeKind::Voucher,
                self.spend_voucher(code, order_id, max).await?,
            ),
            Err(err) => return Err(err),
        };
        Ok(CodeRedemption {
            code: normalize_code(code),
            kind,
            amount,
        })
    }

    /// Undoes [`redeem_code`](Self::redeem_code) for an order that fell
    /// through. A code that was never redeemed for it is a no-op.
    pub async fn release_code(&self, code: &str, order_id: &str) -> Result<(), PromotionError> {
        match self.release_discount(code, order_id).await {
            Ok(_) => Ok(()),
            Err(PromotionError::UnknownCode) => {
                self.refund_voucher(code, order_id).await?;
                Ok(())
            }
            Err(err) => Err(err),
        }
    }
}
