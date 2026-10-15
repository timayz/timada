use evento::{Executor, WriteError};

use crate::aggregator::ConfirmationMailed;
use crate::error::OrdersError;
use crate::value_object::order_confirmation_id;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Takes the sending of an order's confirmation, before it goes out.
    /// Returns whether this caller took it: `false` when it was mailed
    /// already, by another node say.
    pub async fn claim_confirmation(
        &self,
        order_id: &str,
        email: &str,
        actor: &str,
        routing_key: Option<String>,
    ) -> Result<bool, OrdersError> {
        let claimed = evento::append(order_confirmation_id(order_id))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&ConfirmationMailed {
                order_id: order_id.to_owned(),
                email: email.to_owned(),
                mailed_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await;
        match claimed {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }
}
