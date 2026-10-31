use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{PaymentsDisabled, PaymentsEnabled};
use crate::error::OrderError;
use crate::value_object::payment_method_id;

/// What a shopper's browser needs to show the payment form before a payment
/// is open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaymentSetup {
    /// `stripe`.
    pub provider: String,
    pub publishable_key: String,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// How the tenant's shoppers pay, `None` while it takes no payments.
    pub async fn payment_setup(&self) -> anyhow::Result<Option<PaymentSetup>> {
        Ok(self
            .load_payment_method()
            .await?
            .and_then(|state| state.setup))
    }

    /// Brings the payment method in step with the merchant's account:
    /// `None` stops it. Already so is a no-op, so every node may tell it.
    pub async fn set_payment_method(
        &self,
        actor: &str,
        setup: Option<PaymentSetup>,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let state = self.load_payment_method().await?;
        let current = state.as_ref().and_then(|state| state.setup.clone());
        if current == setup {
            return Ok(());
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(payment_method_id());
                write.original_version(0);
                write
            }
        };
        write.routing_key_opt(routing_key);
        match setup {
            Some(setup) => write.event(&PaymentsEnabled {
                provider: setup.provider,
                publishable_key: setup.publishable_key,
                enabled_by: actor.to_owned(),
            }),
            None => write.event(&PaymentsDisabled {
                disabled_by: actor.to_owned(),
            }),
        };
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(())
    }
}
