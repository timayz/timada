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
    /// Which of the Google Ads accounts the grant reaches is this shop's.
    AdsAccountChosen {
        /// The account's number, digits only (`1234567890`).
        customer_id: String,
        /// The manager account it is reached through, digits only; `None`
        /// for an account the grant reaches by itself.
        login_customer_id: Option<String>,
        /// What Google Ads calls the account, for the merchant to tell
        /// which one it is.
        name: String,
        /// The currency the account is billed and reports in (`EUR`).
        currency: String,
        /// The time zone its days are counted in (`Europe/Paris`).
        time_zone: String,
        chosen_by: String,
    },
    /// Which of the Merchant Center accounts the grant reaches is this
    /// shop's.
    MerchantAccountChosen {
        /// The account's number, digits only.
        account_id: String,
        /// What Merchant Center calls the account.
        name: String,
        chosen_by: String,
    },
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

/// What ads cost and brought, as Google Ads counts it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct AdsNumbers {
    /// What the clicks cost, in millionths of the account's currency.
    pub cost_micros: u64,
    pub impressions: u64,
    pub clicks: u64,
    /// The conversions, in thousandths: Google shares one between the
    /// clicks that led to it, so 1,500 is a conversion and a half.
    pub conversions_milli: u64,
    /// What the conversions were worth, in millionths of the account's
    /// currency.
    pub value_micros: u64,
}

/// One campaign on one kind of device on a day.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct AdsCampaignRow {
    pub campaign_id: String,
    pub name: String,
    /// One of [`ADS_DEVICES`](crate::ADS_DEVICES).
    pub device: String,
    pub numbers: AdsNumbers,
}

/// One ad group on a day.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct AdsAdGroupRow {
    pub campaign_id: String,
    pub ad_group_id: String,
    pub name: String,
    pub numbers: AdsNumbers,
}

/// One product of the feed in one ad group on a day.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct AdsProductRow {
    /// The feed's item id as Google Ads reports it: in lower case.
    pub item_id: String,
    pub campaign_id: String,
    pub ad_group_id: String,
    pub numbers: AdsNumbers,
}

/// One search an ad was clicked for, in one ad group on a day.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct AdsTermRow {
    /// What the person typed.
    pub term: String,
    pub campaign_id: String,
    pub ad_group_id: String,
    pub numbers: AdsNumbers,
}

/// One day of a shop's Google Ads account. Its id is made of the shop, the
/// account and the day ([`ads_day_id`](crate::ads_day_id)). Google goes on
/// counting a day after it ends (a conversion is counted on the day of the
/// click that led to it, and may come weeks later), so a day is read again
/// for a while and the stream gets an event each time its numbers moved:
/// the last one stands. It lives in the platform's tenant.
#[evento::aggregate(name = "timada-google/AdsDay")]
pub enum AdsDay {
    AdsDayFetched {
        tenant_id: String,
        customer_id: String,
        /// `2026-11-20`, a day of the account's time zone.
        day: String,
        /// The account's currency (`EUR`).
        currency: String,
        /// Every campaign that was shown, by device: what the day's totals
        /// are added up from.
        campaigns: Vec<AdsCampaignRow>,
        ad_groups: Vec<AdsAdGroupRow>,
        /// The products shown, the ones that cost the most first,
        /// [`MAX_ADS_PRODUCT_ROWS`](crate::MAX_ADS_PRODUCT_ROWS) at most.
        products: Vec<AdsProductRow>,
        /// The searches that brought a click, the ones that cost the most
        /// first, [`MAX_ADS_TERM_ROWS`](crate::MAX_ADS_TERM_ROWS) at most.
        terms: Vec<AdsTermRow>,
        fetched_by: String,
    },
}

/// Where the daily read of a shop's Google Ads account stands. Its id is
/// made of the shop's ([`ads_watch_id`](crate::ads_watch_id)), and it lives
/// in the platform's tenant. A pass leaves one event.
#[evento::aggregate(name = "timada-google/AdsWatch")]
pub enum AdsWatch {
    /// A pass over the account read the days from `first` to `last`.
    /// `behind`: older days are left to read, so the next pass is due
    /// sooner. `calls`: how many questions it asked Google Ads, which the
    /// platform's daily allowance is counted in.
    AdsChecked {
        tenant_id: String,
        customer_id: String,
        first: String,
        last: String,
        behind: bool,
        calls: u32,
        checked_by: String,
    },
    /// A pass over the account failed; `reason` says why, for the merchant.
    /// `attempts` counts the failures in a row, this one included: the
    /// next pass waits longer after each.
    AdsCheckFailed {
        tenant_id: String,
        customer_id: String,
        reason: String,
        attempts: u32,
        calls: u32,
        failed_by: String,
    },
}

/// What Merchant Center holds against one product of the feed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct MerchantIssue {
    /// Google's own name for the issue (`image_too_small`).
    pub code: String,
    /// What it does to the product, one of
    /// [`MERCHANT_SEVERITIES`](crate::MERCHANT_SEVERITIES).
    pub severity: String,
    /// The field of the feed it names (`image_link`); empty when it names
    /// none.
    pub attribute: String,
    /// Where it applies, one of
    /// [`MERCHANT_CONTEXTS`](crate::MERCHANT_CONTEXTS); empty for both.
    pub context: String,
    /// What is wrong, then what to do about it, in Google's words.
    pub description: String,
    pub detail: String,
    /// The page of Google's help that explains it.
    pub documentation: String,
}

/// One product of the feed as Merchant Center has it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct MerchantProductRow {
    /// The feed's item id, as Merchant Center keeps it.
    pub item_id: String,
    /// Where it stands for Shopping ads, one of
    /// [`MERCHANT_STATUSES`](crate::MERCHANT_STATUSES).
    pub ads: String,
    /// The same for the free listings.
    pub listings: String,
    pub issues: Vec<MerchantIssue>,
}

/// Something Merchant Center holds against the account itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct MerchantAccountIssue {
    pub title: String,
    /// One of
    /// [`MERCHANT_ACCOUNT_SEVERITIES`](crate::MERCHANT_ACCOUNT_SEVERITIES).
    pub severity: String,
    pub detail: String,
    pub documentation: String,
}

/// What Merchant Center says of a shop's products. Its id is made of the
/// shop and the account ([`merchant_feed_id`](crate::merchant_feed_id)),
/// and it lives in the platform's tenant. It is read every day and gets an
/// event when something moved: the last one stands.
#[evento::aggregate(name = "timada-google/MerchantFeed")]
pub enum MerchantFeed {
    MerchantRead {
        tenant_id: String,
        account_id: String,
        /// The products, the ones Google does not accept first,
        /// [`MAX_MERCHANT_PRODUCT_ROWS`](crate::MAX_MERCHANT_PRODUCT_ROWS)
        /// at most.
        products: Vec<MerchantProductRow>,
        /// How many accepted products were left out past that.
        more: u32,
        account_issues: Vec<MerchantAccountIssue>,
        /// The items refused now that were not at the read before: what
        /// the shop's owners are told. Empty for the first read.
        newly_refused: Vec<String>,
        /// The titles of the account's issues that the read before did not
        /// have.
        new_account_issues: Vec<String>,
        read_by: String,
    },
}

/// Where the daily read of a shop's Merchant Center account stands. Its id
/// is made of the shop's ([`merchant_watch_id`](crate::merchant_watch_id)),
/// and it lives in the platform's tenant. A pass leaves one event.
#[evento::aggregate(name = "timada-google/MerchantWatch")]
pub enum MerchantWatch {
    /// A pass over the account read its `products` products.
    MerchantChecked {
        tenant_id: String,
        account_id: String,
        products: u32,
        checked_by: String,
    },
    /// A pass over the account failed; `reason` says why, for the merchant.
    /// `attempts` counts the failures in a row, this one included.
    MerchantCheckFailed {
        tenant_id: String,
        account_id: String,
        reason: String,
        attempts: u32,
        failed_by: String,
    },
}

/// The notice, to a shop's owners, that Merchant Center turned against
/// something on a day: a product refused that was not, a new issue on the
/// account. Its id is made of the shop and the day
/// ([`merchant_notice_id`](crate::merchant_notice_id)), so the owners are
/// told once a day at most, and it lives in the platform's tenant. It is
/// mailed by a subscription on every node of the work group: the node that
/// writes the claim first is the one that mails, then records how it went
/// ([`timada_core::Sending`]).
#[evento::aggregate(name = "timada-google/MerchantNotice")]
pub enum MerchantNotice {
    /// What the stream is about, written with the first claim: the notice
    /// of `day` (`2026-11-24`) for the shop, worth sending until
    /// `expires_at` (unix seconds).
    Owed {
        tenant_id: String,
        day: String,
        expires_at: u64,
    },
    /// Node `by` took the sending, until `until` should it never say how it
    /// went.
    MailClaimed { by: String, until: u64 },
    /// The notice was handed to the mail server, to every owner.
    MailSent,
    /// The mail server refused every owner's address for good.
    MailUndeliverable { reason: String },
    /// A send failed for a reason worth a retry, after `attempts` tries in
    /// all; the next may be made at `retry_at` (unix seconds).
    MailDeferred {
        retry_at: u64,
        attempts: u32,
        reason: String,
    },
    /// Given up on after `attempts` tries.
    MailFailed { reason: String, attempts: u32 },
}

/// One setting of a campaign, as the proposal that asked for it named and
/// wrote it (`bid`, `0.30`): the fields are
/// [`CAMPAIGN_FIELDS`](crate::CAMPAIGN_FIELDS).
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct CampaignSetting {
    pub field: String,
    pub value: String,
}

/// One ad group of a Shopping campaign: the products of one category.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct CampaignAdGroup {
    /// Google Ads' own number for the ad group.
    pub ad_group_id: String,
    /// The shop's category whose products it shows; empty for the ad
    /// group of a campaign on the shop's name.
    pub category_id: String,
    pub name: String,
}

/// A campaign timada created in a shop's Google Ads account once a member
/// approved it. Its id is made of the shop and the kind of campaign
/// ([`campaign_stream_id`](crate::campaign_stream_id)), so a shop has one of
/// each kind, and it lives in the platform's tenant.
#[evento::aggregate(name = "timada-google/Campaign")]
pub enum Campaign {
    /// Google Ads took the campaign.
    CampaignCreated {
        tenant_id: String,
        /// The Google Ads account it was created in.
        customer_id: String,
        /// One of [`CAMPAIGN_KINDS`](crate::CAMPAIGN_KINDS).
        kind: String,
        /// Google Ads' own number for the campaign.
        campaign_id: String,
        name: String,
        /// What was approved, field by field.
        settings: Vec<CampaignSetting>,
        ad_groups: Vec<CampaignAdGroup>,
        /// Whether it was created running; paused otherwise.
        live: bool,
        /// The proposal a member approved it through.
        proposal_id: String,
        created_by: String,
    },
    /// A member asked for a paused campaign to run: the host tells Google
    /// Ads.
    LiveRequested { requested_by: String },
    /// The campaign runs since `at` (unix seconds): set running by the
    /// host, or seen showing ads in the daily read after a member set it
    /// running at Google Ads.
    WentLive { at: u64, noticed_by: String },
    /// Google Ads refused to set it running; `reason` says why, for the
    /// member.
    LiveFailed { reason: String, failed_by: String },
    /// The most a click of one ad group is paid was changed at Google Ads,
    /// through a proposal a member approved.
    BidSet {
        ad_group_id: String,
        /// In the account's currency, as a person writes it (`0.32`).
        bid: String,
        proposal_id: String,
        set_by: String,
    },
    /// How much more or less a click on a kind of device is paid was
    /// changed at Google Ads, through a proposal a member approved.
    AdjustmentSet {
        /// One of [`TUNED_DEVICES`](crate::TUNED_DEVICES).
        device: String,
        /// In hundredths (`-55`); empty for none.
        percent: String,
        proposal_id: String,
        set_by: String,
    },
    /// The campaign no longer shows for a search, through a proposal a
    /// member approved.
    TermExcluded {
        term: String,
        proposal_id: String,
        excluded_by: String,
    },
}

/// How a shop's campaign is tuned: the thresholds its rounds decide on,
/// and where the rounds stand. Its id is made of the shop's
/// ([`tuning_id`](crate::tuning_id)), and it lives in the platform's
/// tenant. A round reads the last weeks of the campaign and prepares
/// proposals; nothing is changed at Google Ads until a member approves.
#[evento::aggregate(name = "timada-google/Tuning")]
pub enum Tuning {
    /// The thresholds, each named as
    /// [`TUNING_FIELDS`](crate::TUNING_FIELDS) names it.
    TuningSet {
        tenant_id: String,
        settings: Vec<CampaignSetting>,
        set_by: String,
    },
    /// A round was asked for, by a member or because one was due: the host
    /// reads the numbers and prepares the proposals.
    RoundRequested {
        tenant_id: String,
        requested_by: String,
    },
    /// A round read the days from `first` to `last` and prepared
    /// `proposals` (their ids). `notes` say what it looked at and left
    /// alone, and why; `flags` what a person should look at and no
    /// proposal changes (a product that sells below its break-even).
    RoundPrepared {
        tenant_id: String,
        first: String,
        last: String,
        proposals: Vec<String>,
        notes: Vec<String>,
        flags: Vec<String>,
        prepared_by: String,
    },
    /// A round could not be prepared; `reason` says why, for the member.
    RoundFailed {
        tenant_id: String,
        reason: String,
        failed_by: String,
    },
}
