//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail. No field is an
//! enum, so the lock never freezes one.

/// How a shop wants to be written for. One stream per shop
/// ([`brief_id`](crate::brief_id)).
#[evento::aggregate(name = "timada-writer/Brief")]
pub enum Brief {
    BriefSet {
        /// The voice of the shop, in the merchant's words.
        tone: String,
        /// What a text must never claim or say.
        never: String,
        set_by: String,
    },
}

/// A request for the texts of a product or a category to be drafted, and
/// what became of it. Its id is fresh.
#[evento::aggregate(name = "timada-writer/Draft")]
pub enum Draft {
    Requested {
        /// `product` or `category`.
        kind: String,
        subject_id: String,
        /// The subject's name when the draft was asked for, for a list.
        subject_name: String,
        /// The search the texts are written for; empty to write for the
        /// subject's own name.
        keyword: String,
        requested_by: String,
    },
    /// A node took the request on: it is the one that asks the model.
    Started { started_by: String },
    /// The model answered and its texts wait as the proposal
    /// `proposal_id`. `to_check` lists what the draft states that the
    /// shop's data does not.
    Drafted {
        proposal_id: String,
        /// What the call cost, in millionths of a US dollar.
        cost_usd_micros: u64,
        to_check: Vec<String>,
        drafted_by: String,
    },
    /// Nothing was drafted; `reason` says why, for the member.
    Failed { reason: String, failed_by: String },
}

/// What the model was paid on a day (UTC), by every shop. Its id is made
/// of the day ([`spend_day_id`](crate::spend_day_id)), and it lives in the
/// platform's tenant.
#[evento::aggregate(name = "timada-writer/SpendDay")]
pub enum SpendDay {
    /// Room kept for a call not made yet, at the most it may cost.
    Held {
        hold: String,
        /// The shop the call is for.
        tenant_id: String,
        draft_id: String,
        most_usd_micros: u64,
        held_by: String,
    },
    /// What the call cost, in the hold's place.
    Charged {
        hold: String,
        tenant_id: String,
        draft_id: String,
        cost_usd_micros: u64,
        charged_by: String,
    },
    /// The call gave nothing and cost nothing.
    Released { hold: String, released_by: String },
}
