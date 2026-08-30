/// Everything that can happen to one order's return.
///
/// `ReturnRefunded` duplicates `order_id` (already on `ReturnRequested`) on
/// purpose: the invoice crate's credit-note handler needs only that one event
/// and should not have to replay the return to find the order.
#[evento::aggregate]
pub enum Return {
    /// The customer asked to send the order back. May appear again after a
    /// rejection — a re-request opens the conversation anew.
    ReturnRequested { order_id: String, reason: String },
    /// An admin accepted the return; the refund is now the saga's job.
    ReturnApproved,
    /// An admin declined, with a reason the customer sees.
    ReturnRejected { reason: String },
    /// The provider confirmed the refund. Terminal.
    ReturnRefunded {
        order_id: String,
        payment_id: String,
    },
}
