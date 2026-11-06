//! Grants the seat an accepted invitation promised, should the command that
//! accepted it not have: a crash or a failed write between marking the
//! invitation accepted and granting the seat.
//!
//! Delivery may repeat and every node runs the subscription at its own pace,
//! a node that was away long after the fact: the seat goes only to an
//! account that is not a member and was not removed since it accepted, so an
//! old link is never a way back in.

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};

use crate::aggregator::Accepted;
use crate::command::Command;
use crate::error::TenancyError;

/// The cursor identity of the subscription, in the store. One per node.
pub const INVITATION_GRANTS_SUBSCRIPTION: &str = "timada-tenancy/invitation-grants";

/// The store, to write with (a handler only borrows the executor it runs
/// on).
#[derive(Clone)]
struct Store(Evento);

/// Builds the subscription over every routing key; `store` is the store it
/// follows, which it grants the seats in.
pub fn invitation_grants_subscription<E: Executor>(store: Evento) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(INVITATION_GRANTS_SUBSCRIPTION)
        .data(Store(store))
        .handler(on_accepted())
        .any_routing_key()
        .retry(5)
}

#[evento::subscription]
async fn on_accepted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Accepted>,
) -> anyhow::Result<()> {
    // Invitations live in the platform's own streams, the default tenant:
    // the same event in a shop's streams grants nothing.
    if !event.tenant.is_empty() {
        return Ok(());
    }
    let store: Store = context.extract();
    match Command(store.0)
        .grant_accepted_seat(&event.aggregate_id, None)
        .await
    {
        Ok(()) => Ok(()),
        // The role or the tenant is gone: there is no seat to give, for good.
        Err(err @ (TenancyError::UnknownRole | TenancyError::NotFound)) => {
            tracing::warn!(%err, invitation = %event.aggregate_id, "an accepted invitation grants no seat");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}
