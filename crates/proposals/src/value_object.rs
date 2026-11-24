//! The vocabulary of a proposal: its kinds, their fields, its limits and
//! where it stands.

use crate::error::ProposalError;

/// A change to a product: its texts, what search engines show, a price.
pub const KIND_PRODUCT: &str = "product";
/// A change to a category: its texts and what search engines show.
pub const KIND_CATEGORY: &str = "category";
/// A category to create as a draft, under the proposal's subject (the
/// category above it; none for a top-level one).
pub const KIND_NEW_CATEGORY: &str = "new_category";
/// A bid of a campaign, an ad group or a product group.
pub const KIND_ADS_BID: &str = "ads_bid";
/// A product kept out of a campaign.
pub const KIND_ADS_EXCLUSION: &str = "ads_exclusion";
/// A search term a campaign is no longer to show for.
pub const KIND_ADS_NEGATIVE_TERM: &str = "ads_negative_term";

/// Every kind a proposal can be of.
pub const KINDS: [&str; 6] = [
    KIND_PRODUCT,
    KIND_CATEGORY,
    KIND_NEW_CATEGORY,
    KIND_ADS_BID,
    KIND_ADS_EXCLUSION,
    KIND_ADS_NEGATIVE_TERM,
];

pub const FIELD_NAME: &str = "name";
pub const FIELD_DESCRIPTION: &str = "description";
/// The text below a category's products.
pub const FIELD_BODY: &str = "body";
pub const FIELD_META_TITLE: &str = "meta_title";
pub const FIELD_META_DESCRIPTION: &str = "meta_description";
/// The price of a product sold as is, as a person types it (`19.90`), in
/// the product's currency.
pub const FIELD_PRICE: &str = "price";
/// What the field of one variant's price starts with; the variant's id
/// follows ([`variant_price_field`]).
pub const VARIANT_PRICE_PREFIX: &str = "price:";

/// The fields a proposal about a product may change, a variant's price
/// aside.
pub const PRODUCT_FIELDS: [&str; 5] = [
    FIELD_NAME,
    FIELD_DESCRIPTION,
    FIELD_META_TITLE,
    FIELD_META_DESCRIPTION,
    FIELD_PRICE,
];
/// The fields a proposal about a category may change.
pub const CATEGORY_FIELDS: [&str; 5] = [
    FIELD_NAME,
    FIELD_DESCRIPTION,
    FIELD_BODY,
    FIELD_META_TITLE,
    FIELD_META_DESCRIPTION,
];
/// What a proposal to create a category gives of it.
pub const NEW_CATEGORY_FIELDS: [&str; 2] = [FIELD_NAME, FIELD_META_TITLE];

/// The field of the price of the variant `variant_id`.
pub fn variant_price_field(variant_id: &str) -> String {
    format!("{VARIANT_PRICE_PREFIX}{variant_id}")
}

/// The variant a field prices, when it is one's price.
pub fn priced_variant(field: &str) -> Option<&str> {
    field
        .strip_prefix(VARIANT_PRICE_PREFIX)
        .filter(|id| !id.is_empty())
}

/// Whether a proposal of `kind` may change `field`. The fields of the ads
/// kinds are the host's to name: any short name of lowercase letters,
/// digits, `_` and `:` is one.
pub fn is_field_of(kind: &str, field: &str) -> bool {
    match kind {
        KIND_PRODUCT => PRODUCT_FIELDS.contains(&field) || priced_variant(field).is_some(),
        KIND_CATEGORY => CATEGORY_FIELDS.contains(&field),
        KIND_NEW_CATEGORY => NEW_CATEGORY_FIELDS.contains(&field),
        KIND_ADS_BID | KIND_ADS_EXCLUSION | KIND_ADS_NEGATIVE_TERM => {
            !field.is_empty()
                && field.len() <= MAX_FIELD_LEN
                && field
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == ':')
        }
        _ => false,
    }
}

/// What prepared a proposal a member wrote by hand.
pub const SOURCE_PERSON: &str = "person";

/// The most fields one proposal changes.
pub const MAX_CHANGES: usize = 8;
pub const MAX_FIELD_LEN: usize = 80;
/// The longest value a field takes: a category's body is the longest text
/// a proposal carries.
pub const MAX_VALUE_LEN: usize = 20_000;
pub const MAX_REASON_LEN: usize = 1_000;
pub const MAX_SOURCE_LEN: usize = 40;
pub const MAX_SUBJECT_ID_LEN: usize = 128;
/// The subject's name is a label for the list: cut to this.
pub const MAX_SUBJECT_NAME_LEN: usize = 120;
/// What ending a proposal may be explained with.
pub const MAX_NOTE_LEN: usize = 500;

/// How long a proposal is worth reading when whoever prepared it gave no
/// end: thirty days.
pub const DEFAULT_VALID_SECS: u64 = 30 * 24 * 60 * 60;
/// The longest a proposal may wait: ninety days.
pub const MAX_VALID_SECS: u64 = 90 * 24 * 60 * 60;

/// How long the notice that proposals wait is worth sending: a day, the
/// day it is about.
pub const NOTICE_WINDOW_SECS: u64 = 24 * 60 * 60;

/// Who the host's own writes say made them.
pub const PROPOSALS_ACTOR: &str = "timada-proposals";

/// Where a proposal stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ProposalStatus {
    /// Prepared, and nobody answered yet.
    #[default]
    Waiting,
    /// A member said yes; the change is being applied.
    Approved,
    /// The change was made.
    Applied,
    /// A member said no.
    Refused,
    /// The target changed since the proposal was prepared: not applied.
    OutOfDate,
    /// Approved, and the shop refused the change (a name too long, a
    /// product archived meanwhile).
    Failed,
}

impl ProposalStatus {
    pub const ALL: [Self; 6] = [
        Self::Waiting,
        Self::Approved,
        Self::Applied,
        Self::Refused,
        Self::OutOfDate,
        Self::Failed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Approved => "approved",
            Self::Applied => "applied",
            Self::Refused => "refused",
            Self::OutOfDate => "out_of_date",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
    }
}

pub fn new_proposal_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// The id of the category a proposal to create one makes: the proposal's
/// own, so applying it twice makes one category. A tenant's stream ids are
/// one space whatever the aggregate, so it is not the proposal's id itself.
pub fn proposed_category_id(proposal_id: &str) -> String {
    evento::hash_ids(vec!["proposed-category", proposal_id])
}

/// The UTC day `now` (unix seconds) falls on, as `2026-11-24`: what a
/// shop's notice of waiting proposals is keyed by, so the members are told
/// once a day at most.
pub fn notice_day(now: u64) -> String {
    timada_core::civil::format_date(now)
}

/// The id of the notice of a day ([`Notice`](crate::aggregator::Notice)).
pub fn notice_id(day: &str) -> String {
    evento::hash_ids(vec!["proposal-notice", day])
}

/// A short text: trimmed, at most `max` characters.
pub(crate) fn checked(
    value: &str,
    max: usize,
    err: ProposalError,
) -> Result<String, ProposalError> {
    let value = value.trim();
    if value.chars().count() > max {
        return Err(err);
    }
    Ok(value.to_owned())
}

/// A note as an event keeps it: trimmed and cut to [`MAX_NOTE_LEN`].
pub(crate) fn note(value: &str) -> String {
    value.trim().chars().take(MAX_NOTE_LEN).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_names_its_fields() {
        assert!(is_field_of(KIND_PRODUCT, FIELD_PRICE));
        assert!(is_field_of(KIND_PRODUCT, &variant_price_field("v1")));
        assert!(!is_field_of(KIND_PRODUCT, VARIANT_PRICE_PREFIX));
        assert!(!is_field_of(KIND_PRODUCT, FIELD_BODY));
        assert!(is_field_of(KIND_CATEGORY, FIELD_BODY));
        assert!(!is_field_of(KIND_CATEGORY, FIELD_PRICE));
        assert!(is_field_of(KIND_NEW_CATEGORY, FIELD_NAME));
        assert!(!is_field_of(KIND_NEW_CATEGORY, FIELD_DESCRIPTION));
        assert!(is_field_of(KIND_ADS_BID, "max_cpc"));
        assert!(!is_field_of(KIND_ADS_BID, "Max CPC"));
        assert!(!is_field_of("order", FIELD_NAME));
        assert_eq!(priced_variant("price:v1"), Some("v1"));
        assert_eq!(priced_variant("price"), None);
    }

    #[test]
    fn a_status_round_trips() {
        for status in ProposalStatus::ALL {
            assert_eq!(ProposalStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(ProposalStatus::parse("gone"), None);
    }

    #[test]
    fn a_notice_is_keyed_by_the_day() {
        assert_eq!(notice_day(1_700_000_000), "2023-11-14");
        assert_ne!(notice_id("2023-11-14"), notice_id("2023-11-15"));
        assert_ne!(proposed_category_id("a"), "a");
    }
}
