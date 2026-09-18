use evento::Executor;
use timada_core::Money;

use crate::{
    aggregator::VoucherIssued,
    error::PromotionError,
    value_object::{VoucherKind, normalize_code},
};

use super::voucher_id;

#[derive(Debug, Clone)]
pub struct IssueVoucher {
    pub code: String,
    pub customer_id: Option<String>,
    pub value: Money,
    pub kind: VoucherKind,
    pub expires_at: Option<u64>,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Issues a gift voucher or credit note. The id is derived from the code.
    pub async fn issue_voucher(
        &self,
        cmd: IssueVoucher,
        routing_key: Option<String>,
    ) -> Result<String, PromotionError> {
        let code = normalize_code(&cmd.code);
        if code.is_empty() {
            return Err(PromotionError::Required("code"));
        }
        if !cmd.value.is_positive() {
            return Err(PromotionError::InvalidAmount);
        }

        let id = voucher_id(&code);
        let result = evento::append(&id)
            .routing_key_opt(routing_key)
            .event(&VoucherIssued {
                code: code.clone(),
                customer_id: cmd.customer_id,
                value: cmd.value,
                kind: cmd.kind,
                expires_at: cmd.expires_at,
            })
            .commit(self.executor)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(voucher_id = %id, %code, "voucher issued");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => {
                Err(PromotionError::CodeAlreadyExists(code))
            }
            Err(err) => Err(err.into()),
        }
    }
}
