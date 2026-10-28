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
