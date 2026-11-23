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
