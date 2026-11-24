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
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}
