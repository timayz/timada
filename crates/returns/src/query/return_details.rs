//! One return as the customer and the operator see it, folded from one
//! `Return` stream. Executor-backed snapshots via the bitcode derives.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::Money;

use crate::{
    aggregator::{
        ReplacementAbandoned, ReplacementArranged, ReplacementPlanned, Return, ReturnApproved,
        ReturnCancelled, ReturnCompleted, ReturnGroundStated, ReturnLabelFeeDeducted,
        ReturnLabelIssued, ReturnReceived, ReturnRefused, ReturnRequested,
    },
    value_object::{
        ReceivedLine, RefundMethod, ReplacementLine, ReplacementStatus, ReturnGround, ReturnLine,
        ReturnStatus,
    },
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct ReturnView {
    pub id: String,
    pub rma_number: String,
    pub order_id: String,
    pub customer_id: String,
    pub status: ReturnStatus,
    pub lines: Vec<ReturnLine>,
    pub reason: String,
    /// Unix seconds of `ReturnRequested`.
    pub requested_at: u64,
    pub refused_reason: Option<String>,
    /// What the parcel held; empty until it is received.
    pub received: Vec<ReceivedLine>,
    pub received_at: Option<u64>,
    pub refund_method: Option<RefundMethod>,
    /// Going back to the original payment.
    pub money: Money,
    /// Going back as store credit.
    pub credit: Money,
    pub voucher_code: Option<String>,
    pub completed_at: Option<u64>,
    /// The products sent again instead of a refund, when the operator chose
    /// so.
    pub replacement: Option<ReplacementView>,
    /// Why the articles come back; `None` for returns older than grounds.
    pub ground: Option<ReturnGround>,
    pub approved_at: Option<u64>,
    /// The prepaid label for the way back, when the shop gave one.
    pub label: Option<ReturnLabelView>,
    /// What the label cost the customer in the end: taken off `money` and
    /// `credit` when the parcel was received.
    pub label_fee_deducted: Option<Money>,
}

/// The prepaid label of a return.
#[derive(Debug, Clone, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct ReturnLabelView {
    pub carrier: String,
    pub tracking_number: String,
    /// Where it is downloaded, when it lives at the carrier's.
    pub url: Option<String>,
    /// Its file's name, when the shop holds the file
    /// ([`crate::load_return_label_file`]).
    pub file_name: Option<String>,
    /// What it costs the customer; zero when it is on the shop.
    pub fee: Money,
    pub issued_at: u64,
    /// Handed over with the approval itself, in one go: what announces the
    /// approval then carries the label, and nothing else needs to.
    pub with_approval: bool,
}

/// The replacement of a return: what is sent again, and how far it got.
#[derive(Debug, Clone, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct ReplacementView {
    pub lines: Vec<ReplacementLine>,
    pub status: ReplacementStatus,
    /// The parcel, in the shipping context, once it is arranged.
    pub shipment_id: Option<String>,
    pub abandoned_reason: Option<String>,
    /// What the refund is if the replacement is abandoned.
    pub fallback_money: Money,
    pub fallback_credit: Money,
}

impl ReturnView {
    /// Units asked to be returned, all lines together.
    pub fn units(&self) -> u32 {
        self.lines.iter().map(|l| l.quantity).sum()
    }
}

pub fn create_projection<E: Executor>() -> Projection<E, ReturnView> {
    Projection::new::<Return>()
        .handler(on_return_requested())
        .handler(on_return_approved())
        .handler(on_return_refused())
        .handler(on_return_cancelled())
        .handler(on_return_received())
        .handler(on_return_completed())
        .handler(on_replacement_planned())
        .handler(on_replacement_abandoned())
        .handler(on_replacement_arranged())
        .handler(on_return_ground_stated())
        .handler(on_return_label_issued())
        .handler(on_return_label_fee_deducted())
        // `replacement`, then the ground and the label, joined the snapshot.
        .revision(2)
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<ReturnView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_return_requested(
    event: Event<ReturnRequested>,
    row: &mut ReturnView,
) -> anyhow::Result<()> {
    let currency = event
        .data
        .lines
        .first()
        .map_or_else(|| Money::EUR.to_owned(), |l| l.unit_price.currency.clone());
    row.id = event.aggregate_id.to_owned();
    row.requested_at = event.timestamp;
    row.rma_number = event.data.rma_number;
    row.order_id = event.data.order_id;
    row.customer_id = event.data.customer_id;
    row.lines = event.data.lines;
    row.reason = event.data.reason;
    row.status = ReturnStatus::Requested;
    row.money = Money::zero(&currency);
    row.credit = Money::zero(&currency);
    Ok(())
}

#[evento::handler]
async fn on_return_approved(
    event: Event<ReturnApproved>,
    row: &mut ReturnView,
) -> anyhow::Result<()> {
    row.approved_at = Some(event.timestamp);
    row.status = ReturnStatus::Approved;
    Ok(())
}

#[evento::handler]
async fn on_return_refused(
    event: Event<ReturnRefused>,
    row: &mut ReturnView,
) -> anyhow::Result<()> {
    row.status = ReturnStatus::Refused;
    row.refused_reason = Some(event.data.reason);
    Ok(())
}

#[evento::handler]
async fn on_return_cancelled(
    _event: Event<ReturnCancelled>,
    row: &mut ReturnView,
) -> anyhow::Result<()> {
    row.status = ReturnStatus::Cancelled;
    Ok(())
}

#[evento::handler]
async fn on_return_received(
    event: Event<ReturnReceived>,
    row: &mut ReturnView,
) -> anyhow::Result<()> {
    row.status = ReturnStatus::Received;
    row.received_at = Some(event.timestamp);
    row.received = event.data.lines;
    row.refund_method = Some(event.data.refund_method);
    row.money = event.data.money;
    row.credit = event.data.credit;
    Ok(())
}

#[evento::handler]
async fn on_return_completed(
    event: Event<ReturnCompleted>,
    row: &mut ReturnView,
) -> anyhow::Result<()> {
    row.status = ReturnStatus::Completed;
    row.completed_at = Some(event.timestamp);
    row.money = event.data.refunded;
    row.credit = event.data.credited;
    row.voucher_code = event.data.voucher_code;
    Ok(())
}

#[evento::handler]
async fn on_replacement_planned(
    event: Event<ReplacementPlanned>,
    row: &mut ReturnView,
) -> anyhow::Result<()> {
    row.replacement = Some(ReplacementView {
        lines: event.data.lines,
        status: ReplacementStatus::Planned,
        shipment_id: None,
        abandoned_reason: None,
        fallback_money: event.data.fallback_money,
        fallback_credit: event.data.fallback_credit,
    });
    Ok(())
}

/// The return is a refund again: the amounts it shows are the fallback's.
#[evento::handler]
async fn on_replacement_abandoned(
    event: Event<ReplacementAbandoned>,
    row: &mut ReturnView,
) -> anyhow::Result<()> {
    if let Some(replacement) = &mut row.replacement {
        replacement.status = ReplacementStatus::Abandoned;
        replacement.abandoned_reason = Some(event.data.reason);
        row.money = replacement.fallback_money.clone();
        row.credit = replacement.fallback_credit.clone();
    }
    Ok(())
}

#[evento::handler]
async fn on_replacement_arranged(
    event: Event<ReplacementArranged>,
    row: &mut ReturnView,
) -> anyhow::Result<()> {
    if let Some(replacement) = &mut row.replacement {
        replacement.status = ReplacementStatus::Arranged;
        replacement.shipment_id = Some(event.data.shipment_id);
    }
    Ok(())
}

#[evento::handler]
async fn on_return_ground_stated(
    event: Event<ReturnGroundStated>,
    row: &mut ReturnView,
) -> anyhow::Result<()> {
    row.ground = Some(event.data.ground);
    Ok(())
}

#[evento::handler]
async fn on_return_label_issued(
    event: Event<ReturnLabelIssued>,
    row: &mut ReturnView,
) -> anyhow::Result<()> {
    let issued_at = event.timestamp;
    row.label = Some(ReturnLabelView {
        carrier: event.data.carrier,
        tracking_number: event.data.tracking_number,
        url: event.data.url,
        file_name: event.data.file_name,
        fee: event.data.fee,
        issued_at,
        with_approval: event.data.with_approval,
    });
    Ok(())
}

#[evento::handler]
async fn on_return_label_fee_deducted(
    event: Event<ReturnLabelFeeDeducted>,
    row: &mut ReturnView,
) -> anyhow::Result<()> {
    row.label_fee_deducted = Some(event.data.amount);
    Ok(())
}
