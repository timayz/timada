use crate::error::GoogleError;

/// Reading the queries and pages of a site in Search Console.
pub const ACCESS_SEARCH_CONSOLE: &str = "search_console";
/// Reading and running the campaigns of a Google Ads account.
pub const ACCESS_ADS: &str = "ads";
/// Reading what Merchant Center says of the shop's products.
pub const ACCESS_MERCHANT_CENTER: &str = "merchant_center";

/// Every kind of access a connection may cover.
pub const ACCESSES: [&str; 3] = [ACCESS_SEARCH_CONSOLE, ACCESS_ADS, ACCESS_MERCHANT_CENTER];

/// Who writes what the host notices on its own (a grant that lapsed).
pub const GOOGLE_ACTOR: &str = "timada-google";

/// The longest account address kept.
pub const MAX_ACCOUNT_LEN: usize = 254;

/// The longest Search Console site name kept.
pub const MAX_SITE_LEN: usize = 300;

/// The stream of a shop's connection. Stream ids are one space for every
/// kind of aggregate, and the shop's own id names its tenant's stream.
pub fn connection_id(tenant_id: &str) -> String {
    evento::hash_ids(vec!["google-connection", tenant_id])
}

/// The stream of one finished day of a shop's site.
pub fn search_day_id(tenant_id: &str, site: &str, day: &str) -> String {
    evento::hash_ids(vec!["google-search-day", tenant_id, site, day])
}

/// The stream that says where a shop's daily look for new days stands.
pub fn search_watch_id(tenant_id: &str) -> String {
    evento::hash_ids(vec!["google-search-watch", tenant_id])
}

/// The stream of one day of a shop's Google Ads account.
pub fn ads_day_id(tenant_id: &str, customer_id: &str, day: &str) -> String {
    evento::hash_ids(vec!["google-ads-day", tenant_id, customer_id, day])
}

/// The stream that says where the daily read of a shop's Google Ads
/// account stands.
pub fn ads_watch_id(tenant_id: &str) -> String {
    evento::hash_ids(vec!["google-ads-watch", tenant_id])
}

/// The kinds of device Google Ads tells apart, as they are kept.
pub const ADS_DEVICES: [&str; 5] = ["mobile", "desktop", "tablet", "tv", "other"];

/// How many rows of a campaign on a device are kept of a day: every
/// campaign of an account many times over.
pub const MAX_ADS_CAMPAIGN_ROWS: usize = 500;

/// How many ad groups are kept of a day.
pub const MAX_ADS_AD_GROUP_ROWS: usize = 1_000;

/// How many products are kept of a day: the ones that cost the most.
pub const MAX_ADS_PRODUCT_ROWS: usize = 2_000;

/// How many searches are kept of a day: the ones that cost the most.
pub const MAX_ADS_TERM_ROWS: usize = 1_000;

/// The longest name of a campaign, an ad group or an account kept.
pub const MAX_ADS_NAME_LEN: usize = 200;

/// How many days back a shop that just chose its account is read for.
pub const ADS_BACKFILL_DAYS: u32 = 90;

/// How many of the last days every pass reads again: Google counts a
/// conversion on the day of the click that led to it, and most come within
/// two weeks of it.
pub const ADS_RECENT_DAYS: u32 = 14;

/// How many days one pass reads at most: the days owed to a new shop come
/// a month a pass.
pub const ADS_DAYS_A_PASS: u32 = 31;

/// A Google Ads account's number as the API takes it: its ten digits,
/// without the dashes the pages of Google Ads write it with.
pub fn normalize_customer_id(customer_id: &str) -> Result<String, GoogleError> {
    let digits: String = customer_id
        .trim()
        .chars()
        .filter(|c| *c != '-' && !c.is_whitespace())
        .collect();
    if digits.len() != 10 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(GoogleError::InvalidAdsAccount);
    }
    Ok(digits)
}

/// The first campaign of a shop: Shopping ads, an ad group a category.
pub const CAMPAIGN_SHOPPING: &str = "shopping";
/// The small Search campaign on the shop's own name.
pub const CAMPAIGN_BRAND: &str = "brand";

/// The products that sell well, in a Shopping campaign of their own that
/// outranks the first and bids higher. A tuning round proposes it.
pub const CAMPAIGN_WINNERS: &str = "winners";

/// Every kind of campaign timada creates.
pub const CAMPAIGN_KINDS: [&str; 3] = [CAMPAIGN_SHOPPING, CAMPAIGN_BRAND, CAMPAIGN_WINNERS];

/// The stream of a shop's campaign of one kind.
pub fn campaign_stream_id(tenant_id: &str, kind: &str) -> String {
    evento::hash_ids(vec!["google-campaign", tenant_id, kind])
}

/// What Google Ads calls the campaign.
pub const CAMPAIGN_NAME: &str = "name";
/// What it may spend in a day, in the account's currency (`10.00`).
pub const CAMPAIGN_DAILY_BUDGET: &str = "daily_budget";
/// The most a click is paid, to start with (`0.30`).
pub const CAMPAIGN_BID: &str = "bid";
/// Where the ads show: two-letter countries (`FR, BE`).
pub const CAMPAIGN_COUNTRIES: &str = "countries";
/// How much more or less a click on a phone is paid, in hundredths
/// (`-55`); empty for the same.
pub const CAMPAIGN_PHONE: &str = "phone_adjustment";
/// The same on a tablet.
pub const CAMPAIGN_TABLET: &str = "tablet_adjustment";
/// Whether it is created running ([`START_LIVE`]) or paused
/// ([`START_PAUSED`]).
pub const CAMPAIGN_START: &str = "start";
/// The ad groups of a Shopping campaign, a line each: the category's id,
/// a space, its name.
pub const CAMPAIGN_AD_GROUPS: &str = "ad_groups";
/// The searches a campaign on the shop's name shows for, a line each.
pub const CAMPAIGN_KEYWORDS: &str = "keywords";
/// The headlines of its ad, a line each.
pub const CAMPAIGN_HEADLINES: &str = "headlines";
/// The descriptions of its ad, a line each.
pub const CAMPAIGN_DESCRIPTIONS: &str = "descriptions";

/// The products of a winners campaign, a line each: the feed's item id, a
/// space, the product's name.
pub const CAMPAIGN_PRODUCTS: &str = "products";

pub const START_PAUSED: &str = "paused";
pub const START_LIVE: &str = "live";

/// The settings of a Shopping campaign, in the order a page shows them.
pub const SHOPPING_FIELDS: [&str; 8] = [
    CAMPAIGN_NAME,
    CAMPAIGN_DAILY_BUDGET,
    CAMPAIGN_BID,
    CAMPAIGN_COUNTRIES,
    CAMPAIGN_PHONE,
    CAMPAIGN_TABLET,
    CAMPAIGN_START,
    CAMPAIGN_AD_GROUPS,
];

/// The settings of a campaign on the shop's name.
pub const BRAND_FIELDS: [&str; 8] = [
    CAMPAIGN_NAME,
    CAMPAIGN_DAILY_BUDGET,
    CAMPAIGN_BID,
    CAMPAIGN_COUNTRIES,
    CAMPAIGN_START,
    CAMPAIGN_KEYWORDS,
    CAMPAIGN_HEADLINES,
    CAMPAIGN_DESCRIPTIONS,
];

/// The settings of a winners campaign.
pub const WINNERS_FIELDS: [&str; 6] = [
    CAMPAIGN_NAME,
    CAMPAIGN_DAILY_BUDGET,
    CAMPAIGN_BID,
    CAMPAIGN_COUNTRIES,
    CAMPAIGN_START,
    CAMPAIGN_PRODUCTS,
];

/// The settings a campaign of `kind` takes; none for no kind.
pub fn campaign_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        CAMPAIGN_SHOPPING => &SHOPPING_FIELDS,
        CAMPAIGN_BRAND => &BRAND_FIELDS,
        CAMPAIGN_WINNERS => &WINNERS_FIELDS,
        _ => &[],
    }
}

/// What a first campaign bids to start with, in the account's currency.
pub const DEFAULT_BID: &str = "0.30";
/// What a first campaign pays less for a click on a phone, and on a tablet.
pub const DEFAULT_PHONE: &str = "-55";
pub const DEFAULT_TABLET: &str = "-40";

/// How many days a campaign that went live is left alone before anything
/// is read into its numbers: three weeks.
pub const SETTLE_DAYS: u64 = 21;

/// How many ad groups a campaign is created with at most.
pub const MAX_CAMPAIGN_AD_GROUPS: usize = 100;
/// How many countries a campaign shows in at most.
pub const MAX_CAMPAIGN_COUNTRIES: usize = 30;
/// How many searches a campaign on the shop's name shows for at most.
pub const MAX_CAMPAIGN_KEYWORDS: usize = 10;
/// The longest search Google Ads takes as a keyword.
pub const MAX_KEYWORD_LEN: usize = 80;
/// What Google Ads asks of a responsive search ad: three to fifteen
/// headlines of thirty characters, two to four descriptions of ninety.
pub const MIN_HEADLINES: usize = 3;
pub const MAX_HEADLINES: usize = 15;
pub const MAX_HEADLINE_LEN: usize = 30;
pub const MIN_DESCRIPTIONS: usize = 2;
pub const MAX_DESCRIPTIONS: usize = 4;
pub const MAX_AD_DESCRIPTION_LEN: usize = 90;
/// The least and the most a device's clicks are paid against the others,
/// in hundredths: Google Ads takes from a tenth to ten times the bid.
pub const MIN_ADJUSTMENT: i32 = -90;
pub const MAX_ADJUSTMENT: i32 = 900;
/// The most a campaign may be given to spend in a day, in hundredths of
/// the account's currency: a typing mistake is not a budget.
pub const MAX_DAILY_BUDGET_CENTS: u64 = 1_000_000;

/// The countries Google Ads is told by their two letters, each with its
/// number in ISO 3166-1: Google's own number for a country is that, plus
/// 2,000 (France is 2250).
const COUNTRY_NUMBERS: [(&str, u32); 72] = [
    ("AE", 784),
    ("AR", 32),
    ("AT", 40),
    ("AU", 36),
    ("BE", 56),
    ("BG", 100),
    ("BR", 76),
    ("CA", 124),
    ("CH", 756),
    ("CL", 152),
    ("CO", 170),
    ("CY", 196),
    ("CZ", 203),
    ("DE", 276),
    ("DK", 208),
    ("DZ", 12),
    ("EE", 233),
    ("EG", 818),
    ("ES", 724),
    ("FI", 246),
    ("FR", 250),
    ("GB", 826),
    ("GP", 312),
    ("GR", 300),
    ("HK", 344),
    ("HR", 191),
    ("HU", 348),
    ("ID", 360),
    ("IE", 372),
    ("IL", 376),
    ("IN", 356),
    ("IS", 352),
    ("IT", 380),
    ("JP", 392),
    ("KR", 410),
    ("LI", 438),
    ("LT", 440),
    ("LU", 442),
    ("LV", 428),
    ("MA", 504),
    ("MC", 492),
    ("MQ", 474),
    ("MT", 470),
    ("MX", 484),
    ("MY", 458),
    ("NL", 528),
    ("NO", 578),
    ("NZ", 554),
    ("PE", 604),
    ("PH", 608),
    ("PL", 616),
    ("PT", 620),
    ("RE", 638),
    ("RO", 642),
    ("RS", 688),
    ("SA", 682),
    ("SE", 752),
    ("SG", 702),
    ("SI", 705),
    ("SK", 703),
    ("SN", 686),
    ("TH", 764),
    ("TN", 788),
    ("TR", 792),
    ("TW", 158),
    ("UA", 804),
    ("US", 840),
    ("VN", 704),
    ("ZA", 710),
    ("CI", 384),
    ("CM", 120),
    ("QA", 634),
];

/// Google Ads' number for a country, by its two letters in capitals;
/// `None` for one it is not told by here.
pub fn geo_target_of(country: &str) -> Option<u32> {
    COUNTRY_NUMBERS
        .iter()
        .find(|(code, _)| *code == country)
        .map(|(_, number)| 2_000 + number)
}

/// An amount as a person types it (`0.3`, `12,50`), in hundredths.
fn parse_cents(value: &str) -> Option<u64> {
    let value = value.trim().replace(',', ".");
    let (whole, fraction) = value.split_once('.').unwrap_or((value.as_str(), ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if (whole.is_empty() && fraction.is_empty())
        || fraction.len() > 2
        || whole.len() > 9
        || !digits(whole)
        || !digits(fraction)
    {
        return None;
    }
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let cents: u64 = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u64>().ok()? * 10,
        _ => fraction.parse().ok()?,
    };
    Some(whole * 100 + cents)
}

/// The lines of a text that say something, trimmed.
fn lines(value: &str) -> Vec<&str> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

/// `lines`, no two the same whatever their capitals, `min` to `max` of
/// them and none longer than `longest`.
fn checked_lines(
    value: &str,
    (min, max): (usize, usize),
    longest: usize,
    err: fn() -> GoogleError,
) -> Result<String, GoogleError> {
    let mut kept: Vec<&str> = Vec::new();
    for line in lines(value) {
        if line.chars().count() > longest || line.chars().any(char::is_control) {
            return Err(err());
        }
        if !kept.iter().any(|held| held.eq_ignore_ascii_case(line)) {
            kept.push(line);
        }
    }
    if kept.len() < min || kept.len() > max {
        return Err(err());
    }
    Ok(kept.join("\n"))
}

/// One ad group of a Shopping campaign to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdGroupPlan {
    pub category_id: String,
    pub name: String,
}

/// The line of [`CAMPAIGN_AD_GROUPS`] that names a category.
pub fn ad_group_line(category_id: &str, name: &str) -> String {
    format!("{category_id} {}", name.trim())
}

/// The ad groups a value of [`CAMPAIGN_AD_GROUPS`] names.
fn ad_groups_of(value: &str) -> Result<Vec<AdGroupPlan>, GoogleError> {
    let mut groups: Vec<AdGroupPlan> = Vec::new();
    for line in lines(value) {
        let (id, name) = line.split_once(' ').ok_or(GoogleError::InvalidAdGroups)?;
        let name = name.trim();
        let is_id = !id.is_empty()
            && id.len() <= 128
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !is_id
            || name.is_empty()
            || name.chars().count() > MAX_ADS_NAME_LEN
            || name.chars().any(char::is_control)
            || groups.iter().any(|group| group.category_id == id)
        {
            return Err(GoogleError::InvalidAdGroups);
        }
        groups.push(AdGroupPlan {
            category_id: id.to_owned(),
            name: name.to_owned(),
        });
    }
    if groups.is_empty() || groups.len() > MAX_CAMPAIGN_AD_GROUPS {
        return Err(GoogleError::InvalidAdGroups);
    }
    Ok(groups)
}

/// `value` as a campaign of `kind` keeps it in `field`: what a proposal
/// records, and what [`plan_of`] reads back.
pub fn normalize_campaign_field(
    kind: &str,
    field: &str,
    value: &str,
) -> Result<String, GoogleError> {
    if !campaign_fields(kind).contains(&field) {
        return Err(GoogleError::InvalidCampaign);
    }
    match field {
        CAMPAIGN_NAME => {
            let name = value.trim();
            if name.is_empty()
                || name.chars().count() > MAX_ADS_NAME_LEN
                || name.chars().any(char::is_control)
            {
                return Err(GoogleError::InvalidCampaignName);
            }
            Ok(name.to_owned())
        }
        CAMPAIGN_DAILY_BUDGET | CAMPAIGN_BID => {
            let cents = parse_cents(value)
                .filter(|cents| (1..=MAX_DAILY_BUDGET_CENTS).contains(cents))
                .ok_or(GoogleError::InvalidCampaignAmount)?;
            Ok(timada_core::money::format_amount(cents))
        }
        CAMPAIGN_COUNTRIES => {
            let mut countries: Vec<String> = Vec::new();
            for country in value.split([',', ' ', '\n', ';']) {
                let country = country.trim().to_ascii_uppercase();
                if country.is_empty() {
                    continue;
                }
                if geo_target_of(&country).is_none() {
                    return Err(GoogleError::InvalidCountries);
                }
                if !countries.contains(&country) {
                    countries.push(country);
                }
            }
            if countries.is_empty() || countries.len() > MAX_CAMPAIGN_COUNTRIES {
                return Err(GoogleError::InvalidCountries);
            }
            Ok(countries.join(", "))
        }
        CAMPAIGN_PHONE | CAMPAIGN_TABLET => {
            let typed = value.trim().trim_end_matches('%').trim();
            if typed.is_empty() {
                return Ok(String::new());
            }
            let percent: i32 = typed
                .trim_start_matches('+')
                .parse()
                .ok()
                .filter(|percent| (MIN_ADJUSTMENT..=MAX_ADJUSTMENT).contains(percent))
                .ok_or(GoogleError::InvalidAdjustment)?;
            // No adjustment is no setting at all.
            Ok(if percent == 0 {
                String::new()
            } else {
                percent.to_string()
            })
        }
        CAMPAIGN_START => match value.trim() {
            START_PAUSED => Ok(START_PAUSED.to_owned()),
            START_LIVE => Ok(START_LIVE.to_owned()),
            _ => Err(GoogleError::InvalidStart),
        },
        CAMPAIGN_AD_GROUPS | CAMPAIGN_PRODUCTS => Ok(ad_groups_of(value)?
            .iter()
            .map(|group| ad_group_line(&group.category_id, &group.name))
            .collect::<Vec<_>>()
            .join("\n")),
        CAMPAIGN_KEYWORDS => {
            checked_lines(value, (1, MAX_CAMPAIGN_KEYWORDS), MAX_KEYWORD_LEN, || {
                GoogleError::InvalidKeywords
            })
        }
        CAMPAIGN_HEADLINES => checked_lines(
            value,
            (MIN_HEADLINES, MAX_HEADLINES),
            MAX_HEADLINE_LEN,
            || GoogleError::InvalidHeadlines,
        ),
        CAMPAIGN_DESCRIPTIONS => checked_lines(
            value,
            (MIN_DESCRIPTIONS, MAX_DESCRIPTIONS),
            MAX_AD_DESCRIPTION_LEN,
            || GoogleError::InvalidDescriptions,
        ),
        _ => Err(GoogleError::InvalidCampaign),
    }
}

/// A campaign to create, as Google Ads is told it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CampaignPlan {
    /// One of [`CAMPAIGN_KINDS`].
    pub kind: String,
    pub name: String,
    /// In millionths of the account's currency, as Google Ads counts.
    pub daily_budget_micros: u64,
    pub bid_micros: u64,
    /// Two-letter countries, each one [`geo_target_of`] knows.
    pub countries: Vec<String>,
    /// In hundredths; `None` for no adjustment.
    pub phone_percent: Option<i32>,
    pub tablet_percent: Option<i32>,
    /// Created running; paused otherwise.
    pub live: bool,
    /// Of a Shopping campaign.
    pub ad_groups: Vec<AdGroupPlan>,
    /// Of a winners campaign: the feed's item id (in `category_id`) and
    /// the product's name.
    pub products: Vec<AdGroupPlan>,
    /// Of a campaign on the shop's name.
    pub keywords: Vec<String>,
    pub headlines: Vec<String>,
    pub descriptions: Vec<String>,
}

/// The campaign of `kind` that `settings` (field, value) describe, each
/// value as [`normalize_campaign_field`] keeps it. A setting left out is
/// empty: only the two adjustments may be.
pub fn plan_of<'a>(
    kind: &str,
    settings: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<CampaignPlan, GoogleError> {
    let fields = campaign_fields(kind);
    if fields.is_empty() {
        return Err(GoogleError::InvalidCampaign);
    }
    let given: Vec<(&str, &str)> = settings.into_iter().collect();
    let mut plan = CampaignPlan {
        kind: kind.to_owned(),
        ..CampaignPlan::default()
    };
    for field in fields {
        let value = given
            .iter()
            .find(|(named, _)| named == field)
            .map_or("", |(_, value)| *value);
        let value = normalize_campaign_field(kind, field, value)?;
        let listed = || value.lines().map(str::to_owned).collect::<Vec<_>>();
        let percent = || value.parse::<i32>().ok();
        match *field {
            CAMPAIGN_NAME => plan.name = value,
            CAMPAIGN_DAILY_BUDGET => {
                plan.daily_budget_micros = parse_cents(&value).unwrap_or(0) * 10_000;
            }
            CAMPAIGN_BID => plan.bid_micros = parse_cents(&value).unwrap_or(0) * 10_000,
            CAMPAIGN_COUNTRIES => {
                plan.countries = value.split(", ").map(str::to_owned).collect();
            }
            CAMPAIGN_PHONE => plan.phone_percent = percent(),
            CAMPAIGN_TABLET => plan.tablet_percent = percent(),
            CAMPAIGN_START => plan.live = value == START_LIVE,
            CAMPAIGN_AD_GROUPS => plan.ad_groups = ad_groups_of(&value)?,
            CAMPAIGN_PRODUCTS => plan.products = ad_groups_of(&value)?,
            CAMPAIGN_KEYWORDS => plan.keywords = listed(),
            CAMPAIGN_HEADLINES => plan.headlines = listed(),
            CAMPAIGN_DESCRIPTIONS => plan.descriptions = listed(),
            _ => {}
        }
    }
    // A bid the day's budget does not pay once never shows an ad.
    if plan.bid_micros > plan.daily_budget_micros {
        return Err(GoogleError::BidOverBudget);
    }
    Ok(plan)
}

/// The stream that says how a shop's campaign is tuned.
pub fn tuning_id(tenant_id: &str) -> String {
    evento::hash_ids(vec!["google-tuning", tenant_id])
}

/// Who a round asked for by the schedule says asked.
pub const TUNING_ACTOR: &str = "timada-tuning";

/// The kinds of device a round adjusts the bids of.
pub const TUNED_DEVICES: [&str; 3] = ["mobile", "tablet", "desktop"];

/// How many days back a round reads.
pub const TUNING_DAYS: &str = "days";
/// How many clicks an ad group or a kind of device needs before its bid is
/// moved.
pub const TUNING_MIN_CLICKS: &str = "min_clicks";
/// How many clicks without a sale make a product one to keep out of ads.
pub const TUNING_DRAIN_CLICKS: &str = "drain_clicks";
/// How many clicks a product needs before it is called a winner.
pub const TUNING_WINNER_CLICKS: &str = "winner_clicks";
/// How many clicks without a sale make a search one to stop showing for.
pub const TUNING_TERM_CLICKS: &str = "term_clicks";
/// What a bid moves by, in the account's currency (`0.02`): one step, or
/// two.
pub const TUNING_STEP: &str = "step";
/// The return an ad group is held to while the shop's own sales do not
/// say its break-even (`3.00`).
pub const TUNING_TARGET_RETURN: &str = "target_return";
/// How many days a bid just changed is left to show before another round
/// looks at it.
pub const TUNING_REST_DAYS: &str = "rest_days";
/// Every how many days a round is prepared without being asked; `0` for
/// only when asked.
pub const TUNING_EVERY_DAYS: &str = "every_days";

/// The thresholds of a round, in the order a page shows them, each with
/// its default.
pub const TUNING_FIELDS: [(&str, &str); 9] = [
    (TUNING_DAYS, "21"),
    (TUNING_MIN_CLICKS, "100"),
    (TUNING_DRAIN_CLICKS, "150"),
    (TUNING_WINNER_CLICKS, "50"),
    (TUNING_TERM_CLICKS, "3"),
    (TUNING_STEP, "0.02"),
    (TUNING_TARGET_RETURN, "3.00"),
    (TUNING_REST_DAYS, "14"),
    (TUNING_EVERY_DAYS, "0"),
];

/// `value` as a round keeps the threshold `field`.
pub fn normalize_tuning_field(field: &str, value: &str) -> Result<String, GoogleError> {
    let value = value.trim();
    let whole = |range: std::ops::RangeInclusive<u64>| {
        value
            .parse::<u64>()
            .ok()
            .filter(|n| range.contains(n))
            .map(|n| n.to_string())
            .ok_or(GoogleError::InvalidTuning)
    };
    match field {
        TUNING_DAYS => whole(7..=90),
        TUNING_MIN_CLICKS | TUNING_DRAIN_CLICKS | TUNING_WINNER_CLICKS => whole(1..=100_000),
        TUNING_TERM_CLICKS => whole(1..=1_000),
        TUNING_REST_DAYS => whole(0..=90),
        // Not asked for, or at least a week apart.
        TUNING_EVERY_DAYS => whole(0..=90).and_then(|days| match days.as_str() {
            "1" | "2" | "3" | "4" | "5" | "6" => Err(GoogleError::InvalidTuning),
            _ => Ok(days),
        }),
        TUNING_STEP => parse_cents(value)
            .filter(|cents| (1..=100).contains(cents))
            .map(timada_core::money::format_amount)
            .ok_or(GoogleError::InvalidTuning),
        TUNING_TARGET_RETURN => parse_cents(value)
            .filter(|hundredths| (10..=10_000).contains(hundredths))
            .map(timada_core::money::format_amount)
            .ok_or(GoogleError::InvalidTuning),
        _ => Err(GoogleError::InvalidTuning),
    }
}

/// The thresholds a round decides on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuningSettings {
    pub days: u32,
    pub min_clicks: u64,
    pub drain_clicks: u64,
    pub winner_clicks: u64,
    pub term_clicks: u64,
    /// In hundredths of the account's currency.
    pub step_cents: u64,
    /// In hundredths: 300 is 3.00 sold for 1 spent.
    pub target_return: i64,
    pub rest_days: u64,
    /// 0 for only when asked.
    pub every_days: u64,
}

impl Default for TuningSettings {
    fn default() -> Self {
        tuning_settings_of(std::iter::empty()).expect("the defaults are thresholds")
    }
}

impl TuningSettings {
    /// What `field` is set at, as a person reads it.
    pub fn value(&self, field: &str) -> String {
        match field {
            TUNING_DAYS => self.days.to_string(),
            TUNING_MIN_CLICKS => self.min_clicks.to_string(),
            TUNING_DRAIN_CLICKS => self.drain_clicks.to_string(),
            TUNING_WINNER_CLICKS => self.winner_clicks.to_string(),
            TUNING_TERM_CLICKS => self.term_clicks.to_string(),
            TUNING_STEP => timada_core::money::format_amount(self.step_cents),
            TUNING_TARGET_RETURN => {
                timada_core::money::format_amount(self.target_return.max(0) as u64)
            }
            TUNING_REST_DAYS => self.rest_days.to_string(),
            TUNING_EVERY_DAYS => self.every_days.to_string(),
            _ => String::new(),
        }
    }
}

/// The thresholds that `settings` (field, value) say, the default for each
/// one left out.
pub fn tuning_settings_of<'a>(
    settings: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<TuningSettings, GoogleError> {
    let given: Vec<(&str, &str)> = settings.into_iter().collect();
    let kept = |field: &str| -> Result<String, GoogleError> {
        let default = TUNING_FIELDS
            .iter()
            .find(|(named, _)| *named == field)
            .map_or("", |(_, default)| *default);
        let value = given
            .iter()
            .find(|(named, _)| *named == field)
            .map_or(default, |(_, value)| *value);
        normalize_tuning_field(field, value)
    };
    let whole = |field: &str| -> Result<u64, GoogleError> {
        kept(field)?.parse().map_err(|_| GoogleError::InvalidTuning)
    };
    let cents = |field: &str| -> Result<u64, GoogleError> {
        parse_cents(&kept(field)?).ok_or(GoogleError::InvalidTuning)
    };
    Ok(TuningSettings {
        days: whole(TUNING_DAYS)? as u32,
        min_clicks: whole(TUNING_MIN_CLICKS)?,
        drain_clicks: whole(TUNING_DRAIN_CLICKS)?,
        winner_clicks: whole(TUNING_WINNER_CLICKS)?,
        term_clicks: whole(TUNING_TERM_CLICKS)?,
        step_cents: cents(TUNING_STEP)?,
        target_return: cents(TUNING_TARGET_RETURN)? as i64,
        rest_days: whole(TUNING_REST_DAYS)?,
        every_days: whole(TUNING_EVERY_DAYS)?,
    })
}

/// A bid as a campaign keeps it (`0.32`), in hundredths.
pub fn bid_cents(bid: &str) -> Option<u64> {
    parse_cents(bid).filter(|cents| *cents > 0)
}

/// A device's adjustment as a campaign keeps it (`-55`, or empty for
/// none), in hundredths.
pub fn adjustment_percent(value: &str) -> Option<i32> {
    match value.trim() {
        "" => Some(0),
        typed => typed
            .parse()
            .ok()
            .filter(|percent| (MIN_ADJUSTMENT..=MAX_ADJUSTMENT).contains(percent)),
    }
}

/// The stream of what Merchant Center says of a shop's products.
pub fn merchant_feed_id(tenant_id: &str, account_id: &str) -> String {
    evento::hash_ids(vec!["google-merchant-feed", tenant_id, account_id])
}

/// The stream that says where the daily read of a shop's Merchant Center
/// account stands.
pub fn merchant_watch_id(tenant_id: &str) -> String {
    evento::hash_ids(vec!["google-merchant-watch", tenant_id])
}

/// The stream of the notice owed to a shop's owners on a day.
pub fn merchant_notice_id(tenant_id: &str, day: &str) -> String {
    evento::hash_ids(vec!["google-merchant-notice", tenant_id, day])
}

/// The day (UTC) a shop's notice is keyed by, so its owners are told once
/// a day at most.
pub fn merchant_notice_day(now: u64) -> String {
    timada_core::civil::format_date(now)
}

/// How long a notice is worth sending: a day.
pub const MERCHANT_NOTICE_WINDOW_SECS: u64 = 24 * 60 * 60;

/// Google shows the product.
pub const MERCHANT_ACCEPTED: &str = "accepted";
/// Google is still looking at it.
pub const MERCHANT_PENDING: &str = "pending";
/// Google does not show it.
pub const MERCHANT_REFUSED: &str = "refused";
/// The product is not sent there at all.
pub const MERCHANT_ABSENT: &str = "none";

/// Where a product may stand in Merchant Center, for Shopping ads or for
/// the free listings.
pub const MERCHANT_STATUSES: [&str; 4] = [
    MERCHANT_ACCEPTED,
    MERCHANT_PENDING,
    MERCHANT_REFUSED,
    MERCHANT_ABSENT,
];

/// What an issue does to a product: it is not shown, shown less, or shown
/// all the same.
pub const MERCHANT_SEVERITIES: [&str; 3] = ["refused", "limited", "none"];

/// Where an issue applies.
pub const MERCHANT_CONTEXTS: [&str; 2] = ["ads", "listings"];

/// What an issue of the account does: nothing is shown, something is
/// wrong, or something could be better.
pub const MERCHANT_ACCOUNT_SEVERITIES: [&str; 3] = ["critical", "error", "suggestion"];

/// How many products a read keeps: every one Google does not accept
/// first, then the accepted ones up to this.
pub const MAX_MERCHANT_PRODUCT_ROWS: usize = 5_000;

/// How many issues are kept of a product, and of an account.
pub const MAX_MERCHANT_ISSUES: usize = 10;

/// The longest text kept of an issue.
pub const MAX_MERCHANT_TEXT_LEN: usize = 500;

/// A Merchant Center account's number: digits.
pub fn normalize_merchant_account(account_id: &str) -> Result<String, GoogleError> {
    let digits = account_id.trim();
    if digits.is_empty() || digits.len() > 20 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(GoogleError::InvalidMerchantAccount);
    }
    Ok(digits.to_owned())
}

/// How many queries, and how many pages, are kept of a day: its most
/// clicked. A day's event carries both lists.
pub const MAX_SEARCH_ROWS: usize = 500;

/// How many pairs of a page and a query it showed for are kept of a day:
/// its most shown. They ride in the day's event with the two lists.
pub const MAX_PAGE_QUERY_ROWS: usize = 1_000;

/// The longest query or address kept: Google's own filters take 4,096
/// characters, and nothing a person types or a shop serves comes near.
pub const MAX_SEARCH_KEY_LEN: usize = 1_000;

/// The longest reason kept for a pass that failed.
pub const MAX_REASON_LEN: usize = 300;

/// How many days back a shop just connected is read for.
pub const BACKFILL_DAYS: u32 = 90;

/// For how many days a site's numbers are kept: sixteen months, which is
/// as far back as Search Console itself goes.
pub const KEPT_DAYS: u32 = 486;

/// For how many days the queries and the pages of a day are kept: the
/// longest period a page adds up, and a little more.
pub const KEPT_ROW_DAYS: u32 = 92;

/// How long after a pass that found everything the next is due.
pub const CHECK_EVERY_SECS: u64 = 24 * 60 * 60;

/// How long after a pass that left older days to fetch the next is due.
pub const CATCH_UP_SECS: u64 = 10 * 60;

/// How long after a pass that failed the next is due: an hour, doubled for
/// each failure in a row before it, a day at most.
pub fn retry_delay_secs(attempts: u32) -> u64 {
    let doublings = attempts.saturating_sub(1).min(5);
    (3_600u64 << doublings).min(CHECK_EVERY_SECS)
}

/// The day `days` before `day` (`2026-11-20`), as text that sorts the way
/// days do; `day` itself when it is no day.
pub fn day_before(day: &str, days: u32) -> String {
    match timada_core::civil::parse_date(day) {
        Some(at) => timada_core::civil::format_date(at.saturating_sub(u64::from(days) * 86_400)),
        None => day.to_owned(),
    }
}

/// What Google calls a kind of access when asked for it; `None` for a kind
/// that is not one of [`ACCESSES`].
pub fn scope_of(access: &str) -> Option<&'static str> {
    match access {
        ACCESS_SEARCH_CONSOLE => Some("https://www.googleapis.com/auth/webmasters.readonly"),
        ACCESS_ADS => Some("https://www.googleapis.com/auth/adwords"),
        ACCESS_MERCHANT_CENTER => Some("https://www.googleapis.com/auth/content"),
        _ => None,
    }
}

/// The kinds of access among `scopes`, as Google lists what an account
/// granted, in the order of [`ACCESSES`]. Full access to Search Console
/// reads it too.
pub fn accesses_of<'a>(scopes: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let scopes: Vec<&str> = scopes.into_iter().collect();
    ACCESSES
        .into_iter()
        .filter(|access| {
            scopes.iter().any(|scope| {
                Some(*scope) == scope_of(access)
                    || (*access == ACCESS_SEARCH_CONSOLE
                        && *scope == "https://www.googleapis.com/auth/webmasters")
            })
        })
        .map(str::to_owned)
        .collect()
}

/// A Search Console site as Search Console names it: an address with its
/// scheme and its closing slash (`https://shop.example/`), or a whole
/// domain (`sc-domain:shop.example`).
pub fn normalize_site(site: &str) -> Result<String, GoogleError> {
    let site = site.trim();
    let named = match site.strip_prefix("sc-domain:") {
        Some(domain) => !domain.is_empty() && !domain.contains('/'),
        None => {
            let rest = site
                .strip_prefix("https://")
                .or_else(|| site.strip_prefix("http://"));
            rest.is_some_and(|rest| rest.len() > 1 && rest.ends_with('/'))
        }
    };
    if !named
        || site.len() > MAX_SITE_LEN
        || site.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(GoogleError::InvalidSite);
    }
    Ok(site.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scopes_granted_name_the_kinds_of_access() {
        assert_eq!(
            accesses_of([
                "openid",
                "https://www.googleapis.com/auth/userinfo.email",
                "https://www.googleapis.com/auth/content",
                "https://www.googleapis.com/auth/webmasters.readonly",
            ]),
            vec![ACCESS_SEARCH_CONSOLE, ACCESS_MERCHANT_CENTER]
        );
        assert_eq!(
            accesses_of(["https://www.googleapis.com/auth/webmasters"]),
            vec![ACCESS_SEARCH_CONSOLE]
        );
        assert!(accesses_of(["openid"]).is_empty());
        for access in ACCESSES {
            assert_eq!(accesses_of(scope_of(access)), vec![access]);
        }
    }

    #[test]
    fn a_failed_pass_waits_longer_each_time_up_to_a_day() {
        assert_eq!(retry_delay_secs(0), 3_600);
        assert_eq!(retry_delay_secs(1), 3_600);
        assert_eq!(retry_delay_secs(2), 7_200);
        assert_eq!(retry_delay_secs(5), 57_600);
        assert_eq!(retry_delay_secs(6), 86_400);
        assert_eq!(retry_delay_secs(u32::MAX), 86_400);
    }

    #[test]
    fn days_are_counted_back_across_months_and_years() {
        assert_eq!(day_before("2026-11-20", 0), "2026-11-20");
        assert_eq!(day_before("2026-11-20", 20), "2026-10-31");
        assert_eq!(day_before("2026-01-01", 1), "2025-12-31");
        assert_eq!(day_before("2024-03-01", 1), "2024-02-29");
        assert_eq!(day_before("soon", 3), "soon");
        // Every stream has an id of its own.
        assert_ne!(search_watch_id("shop"), connection_id("shop"));
        assert_ne!(ads_watch_id("shop"), search_watch_id("shop"));
        assert_ne!(
            ads_day_id("shop", "1234567890", "2026-11-20"),
            ads_day_id("shop", "1234567891", "2026-11-20")
        );
        assert_ne!(
            search_day_id("shop", "sc-domain:a.example", "2026-11-20"),
            search_day_id("shop", "sc-domain:b.example", "2026-11-20")
        );
    }

    #[test]
    fn an_ads_account_is_its_ten_digits() {
        assert_eq!(normalize_customer_id("123-456-7890").unwrap(), "1234567890");
        assert_eq!(normalize_customer_id(" 1234567890 ").unwrap(), "1234567890");
        for bad in [
            "",
            "123456789",
            "12345678901",
            "123-456-789O",
            "customers/1",
        ] {
            assert!(
                matches!(
                    normalize_customer_id(bad),
                    Err(GoogleError::InvalidAdsAccount)
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_country_is_told_by_googles_number_for_it() {
        assert_eq!(geo_target_of("FR"), Some(2250));
        assert_eq!(geo_target_of("DE"), Some(2276));
        assert_eq!(geo_target_of("US"), Some(2840));
        assert_eq!(geo_target_of("BE"), Some(2056));
        assert_eq!(geo_target_of("fr"), None);
        assert_eq!(geo_target_of("XX"), None);
        // No country twice.
        for (at, (code, _)) in COUNTRY_NUMBERS.iter().enumerate() {
            assert!(
                COUNTRY_NUMBERS[..at].iter().all(|(other, _)| other != code),
                "{code}"
            );
        }
    }

    #[test]
    fn a_campaigns_settings_are_kept_as_a_person_reads_them() {
        let kept = |field: &str, value: &str| {
            normalize_campaign_field(CAMPAIGN_SHOPPING, field, value).unwrap()
        };
        assert_eq!(kept(CAMPAIGN_NAME, "  Shopping · Acme "), "Shopping · Acme");
        assert_eq!(kept(CAMPAIGN_BID, "0,3"), "0.30");
        assert_eq!(kept(CAMPAIGN_DAILY_BUDGET, "12"), "12.00");
        assert_eq!(kept(CAMPAIGN_COUNTRIES, "fr,be ; FR\nlu"), "FR, BE, LU");
        assert_eq!(kept(CAMPAIGN_PHONE, " -55 % "), "-55");
        assert_eq!(kept(CAMPAIGN_TABLET, "+20"), "20");
        assert_eq!(kept(CAMPAIGN_TABLET, "0"), "");
        assert_eq!(kept(CAMPAIGN_TABLET, ""), "");
        assert_eq!(kept(CAMPAIGN_START, " live "), START_LIVE);
        assert_eq!(
            kept(
                CAMPAIGN_AD_GROUPS,
                "\n 01CAT   Arbres à chat \n\n01TOY Jouets\n"
            ),
            "01CAT Arbres à chat\n01TOY Jouets"
        );
        let refused = |kind: &str, field: &str, value: &str| {
            normalize_campaign_field(kind, field, value)
                .unwrap_err()
                .code()
                .unwrap()
        };
        let shopping = CAMPAIGN_SHOPPING;
        assert_eq!(
            refused(shopping, CAMPAIGN_NAME, " "),
            "google_invalid_campaign_name"
        );
        assert_eq!(
            refused(shopping, CAMPAIGN_BID, "0"),
            "google_invalid_campaign_amount"
        );
        assert_eq!(
            refused(shopping, CAMPAIGN_BID, "0.305"),
            "google_invalid_campaign_amount"
        );
        assert_eq!(
            refused(shopping, CAMPAIGN_BID, "cheap"),
            "google_invalid_campaign_amount"
        );
        assert_eq!(
            refused(shopping, CAMPAIGN_DAILY_BUDGET, "10001"),
            "google_invalid_campaign_amount"
        );
        assert_eq!(
            refused(shopping, CAMPAIGN_COUNTRIES, ""),
            "google_invalid_countries"
        );
        assert_eq!(
            refused(shopping, CAMPAIGN_COUNTRIES, "FR, Mars"),
            "google_invalid_countries"
        );
        assert_eq!(
            refused(shopping, CAMPAIGN_PHONE, "-95"),
            "google_invalid_adjustment"
        );
        assert_eq!(
            refused(shopping, CAMPAIGN_PHONE, "less"),
            "google_invalid_adjustment"
        );
        assert_eq!(
            refused(shopping, CAMPAIGN_START, "soon"),
            "google_invalid_start"
        );
        assert_eq!(
            refused(shopping, CAMPAIGN_AD_GROUPS, ""),
            "google_invalid_ad_groups"
        );
        assert_eq!(
            refused(shopping, CAMPAIGN_AD_GROUPS, "01CAT"),
            "google_invalid_ad_groups"
        );
        assert_eq!(
            refused(
                shopping,
                CAMPAIGN_AD_GROUPS,
                "01CAT Chats\n01CAT Chats encore"
            ),
            "google_invalid_ad_groups"
        );
        // A setting of the other kind of campaign.
        assert_eq!(
            refused(shopping, CAMPAIGN_KEYWORDS, "acme"),
            "google_invalid_campaign"
        );
        assert_eq!(
            refused(CAMPAIGN_BRAND, CAMPAIGN_PHONE, "-55"),
            "google_invalid_campaign"
        );
        assert_eq!(
            refused("display", CAMPAIGN_NAME, "Acme"),
            "google_invalid_campaign"
        );

        let brand = |field: &str, value: &str| {
            normalize_campaign_field(CAMPAIGN_BRAND, field, value).unwrap()
        };
        assert_eq!(
            brand(CAMPAIGN_KEYWORDS, "acme\n Acme \nacme moto"),
            "acme\nacme moto"
        );
        assert_eq!(
            refused(CAMPAIGN_BRAND, CAMPAIGN_HEADLINES, "Acme\nOfficial shop"),
            "google_invalid_headlines"
        );
        assert_eq!(
            refused(
                CAMPAIGN_BRAND,
                CAMPAIGN_HEADLINES,
                "Acme\nOfficial shop\nA headline of more than thirty characters"
            ),
            "google_invalid_headlines"
        );
        assert_eq!(
            refused(CAMPAIGN_BRAND, CAMPAIGN_DESCRIPTIONS, "One only."),
            "google_invalid_descriptions"
        );
    }

    #[test]
    fn settings_make_a_campaign_or_say_what_is_wrong() {
        let shopping = [
            (CAMPAIGN_NAME, "Shopping · Acme"),
            (CAMPAIGN_DAILY_BUDGET, "10.00"),
            (CAMPAIGN_BID, "0.30"),
            (CAMPAIGN_COUNTRIES, "FR, BE"),
            (CAMPAIGN_PHONE, "-55"),
            (CAMPAIGN_START, START_LIVE),
            (CAMPAIGN_AD_GROUPS, "01CAT Arbres à chat"),
        ];
        let plan = plan_of(CAMPAIGN_SHOPPING, shopping).unwrap();
        assert_eq!(
            plan,
            CampaignPlan {
                kind: CAMPAIGN_SHOPPING.into(),
                name: "Shopping · Acme".into(),
                daily_budget_micros: 10_000_000,
                bid_micros: 300_000,
                countries: vec!["FR".into(), "BE".into()],
                phone_percent: Some(-55),
                // Left out: no adjustment.
                tablet_percent: None,
                live: true,
                ad_groups: vec![AdGroupPlan {
                    category_id: "01CAT".into(),
                    name: "Arbres à chat".into(),
                }],
                ..CampaignPlan::default()
            }
        );
        // What a campaign needs is not left out.
        let without = |field: &str| {
            plan_of(
                CAMPAIGN_SHOPPING,
                shopping.into_iter().filter(|(named, _)| *named != field),
            )
            .unwrap_err()
            .code()
            .unwrap()
        };
        assert_eq!(without(CAMPAIGN_NAME), "google_invalid_campaign_name");
        assert_eq!(without(CAMPAIGN_BID), "google_invalid_campaign_amount");
        assert_eq!(without(CAMPAIGN_COUNTRIES), "google_invalid_countries");
        assert_eq!(without(CAMPAIGN_START), "google_invalid_start");
        assert_eq!(without(CAMPAIGN_AD_GROUPS), "google_invalid_ad_groups");
        let mut dear = shopping;
        dear[2].1 = "10.01";
        assert!(matches!(
            plan_of(CAMPAIGN_SHOPPING, dear),
            Err(GoogleError::BidOverBudget)
        ));

        let brand = plan_of(
            CAMPAIGN_BRAND,
            [
                (CAMPAIGN_NAME, "Acme · name"),
                (CAMPAIGN_DAILY_BUDGET, "2.00"),
                (CAMPAIGN_BID, "0.30"),
                (CAMPAIGN_COUNTRIES, "FR"),
                (CAMPAIGN_START, START_PAUSED),
                (CAMPAIGN_KEYWORDS, "acme"),
                (CAMPAIGN_HEADLINES, "Acme\nOfficial shop\nOrder online"),
                (
                    CAMPAIGN_DESCRIPTIONS,
                    "The official shop.\nDelivered to your door.",
                ),
            ],
        )
        .unwrap();
        assert!(!brand.live);
        assert_eq!(brand.keywords, vec!["acme"]);
        assert_eq!(brand.headlines.len(), 3);
        assert_eq!(brand.descriptions.len(), 2);
        assert!(brand.ad_groups.is_empty());
        assert_ne!(
            campaign_stream_id("shop", CAMPAIGN_SHOPPING),
            campaign_stream_id("shop", CAMPAIGN_BRAND)
        );
    }

    #[test]
    fn a_site_is_named_as_search_console_names_it() {
        assert_eq!(
            normalize_site(" https://shop.example/ ").unwrap(),
            "https://shop.example/"
        );
        assert_eq!(
            normalize_site("sc-domain:shop.example").unwrap(),
            "sc-domain:shop.example"
        );
        assert_eq!(
            normalize_site("http://shop.example/fr/").unwrap(),
            "http://shop.example/fr/"
        );
        for site in [
            "",
            "shop.example",
            "https://shop.example",
            "https://",
            "sc-domain:",
            "sc-domain:shop.example/fr",
            "https://shop .example/",
        ] {
            assert!(
                matches!(normalize_site(site), Err(GoogleError::InvalidSite)),
                "{site}"
            );
        }
    }
}
