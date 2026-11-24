//! The vocabulary of a draft: what it may be about, its limits, where it
//! stands, and the day's caps.

/// A draft of a product's texts.
pub const KIND_PRODUCT: &str = "product";
/// A draft of a category's texts.
pub const KIND_CATEGORY: &str = "category";
pub const KINDS: [&str; 2] = [KIND_PRODUCT, KIND_CATEGORY];

pub const MAX_KEYWORD_LEN: usize = 120;
/// The tone, and what never to claim: each at most this long.
pub const MAX_BRIEF_LEN: usize = 1_000;
pub const MAX_SUBJECT_ID_LEN: usize = 128;
/// The subject's name is a label for the list: cut to this.
pub const MAX_SUBJECT_NAME_LEN: usize = 120;
pub const MAX_REASON_LEN: usize = 500;
/// How many things a draft is marked to check for at most, and how long
/// each reads.
pub const MAX_TO_CHECK: usize = 20;
pub const MAX_TO_CHECK_LEN: usize = 60;

/// How long a request a node took on may stay without an outcome before it
/// counts as lost: the node stopped. Also how long room held for its call
/// counts against the caps. Well over what a call takes.
pub const DRAFT_STALE_SECS: u64 = 10 * 60;

/// The most holds a day takes: an evento stream version is 16 bits, and a
/// call leaves two events.
pub const MAX_DAY_HOLDS: u32 = 20_000;

/// Who the host's own writes say made them, and what prepared the
/// proposals a draft becomes.
pub const WRITER_ACTOR: &str = "timada-writer";
/// What a draft's proposal says prepared it.
pub const WRITER_SOURCE: &str = "writer";

/// Where a draft stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum DraftStatus {
    /// Asked for, and no node took it on yet.
    #[default]
    Requested,
    /// A node is asking the model.
    Started,
    /// Its texts wait as a proposal.
    Drafted,
    /// Nothing was drafted.
    Failed,
}

impl DraftStatus {
    pub const ALL: [Self; 4] = [Self::Requested, Self::Started, Self::Drafted, Self::Failed];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Started => "started",
            Self::Drafted => "drafted",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
    }

    /// Whether it ended, one way or the other.
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Drafted | Self::Failed)
    }
}

/// What the model may be paid in a day (UTC), in millionths of a US
/// dollar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpendCaps {
    /// For one shop.
    pub shop_usd_micros: u64,
    /// For every shop together.
    pub platform_usd_micros: u64,
}

/// Which cap a call would pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendCap {
    Shop,
    Platform,
}

/// The id of a shop's brief ([`Brief`](crate::aggregator::Brief)): one per
/// tenant.
pub fn brief_id() -> String {
    evento::hash_ids(vec!["writer-brief"])
}

/// The UTC day `now` (unix seconds) falls on, as `2026-11-24`.
pub fn spend_day(now: u64) -> String {
    timada_core::civil::format_date(now)
}

/// Unix seconds at which the day of `now` ends and the caps free.
pub fn spend_day_ends(now: u64) -> u64 {
    (now / 86_400 + 1) * 86_400
}

/// The id of the spending of a day
/// ([`SpendDay`](crate::aggregator::SpendDay)).
pub fn spend_day_id(day: &str) -> String {
    evento::hash_ids(vec!["writer-spend", day])
}

pub fn new_draft_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// A text as an event keeps it: trimmed and cut to `max` characters.
pub(crate) fn cut(value: &str, max: usize) -> String {
    value.trim().chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_round_trips() {
        for status in DraftStatus::ALL {
            assert_eq!(DraftStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(DraftStatus::parse("gone"), None);
        assert!(DraftStatus::Failed.is_settled());
        assert!(!DraftStatus::Started.is_settled());
    }

    #[test]
    fn a_day_ends_at_midnight_utc() {
        assert_eq!(spend_day(1_700_000_000), "2023-11-14");
        assert_eq!(spend_day_ends(1_700_000_000), 1_700_006_400);
        assert_eq!(spend_day(1_700_006_400), "2023-11-15");
        assert_ne!(spend_day_id("2023-11-14"), spend_day_id("2023-11-15"));
        assert_ne!(brief_id(), "writer-brief");
    }
}
