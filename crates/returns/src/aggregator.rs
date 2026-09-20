use timada_core::Money;

use crate::value_object::{ReceivedLine, RefundMethod, ReplacementLine, ReturnGround, ReturnLine};

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

    /// The operator chose to send the same products again rather than to
    /// refund: `ReturnReceived`, committed together with this one, then says
    /// that no money and no credit go back. The fallback amounts are what the
    /// refund would have been — settled now, like a refund's, and used only
    /// if the replacement turns out impossible.
    ReplacementPlanned {
        lines: Vec<ReplacementLine>,
        fallback_money: Money,
        fallback_credit: Money,
    },

    /// The replacement could not be sent (no stock left by the time it was
    /// reserved): the customer is refunded the fallback amounts instead.
    ReplacementAbandoned { reason: String },

    /// The replacement parcel waits for the carrier in the shipping context.
    /// Committed together with `ReturnCompleted`.
    ReplacementArranged { shipment_id: String },

    /// Why the articles come back, as a ground the policy can reason about.
    /// Committed together with `ReturnRequested`; returns older than grounds
    /// have none.
    ReturnGroundStated { ground: ReturnGround },

    /// The shop gave the customer a prepaid label for the way back: a link,
    /// a file (kept in `return_label_file`, named here), or both. `fee` is
    /// what it costs the customer — zero when the shop is at fault or the
    /// operator waived it — and comes off the refund. `with_approval` says
    /// the label was handed over in the same go as `ReturnApproved`: whatever
    /// announces the approval then carries the label too.
    ReturnLabelIssued {
        carrier: String,
        tracking_number: String,
        url: Option<String>,
        file_name: Option<String>,
        fee: Money,
        with_approval: bool,
    },

    /// The label's fee was taken off what goes back: `ReturnReceived`,
    /// committed together with this one, holds the net amounts.
    ReturnLabelFeeDeducted { amount: Money },
}
