use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::{
    CancellationRequested, Cancelled, CaptureFailed, CaptureRequested, Fulfilled, Paid,
    PaymentReleased, RefundRequested, Refunded, SupplierOrderRefused, SupplierOrderStarted,
};
use crate::error::OrderError;
use crate::value_object::{
    MAX_REASON_LEN, SHOPPER_CANCEL_REASON, STOREFRONT_ACTOR, normalize_reason, normalize_tracking,
};

use super::{OrderState, OrderStatus, PendingCapture, PendingRefund, SupplierOrder};

/// How many times a write to an order is tried again when another one
/// landed at the same moment.
const ORDER_WRITE_ATTEMPTS: usize = 3;

/// What asking for the held money came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureClaim {
    /// This call asked: it is the one to speak to the provider.
    Claimed(PendingCapture),
    /// Someone asked already and the provider has not answered yet.
    Pending(PendingCapture),
    /// The money was taken already.
    Paid,
}

/// What a cancellation still needs once asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelPath {
    /// Nothing was taken: the hold goes back to the shopper with the
    /// provider, then [`record_release`](super::Command::record_release)
    /// cancels the order.
    Release { payment_id: String },
    /// The money was taken: what is outstanding is refunded first
    /// ([`request_refund`](super::Command::request_refund), the provider,
    /// [`record_refund`](super::Command::record_refund)), then
    /// [`complete_cancellation`](super::Command::complete_cancellation).
    Refund,
    /// It is cancelled already.
    Done,
}

impl OrderState {
    /// What the cancellation of this order needs, now that it is asked for.
    pub fn cancel_path(&self) -> CancelPath {
        if self.status == OrderStatus::Cancelled {
            CancelPath::Done
        } else if self.is_paid() {
            CancelPath::Refund
        } else {
            CancelPath::Release {
                payment_id: self.payment.payment_id.clone(),
            }
        }
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records that an order left, with its tracking number if any; already
    /// fulfilled is a no-op. The money must have been taken first.
    pub async fn fulfil_order(
        &self,
        actor: &str,
        order_id: &str,
        tracking: Option<String>,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let tracking = normalize_tracking(tracking.as_deref())?;
        let order = self.require_order(order_id).await?;
        match order.status {
            OrderStatus::Paid => {}
            OrderStatus::Fulfilled => return Ok(()),
            OrderStatus::Cancelled => return Err(OrderError::OrderCancelled),
            OrderStatus::Authorized => return Err(OrderError::PaymentNotCaptured),
        }
        if order.pending_cancellation.is_some() {
            return Err(OrderError::CancellationPending);
        }
        order
            .write()?
            .routing_key_opt(routing_key)
            .event(&Fulfilled {
                tracking,
                fulfilled_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Asks for the money held on the shopper's card, before the provider
    /// is: whoever writes the request is the one to speak to it
    /// ([`CaptureClaim::Claimed`]). A request already standing, or the money
    /// taken, is told rather than asked again; a cancelled order, or one
    /// being cancelled, refuses.
    pub async fn request_capture(
        &self,
        actor: &str,
        order_id: &str,
        routing_key: Option<String>,
    ) -> Result<CaptureClaim, OrderError> {
        for attempt in 1..=ORDER_WRITE_ATTEMPTS {
            let order = self.require_order(order_id).await?;
            if order.is_paid() {
                return Ok(CaptureClaim::Paid);
            }
            if let Some(pending) = &order.pending_capture {
                return Ok(CaptureClaim::Pending(pending.clone()));
            }
            if order.status == OrderStatus::Cancelled || order.released_at.is_some() {
                return Err(OrderError::OrderCancelled);
            }
            if order.pending_cancellation.is_some() {
                return Err(OrderError::CancellationPending);
            }
            let requested = CaptureRequested {
                payment_id: order.payment.payment_id.clone(),
                amount_cents: order.payment.amount_cents,
                requested_by: actor.to_owned(),
            };
            let written = order
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&requested)
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => {
                    return Ok(CaptureClaim::Claimed(
                        self.require_order(order_id)
                            .await?
                            .pending_capture
                            .unwrap_or(PendingCapture {
                                payment_id: requested.payment_id,
                                amount_cents: requested.amount_cents,
                                requested_by: requested.requested_by,
                                requested_at: 0,
                            }),
                    ));
                }
                Err(WriteError::InvalidOriginalVersion) if attempt < ORDER_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Err(OrderError::Write(WriteError::InvalidOriginalVersion))
    }

    /// Records that the provider took the money; told twice, the second
    /// time is a no-op. A cancelled order refuses (`OrderCancelled`): the
    /// caller gives the money back.
    pub async fn record_payment(
        &self,
        actor: &str,
        order_id: &str,
        payment_id: &str,
        amount_cents: u64,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        for attempt in 1..=ORDER_WRITE_ATTEMPTS {
            let order = self.require_order(order_id).await?;
            if order.is_paid() {
                return Ok(());
            }
            if order.payment.payment_id != payment_id {
                return Err(OrderError::PaymentNotFound);
            }
            if order.status == OrderStatus::Cancelled {
                return Err(OrderError::OrderCancelled);
            }
            let written = order
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Paid {
                    payment_id: payment_id.to_owned(),
                    amount_cents,
                    paid_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) if attempt < ORDER_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// Records that the provider would not take the money; `reason` is what
    /// it said. Only a standing request is settled this way: anything else
    /// is a no-op.
    pub async fn record_capture_failure(
        &self,
        actor: &str,
        order_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let reason: String = reason.trim().chars().take(MAX_REASON_LEN).collect();
        for attempt in 1..=ORDER_WRITE_ATTEMPTS {
            let order = self.require_order(order_id).await?;
            let Some(pending) = order.pending_capture.clone() else {
                return Ok(());
            };
            let written = order
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&CaptureFailed {
                    payment_id: pending.payment_id,
                    reason: reason.clone(),
                    failed_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) if attempt < ORDER_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// Asks for an order to be cancelled, before the provider is asked to
    /// give anything back; returns what is left to do. Asked again while it
    /// stands, the same path comes back. A fulfilled order is
    /// `NotCancellable`; one whose money is being asked for is
    /// `CapturePending` until the provider answered; one whose goods were
    /// ordered from the supplier is `SupplierOrdered` unless the caller
    /// knows (`supplier_known`: the merchant cancels it there, or the
    /// supplier closed it).
    pub async fn request_cancellation(
        &self,
        actor: &str,
        order_id: &str,
        reason: String,
        supplier_known: bool,
        routing_key: Option<String>,
    ) -> Result<CancelPath, OrderError> {
        let reason = normalize_reason(&reason)?;
        for attempt in 1..=ORDER_WRITE_ATTEMPTS {
            let order = self.require_order(order_id).await?;
            match order.status {
                OrderStatus::Cancelled => return Ok(CancelPath::Done),
                OrderStatus::Fulfilled => return Err(OrderError::NotCancellable),
                OrderStatus::Authorized | OrderStatus::Paid => {}
            }
            if order.pending_cancellation.is_some() {
                return Ok(order.cancel_path());
            }
            if order.pending_capture.is_some() {
                return Err(OrderError::CapturePending);
            }
            if order.supplier_order == SupplierOrder::Started && !supplier_known {
                return Err(OrderError::SupplierOrdered);
            }
            let written = order
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&CancellationRequested {
                    reason: reason.clone(),
                    requested_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(order.cancel_path()),
                Err(WriteError::InvalidOriginalVersion) if attempt < ORDER_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Err(OrderError::Write(WriteError::InvalidOriginalVersion))
    }

    /// The shopper asks for their order to be cancelled, before the provider
    /// gives the hold back; returns what is left to do. Only while
    /// [`cancellable_by_shopper`](OrderState::cancellable_by_shopper) holds:
    /// once the supplier may have it, or its money is being taken, the shop
    /// is preparing it (`OrderInPreparation`); a fulfilled order is
    /// `NotCancellable`. Asked again, the same path comes back.
    pub async fn request_shopper_cancellation(
        &self,
        order_id: &str,
        routing_key: Option<String>,
    ) -> Result<CancelPath, OrderError> {
        for attempt in 1..=ORDER_WRITE_ATTEMPTS {
            let order = self.require_order(order_id).await?;
            match order.status {
                OrderStatus::Cancelled => return Ok(CancelPath::Done),
                OrderStatus::Fulfilled => return Err(OrderError::NotCancellable),
                OrderStatus::Authorized | OrderStatus::Paid => {}
            }
            if order
                .pending_cancellation
                .as_ref()
                .is_some_and(|asked| asked.requested_by == STOREFRONT_ACTOR)
            {
                return Ok(order.cancel_path());
            }
            if !order.cancellable_by_shopper() {
                return Err(OrderError::OrderInPreparation);
            }
            let written = order
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&CancellationRequested {
                    reason: SHOPPER_CANCEL_REASON.to_owned(),
                    requested_by: STOREFRONT_ACTOR.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(order.cancel_path()),
                Err(WriteError::InvalidOriginalVersion) if attempt < ORDER_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Err(OrderError::Write(WriteError::InvalidOriginalVersion))
    }

    /// Asks for the goods to be ordered from `supplier`, right before it is:
    /// written at the version read, on every try, so a cancellation landing
    /// first refuses it and one landing after finds it. Refused as
    /// [`may_order_from_supplier`](OrderState::may_order_from_supplier) says
    /// at `now` (unix seconds).
    pub async fn begin_supplier_order(
        &self,
        actor: &str,
        order_id: &str,
        supplier: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        for attempt in 1..=ORDER_WRITE_ATTEMPTS {
            let order = self.require_order(order_id).await?;
            order.may_order_from_supplier(now)?;
            let written = order
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&SupplierOrderStarted {
                    supplier: supplier.to_owned(),
                    started_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) if attempt < ORDER_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Err(OrderError::Write(WriteError::InvalidOriginalVersion))
    }

    /// Records that the supplier surely refused the order it was asked for
    /// (it took nothing): the shopper may cancel again. A no-op unless an
    /// order to the supplier stands.
    pub async fn record_supplier_refusal(
        &self,
        actor: &str,
        order_id: &str,
        supplier: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let reason: String = reason.trim().chars().take(MAX_REASON_LEN).collect();
        for attempt in 1..=ORDER_WRITE_ATTEMPTS {
            let order = self.require_order(order_id).await?;
            if order.supplier_order != SupplierOrder::Started {
                return Ok(());
            }
            let written = order
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&SupplierOrderRefused {
                    supplier: supplier.to_owned(),
                    reason: reason.clone(),
                    refused_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) if attempt < ORDER_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// Records that the hold went back to the shopper, nothing taken, and
    /// cancels the order in the same write: with the reason the cancellation
    /// was asked with, or `reason` when the provider dropped the hold on its
    /// own (it lapsed). Told twice, the second time is a no-op; an order
    /// whose money was taken refuses (`NotCancellable`): there is nothing
    /// held to release.
    pub async fn record_release(
        &self,
        actor: &str,
        order_id: &str,
        payment_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        for attempt in 1..=ORDER_WRITE_ATTEMPTS {
            let order = self.require_order(order_id).await?;
            if order.released_at.is_some() || order.status == OrderStatus::Cancelled {
                return Ok(());
            }
            if order.payment.payment_id != payment_id {
                return Err(OrderError::PaymentNotFound);
            }
            if order.is_paid() || order.status == OrderStatus::Fulfilled {
                return Err(OrderError::NotCancellable);
            }
            let (reason, cancelled_by) = match &order.pending_cancellation {
                Some(asked) => (asked.reason.clone(), asked.requested_by.clone()),
                None => (reason.to_owned(), actor.to_owned()),
            };
            let written = order
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&PaymentReleased {
                    payment_id: payment_id.to_owned(),
                    released_by: actor.to_owned(),
                })
                .event(&Cancelled {
                    reason,
                    cancelled_by,
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) if attempt < ORDER_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// Cancels an order whose money was taken, once all of it went back:
    /// with the reason and the person of the cancellation asked for. A no-op
    /// when it is cancelled already, or no cancellation stands;
    /// `RefundPending` while a refund is asked for and not recorded, and
    /// `InvalidRefund` while money is still outstanding.
    pub async fn complete_cancellation(
        &self,
        order_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        for attempt in 1..=ORDER_WRITE_ATTEMPTS {
            let order = self.require_order(order_id).await?;
            if order.status == OrderStatus::Cancelled {
                return Ok(());
            }
            let Some(asked) = order.pending_cancellation.clone() else {
                return Ok(());
            };
            if order.pending_refund.is_some() {
                return Err(OrderError::RefundPending);
            }
            if order.outstanding_cents() > 0 {
                return Err(OrderError::InvalidRefund);
            }
            let written = order
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Cancelled {
                    reason: asked.reason,
                    cancelled_by: asked.requested_by,
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) if attempt < ORDER_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// Asks for `amount_cents` of the money taken to go back, before the
    /// provider is: the key the provider must be asked with comes back, and
    /// asking again while the refund is pending gives the same key, so a
    /// retry (or another node) makes the same refund. The money must have
    /// been taken (`PaymentNotCaptured`: a hold is released, not refunded),
    /// the order not left (`NotCancellable`), and the amount within what is
    /// still outstanding (`InvalidRefund`). Settled by
    /// [`record_refund`](Self::record_refund).
    pub async fn request_refund(
        &self,
        actor: &str,
        order_id: &str,
        amount_cents: u64,
        routing_key: Option<String>,
    ) -> Result<PendingRefund, OrderError> {
        let order = self.require_order(order_id).await?;
        if let Some(pending) = &order.pending_refund {
            return Ok(pending.clone());
        }
        if !order.is_paid() {
            return Err(OrderError::PaymentNotCaptured);
        }
        if order.status == OrderStatus::Fulfilled {
            return Err(OrderError::NotCancellable);
        }
        if amount_cents == 0 || amount_cents > order.outstanding_cents() {
            return Err(OrderError::InvalidRefund);
        }
        let idempotency_key = format!("timada-refund-{order_id}-{}", order.refund_ids.len() + 1);
        let requested = RefundRequested {
            payment_id: order.payment.payment_id.clone(),
            amount_cents,
            idempotency_key: idempotency_key.clone(),
            requested_by: actor.to_owned(),
        };
        order
            .write()?
            .routing_key_opt(routing_key)
            .event(&requested)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(self
            .require_order(order_id)
            .await?
            .pending_refund
            .unwrap_or(PendingRefund {
                payment_id: requested.payment_id,
                amount_cents,
                idempotency_key,
                requested_at: 0,
            }))
    }

    /// Records money the provider gave back for the order's payment; the same
    /// refund recorded twice is a no-op. Settles the pending request, if any.
    pub async fn record_refund(
        &self,
        actor: &str,
        order_id: &str,
        refund_id: &str,
        amount_cents: u64,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let order = self.require_order(order_id).await?;
        if order.refund_ids.iter().any(|id| id == refund_id) {
            return Ok(());
        }
        order
            .write()?
            .routing_key_opt(routing_key)
            .event(&Refunded {
                payment_id: order.payment.payment_id.clone(),
                refund_id: refund_id.to_owned(),
                amount_cents,
                refunded_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
