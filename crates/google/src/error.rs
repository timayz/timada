use evento::WriteError;

#[derive(Debug, thiserror::Error)]
pub enum GoogleError {
    #[error("the Google account granted none of the access asked for")]
    NothingGranted,
    #[error("no such kind of access")]
    UnknownAccess,
    #[error("the shop is not connected to a Google account")]
    NotConnected,
    #[error("Google no longer honours the connection: connect again")]
    Lapsed,
    #[error("the connection does not cover Search Console")]
    NoSearchConsole,
    #[error("a Search Console site reads https://shop.example/ or sc-domain:shop.example")]
    InvalidSite,
    #[error("a day reads 2026-11-20")]
    InvalidDay,
    #[error("the connection does not cover Google Ads")]
    NoAds,
    #[error("a Google Ads account is a number of ten digits")]
    InvalidAdsAccount,
    #[error("the connection does not cover Merchant Center")]
    NoMerchantCenter,
    #[error("a Merchant Center account is a number")]
    InvalidMerchantAccount,
    #[error("no such campaign")]
    InvalidCampaign,
    #[error("a campaign's name is one to 200 characters")]
    InvalidCampaignName,
    #[error("an amount reads 0.30, in the account's currency")]
    InvalidCampaignAmount,
    #[error("the bid is more than the campaign may spend in a day")]
    BidOverBudget,
    #[error(
        "countries read FR, BE: the two letters of each, among the ones Google Ads is told by here"
    )]
    InvalidCountries,
    #[error("an adjustment is a whole percentage between -90 and 900, or nothing")]
    InvalidAdjustment,
    #[error("a campaign starts paused or live")]
    InvalidStart,
    #[error("an ad group is a line made of a category's id and its name, one to a hundred of them")]
    InvalidAdGroups,
    #[error(
        "a campaign on the shop's name takes one to ten searches, a line each, of 80 characters at most"
    )]
    InvalidKeywords,
    #[error("an ad takes three to fifteen headlines, a line each, of 30 characters at most")]
    InvalidHeadlines,
    #[error("an ad takes two to four descriptions, a line each, of 90 characters at most")]
    InvalidDescriptions,
    #[error("the campaign was created already")]
    CampaignExists,
    #[error("the campaign was not created yet")]
    NoCampaign,
    #[error("the campaign runs already")]
    CampaignLive,
    #[error(
        "a threshold is a whole number within its range, a step or a return an amount like 0.02"
    )]
    InvalidTuning,
    #[error("the campaign does not run yet, or has not had its first three weeks")]
    CampaignSettling,
    #[error("no such ad group, device or search in the campaign")]
    InvalidTuned,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl GoogleError {
    /// Whether the message can be shown to the person who made the request
    /// (as opposed to an infrastructure failure).
    pub fn is_user_facing(&self) -> bool {
        match self {
            // A write the store is not taking right now is told to the person.
            Self::Write(err) => timada_core::write_refusal(err).is_some(),
            Self::Other(_) => false,
            _ => true,
        }
    }

    /// Whether the store failed rather than the request: a pass that meets
    /// it stops and tries again.
    pub fn is_store_fault(&self) -> bool {
        match self {
            Self::Write(err) => timada_core::is_store_write_fault(err),
            Self::Other(_) => true,
            _ => false,
        }
    }

    /// A stable code for a page to carry in a URL; `None` for an
    /// infrastructure failure.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::NothingGranted => "google_nothing_granted",
            Self::UnknownAccess => "google_unknown_access",
            Self::NotConnected => "google_not_connected",
            Self::Lapsed => "google_connect_again",
            Self::NoSearchConsole => "google_no_search_console",
            Self::InvalidSite => "google_invalid_site",
            Self::InvalidDay => "google_invalid_day",
            Self::NoAds => "google_no_ads",
            Self::InvalidAdsAccount => "google_invalid_ads_account",
            Self::NoMerchantCenter => "google_no_merchant_center",
            Self::InvalidMerchantAccount => "google_invalid_merchant_account",
            Self::InvalidCampaign => "google_invalid_campaign",
            Self::InvalidCampaignName => "google_invalid_campaign_name",
            Self::InvalidCampaignAmount => "google_invalid_campaign_amount",
            Self::BidOverBudget => "google_bid_over_budget",
            Self::InvalidCountries => "google_invalid_countries",
            Self::InvalidAdjustment => "google_invalid_adjustment",
            Self::InvalidStart => "google_invalid_start",
            Self::InvalidAdGroups => "google_invalid_ad_groups",
            Self::InvalidKeywords => "google_invalid_keywords",
            Self::InvalidHeadlines => "google_invalid_headlines",
            Self::InvalidDescriptions => "google_invalid_descriptions",
            Self::CampaignExists => "google_campaign_exists",
            Self::NoCampaign => "google_no_campaign",
            Self::CampaignLive => "google_campaign_live",
            Self::InvalidTuning => "google_invalid_tuning",
            Self::CampaignSettling => "google_campaign_settling",
            Self::InvalidTuned => "google_invalid_tuned",
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}
