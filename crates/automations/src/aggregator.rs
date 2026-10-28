//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail. No field is an
//! enum, so the lock never freezes one: kinds and settings live in the
//! definition's JSON.
//!
//! The tenant's store is readable by every storefront key of the tenant, so
//! no event here holds personal data or a secret.

/// The scenario events, in their own module: the macro names an event struct
/// after its variant, and later aggregates of this crate reuse names such as
/// `Started` or `Cancelled`.
pub mod scenario {
    /// One scenario a shop's team draws. Its id is a fresh
    /// [`new_scenario_id`](crate::new_scenario_id).
    #[evento::aggregate(name = "timada-automations/Scenario")]
    pub enum Scenario {
        /// Named; it has no trigger and no step yet.
        Created {
            name: String,
            created_by: String,
        },
        Renamed {
            name: String,
            renamed_by: String,
        },
        /// The draft rewritten, whole: a [`Definition`](crate::Definition)
        /// as JSON. Nothing runs from a draft.
        Saved {
            definition: String,
            saved_by: String,
        },
        /// The draft frozen as version `version` (1, 2, …): from now on what
        /// triggers it starts a run of this version, and a run started on an
        /// earlier one carries on with that one.
        Published {
            version: u32,
            definition: String,
            published_by: String,
        },
        /// Holds it: nothing new starts and the runs under way wait.
        Paused {
            paused_by: String,
        },
        Resumed {
            resumed_by: String,
        },
        /// Deleted from the merchant's view: nothing starts any more and the
        /// runs under way are cancelled. It cannot be edited again.
        Archived {
            archived_by: String,
        },
    }
}

/// The run events: one stream per scenario and event that started it.
pub mod run {
    /// One run of a published scenario: what started it, then what each
    /// step it reached did. Its id is [`run_id`](crate::run_id) of the
    /// scenario and the event, so an event starts a scenario once however
    /// many nodes see it. Only ids are kept: what a step needs is read
    /// from the subject when it runs.
    #[evento::aggregate(name = "timada-automations/Run")]
    pub enum Run {
        /// `trigger` is the trigger's kind; `subject_id` the order, review,
        /// message, customer or subscriber it is about.
        Started {
            scenario_id: String,
            scenario_version: u32,
            trigger: String,
            trigger_event_id: String,
            subject_id: String,
            started_by: String,
        },
        /// A wait began: the run goes on once the clock passes `until`.
        Waiting {
            step: String,
            until: u64,
            waited_by: String,
        },
        /// A condition was checked: the run goes down lane `branch` (0 is
        /// Yes).
        Branched {
            step: String,
            branch: u32,
            branched_by: String,
        },
        /// A step that acts outside the run was taken, before it acts: the
        /// node that writes this is the one that acts.
        StepClaimed {
            step: String,
            attempt: u32,
            claimed_by: String,
        },
        /// It acted; `output` is what later steps may use, as JSON.
        StepSucceeded {
            step: String,
            output: String,
            recorded_by: String,
        },
        /// It had nothing to do (nobody to tell, say); the run goes on.
        StepSkipped {
            step: String,
            reason: String,
            recorded_by: String,
        },
        /// It did not act. With `retry_at`, it is tried again then; without,
        /// the run stops there.
        StepFailed {
            step: String,
            attempt: u32,
            reason: String,
            retry_at: Option<u64>,
            recorded_by: String,
        },
        /// Every step reached was done.
        Finished { finished_by: String },
        /// Stopped before the end: by a member, or because its scenario was
        /// deleted.
        Cancelled {
            reason: String,
            cancelled_by: String,
        },
    }
}

/// The shop's list of addresses that stopped its automated mails.
pub mod opt_out {
    /// One address that stopped the shop's automated mails. Its id is
    /// [`opt_out_id`](crate::opt_out_id) of the address: the store keeps no
    /// address, only that hash.
    #[evento::aggregate(name = "timada-automations/OptOut")]
    pub enum OptOut {
        /// Through the link of a mail, or by a member at the customer's
        /// request.
        OptedOut { opted_out_by: String },
        /// Taken off the list by a member: the address gets the mails again.
        Withdrawn { withdrawn_by: String },
    }
}

/// One scenario's mail to one address on one day, claimed before it goes
/// out: an address gets at most one mail a day from a scenario, however
/// often its trigger fires (a contact form or a checkout is no relay).
pub mod mail_claim {
    /// Its id is [`mail_claim_id`](crate::mail_claim_id) of the scenario, the
    /// address and the day; the store keeps no address.
    #[evento::aggregate(name = "timada-automations/MailClaim")]
    pub enum MailClaim {
        Claimed {
            scenario_id: String,
            claimed_by: String,
        },
    }
}
