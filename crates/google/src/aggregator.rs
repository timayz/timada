//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail. No field is an
//! enum, so the lock never freezes one.

/// A shop's connection to a Google account. Its id is made of the shop's
/// ([`connection_id`](crate::connection_id)), and it lives in the platform's
/// tenant, out of reach of the shop's storefront.
#[evento::aggregate(name = "timada-google/Connection")]
pub enum Connection {
    /// A Google account said yes to the platform's application. It replaces
    /// the grant held before, whole: a wider one asked for later covers
    /// what the earlier one did.
    Connected {
        /// The shop: the stream's id does not give it back.
        tenant_id: String,
        /// What renews the access, sealed by the host for this shop. The
        /// access itself lasts an hour and is never recorded.
        refresh_token_sealed: Vec<u8>,
        /// The account's e-mail address, for the merchant to tell which one
        /// it is.
        account: String,
        /// The kinds of access the account granted, each one of
        /// [`ACCESSES`](crate::ACCESSES).
        accesses: Vec<String>,
        connected_by: String,
    },
    /// Which of the account's Search Console sites is this shop's, as
    /// Search Console names it (`https://shop.example/` or
    /// `sc-domain:shop.example`).
    SiteChosen { site: String, chosen_by: String },
    /// Google no longer honours the grant (withdrawn by the account, or
    /// left unused too long): nothing is read through it until the merchant
    /// connects again.
    Lapsed { noticed_by: String },
    /// The merchant took the connection off: the grant is forgotten.
    Disconnected { disconnected_by: String },
}

/// One query or one page of a site on a day, as Search Console gave it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct SearchRow {
    /// The query as typed, or the page's address.
    pub key: String,
    pub clicks: u64,
    pub impressions: u64,
    /// The average position, in thousandths: 3,400 is position 3.4.
    pub position_milli: u32,
}

/// One query a page of a site showed for on a day, as Search Console gave
/// it: which page answers which query, which neither list alone tells.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct PageQueryRow {
    /// The page's address.
    pub page: String,
    /// The query as typed.
    pub query: String,
    pub clicks: u64,
    pub impressions: u64,
    /// The average position, in thousandths: 3,400 is position 3.4.
    pub position_milli: u32,
}

/// One finished day of a shop's site in Search Console. Its id is made of
/// the shop, the site and the day ([`search_day_id`](crate::search_day_id)):
/// Google does not change a day it has finished counting, so the stream
/// holds this one event. It lives in the platform's tenant.
#[evento::aggregate(name = "timada-google/SearchDay")]
pub enum SearchDay {
    DayFetched {
        tenant_id: String,
        site: String,
        /// `2026-11-20`, a day of Google's (Pacific time).
        day: String,
        /// The site's own numbers over the day.
        clicks: u64,
        impressions: u64,
        position_milli: u32,
        /// The most clicked queries, [`MAX_SEARCH_ROWS`](crate::MAX_SEARCH_ROWS)
        /// at most. They add up to less than the site's numbers: Google
        /// withholds the queries too few people typed.
        queries: Vec<SearchRow>,
        /// The most clicked pages, as many at most. They may add up to more
        /// than the site's numbers: a search that shows two pages counts
        /// once for the site and once for each.
        pages: Vec<SearchRow>,
        /// The queries each page showed for, the most shown first,
        /// [`MAX_PAGE_QUERY_ROWS`](crate::MAX_PAGE_QUERY_ROWS) at most. A
        /// page shown on the second page of results has few clicks or
        /// none, so these are kept by impressions.
        page_queries: Vec<PageQueryRow>,
        fetched_by: String,
    },
}

/// Where the daily look for a shop's new days stands. Its id is made of
/// the shop's ([`search_watch_id`](crate::search_watch_id)), and it lives
/// in the platform's tenant. A pass leaves one event, so the stream grows
/// by one or two a day.
#[evento::aggregate(name = "timada-google/SearchWatch")]
pub enum SearchWatch {
    /// A pass over `site` found what there was. `behind`: older days are
    /// left to fetch, so the next pass is due sooner.
    SearchChecked {
        tenant_id: String,
        site: String,
        behind: bool,
        checked_by: String,
    },
    /// A pass over `site` failed; `reason` says why, for the merchant.
    /// `attempts` counts the failures in a row, this one included: the
    /// next pass waits longer after each.
    SearchCheckFailed {
        tenant_id: String,
        site: String,
        reason: String,
        attempts: u32,
        failed_by: String,
    },
}
