use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};

use crate::aggregator::{ReturnPolicyCleared, ReturnPolicySet};
use crate::error::OrderError;
use crate::value_object::{MAX_RETURN_DAYS, return_policy_id};

/// What the shop says of returns, as data: for how long a buyer may send
/// goods back, and who pays for sending them. The page a shop writes about
/// its returns says the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReturnTerms {
    /// Days from the delivery, 1 to [`MAX_RETURN_DAYS`].
    pub days: u32,
    /// The shop pays for the way back; else the buyer does.
    pub free: bool,
}

/// The write model of the shop's return policy, and what a storefront
/// copies of it.
#[evento::projection(id = id, name = "timada-orders/ReturnPolicyState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ReturnPolicyState {
    pub id: String,
    /// `None`: the shop states none as data.
    pub terms: Option<ReturnTerms>,
}

pub fn return_policy_projection<E: Executor>() -> Projection<E, ReturnPolicyState> {
    Projection::new::<crate::aggregator::ReturnPolicy>()
        .handler(on_return_policy_set())
        .handler(on_return_policy_cleared())
        .strict()
}

#[evento::handler]
async fn on_return_policy_set(
    event: Event<ReturnPolicySet>,
    state: &mut ReturnPolicyState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.terms = Some(ReturnTerms {
        days: event.data.days,
        free: event.data.free,
    });
    Ok(())
}

#[evento::handler]
async fn on_return_policy_cleared(
    event: Event<ReturnPolicyCleared>,
    state: &mut ReturnPolicyState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.terms = None;
    Ok(())
}

impl<E: Executor> super::Command<E> {
    pub async fn load_return_policy(&self) -> anyhow::Result<Option<ReturnPolicyState>> {
        return_policy_projection()
            .load(return_policy_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The shop's return terms; `None` while it states none as data.
    pub async fn return_policy(&self) -> anyhow::Result<Option<ReturnTerms>> {
        Ok(self
            .load_return_policy()
            .await?
            .and_then(|state| state.terms))
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// The merchant states the shop's return terms, or with `None` takes
    /// them back. Already so is a no-op.
    pub async fn set_return_policy(
        &self,
        actor: &str,
        terms: Option<ReturnTerms>,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        if terms.is_some_and(|terms| !(1..=MAX_RETURN_DAYS).contains(&terms.days)) {
            return Err(OrderError::InvalidReturnDays);
        }
        let state = self.load_return_policy().await?;
        if state.as_ref().and_then(|state| state.terms) == terms {
            return Ok(());
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(return_policy_id());
                write.original_version(0);
                write
            }
        };
        write.routing_key_opt(routing_key);
        match terms {
            Some(terms) => write.event(&ReturnPolicySet {
                days: terms.days,
                free: terms.free,
                set_by: actor.to_owned(),
            }),
            None => write.event(&ReturnPolicyCleared {
                cleared_by: actor.to_owned(),
            }),
        };
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(())
    }
}
