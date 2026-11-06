use evento::{Executor, WriteError};

use crate::aggregator::Mailed;
use crate::error::OrderError;
use crate::value_object::order_mail_id;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Takes the sending of the mail `mail` about an order, before it goes
    /// out. Returns whether this caller took it: `false` when it was mailed
    /// already, by another node say.
    pub async fn claim_order_mail(
        &self,
        order_id: &str,
        mail: &str,
        email: &str,
        actor: &str,
        routing_key: Option<String>,
    ) -> Result<bool, OrderError> {
        let claimed = evento::append(order_mail_id(order_id, mail))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Mailed {
                order_id: order_id.to_owned(),
                mail: mail.to_owned(),
                email: email.to_owned(),
                mailed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match claimed {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }
}
