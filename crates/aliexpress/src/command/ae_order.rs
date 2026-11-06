//! The platform's record of the orders AliExpress holds for the companies'
//! accounts: which shop's forward placed each, and what AliExpress pushed of
//! it ([`crate::aggregator::ae_order`]). A push is recorded before it is
//! answered, whether or not a forward is known to have placed the order yet,
//! and the forward records its orders as it learns them: whichever comes
//! second finds the other in the one stream.

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::ae_order::{AeOrder, Claimed, Pushed};
use crate::error::AliexpressError;
use crate::value_object::ae_order_index_id;
use crate::webhook::ORDER_CLOSED;

/// How many times a write to an AliExpress order's stream is tried when
/// another landed at the same moment (a push and the forward's record).
const WRITE_ATTEMPTS: usize = 3;

/// The longest status a push records, in characters.
const MAX_STATUS_LEN: usize = 64;

/// The forward of a shop that placed an AliExpress order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForwardRef {
    pub tenant_id: String,
    pub forward_id: String,
}

/// An AliExpress order, as the platform knows it.
#[evento::projection(id = id, name = "timada-aliexpress/AeOrderIndex")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct AeOrderIndex {
    pub id: String,
    pub ae_order_id: String,
    /// The forward that placed it, once it recorded so.
    pub forward: Option<ForwardRef>,
    /// The version of the stream the forward recorded it at: a push before
    /// it waited for that record.
    pub claimed_version: Option<u16>,
    /// How many pushes came before the forward recorded it: what that record
    /// reads the order again for.
    pub waiting: u16,
    /// A push said AliExpress closed it without shipping it.
    pub closed: bool,
}

impl AeOrderIndex {
    /// Whether the push recorded at `version` came before the forward
    /// recorded the order (or no forward did yet): its read waits for that
    /// record.
    pub fn waited(&self, version: u16) -> bool {
        self.claimed_version.is_none_or(|claimed| claimed > version)
    }
}

pub fn ae_order_projection<E: Executor>() -> Projection<E, AeOrderIndex> {
    Projection::new::<AeOrder>()
        .handler(on_claimed())
        .handler(on_pushed())
        .strict()
}

#[evento::handler]
async fn on_claimed(event: Event<Claimed>, order: &mut AeOrderIndex) -> anyhow::Result<()> {
    order.id = event.aggregate_id.to_owned();
    order.ae_order_id = event.data.ae_order_id.to_owned();
    if order.forward.is_none() {
        order.forward = Some(ForwardRef {
            tenant_id: event.data.tenant_id.to_owned(),
            forward_id: event.data.forward_id.to_owned(),
        });
        order.claimed_version = Some(event.version);
    }
    Ok(())
}

#[evento::handler]
async fn on_pushed(event: Event<Pushed>, order: &mut AeOrderIndex) -> anyhow::Result<()> {
    order.id = event.aggregate_id.to_owned();
    order.ae_order_id = event.data.ae_order_id.to_owned();
    if order.forward.is_none() {
        order.waiting = order.waiting.saturating_add(1);
    }
    if event.data.status == ORDER_CLOSED {
        order.closed = true;
    }
    Ok(())
}

/// The platform's record of the AliExpress order `ae_order_id`, if anything
/// was ever recorded of it.
pub async fn load_ae_order<E: Executor>(
    executor: &E,
    ae_order_id: &str,
) -> anyhow::Result<Option<AeOrderIndex>> {
    load_ae_order_by_id(executor, &ae_order_index_id(ae_order_id)).await
}

/// The same, by the id of its stream.
pub async fn load_ae_order_by_id<E: Executor>(
    executor: &E,
    id: &str,
) -> anyhow::Result<Option<AeOrderIndex>> {
    ae_order_projection().load(id).execute(executor).await
}

/// Records that the forward `forward_id` of the shop `tenant_id` placed the
/// AliExpress order `ae_order_id`. Recorded already, nothing more is written;
/// the first forward recorded stands (AliExpress never gives one number
/// twice).
pub async fn claim_ae_order<E: Executor>(
    executor: &E,
    ae_order_id: &str,
    tenant_id: &str,
    forward_id: &str,
) -> Result<(), AliexpressError> {
    let claimed = Claimed {
        ae_order_id: ae_order_id.to_owned(),
        tenant_id: tenant_id.to_owned(),
        forward_id: forward_id.to_owned(),
    };
    for attempt in 1..=WRITE_ATTEMPTS {
        let written = match load_ae_order(executor, ae_order_id).await? {
            Some(order) if order.forward.is_some() => return Ok(()),
            Some(order) => order.write()?.event(&claimed).commit(executor).await,
            None => {
                evento::append(ae_order_index_id(ae_order_id))
                    .original_version(0)
                    .event(&claimed)
                    .commit(executor)
                    .await
            }
        };
        match written {
            Ok(_) => return Ok(()),
            Err(WriteError::InvalidOriginalVersion) if attempt < WRITE_ATTEMPTS => {}
            Err(err) => return Err(err.into()),
        }
    }
    Err(WriteError::InvalidOriginalVersion.into())
}

/// Records what AliExpress pushed of the order `ae_order_id`, whichever
/// shop's it is, or none's yet; the forward that placed it, when one recorded
/// so. Each push is recorded, the same one twice included: reading the order
/// again for it changes nothing the first read did not.
pub async fn record_push<E: Executor>(
    executor: &E,
    ae_order_id: &str,
    status: &str,
) -> Result<Option<ForwardRef>, AliexpressError> {
    let pushed = Pushed {
        ae_order_id: ae_order_id.to_owned(),
        status: status.chars().take(MAX_STATUS_LEN).collect(),
    };
    for attempt in 1..=WRITE_ATTEMPTS {
        let order = load_ae_order(executor, ae_order_id).await?;
        let written = match &order {
            Some(order) => order.write()?.event(&pushed).commit(executor).await,
            None => {
                evento::append(ae_order_index_id(ae_order_id))
                    .original_version(0)
                    .event(&pushed)
                    .commit(executor)
                    .await
            }
        };
        match written {
            Ok(_) => return Ok(order.and_then(|order| order.forward)),
            Err(WriteError::InvalidOriginalVersion) if attempt < WRITE_ATTEMPTS => {}
            Err(err) => return Err(err.into()),
        }
    }
    Err(WriteError::InvalidOriginalVersion.into())
}
