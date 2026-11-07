//! Frees the index entries of the sign-in identities a user let go of.
//!
//! Removing a passkey, unlinking a provider identity and deleting an account
//! are written on the user first; the command then frees the index entries
//! at once. A crash or a failed write in between leaves an entry pointing at
//! an account that no longer lists what it indexes. Such an entry already
//! signs nobody in and may be claimed again; this subscription frees it for
//! good.
//!
//! Delivery may repeat and every node runs the subscription at its own pace,
//! a node that was away long after the fact: an entry is freed only while
//! its user does not list it again, and never when it was claimed after the
//! user let go of it (an add of the same identity, on its way to the user's
//! stream).

use evento::{
    Executor, WriteError,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};

use crate::aggregator::{Deleted, OAuth2AccountUnlinked, PasskeyRemoved};
use crate::command::{
    event_millis, free_indexes, free_oauth2_index, free_passkey_index, user_projection,
};
use crate::error::IdentityError;

/// The cursor identity of the subscription, in the store. One per node.
pub const INDEX_RELEASE_SUBSCRIPTION: &str = "timada-identity/index-release";

/// How often a freeing step that lost a race with another writer of the
/// entry is decided again, from what the store holds then.
const FREE_ATTEMPTS: usize = 3;

/// Builds the subscription over every routing key. Start it with
/// `.start(&executor)` and keep the handle for shutdown.
pub fn index_release_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(INDEX_RELEASE_SUBSCRIPTION)
        .handler(on_passkey_removed())
        .handler(on_oauth2_account_unlinked())
        .handler(on_deleted())
        .any_routing_key()
        .retry(5)
}

/// Users live in the platform's own streams, the default tenant: the same
/// event in a shop's streams frees nothing.
fn is_platform(event: &evento::Event) -> bool {
    event.tenant.is_empty()
}

/// Runs `step` again while it loses a race on the entry.
async fn again_on_conflict<F, Fut>(step: F) -> anyhow::Result<()>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<(), IdentityError>>,
{
    let mut attempt = 0;
    loop {
        attempt += 1;
        match step().await {
            Err(IdentityError::Write(WriteError::InvalidOriginalVersion))
                if attempt < FREE_ATTEMPTS => {}
            outcome => return outcome.map_err(Into::into),
        }
    }
}

#[evento::subscription]
async fn on_passkey_removed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<PasskeyRemoved>,
) -> anyhow::Result<()> {
    if !is_platform(&event) {
        return Ok(());
    }
    let let_go_at = event_millis(&event);
    again_on_conflict(|| {
        free_passkey_index(
            context.executor,
            &event.aggregate_id,
            &event.data.credential_id,
            Some(let_go_at),
        )
    })
    .await
}

#[evento::subscription]
async fn on_oauth2_account_unlinked<E: Executor>(
    context: &Context<'_, E>,
    event: Event<OAuth2AccountUnlinked>,
) -> anyhow::Result<()> {
    if !is_platform(&event) {
        return Ok(());
    }
    let let_go_at = event_millis(&event);
    again_on_conflict(|| {
        free_oauth2_index(
            context.executor,
            &event.aggregate_id,
            &event.data.provider,
            &event.data.provider_user_id,
            Some(let_go_at),
        )
    })
    .await
}

#[evento::subscription]
async fn on_deleted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Deleted>,
) -> anyhow::Result<()> {
    if !is_platform(&event) {
        return Ok(());
    }
    let Some(user) = user_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    let let_go_at = event_millis(&event);
    again_on_conflict(|| free_indexes(context.executor, &user, Some(let_go_at))).await
}
