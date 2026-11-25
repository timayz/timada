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

/// One of the first organic results of a search, as the source gave it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct ResultRow {
    /// Its place among the organic results, from 1.
    pub position: u8,
    /// The site, lowercase and without `www.`.
    pub domain: String,
    pub url: String,
    pub title: String,
}

/// One category of a study's tree as it is to be created in a shop.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct PlannedCategory {
    /// The keyword of the tree it is made of.
    pub keyword: String,
    /// The id the category goes by in the shop, chosen with the request so
    /// asking twice creates it once.
    pub category_id: String,
    /// The category above it in the shop, by id; `None` for a parent
    /// category.
    pub parent_id: Option<String>,
    pub name: String,
    pub slug: String,
    /// The title search engines show: it starts with the keyword.
    pub meta_title: String,
    /// Campaigns are to leave its products out.
    pub out_of_ads: bool,
    /// Where it sorts among its siblings, lowest first.
    pub position: u32,
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
    /// What the owner says of the niche, whole: one typical sale, the words
    /// that drop a keyword, and what they looked up by hand. It replaces
    /// what an earlier one said. The study is scored from it and from its
    /// keywords, without a fetch.
    InputsSet {
        /// ISO 4217, uppercase: the currency of the sale's amounts.
        currency: String,
        /// The average price a shopper would pay, VAT included; `None`
        /// until said.
        price_cents: Option<u64>,
        /// What the supplier charges; `None` until said.
        cost_cents: Option<u64>,
        shipping_cents: u64,
        import_cents: u64,
        /// The VAT rate in the price, in basis points.
        vat_bps: u32,
        fixed_cents: u64,
        payment_fee_bps: u32,
        payment_fee_cents: u64,
        /// Brands whose keywords are dropped.
        brands: Vec<String>,
        /// Words that drop the keywords that hold them.
        excluded_words: Vec<String>,
        /// How many good products the owner found at the supplier; `None`
        /// until they looked.
        supplier_products: Option<u32>,
        /// What one unit of `currency` was worth in millionths of a US
        /// dollar when this was said, to read a cost per click in it;
        /// `None` when no rate was known, or none is needed.
        usd_micros: Option<u64>,
        set_by: String,
    },
    /// The owner dropped `keyword`, as its row spells it, whatever the
    /// filters say; it is no longer one they brought back.
    KeywordDropped {
        keyword: String,
        dropped_by: String,
    },
    /// The owner brought `keyword` back: kept whatever the filters say, and
    /// no longer one they dropped.
    KeywordRestored {
        keyword: String,
        restored_by: String,
    },
    /// The owner asked who holds the first results of a search for
    /// `keyword`, one of the study's main keywords; the host fetches them.
    ResultsRequested {
        keyword: String,
        requested_by: String,
    },
    /// One node took the search on, so no other pays for a second call.
    ResultsStarted {
        keyword: String,
        started_by: String,
    },
    /// The first organic results of a search for `keyword` in the study's
    /// country and language, replacing what an earlier search of it gave:
    /// [`MAX_RESULTS`](crate::MAX_RESULTS) at most, in their order.
    ResultsFetched {
        keyword: String,
        results: Vec<ResultRow>,
        /// What the source charged for the call, in millionths of a US
        /// dollar.
        cost_usd_micros: u64,
        fetched_by: String,
    },
    /// The source gave nothing; `reason` says why, for the owner. The
    /// results of an earlier search stay.
    ResultsFailed {
        keyword: String,
        reason: String,
        failed_by: String,
    },
    /// The owner put `keyword`, one the study keeps, in its category tree:
    /// as a parent category with no `parent`, or under the parent category
    /// `parent` names by its keyword. It replaces where the keyword stood,
    /// in the tree or parked.
    CategoryPlaced {
        keyword: String,
        parent: Option<String>,
        placed_by: String,
    },
    /// The owner parked `keyword` for later: out of the tree, and kept in
    /// sight.
    CategoryParked {
        keyword: String,
        parked_by: String,
    },
    /// The owner refused `keyword` as a category: out of the tree and of
    /// the parked ones, and out of what is left to choose, so it is not
    /// read again. The score keeps it all the same.
    CategoryRefused {
        keyword: String,
        refused_by: String,
    },
    /// `keyword` is out of the tree, no longer parked and no longer
    /// refused.
    CategoryRemoved {
        keyword: String,
        removed_by: String,
    },
    /// The average price of the category of `keyword`, VAT included, in
    /// the currency of the study's sale, when it is not the study's;
    /// `None` goes back to the study's.
    CategoryPriced {
        keyword: String,
        price_cents: Option<u64>,
        priced_by: String,
    },
    /// The niche's own words, the list whole: a category's name holds one
    /// of them at least. Empty when the owner wants no name checked.
    NicheWordsSet {
        words: Vec<String>,
        set_by: String,
    },
    /// The owner asked for the tree to be created as draft categories in
    /// the shop `tenant_id`: one they may manage the catalog of, or with
    /// `shop_name` a shop to create first under that id, theirs. The host
    /// creates what `categories` lists, parents before their children.
    CreationRequested {
        /// A fresh id, which what follows names.
        request: String,
        tenant_id: String,
        shop_name: Option<String>,
        categories: Vec<PlannedCategory>,
        requested_by: String,
    },
    /// The study's shop: the one its categories are created in, from then
    /// on.
    ShopLinked {
        tenant_id: String,
        linked_by: String,
    },
    /// The category of `keyword` exists in the study's shop, as a draft.
    CategoryCreated {
        request: String,
        keyword: String,
        category_id: String,
        slug: String,
        created_by: String,
    },
    /// The shop refused the category of `keyword`; `reason` says why, for
    /// the owner. It is left to create.
    CategoryNotCreated {
        request: String,
        keyword: String,
        reason: String,
        failed_by: String,
    },
    /// Nothing of the request could be created: no such shop, or one its
    /// asker may not manage. `reason` says why, for the owner.
    CreationFailed {
        request: String,
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
        /// What the source is asked about: a seed for its keywords, or a
        /// keyword for its first results.
        query: String,
        most_usd_micros: u64,
        held_by: String,
    },
    /// The call was made: what the source says it cost, in millionths of a
    /// US dollar, takes the place of what was held for it.
    Charged {
        hold: String,
        user_id: String,
        study_id: String,
        query: String,
        cost_usd_micros: u64,
        charged_by: String,
    },
    /// The call gave nothing and cost nothing: its room is free again.
    Released { hold: String, released_by: String },
}

/// What one user says of the sites that hold the first results: which are
/// specialist shops and which are generalists (marketplaces, department
/// stores). Its id is the user's, and it lives in the platform's tenant: a
/// mark made on one study counts on every study of the user.
#[evento::aggregate(name = "timada-research/DomainMarks")]
pub enum DomainMarks {
    /// `domain` is a specialist shop, or a generalist; it replaces what the
    /// user said of it before.
    DomainMarked {
        domain: String,
        specialist: bool,
        marked_by: String,
    },
}
