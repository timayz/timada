use timada_core::Money;

use crate::value_object::{ReceivedLine, RefundMethod, ReturnLine};

// The explicit name pins the on-disk identity: renaming the crate or the enum
// must never orphan stored events.
#[evento::aggregate(name = "timada-returns/Return")]
pub enum Return {
    /// A customer asked to send lines of a shipped order back.
    ReturnRequested {
        /// "R2026-000042", printed on the return slip.
        rma_number: String,
        order_id: String,
        customer_id: String,
        lines: Vec<ReturnLine>,
        reason: String,
    },

    /// An operator accepted the request: the customer may ship the parcel.
    ReturnApproved,

    /// An operator turned the request down.
    ReturnRefused { reason: String },

    /// The customer changed their mind before the parcel was received.
    ReturnCancelled,

    /// The parcel came in. What was accepted, line by line, and what goes
    /// back to the customer: `money` to the original payment, `credit` as a
    /// store-credit voucher. Both are final — the process manager only
    /// executes them.
    ReturnReceived {
        lines: Vec<ReceivedLine>,
        refund_method: RefundMethod,
        money: Money,
        credit: Money,
    },

    /// Stock put back, customer refunded: nothing left to do.
    ReturnCompleted {
        refunded: Money,
        credited: Money,
        voucher_code: Option<String>,
    },
}
