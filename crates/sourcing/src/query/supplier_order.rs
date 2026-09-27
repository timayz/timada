//! A purchase as the back office shows it. The one snapshotted view of the
//! `SupplierOrder` aggregate.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::{Address, Money};

use crate::{
    aggregator::{
        SupplierOrder, SupplierOrderCancelled, SupplierOrderDrafted, SupplierOrderPlaced,
        SupplierOrderRecordedByHand, SupplierOrderRefused, SupplierOrderShipped,
    },
    value_object::{SupplierOrderLine, SupplierOrderStatus},
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct SupplierOrderView {
    pub id: String,
    pub order_id: String,
    pub supplier_id: String,
    pub lines: Vec<SupplierOrderLine>,
    pub ship_to: Address,
    /// What it was expected to come to, at the costs last quoted.
    pub cost: Money,
    /// What the supplier actually charged, once it took the order.
    pub charged: Option<Money>,
    pub status: SupplierOrderStatus,
    pub external_order_id: Option<String>,
    pub carrier: Option<String>,
    pub tracking_number: Option<String>,
    /// Why it was refused or called off, or what the operator noted when they
    /// bought it by hand.
    pub note: Option<String>,
    pub drafted_at: u64,
    /// When it stopped being the shop's to do anything about.
    pub settled_at: Option<u64>,
}

impl SupplierOrderView {
    pub fn units(&self) -> u32 {
        self.lines.iter().map(|line| line.quantity).sum()
    }

    /// What the supplier charged over what it quoted, if it did.
    pub fn overrun(&self) -> Option<Money> {
        let charged = self.charged.as_ref()?;
        let over = charged.checked_sub(&self.cost).ok()?;
        (over.minor > 0).then_some(over)
    }
}

pub fn create_projection<E: Executor>() -> Projection<E, SupplierOrderView> {
    Projection::new::<SupplierOrder>()
        .handler(on_drafted())
        .handler(on_placed())
        .handler(on_by_hand())
        .handler(on_refused())
        .handler(on_shipped())
        .handler(on_cancelled())
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<SupplierOrderView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_drafted(
    event: Event<SupplierOrderDrafted>,
    row: &mut SupplierOrderView,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.drafted_at = event.timestamp;
    row.order_id = event.data.order_id;
    row.supplier_id = event.data.supplier_id;
    row.lines = event.data.lines;
    row.ship_to = event.data.ship_to;
    row.cost = event.data.cost;
    row.status = SupplierOrderStatus::Drafted;
    Ok(())
}

#[evento::handler]
async fn on_placed(
    event: Event<SupplierOrderPlaced>,
    row: &mut SupplierOrderView,
) -> anyhow::Result<()> {
    row.status = SupplierOrderStatus::Placed;
    row.external_order_id = Some(event.data.external_order_id);
    row.charged = Some(event.data.cost);
    Ok(())
}

#[evento::handler]
async fn on_by_hand(
    event: Event<SupplierOrderRecordedByHand>,
    row: &mut SupplierOrderView,
) -> anyhow::Result<()> {
    row.status = SupplierOrderStatus::Placed;
    row.external_order_id = Some(event.data.external_order_id);
    row.note = (!event.data.note.trim().is_empty()).then_some(event.data.note);
    Ok(())
}

#[evento::handler]
async fn on_refused(
    event: Event<SupplierOrderRefused>,
    row: &mut SupplierOrderView,
) -> anyhow::Result<()> {
    row.status = SupplierOrderStatus::Refused;
    row.settled_at = Some(event.timestamp);
    row.note = Some(event.data.reason);
    Ok(())
}

#[evento::handler]
async fn on_shipped(
    event: Event<SupplierOrderShipped>,
    row: &mut SupplierOrderView,
) -> anyhow::Result<()> {
    row.status = SupplierOrderStatus::Shipped;
    row.settled_at = Some(event.timestamp);
    row.carrier = Some(event.data.carrier);
    row.tracking_number = Some(event.data.tracking_number);
    Ok(())
}

#[evento::handler]
async fn on_cancelled(
    event: Event<SupplierOrderCancelled>,
    row: &mut SupplierOrderView,
) -> anyhow::Result<()> {
    row.status = SupplierOrderStatus::Cancelled;
    row.settled_at = Some(event.timestamp);
    row.note = Some(event.data.reason);
    Ok(())
}
