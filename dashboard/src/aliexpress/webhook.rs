//! What a push from AliExpress does once it reached the platform: check the
//! signature, find the shop order that bought the AliExpress order, and read
//! that order again right away. The sweep then has no reason to read it for a
//! while, so the app's calls go by what happens to the orders, not by how many
//! are open. Kept apart from the HTTP route so it is tested without one.
//!
//! AliExpress wants its 200 within half a second and delivers at least once,
//! so the route answers as soon as the push checks out and the forward is
//! found; the read runs behind it, and a second push for the same order
//! finds nothing left to record (`record_checked` writes against the
//! version it started from). A push for an order the read model does not
//! know yet is let go: the next one reads it, or the sweep at the latest.

use std::sync::Arc;

use timada_aliexpress::{Push, forward_of_ae_order, parse_push, push_signed};

use super::Aliexpress;
use crate::cluster::Executor;
use crate::db::Db;

/// What the route answers AliExpress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The order is being read again.
    Accepted {
        tenant_id: String,
        forward_id: String,
    },
    /// Another topic, or an order no shop of ours placed (or not placed yet
    /// as far as the read model knows).
    Ignored,
    /// The signature does not check out.
    Rejected,
    /// The body is not a push.
    Malformed,
    /// The platform has no AliExpress app.
    Disabled,
}

/// Handles one push: `body` as received, `authorization` its header.
pub async fn receive(
    aliexpress: Arc<Aliexpress>,
    db: &Db,
    store: Executor,
    body: &[u8],
    authorization: Option<&str>,
    now: u64,
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
    let Some(forward) = forward_of_ae_order(db, &ae_order_id).await? else {
        tracing::debug!(ae_order = %ae_order_id, %status, "AliExpress pushed an order no forward knows");
        return Ok(Outcome::Ignored);
    };
    tracing::info!(tenant = %forward.tenant_id, forward = %forward.id, ae_order = %ae_order_id, %status, "AliExpress pushed an order's status");
    let outcome = Outcome::Accepted {
        tenant_id: forward.tenant_id.clone(),
        forward_id: forward.id.clone(),
    };
    let db = db.clone();
    // A closed order closes it on the forward even if it cannot be read.
    let closed = (status == ORDER_CLOSED).then_some(ae_order_id);
    tokio::spawn(async move {
        if let Err(err) = check(
            aliexpress,
            &db,
            store,
            &forward.tenant_id,
            &forward.id,
            closed.as_deref(),
            now,
        )
        .await
        {
            tracing::warn!(tenant = %forward.tenant_id, forward = %forward.id, error = %err, "AliExpress order not read after its push");
        }
    });
    Ok(outcome)
}

/// What AliExpress's push says of an order it closed without shipping it.
const ORDER_CLOSED: &str = "OrderClosed";

/// Reads the forward's AliExpress orders again, as the sweep would, which then
/// has no reason to for a while; `closed` is the order the push said
/// AliExpress closed.
pub(super) async fn check(
    aliexpress: Arc<Aliexpress>,
    db: &Db,
    store: Executor,
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
