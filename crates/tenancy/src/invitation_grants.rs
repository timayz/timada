//! Settles the seat an accepted invitation promised, should the command that
//! accepted it not have: a crash or a failed write between marking the
//! invitation accepted and settling the seat.
//!
//! Delivery may repeat and every node runs the subscription at its own pace,
//! a node that was away long after the fact. The first answer is recorded on
//! the invitation and every node follows it: a seat refused because the role
//! was gone stays refused should the role come back, and a seat granted is
//! not granted again to an account removed since.

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

/// Builds the subscription over every routing key of the platform tenant,
/// where invitations live; `store` is the store it follows, which it grants
/// the seats in.
pub fn invitation_grants_subscription<E: Executor>(store: Evento) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(INVITATION_GRANTS_SUBSCRIPTION)
        .data(Store(store))
        .handler(on_accepted())
        // A saga: it follows the node's own region (the executor's default
        // routing key), so a region's nodes do its work and the other
        // regions' nodes leave it to them.
        .tenant("")
        .retry(5)
}

#[evento::subscription]
async fn on_accepted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Accepted>,
) -> anyhow::Result<()> {
    let store: Store = context.extract();
    match Command(store.0)
        .settle_seat(&event.aggregate_id, None)
        .await
    {
        Ok(_) => Ok(()),
        // The tenant is gone: there is no seat to give, for good.
        Err(err @ TenancyError::NotFound) => {
            tracing::warn!(%err, invitation = %event.aggregate_id, "an accepted invitation grants no seat");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}
