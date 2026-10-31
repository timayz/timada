use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{BusinessBuyersAccepted, BusinessBuyersRefused};
use crate::error::OrderError;
use crate::value_object::business_buyers_id;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Whether a shopper may buy as a business (an EU VAT number, invoiced
    /// under reverse charge). A shop does not until its merchant says so.
    pub async fn accepts_business_buyers(&self) -> anyhow::Result<bool> {
        Ok(self
            .load_business_buyers()
            .await?
            .is_some_and(|state| state.accepted))
    }

    /// The merchant opens or closes the shop to business buyers. Already so
    /// is a no-op.
    pub async fn set_business_buyers(
        &self,
        actor: &str,
        accepted: bool,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let state = self.load_business_buyers().await?;
        if state.as_ref().is_some_and(|state| state.accepted) == accepted {
            return Ok(());
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(business_buyers_id());
                write.original_version(0);
                write
            }
        };
        write.routing_key_opt(routing_key);
        if accepted {
            write.event(&BusinessBuyersAccepted {
                accepted_by: actor.to_owned(),
            });
        } else {
            write.event(&BusinessBuyersRefused {
                refused_by: actor.to_owned(),
            });
        }
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(())
    }
}
