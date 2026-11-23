//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail. No field is an
//! enum, so the lock never freezes one.

/// One keyword as the source gave it. A figure the source has nothing for
/// is `None`, never a zero.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct KeywordRow {
    pub keyword: String,
    /// Searches a month.
    pub volume: Option<u64>,
    /// What advertisers pay for a click on average, in US cents.
    pub cpc_usd_cents: Option<u64>,
    /// How hard the first page is to reach, 0 (easy) to 100.
    pub difficulty: Option<u8>,
    /// What the searcher is after, one of [`INTENTS`](crate::INTENTS).
    pub intent: Option<String>,
}

/// A niche study. Its id is a fresh [`new_study_id`](crate::new_study_id),
/// and it lives in the platform's tenant: it exists before any shop does.
#[evento::aggregate(name = "timada-research/Study")]
pub enum Study {
    /// Started by `created_by`, who owns it. Its seeds follow in the same
    /// write, one `SeedAdded` each.
    Created {
        name: String,
        /// ISO 3166-1 alpha-2, uppercase: where the keywords are searched.
        country: String,
        /// Lowercase (`fr`): the language they are searched in.
        language: String,
        created_by: String,
    },
    Renamed {
        name: String,
        renamed_by: String,
    },
    /// One more keyword the study starts from, lowercased.
    SeedAdded {
        seed: String,
        added_by: String,
    },
    /// The owner asked for the keywords of `seed`; the host fetches them.
    FetchRequested {
        seed: String,
        requested_by: String,
    },
    /// One node took the fetch on, so no other pays for a second call.
    FetchStarted {
        seed: String,
        started_by: String,
    },
    /// What the source gave for `seed`, replacing what an earlier fetch of
    /// it gave: [`MAX_FETCH_ROWS`](crate::MAX_FETCH_ROWS) rows at most, the
    /// seed's own first when the source knows it, then the most searched.
    KeywordsFetched {
        seed: String,
        rows: Vec<KeywordRow>,
        /// What the source charged for the call, in millionths of a US
        /// dollar.
        cost_usd_micros: u64,
        fetched_by: String,
    },
    /// The seed was fetched not long ago, for this study or another of the
    /// same country and language: its rows are that fetch's, and nothing
    /// was paid. They replace what an earlier fetch of the seed gave.
    KeywordsReused {
        seed: String,
        rows: Vec<KeywordRow>,
        /// The study whose fetch gave the rows; this one's own id when the
        /// seed is fetched again too soon.
        from_study: String,
        /// Unix seconds of that fetch: how old the rows are.
        fetched_at: u64,
        reused_by: String,
    },
    /// The source gave nothing; `reason` says why, for the owner. The rows
    /// of an earlier fetch stay.
    FetchFailed {
        seed: String,
        reason: String,
        failed_by: String,
    },
    /// Gone for its owner; the stream stays.
    Deleted {
        deleted_by: String,
    },
}

/// What the keyword source was paid on one day (UTC), over every user. Its
/// id is the day ([`spend_day`](crate::spend_day), `2026-11-23`), and it
/// lives in the platform's tenant. Every paid call goes through it, on
/// whichever node makes it, so the day's caps hold over a cluster.
#[evento::aggregate(name = "timada-research/SpendDay")]
pub enum SpendDay {
    /// Room kept for a call about to be made for `user_id`: the most it may
    /// cost counts against the caps until the call is charged or let go.
    Held {
        /// A fresh id, which the charge or the release names.
        hold: String,
        user_id: String,
        study_id: String,
        seed: String,
        most_usd_micros: u64,
        held_by: String,
    },
    /// The call was made: what the source says it cost, in millionths of a
    /// US dollar, takes the place of what was held for it.
    Charged {
        hold: String,
        user_id: String,
        study_id: String,
        seed: String,
        cost_usd_micros: u64,
        charged_by: String,
    },
    /// The call gave nothing and cost nothing: its room is free again.
    Released { hold: String, released_by: String },
}
