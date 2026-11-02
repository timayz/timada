//! What a push from AliExpress does once it reached the platform: check the
//! signature, find the shop order that bought the AliExpress order, and read
//! that order again right away instead of waiting for the sweep. Kept apart
//! from the HTTP route so it is tested without one.
//!
//! AliExpress wants its 200 within half a second and delivers at least once,
//! so the route answers as soon as the push checks out and the forward is
//! found; the read runs behind it, and a second push for the same order
//! finds nothing left to record (`record_checked` writes against the
//! version it started from). A push for an order the read model does not
//! know yet is let go: the sweep reads every placed order anyway.

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
    tokio::spawn(async move {
        if let Err(err) = check(aliexpress, store, &forward.tenant_id, &forward.id, now).await {
            tracing::warn!(tenant = %forward.tenant_id, forward = %forward.id, error = %err, "AliExpress order not read after its push");
        }
    });
    Ok(outcome)
}

/// Reads the forward's AliExpress orders again, as the sweep would.
async fn check(
    aliexpress: Arc<Aliexpress>,
    store: Executor,
    tenant_id: &str,
    forward_id: &str,
    now: u64,
) -> anyhow::Result<()> {
    let Some((tenant, company)) = aliexpress.connected_shop(tenant_id, now).await? else {
        return Ok(());
    };
    aliexpress
        .check_forward(&store, &tenant, &company, forward_id, now)
        .await?;
    Ok(())
}
