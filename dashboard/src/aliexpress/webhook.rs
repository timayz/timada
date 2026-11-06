//! What a push from AliExpress does once it reached the platform: check the
//! signature, record the push in the platform's stream of the order it names
//! (`timada-aliexpress/AeOrder`), and answer. AliExpress wants its 200 within
//! half a second and delivers at least once, retrying what it got no 200 for:
//! a push is answered once recorded, so a node stopping right after loses
//! nothing, and one the store did not take is answered with an error and sent
//! again.
//!
//! The subscription below reads the order again, on the node its forward
//! belongs to ([`Share`](timada_aliexpress::Share)), as the sweep would. The
//! sweep then has no reason to read it for a while, so the app's calls go by
//! what happens to the orders, not by how many are open. A second push for
//! the same news finds nothing left to record (`record_checked` writes
//! against the version it started from).
//!
//! A push may name an order no forward recorded yet: AliExpress answers the
//! placing and pushes its first news at the same moment, a forward could not
//! record its orders, or the account ordered something by hand. It waits in
//! the order's stream, and the forward's record of the order, written in that
//! stream (`Claimed`), reads it then. Kept apart from the HTTP route so it is
//! tested without one.

use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_aliexpress::aggregator::ae_order::{Claimed, Pushed};
use timada_aliexpress::{
    ForwardRef, ORDER_CLOSED, Push, load_ae_order_by_id, parse_push, push_signed, record_push,
};

use super::Aliexpress;
use crate::db::Db;

/// The cursor identity of [`pushes_subscription`], in the store.
pub const PUSHES_SUBSCRIPTION: &str = "timada-dashboard/aliexpress-pushes";

/// A push older than this is history being replayed (a node catching up):
/// left to the sweep, which reads the order within its period anyway.
const PUSH_WINDOW_SECS: u64 = 10 * 60;

/// What the route answers AliExpress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Recorded: the order is read again, at once when `forward` placed it,
    /// else once a forward records it.
    Accepted { forward: Option<ForwardRef> },
    /// Another topic.
    Ignored,
    /// The signature does not check out.
    Rejected,
    /// The body is not a push.
    Malformed,
    /// The platform has no AliExpress app.
    Disabled,
}

/// Handles one push: `body` as received, `authorization` its header. An
/// error is a push not recorded, for the route to answer so.
pub async fn receive<E: evento::Executor>(
    aliexpress: &Aliexpress,
    store: &E,
    body: &[u8],
    authorization: Option<&str>,
) -> anyhow::Result<Outcome> {
    let Ok((app_key, app_secret)) = aliexpress.app() else {
        return Ok(Outcome::Disabled);
    };
    if !authorization.is_some_and(|given| push_signed(app_key, app_secret, body, given)) {
        return Ok(Outcome::Rejected);
    }
    let Some(push) = parse_push(body) else {
        return Ok(Outcome::Malformed);
    };
    let Push::OrderStatus {
        ae_order_id,
        status,
    } = push
    else {
        return Ok(Outcome::Ignored);
    };
    if ae_order_id.is_empty() {
        return Ok(Outcome::Malformed);
    }
    let forward = record_push(store, &ae_order_id, &status).await?;
    match &forward {
        Some(forward) => {
            tracing::info!(tenant = %forward.tenant_id, forward = %forward.forward_id, ae_order = %ae_order_id, %status, "AliExpress pushed an order's status");
        }
        None => {
            tracing::info!(ae_order = %ae_order_id, %status, "AliExpress pushed the status of an order no forward recorded yet");
        }
    }
    Ok(Outcome::Accepted { forward })
}

/// Reads the forward's AliExpress orders again, as the sweep would, which then
/// has no reason to for a while; `closed` is the order the push said
/// AliExpress closed.
pub(super) async fn check<E: evento::Executor + Clone>(
    aliexpress: Arc<Aliexpress>,
    db: &Db,
    store: E,
    tenant_id: &str,
    forward_id: &str,
    closed: Option<&str>,
    now: u64,
) -> anyhow::Result<()> {
    let Some((tenant, company)) = aliexpress.connected_shop(tenant_id, now).await? else {
        return Ok(());
    };
    let read = aliexpress
        .check_forward(&store, &tenant, &company, forward_id, closed, now)
        .await;
    timada_aliexpress::note_forward_read(db, tenant_id, forward_id, now).await?;
    read?;
    Ok(())
}

/// The store, to write with.
#[derive(Clone)]
struct Store(Evento);

/// Reads an order again for the pushes recorded of it. `db` is the read-model
/// database, which must be allowed to write (it keeps when each forward was
/// read).
pub fn pushes_subscription<E: Executor>(
    aliexpress: Arc<Aliexpress>,
    db: Db,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(PUSHES_SUBSCRIPTION)
        .data(aliexpress)
        .data(db)
        .data(Store(store))
        .handler(on_pushed())
        .handler(on_claimed())
        .any_routing_key()
        .retry(5)
}

/// A push of an order a forward recorded: read again. One that came first
/// waits for that record.
#[evento::subscription]
async fn on_pushed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Pushed>,
) -> anyhow::Result<()> {
    let store: Store = context.extract();
    let Some(order) = load_ae_order_by_id(&store.0, &event.aggregate_id).await? else {
        return Ok(());
    };
    if order.waited(event.version) {
        return Ok(());
    }
    let closed = (event.data.status == ORDER_CLOSED).then_some(event.data.ae_order_id.as_str());
    read_again(context, order.forward.as_ref(), event.timestamp, closed).await;
    Ok(())
}

/// A forward recorded an order that pushes came for before: read again, for
/// them all.
#[evento::subscription]
async fn on_claimed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Claimed>,
) -> anyhow::Result<()> {
    let store: Store = context.extract();
    let Some(order) = load_ae_order_by_id(&store.0, &event.aggregate_id).await? else {
        return Ok(());
    };
    if order.waiting == 0 {
        return Ok(());
    }
    let closed = order.closed.then_some(event.data.ae_order_id.as_str());
    read_again(context, order.forward.as_ref(), event.timestamp, closed).await;
    Ok(())
}

/// Reads `forward`'s orders again for what was recorded at `at` (unix
/// seconds), on the node it belongs to and while that is news. A read that
/// fails is logged and left to the sweep, so one shop's trouble never stops
/// the pushes of every other.
async fn read_again<E: Executor>(
    context: &Context<'_, E>,
    forward: Option<&ForwardRef>,
    at: u64,
    closed: Option<&str>,
) {
    let Some(forward) = forward else {
        return;
    };
    let aliexpress: Arc<Aliexpress> = context.extract();
    let now = crate::auth::now();
    if now.saturating_sub(at) > PUSH_WINDOW_SECS || !aliexpress.share().owns(&forward.forward_id) {
        return;
    }
    let db: Db = context.extract();
    let store: Store = context.extract();
    if let Err(err) = check(
        aliexpress,
        &db,
        store.0,
        &forward.tenant_id,
        &forward.forward_id,
        closed,
        now,
    )
    .await
    {
        tracing::warn!(tenant = %forward.tenant_id, forward = %forward.forward_id, error = %err, "AliExpress order not read after its push");
    }
}
