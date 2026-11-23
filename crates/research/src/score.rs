//! Whether the niche of a study is worth a shop: which of its keywords
//! count, what one typical sale leaves, what a click may cost, and the tests
//! that decide. Pure arithmetic over the rows a study fetched and what its
//! owner said of it: nothing here reads a store or calls a source, so a
//! study is scored again whenever one of its numbers changes.
//!
//! What a sale leaves and the return on ad spend at which an ad pays for
//! itself are the catalog's ([`Margin`]): a study and the shop that comes of
//! it reckon them the same way.

use std::collections::{BTreeSet, HashMap};

use timada_catalog::{Margin, ShopCosts, UnitCosts};
use timada_core::db::search_key;

use crate::aggregator::KeywordRow;

/// From how many searches a month a kept keyword is a main one.
pub const MAIN_VOLUME: u64 = 1_000;

/// From how many searches a month a keyword is kept at all: under it, it is
/// not worth a page.
pub const LONG_TAIL_VOLUME: u64 = 150;

/// How many products the supplier must have for the niche to fill a shop.
pub const ENOUGH_PRODUCTS: u32 = 200;

/// The currency a cost per click is given in, whatever the country.
pub const CPC_CURRENCY: &str = "USD";

/// Words that open a question, by language: a keyword that holds one is
/// asked by someone who wants an answer, not a product. Folded as
/// [`search_key`] folds; `où` is told apart from `ou` before folding.
const QUESTION_WORDS: &[(&str, &[&str])] = &[
    (
        "fr",
        &[
            "comment", "pourquoi", "quand", "quel", "quelle", "quels", "quelles", "qui", "quoi",
            "combien",
        ],
    ),
    (
        "en",
        &["how", "what", "why", "when", "where", "who", "which"],
    ),
];

/// The thresholds a study is scored against: settings, each with the
/// default the method starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    /// The highest difficulty kept, on the keyword source's own scale (0 to
    /// 100). The usual rule is written for other tools (0 to 2 on Ahrefs,
    /// 35 on Semrush); how the source's score compares with theirs is not
    /// measured yet (`docs/keywords.md`), so the default is Semrush's
    /// figure, whose scale has the same bounds.
    pub max_difficulty: u8,
    /// How many main keywords are enough.
    pub enough_main: u32,
    /// How many long-tail keywords are enough.
    pub enough_long_tail: u32,
    /// What is asked of an ad above its break-even return on ad spend, in
    /// hundredths (`40` is 0.4).
    pub safety: u32,
    /// The share of clicks that buy, in basis points (`150` is 1.5 %).
    pub conversion_bps: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            max_difficulty: 35,
            enough_main: 3,
            enough_long_tail: 10,
            safety: 40,
            conversion_bps: 150,
        }
    }
}

/// One typical sale of the niche, as the study's owner reckons it, in
/// `currency`. Without a price or a supplier cost nothing is worked out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sale {
    /// ISO 4217, uppercase.
    pub currency: String,
    /// The average price a shopper would pay, VAT included.
    pub price_cents: Option<u64>,
    /// What the supplier charges for it.
    pub cost_cents: Option<u64>,
    /// What sending it costs.
    pub shipping_cents: u64,
    /// What the border charges for it.
    pub import_cents: u64,
    /// The VAT rate in the price, in basis points.
    pub vat_bps: u32,
    /// The share of the fixed costs a sale carries.
    pub fixed_cents: u64,
    /// What the payment provider keeps of the price, in basis points.
    pub payment_fee_bps: u32,
    /// And for each payment whatever its amount.
    pub payment_fee_cents: u64,
}

impl Sale {
    /// What the sale leaves, line by line; `None` until its price and its
    /// supplier cost are both said.
    pub fn margin(&self) -> Option<Margin> {
        let costs = ShopCosts {
            currency: self.currency.clone(),
            shipping_cents: self.shipping_cents,
            import_cents: self.import_cents,
            fixed_cents: self.fixed_cents,
            payment_fee_bps: self.payment_fee_bps,
            payment_fee_cents: self.payment_fee_cents,
        };
        let unit = UnitCosts {
            cost_cents: self.cost_cents,
            ..UnitCosts::default()
        };
        Margin::of(
            self.price_cents?,
            &self.currency,
            self.vat_bps,
            unit,
            Some(&costs),
        )
    }
}

/// What the owner said of a study, beside its keywords.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StudyInputs {
    pub sale: Sale,
    /// Brands whose keywords are another shop's: a keyword that holds one,
    /// as whole words, is dropped.
    pub brands: Vec<String>,
    /// Words that mark a keyword as none of the niche's, matched the same
    /// way.
    pub excluded_words: Vec<String>,
    /// Keywords dropped one by one, as their rows spell them.
    pub dropped: BTreeSet<String>,
    /// Keywords brought back one by one: kept whatever the filters say.
    pub restored: BTreeSet<String>,
    /// How many good products the owner found at the supplier; `None`
    /// until they looked.
    pub supplier_products: Option<u32>,
    /// What one unit of the sale's currency is worth in millionths of a US
    /// dollar, to read a cost per click in that currency. `None` when no
    /// rate is known: the two are then compared as they are, and
    /// [`Score::cpc_converted`] says so.
    pub usd_micros: Option<u64>,
}

impl StudyInputs {
    /// What one unit of the sale's currency is worth in US dollars, when
    /// the two differ and a rate is known.
    fn rate(&self) -> Option<u64> {
        if self.sale.currency == CPC_CURRENCY {
            None
        } else {
            self.usd_micros.filter(|rate| *rate > 0)
        }
    }

    /// Whether a cost per click is read in the sale's currency: it is in US
    /// dollars already, or a rate says what it is worth.
    pub fn cpc_converted(&self) -> bool {
        self.sale.currency == CPC_CURRENCY || self.rate().is_some()
    }

    /// A cost per click in US cents as thousandths of a cent of the sale's
    /// currency; of a US cent when [`cpc_converted`](Self::cpc_converted)
    /// is false.
    pub fn cpc_millis(&self, cpc_usd_cents: u64) -> u64 {
        cpc_millis(cpc_usd_cents, self.rate())
    }

    /// The economics of the sale under `thresholds`; `None` until its price
    /// and its supplier cost are both said.
    pub fn sale_score(&self, thresholds: &Thresholds) -> Option<SaleScore> {
        let margin = self.sale.margin()?;
        let needed = needed_ratio(&margin, thresholds);
        Some(SaleScore {
            margin,
            needed_price_per_click: needed,
            highest_cpc_millis: needed.map(|needed| {
                let cpc = u128::from(margin.price_cents) * 100_000 / u128::from(needed.max(1));
                u64::try_from(cpc).unwrap_or(u64::MAX)
            }),
        })
    }
}

/// Why a keyword does not count. A keyword that several reasons would drop
/// carries the first, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DropReason {
    /// Dropped by hand, or it holds a word the owner excluded.
    Excluded,
    /// It holds a brand the owner listed.
    Brand,
    /// The searcher wants an answer: the source says so, or the keyword
    /// holds a question word.
    Informational,
    /// Harder than [`Thresholds::max_difficulty`].
    TooDifficult,
    /// Searched less than [`LONG_TAIL_VOLUME`] times a month, or the source
    /// has no volume for it.
    LowVolume,
}

impl DropReason {
    /// Every reason, in the order they are tried.
    pub const ALL: [DropReason; 5] = [
        DropReason::Excluded,
        DropReason::Brand,
        DropReason::Informational,
        DropReason::TooDifficult,
        DropReason::LowVolume,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Excluded => "excluded",
            Self::Brand => "brand",
            Self::Informational => "informational",
            Self::TooDifficult => "too_difficult",
            Self::LowVolume => "low_volume",
        }
    }
}

/// How much a kept keyword is searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// [`MAIN_VOLUME`] searches a month or more.
    Main,
    /// Fewer: [`LONG_TAIL_VOLUME`] at the least, unless brought back by
    /// hand.
    LongTail,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::LongTail => "long_tail",
        }
    }
}

/// What became of a keyword.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Kept(Tier),
    Dropped(DropReason),
    /// The same search as the kept keyword `of`, spelled with other accents
    /// or in the plural: it counts under that one, which is searched more.
    Variant {
        of: String,
    },
}

impl Outcome {
    /// `main`, `long_tail`, `variant`, or the reason it was dropped.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Kept(tier) => tier.as_str(),
            Self::Dropped(reason) => reason.as_str(),
            Self::Variant { .. } => VARIANT,
        }
    }
}

/// What [`Outcome::as_str`] says of a keyword that counts under another.
pub const VARIANT: &str = "variant";

/// Where a kept keyword can bring buyers from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// A click on it costs little enough for an ad to pay.
    Ads,
    /// A click costs too much, or no ad pays at this margin: it is worth a
    /// page, not a campaign.
    SeoOnly,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ads => "ads",
            Self::SeoOnly => "seo_only",
        }
    }
}

/// One keyword of a study and what the score makes of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoredKeyword {
    pub row: KeywordRow,
    pub outcome: Outcome,
    /// The other spellings that count under it, when it is kept.
    pub variants: Vec<String>,
    /// Its cost per click in thousandths of a cent of the sale's currency
    /// (of a US cent when [`Score::cpc_converted`] is false); `None` when
    /// the source has none.
    pub cpc_millis: Option<u64>,
    /// The sale's price over its cost per click, in hundredths; `None`
    /// without a price or a cost per click, or when a click is free.
    pub price_per_click: Option<u64>,
    /// `None` for a keyword that is not kept, and while the sale or the
    /// keyword's cost per click is not known.
    pub channel: Option<Channel>,
}

impl ScoredKeyword {
    pub fn tier(&self) -> Option<Tier> {
        match self.outcome {
            Outcome::Kept(tier) => Some(tier),
            _ => None,
        }
    }
}

/// The economics of one typical sale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaleScore {
    pub margin: Margin,
    /// The price over a click's cost the sale needs for an ad to pay: its
    /// break-even return on ad spend plus the safety, over the conversion
    /// rate, in hundredths and rounded up. `None` when the sale leaves
    /// nothing: no ad pays for itself then.
    pub needed_price_per_click: Option<u64>,
    /// The most a click may cost at that ratio, in thousandths of a cent
    /// of the sale's currency.
    pub highest_cpc_millis: Option<u64>,
}

impl SaleScore {
    /// The sale's price over a cost per click (in thousandths of a cent),
    /// in hundredths; `None` when a click is free.
    pub fn price_per_click(&self, cpc_millis: u64) -> Option<u64> {
        ratio(self.margin.price_cents, cpc_millis)
    }

    /// Where a kept keyword whose click costs `cpc_millis` can bring buyers
    /// from; `None` while its cost per click is not known.
    pub fn channel(&self, cpc_millis: Option<u64>) -> Option<Channel> {
        let Some(needed) = self.needed_price_per_click else {
            return Some(Channel::SeoOnly);
        };
        cpc_millis.map(|cpc| {
            if self
                .price_per_click(cpc)
                .is_none_or(|ratio| ratio >= needed)
            {
                Channel::Ads
            } else {
                Channel::SeoOnly
            }
        })
    }
}

/// One of the tests a study passes to be a go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Test {
    /// [`Thresholds::enough_main`] main keywords.
    MainKeywords,
    /// [`Thresholds::enough_long_tail`] long-tail keywords.
    LongTail,
    /// A sale leaves something.
    Margin,
    /// The price over the cost per click, averaged by volume over the kept
    /// keywords, is at or above what the sale needs.
    PricePerClick,
    /// [`ENOUGH_PRODUCTS`] products at the supplier; entered by hand.
    SupplierProducts,
    /// No specialist shop in the first results of several main keywords
    /// ([`competition`]).
    NoSpecialist,
}

impl Test {
    /// Every test, in the order a report lists them.
    pub const ALL: [Test; 6] = [
        Test::MainKeywords,
        Test::LongTail,
        Test::Margin,
        Test::PricePerClick,
        Test::SupplierProducts,
        Test::NoSpecialist,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MainKeywords => "main_keywords",
            Self::LongTail => "long_tail",
            Self::Margin => "margin",
            Self::PricePerClick => "price_per_click",
            Self::SupplierProducts => "supplier_products",
            Self::NoSpecialist => "no_specialist",
        }
    }
}

/// Where a test stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TestState {
    Passed,
    Failed,
    /// What it reads is not there yet: nothing fetched, a number the owner
    /// has not entered.
    NotChecked,
}

impl TestState {
    fn of(passed: bool) -> Self {
        if passed { Self::Passed } else { Self::Failed }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::NotChecked => "not_checked",
        }
    }

    /// What [`as_str`](Self::as_str) said; anything else is not checked.
    pub fn parse(text: &str) -> Self {
        match text {
            "passed" => Self::Passed,
            "failed" => Self::Failed,
            _ => Self::NotChecked,
        }
    }
}

/// What the tests say together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    /// Every test passes.
    Go,
    /// One test fails, whatever the others say.
    NoGo,
    /// None fails and one at least is not checked.
    Incomplete,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Go => "go",
            Self::NoGo => "no_go",
            Self::Incomplete => "incomplete",
        }
    }

    /// What [`as_str`](Self::as_str) said; anything else is incomplete.
    pub fn parse(text: &str) -> Self {
        match text {
            "go" => Self::Go,
            "no_go" => Self::NoGo,
            _ => Self::Incomplete,
        }
    }

    /// The verdict of these tests and one more, which stands at `state`.
    pub fn with(self, state: TestState) -> Self {
        match (self, state) {
            (Self::NoGo, _) | (_, TestState::Failed) => Self::NoGo,
            (Self::Go, TestState::Passed) => Self::Go,
            _ => Self::Incomplete,
        }
    }
}

/// From how many main keywords a specialist shop found in their first
/// results holds the niche.
pub const SEVERAL_KEYWORDS: u32 = 2;

/// Who holds the first results of a study's main keywords.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Competition {
    pub state: TestState,
    /// How many sites were found for [`SEVERAL_KEYWORDS`] main keywords or
    /// more: the ones that decide.
    pub recurring: u32,
    /// How many of those the owner has not marked yet.
    pub unmarked: u32,
    /// How many of those are specialist shops.
    pub specialists: u32,
}

/// The competition test. `searched` main keywords have their first results
/// and `owed` are still without; `sites` are the sites found, each with how
/// many of the searched keywords it appears for and what the owner said of
/// it (`true` for a specialist shop).
///
/// One specialist found for [`SEVERAL_KEYWORDS`] keywords or more fails it,
/// whatever is left to look at. It passes once every main keyword was
/// searched and every site found for several is marked, none a specialist.
/// A site found for one keyword decides nothing, so it needs no mark.
pub fn competition(
    searched: u32,
    owed: u32,
    sites: impl IntoIterator<Item = (u32, Option<bool>)>,
) -> Competition {
    let (mut recurring, mut unmarked, mut specialists) = (0u32, 0u32, 0u32);
    for (keywords, specialist) in sites {
        if keywords < SEVERAL_KEYWORDS {
            continue;
        }
        recurring += 1;
        match specialist {
            Some(true) => specialists += 1,
            Some(false) => {}
            None => unmarked += 1,
        }
    }
    let state = if specialists > 0 {
        TestState::Failed
    } else if searched == 0 || owed > 0 || unmarked > 0 {
        TestState::NotChecked
    } else {
        TestState::Passed
    };
    Competition {
        state,
        recurring,
        unmarked,
        specialists,
    }
}

/// What the keywords of a study come to, once each is kept or dropped: all
/// the tests read of them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    /// How many keywords the study holds, kept or not.
    pub keywords: u64,
    pub main: u32,
    pub long_tail: u32,
    /// Searches a month over the kept keywords, a variant's not counted.
    pub kept_volume: u64,
    /// The cost per click of the kept keywords averaged by volume, in the
    /// unit of [`ScoredKeyword::cpc_millis`]; `None` when none has both a
    /// cost per click and a volume.
    pub avg_cpc_millis: Option<u64>,
}

/// What the tests say of a [`Tally`] and of what the owner said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Judgement {
    /// `None` until the sale's price and supplier cost are both said.
    pub sale: Option<SaleScore>,
    /// See [`Score::cpc_converted`].
    pub cpc_converted: bool,
    /// The sale's price over the average cost per click, in hundredths.
    pub price_per_click: Option<u64>,
    pub tests: [(Test, TestState); 6],
    pub verdict: Verdict,
}

impl Judgement {
    pub fn test(&self, test: Test) -> TestState {
        self.tests
            .iter()
            .find(|(of, _)| *of == test)
            .map_or(TestState::NotChecked, |(_, state)| *state)
    }
}

/// Runs the tests over what a study's keywords come to, with where the
/// competition test stands ([`competition`]). A page that keeps the tally
/// reads the verdict again from it when the owner's numbers change, without
/// the keywords.
pub fn judge(
    tally: &Tally,
    competition: TestState,
    inputs: &StudyInputs,
    thresholds: &Thresholds,
) -> Judgement {
    let sale = inputs.sale_score(thresholds);
    let price_per_click = sale
        .zip(tally.avg_cpc_millis)
        .and_then(|(sale, cpc)| sale.price_per_click(cpc));
    let enough = |count: u32, enough: u32| {
        if tally.keywords == 0 {
            TestState::NotChecked
        } else {
            TestState::of(count >= enough)
        }
    };
    let clicks = match (sale, tally.avg_cpc_millis) {
        (None, _) => TestState::NotChecked,
        (Some(sale), _) if sale.needed_price_per_click.is_none() => TestState::Failed,
        (Some(_), None) => TestState::NotChecked,
        (Some(sale), Some(_)) => TestState::of(
            price_per_click
                .zip(sale.needed_price_per_click)
                .is_none_or(|(ratio, needed)| ratio >= needed),
        ),
    };
    let tests = [
        (
            Test::MainKeywords,
            enough(tally.main, thresholds.enough_main),
        ),
        (
            Test::LongTail,
            enough(tally.long_tail, thresholds.enough_long_tail),
        ),
        (
            Test::Margin,
            sale.map_or(TestState::NotChecked, |sale| {
                TestState::of(sale.margin.margin_cents() > 0)
            }),
        ),
        (Test::PricePerClick, clicks),
        (
            Test::SupplierProducts,
            inputs
                .supplier_products
                .map_or(TestState::NotChecked, |products| {
                    TestState::of(products >= ENOUGH_PRODUCTS)
                }),
        ),
        (Test::NoSpecialist, competition),
    ];
    let verdict = if tests.iter().any(|(_, state)| *state == TestState::Failed) {
        Verdict::NoGo
    } else if tests.iter().all(|(_, state)| *state == TestState::Passed) {
        Verdict::Go
    } else {
        Verdict::Incomplete
    };
    Judgement {
        sale,
        cpc_converted: inputs.cpc_converted(),
        price_per_click,
        tests,
        verdict,
    }
}

/// A study scored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Score {
    /// Every keyword once, in the order given.
    pub keywords: Vec<ScoredKeyword>,
    pub main: u32,
    pub long_tail: u32,
    /// Searches a month over the kept keywords, a variant's not counted.
    pub kept_volume: u64,
    /// `None` until the sale's price and supplier cost are both said.
    pub sale: Option<SaleScore>,
    /// Whether the costs per click were read in the sale's currency. When
    /// false no rate was given: a price in its currency stands against
    /// clicks in US dollars, and a page must say so.
    pub cpc_converted: bool,
    /// The cost per click of the kept keywords averaged by volume, in the
    /// unit of [`ScoredKeyword::cpc_millis`]; `None` when none has both a
    /// cost per click and a volume.
    pub avg_cpc_millis: Option<u64>,
    /// The sale's price over that average, in hundredths.
    pub price_per_click: Option<u64>,
    pub tests: [(Test, TestState); 6],
    pub verdict: Verdict,
}

impl Score {
    /// What its keywords come to.
    pub fn tally(&self) -> Tally {
        Tally {
            keywords: self.keywords.len() as u64,
            main: self.main,
            long_tail: self.long_tail,
            kept_volume: self.kept_volume,
            avg_cpc_millis: self.avg_cpc_millis,
        }
    }

    pub fn test(&self, test: Test) -> TestState {
        self.tests
            .iter()
            .find(|(of, _)| *of == test)
            .map_or(TestState::NotChecked, |(_, state)| *state)
    }
}

/// The words of `text`, folded: lowercase, without accents, cut wherever a
/// character is neither a letter nor a digit.
fn words(text: &str) -> Vec<String> {
    search_key(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether `words` holds the words of one of `phrases`, whole and in a row:
/// `nike` is in `chaussures nike`, not in `nikel`.
fn holds_one_of(words: &[String], phrases: &[Vec<String>]) -> bool {
    phrases.iter().any(|phrase| {
        words
            .windows(phrase.len())
            .any(|window| window == phrase.as_slice())
    })
}

/// The phrases of a list the owner typed, each as its folded words; a blank
/// one matches nothing.
fn phrases(list: &[String]) -> Vec<Vec<String>> {
    list.iter()
        .map(|phrase| words(phrase))
        .filter(|phrase| !phrase.is_empty())
        .collect()
}

/// Whether `keyword`, searched in `language`, holds a word that opens a
/// question.
fn asks_a_question(keyword: &str, folded: &[String], language: &str) -> bool {
    let language = language.split('-').next().unwrap_or_default();
    let Some((_, question_words)) = QUESTION_WORDS.iter().find(|(of, _)| *of == language) else {
        return false;
    };
    if language == "fr"
        && keyword
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| word == "où")
    {
        return true;
    }
    folded
        .iter()
        .any(|word| question_words.contains(&word.as_str()))
}

/// What two spellings of one search share: the folded words, each without
/// the `s` or the `x` of a plural. `Arbres à chat` and `arbre a chats` are
/// one. A word of three letters or fewer keeps its end (`bas`, `os`).
fn variant_key(folded: &[String]) -> String {
    folded
        .iter()
        .map(|word| {
            if word.chars().count() > 3 {
                word.strip_suffix(['s', 'x']).unwrap_or(word)
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A US cost per click in thousandths of a cent of the sale's currency, a
/// unit of which is worth `usd_micros`; as it is without a rate.
fn cpc_millis(cpc_usd_cents: u64, usd_micros: Option<u64>) -> u64 {
    let millis = u128::from(cpc_usd_cents) * 1_000;
    let millis = match usd_micros.filter(|rate| *rate > 0) {
        Some(rate) => (millis * 1_000_000 + u128::from(rate) / 2) / u128::from(rate),
        None => millis,
    };
    u64::try_from(millis).unwrap_or(u64::MAX)
}

/// A price over a cost per click, in hundredths, rounded down; `None` when
/// a click is free.
fn ratio(price_cents: u64, cpc_millis: u64) -> Option<u64> {
    (cpc_millis > 0).then(|| {
        let ratio = u128::from(price_cents) * 100_000 / u128::from(cpc_millis);
        u64::try_from(ratio).unwrap_or(u64::MAX)
    })
}

/// The price over a click's cost a sale needs: (break-even + safety) over
/// the conversion rate, in hundredths, rounded up.
fn needed_ratio(margin: &Margin, thresholds: &Thresholds) -> Option<u64> {
    let roas = u128::from(margin.break_even_roas()?) + u128::from(thresholds.safety);
    let needed = (roas * 10_000).div_ceil(u128::from(thresholds.conversion_bps.max(1)));
    Some(u64::try_from(needed).unwrap_or(u64::MAX))
}

/// What the filters make of one row, before variants are folded.
fn filter(
    row: &KeywordRow,
    folded: &[String],
    language: &str,
    inputs: &StudyInputs,
    brands: &[Vec<String>],
    excluded: &[Vec<String>],
    thresholds: &Thresholds,
) -> Outcome {
    let volume = row.volume.unwrap_or(0);
    let tier = if volume >= MAIN_VOLUME {
        Tier::Main
    } else {
        Tier::LongTail
    };
    if inputs.restored.contains(&row.keyword) {
        return Outcome::Kept(tier);
    }
    let reason = if inputs.dropped.contains(&row.keyword) || holds_one_of(folded, excluded) {
        DropReason::Excluded
    } else if holds_one_of(folded, brands) {
        DropReason::Brand
    } else if row.intent.as_deref() == Some("informational")
        || asks_a_question(&row.keyword, folded, language)
    {
        DropReason::Informational
    } else if row
        .difficulty
        .is_some_and(|difficulty| difficulty > thresholds.max_difficulty)
    {
        DropReason::TooDifficult
    } else if volume < LONG_TAIL_VOLUME {
        DropReason::LowVolume
    } else {
        return Outcome::Kept(tier);
    };
    Outcome::Dropped(reason)
}

/// Scores a study: `rows` are the keywords of all its seeds, searched in
/// `language` (`fr`). A keyword two seeds gave is scored once, with the
/// larger of its volumes. `competition` is where the competition test
/// stands, which reads more than the rows.
pub fn score(
    rows: impl IntoIterator<Item = KeywordRow>,
    language: &str,
    inputs: &StudyInputs,
    competition: TestState,
    thresholds: &Thresholds,
) -> Score {
    let mut distinct: Vec<KeywordRow> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for row in rows {
        match seen.get(&row.keyword) {
            Some(at) => {
                if row.volume > distinct[*at].volume {
                    distinct[*at] = row;
                }
            }
            None => {
                seen.insert(row.keyword.clone(), distinct.len());
                distinct.push(row);
            }
        }
    }

    let brands = phrases(&inputs.brands);
    let excluded = phrases(&inputs.excluded_words);
    let sale = inputs.sale_score(thresholds);

    // The kept keyword each spelling counts under: the most searched of
    // its variants, the first in the alphabet between two as searched.
    let mut leaders: HashMap<String, usize> = HashMap::new();
    let mut keys: Vec<Option<String>> = Vec::with_capacity(distinct.len());
    let mut keywords: Vec<ScoredKeyword> = Vec::with_capacity(distinct.len());
    for row in distinct {
        let folded = words(&row.keyword);
        let outcome = filter(
            &row, &folded, language, inputs, &brands, &excluded, thresholds,
        );
        let key = matches!(outcome, Outcome::Kept(_)).then(|| variant_key(&folded));
        keys.push(key);
        let cpc = row.cpc_usd_cents.map(|cents| inputs.cpc_millis(cents));
        keywords.push(ScoredKeyword {
            row,
            outcome,
            variants: Vec::new(),
            cpc_millis: cpc,
            price_per_click: None,
            channel: None,
        });
    }
    for (at, key) in keys.iter().enumerate() {
        let Some(key) = key else { continue };
        let searched = |at: usize| {
            let row = &keywords[at].row;
            (row.volume, std::cmp::Reverse(row.keyword.as_str()))
        };
        let leader = leaders.entry(key.clone()).or_insert(at);
        if searched(at) > searched(*leader) {
            *leader = at;
        }
    }
    for at in 0..keywords.len() {
        let Some(leader) = keys[at].as_ref().and_then(|key| leaders.get(key)).copied() else {
            continue;
        };
        if leader != at {
            let spelling = keywords[at].row.keyword.clone();
            keywords[at].outcome = Outcome::Variant {
                of: keywords[leader].row.keyword.clone(),
            };
            keywords[leader].variants.push(spelling);
        }
    }

    let (mut main, mut long_tail, mut kept_volume) = (0u32, 0u32, 0u64);
    let (mut weighed_cpc, mut weight) = (0u128, 0u128);
    for keyword in &mut keywords {
        keyword.price_per_click = sale
            .zip(keyword.cpc_millis)
            .and_then(|(sale, cpc)| sale.price_per_click(cpc));
        let Some(tier) = keyword.tier() else {
            continue;
        };
        match tier {
            Tier::Main => main += 1,
            Tier::LongTail => long_tail += 1,
        }
        let volume = keyword.row.volume.unwrap_or(0);
        kept_volume = kept_volume.saturating_add(volume);
        if let Some(cpc) = keyword.cpc_millis.filter(|_| volume > 0) {
            weighed_cpc += u128::from(cpc) * u128::from(volume);
            weight += u128::from(volume);
        }
        keyword.channel = sale.and_then(|sale| sale.channel(keyword.cpc_millis));
    }
    let avg_cpc_millis = (weight > 0)
        .then(|| u64::try_from((weighed_cpc + weight / 2) / weight).unwrap_or(u64::MAX));
    let tally = Tally {
        keywords: keywords.len() as u64,
        main,
        long_tail,
        kept_volume,
        avg_cpc_millis,
    };
    let judgement = judge(&tally, competition, inputs, thresholds);

    Score {
        keywords,
        main,
        long_tail,
        kept_volume,
        sale,
        cpc_converted: judgement.cpc_converted,
        avg_cpc_millis,
        price_per_click: judgement.price_per_click,
        tests: judgement.tests,
        verdict: judgement.verdict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(keyword: &str, volume: u64) -> KeywordRow {
        KeywordRow {
            keyword: keyword.to_owned(),
            volume: Some(volume),
            cpc_usd_cents: None,
            difficulty: None,
            intent: None,
        }
    }

    fn clicked(keyword: &str, volume: u64, cpc_usd_cents: u64) -> KeywordRow {
        KeywordRow {
            cpc_usd_cents: Some(cpc_usd_cents),
            ..row(keyword, volume)
        }
    }

    /// The sale of the catalog's own margin test: 60.00 with 20 % VAT
    /// leaves 23.85, and an ad breaks even at 2.52.
    fn sale() -> Sale {
        Sale {
            currency: "EUR".into(),
            price_cents: Some(6_000),
            cost_cents: Some(1_500),
            shipping_cents: 400,
            import_cents: 500,
            vat_bps: 2_000,
            fixed_cents: 100,
            payment_fee_bps: 150,
            payment_fee_cents: 25,
        }
    }

    /// A euro at 1.10 dollar.
    fn inputs() -> StudyInputs {
        StudyInputs {
            sale: sale(),
            usd_micros: Some(1_100_000),
            ..StudyInputs::default()
        }
    }

    fn scored(rows: Vec<KeywordRow>, inputs: &StudyInputs) -> Score {
        score(
            rows,
            "fr",
            inputs,
            TestState::NotChecked,
            &Thresholds::default(),
        )
    }

    fn outcome<'a>(score: &'a Score, keyword: &str) -> &'a ScoredKeyword {
        score
            .keywords
            .iter()
            .find(|scored| scored.row.keyword == keyword)
            .unwrap_or_else(|| panic!("no {keyword:?}"))
    }

    /// Three main keywords and ten of the long tail, a click at 0.22 $.
    fn enough_rows() -> Vec<KeywordRow> {
        let main = (0..3).map(|n| clicked(&format!("griffoir {n}"), 2_000, 22));
        let long = (0..10).map(|n| clicked(&format!("griffoir mural {n}"), 300, 22));
        main.chain(long).collect()
    }

    #[test]
    fn a_dropped_keyword_carries_the_first_reason_that_drops_it() {
        let inputs = StudyInputs {
            brands: vec!["Zolux".into()],
            excluded_words: vec!["occasion".into()],
            dropped: BTreeSet::from(["arbre à chat pas cher".to_owned()]),
            ..inputs()
        };
        let hard = KeywordRow {
            difficulty: Some(36),
            ..row("arbre à chat design", 900)
        };
        let easy = KeywordRow {
            difficulty: Some(35),
            ..row("arbre à chat mural", 900)
        };
        let told = KeywordRow {
            intent: Some("informational".into()),
            ..row("arbre à chat dangereux", 900)
        };
        // Every reason at once: the hand comes first.
        let all = KeywordRow {
            difficulty: Some(90),
            intent: Some("informational".into()),
            ..row("comment monter arbre à chat zolux occasion", 10)
        };
        let unknown = KeywordRow {
            volume: None,
            ..row("arbre à chat géant", 0)
        };
        let score = scored(
            vec![
                row("arbre à chat pas cher", 5_000),
                row("arbre à chat d'occasion", 5_000),
                row("arbre à chat zolux", 5_000),
                row("comment fabriquer un arbre à chat", 5_000),
                told,
                hard,
                easy,
                row("arbre à chat xxl bois", 149),
                unknown,
                all,
            ],
            &inputs,
        );
        let reason = |keyword: &str| match &outcome(&score, keyword).outcome {
            Outcome::Dropped(reason) => Some(*reason),
            _ => None,
        };
        assert_eq!(reason("arbre à chat pas cher"), Some(DropReason::Excluded));
        assert_eq!(
            reason("arbre à chat d'occasion"),
            Some(DropReason::Excluded)
        );
        assert_eq!(reason("arbre à chat zolux"), Some(DropReason::Brand));
        assert_eq!(
            reason("comment fabriquer un arbre à chat"),
            Some(DropReason::Informational)
        );
        assert_eq!(
            reason("arbre à chat dangereux"),
            Some(DropReason::Informational)
        );
        assert_eq!(
            reason("arbre à chat design"),
            Some(DropReason::TooDifficult)
        );
        assert_eq!(reason("arbre à chat mural"), None, "35 is still kept");
        assert_eq!(reason("arbre à chat xxl bois"), Some(DropReason::LowVolume));
        assert_eq!(reason("arbre à chat géant"), Some(DropReason::LowVolume));
        assert_eq!(
            reason("comment monter arbre à chat zolux occasion"),
            Some(DropReason::Excluded)
        );
    }

    #[test]
    fn a_brand_or_an_excluded_word_is_matched_as_whole_words_whatever_the_accents() {
        let inputs = StudyInputs {
            brands: vec!["Royal Canin".into(), "  ".into()],
            excluded_words: vec!["Bébé".into()],
            ..inputs()
        };
        let score = scored(
            vec![
                row("croquettes royal canin chaton", 500),
                row("croquettes canin royal", 500),
                row("croquettes royales", 500),
                row("croquettes bebe chat", 500),
                row("croquettes bebert", 500),
            ],
            &inputs,
        );
        let dropped =
            |keyword: &str| matches!(outcome(&score, keyword).outcome, Outcome::Dropped(_));
        assert!(dropped("croquettes royal canin chaton"));
        assert!(!dropped("croquettes canin royal"), "the words in a row");
        assert!(!dropped("croquettes royales"));
        assert!(dropped("croquettes bebe chat"));
        assert!(!dropped("croquettes bebert"));
    }

    #[test]
    fn a_question_word_is_read_in_the_language_of_the_study() {
        let rows = || {
            vec![
                row("où acheter un griffoir", 500),
                row("griffoir ou arbre à chat", 500),
                row("how to build a scratching post", 500),
                row("quel griffoir choisir", 500),
            ]
        };
        let informational = |score: &Score, keyword: &str| {
            outcome(score, keyword).outcome == Outcome::Dropped(DropReason::Informational)
        };
        let french = scored(rows(), &inputs());
        assert!(informational(&french, "où acheter un griffoir"));
        assert!(!informational(&french, "griffoir ou arbre à chat"));
        assert!(informational(&french, "quel griffoir choisir"));
        assert!(!informational(&french, "how to build a scratching post"));

        let english = score(
            rows(),
            "en-gb",
            &inputs(),
            TestState::NotChecked,
            &Thresholds::default(),
        );
        assert!(informational(&english, "how to build a scratching post"));
        assert!(!informational(&english, "quel griffoir choisir"));

        // A language with no list is left to what the source says.
        let german = score(
            rows(),
            "de",
            &inputs(),
            TestState::NotChecked,
            &Thresholds::default(),
        );
        assert!(german.keywords.iter().all(|k| k.tier().is_some()));
    }

    #[test]
    fn accent_and_plural_variants_count_once_under_the_most_searched() {
        let score = scored(
            vec![
                row("arbre a chat", 800),
                row("arbres à chat", 400),
                row("arbre à chat", 1_200),
                row("arbre-à-chats", 200),
                // Too little searched to be anybody's variant.
                row("arbre à chàt", 20),
                row("griffoir", 300),
            ],
            &inputs(),
        );
        let leader = outcome(&score, "arbre à chat");
        assert_eq!(leader.outcome, Outcome::Kept(Tier::Main));
        assert_eq!(
            leader.variants,
            ["arbre a chat", "arbres à chat", "arbre-à-chats"]
        );
        for variant in &leader.variants {
            assert_eq!(
                outcome(&score, variant).outcome,
                Outcome::Variant {
                    of: "arbre à chat".into()
                }
            );
        }
        assert_eq!(
            outcome(&score, "arbre à chàt").outcome,
            Outcome::Dropped(DropReason::LowVolume)
        );
        assert_eq!((score.main, score.long_tail), (1, 1));
        assert_eq!(score.kept_volume, 1_500, "a variant's volume is not added");

        // As searched as each other: the first in the alphabet leads.
        let tie = scored(vec![row("gants", 500), row("gant", 500)], &inputs());
        assert_eq!(outcome(&tie, "gant").variants, ["gants"]);
        // A short word keeps its last letter.
        let short = scored(vec![row("bas", 500), row("ba", 500)], &inputs());
        assert_eq!((short.main, short.long_tail), (0, 2));
    }

    #[test]
    fn kept_keywords_are_main_from_a_thousand_searches_and_long_tail_from_150() {
        let score = scored(
            vec![
                row("griffoir", 1_000),
                row("griffoir carton", 999),
                row("griffoir mural", 150),
                row("griffoir angle", 149),
            ],
            &inputs(),
        );
        assert_eq!(outcome(&score, "griffoir").tier(), Some(Tier::Main));
        assert_eq!(
            outcome(&score, "griffoir carton").tier(),
            Some(Tier::LongTail)
        );
        assert_eq!(
            outcome(&score, "griffoir mural").tier(),
            Some(Tier::LongTail)
        );
        assert_eq!(outcome(&score, "griffoir angle").tier(), None);
        assert_eq!((score.main, score.long_tail), (1, 2));
        assert_eq!(score.kept_volume, 2_149);
    }

    #[test]
    fn a_keyword_brought_back_is_kept_whatever_the_filters_say() {
        let hard = KeywordRow {
            difficulty: Some(80),
            intent: Some("informational".into()),
            ..row("griffoir zolux", 40)
        };
        let inputs = StudyInputs {
            brands: vec!["zolux".into()],
            dropped: BTreeSet::from(["griffoir zolux".to_owned()]),
            restored: BTreeSet::from(["griffoir zolux".to_owned()]),
            ..inputs()
        };
        let score = scored(vec![hard], &inputs);
        assert_eq!(
            outcome(&score, "griffoir zolux").outcome,
            Outcome::Kept(Tier::LongTail)
        );
    }

    #[test]
    fn a_keyword_two_seeds_gave_is_scored_once() {
        let score = scored(
            vec![
                row("griffoir", 300),
                row("griffoir", 1_300),
                row("griffoir", 200),
            ],
            &inputs(),
        );
        assert_eq!(score.keywords.len(), 1);
        assert_eq!(score.keywords[0].row.volume, Some(1_300));
        assert!(score.keywords[0].variants.is_empty());
    }

    #[test]
    fn the_economics_of_a_sale_are_the_catalog_s_margin() {
        let score = scored(vec![], &inputs());
        let economics = score.sale.unwrap();
        assert_eq!(economics.margin.margin_cents(), 2_385);
        assert_eq!(economics.margin.break_even_roas(), Some(252));
        // (2.52 + 0.4) / 1.5 % = 194.67, rounded up.
        assert_eq!(economics.needed_price_per_click, Some(19_467));
        // 60.00 / 194.67: a click may cost 0.308.
        assert_eq!(economics.highest_cpc_millis, Some(30_821));

        let set = Thresholds {
            safety: 100,
            conversion_bps: 300,
            ..Thresholds::default()
        };
        let score = score_with(&set);
        // (2.52 + 1) / 3 %.
        assert_eq!(score.sale.unwrap().needed_price_per_click, Some(11_734));

        // Until a price and a cost are said, nothing is worked out.
        for sale in [
            Sale {
                price_cents: None,
                ..sale()
            },
            Sale {
                cost_cents: None,
                ..sale()
            },
        ] {
            let score = scored(enough_rows(), &StudyInputs { sale, ..inputs() });
            assert_eq!(score.sale, None);
            assert_eq!(score.test(Test::Margin), TestState::NotChecked);
            assert_eq!(score.test(Test::PricePerClick), TestState::NotChecked);
            assert!(score.keywords.iter().all(|k| k.channel.is_none()));
        }
    }

    fn score_with(thresholds: &Thresholds) -> Score {
        score(
            Vec::new(),
            "fr",
            &inputs(),
            TestState::NotChecked,
            thresholds,
        )
    }

    #[test]
    fn a_keyword_is_for_ads_when_its_own_click_costs_little_enough() {
        let score = scored(
            vec![
                // 0.33 $ is 0.30 €: 60.00 / 0.30 = 200.
                clicked("griffoir", 500, 33),
                // 0.35 $ is 0.318 €: 188.57.
                clicked("griffoir carton", 500, 35),
                row("griffoir mural", 500),
                clicked("griffoir gratuit", 500, 0),
                clicked("griffoir angle", 20, 1),
            ],
            &inputs(),
        );
        assert!(score.cpc_converted);
        let cheap = outcome(&score, "griffoir");
        assert_eq!(cheap.cpc_millis, Some(30_000));
        assert_eq!(cheap.price_per_click, Some(20_000));
        assert_eq!(cheap.channel, Some(Channel::Ads));
        let dear = outcome(&score, "griffoir carton");
        assert_eq!(dear.cpc_millis, Some(31_818));
        assert_eq!(dear.price_per_click, Some(18_857));
        assert_eq!(dear.channel, Some(Channel::SeoOnly));
        // No cost per click from the source: nothing is said.
        assert_eq!(outcome(&score, "griffoir mural").channel, None);
        let free = outcome(&score, "griffoir gratuit");
        assert_eq!(free.price_per_click, None);
        assert_eq!(free.channel, Some(Channel::Ads));
        // A dropped keyword has its ratio, and no channel.
        let dropped = outcome(&score, "griffoir angle");
        assert!(dropped.price_per_click.is_some());
        assert_eq!(dropped.channel, None);
    }

    #[test]
    fn without_a_rate_dollars_stand_against_the_price_as_they_are() {
        let rows = || vec![clicked("griffoir", 500, 33)];
        let unconverted = scored(
            rows(),
            &StudyInputs {
                usd_micros: None,
                ..inputs()
            },
        );
        assert!(!unconverted.cpc_converted);
        assert_eq!(unconverted.keywords[0].cpc_millis, Some(33_000));
        assert_eq!(unconverted.keywords[0].channel, Some(Channel::SeoOnly));

        // A sale in dollars needs no rate, and ignores one.
        let dollars = scored(
            rows(),
            &StudyInputs {
                sale: Sale {
                    currency: "USD".into(),
                    ..sale()
                },
                usd_micros: Some(2_000_000),
                ..inputs()
            },
        );
        assert!(dollars.cpc_converted);
        assert_eq!(dollars.keywords[0].cpc_millis, Some(33_000));
    }

    #[test]
    fn the_study_is_judged_on_the_click_averaged_by_volume() {
        // 0.11 $ (0.10 €) over 3,000 searches, 0.66 $ (0.60 €) over 1,000:
        // 0.225 € on average, where a plain mean would say 0.35.
        let rows = vec![
            clicked("griffoir", 3_000, 11),
            clicked("griffoir carton", 1_000, 66),
            // Neither a dropped keyword nor one without a click weighs.
            clicked("griffoir angle", 100, 900),
            row("griffoir mural", 5_000),
        ];
        let score = scored(rows, &inputs());
        assert_eq!(score.avg_cpc_millis, Some(22_500));
        assert_eq!(score.price_per_click, Some(26_666));
        assert_eq!(score.test(Test::PricePerClick), TestState::Passed);
        // Its own click is too dear for one keyword all the same.
        assert_eq!(
            outcome(&score, "griffoir carton").channel,
            Some(Channel::SeoOnly)
        );

        let dear = scored(vec![clicked("griffoir", 3_000, 40)], &inputs());
        assert_eq!(dear.test(Test::PricePerClick), TestState::Failed);
        assert_eq!(dear.verdict, Verdict::NoGo);

        // No kept keyword has a cost per click: not checked.
        let silent = scored(vec![row("griffoir", 3_000)], &inputs());
        assert_eq!(silent.avg_cpc_millis, None);
        assert_eq!(silent.test(Test::PricePerClick), TestState::NotChecked);
    }

    #[test]
    fn a_sale_that_leaves_nothing_fails_the_margin_and_pays_for_no_ad() {
        let inputs = StudyInputs {
            sale: Sale {
                cost_cents: Some(5_000),
                ..sale()
            },
            ..inputs()
        };
        let score = scored(enough_rows(), &inputs);
        let sale = score.sale.unwrap();
        assert!(sale.margin.margin_cents() < 0);
        assert_eq!(sale.needed_price_per_click, None);
        assert_eq!(sale.highest_cpc_millis, None);
        assert_eq!(score.test(Test::Margin), TestState::Failed);
        assert_eq!(score.test(Test::PricePerClick), TestState::Failed);
        assert!(
            score
                .keywords
                .iter()
                .all(|k| k.channel == Some(Channel::SeoOnly))
        );
        assert_eq!(score.verdict, Verdict::NoGo);
    }

    #[test]
    fn a_study_is_a_go_when_every_test_passes() {
        let checked = StudyInputs {
            supplier_products: Some(200),
            ..inputs()
        };
        let all = |inputs: &StudyInputs, competition: TestState| {
            score(
                enough_rows(),
                "fr",
                inputs,
                competition,
                &Thresholds::default(),
            )
        };
        let score = all(&checked, TestState::Passed);
        assert_eq!((score.main, score.long_tail), (3, 10));
        assert_eq!(
            score.tests.map(|(test, _)| test),
            Test::ALL,
            "in the order of a report"
        );
        assert!(
            score
                .tests
                .iter()
                .all(|(_, state)| *state == TestState::Passed),
            "{:?}",
            score.tests
        );
        assert_eq!(score.verdict, Verdict::Go);

        // What is counted by hand is not checked until it is, nor the
        // competition until the first results are looked at.
        let unchecked = scored(enough_rows(), &inputs());
        assert_eq!(
            unchecked.test(Test::SupplierProducts),
            TestState::NotChecked
        );
        assert_eq!(unchecked.test(Test::NoSpecialist), TestState::NotChecked);
        assert_eq!(unchecked.verdict, Verdict::Incomplete);

        // One failure is a no-go, whatever else is missing.
        let few = StudyInputs {
            supplier_products: Some(199),
            ..checked.clone()
        };
        assert_eq!(all(&few, TestState::Passed).verdict, Verdict::NoGo);
        assert_eq!(all(&inputs(), TestState::Failed).verdict, Verdict::NoGo);
        assert_eq!(
            all(&checked, TestState::NotChecked).verdict,
            Verdict::Incomplete
        );
    }

    #[test]
    fn enough_keywords_is_a_setting() {
        let checked = StudyInputs {
            supplier_products: Some(500),
            ..inputs()
        };
        let mut rows = enough_rows();
        rows.pop();
        let short = score(
            rows.clone(),
            "fr",
            &checked,
            TestState::Passed,
            &Thresholds::default(),
        );
        assert_eq!(short.test(Test::MainKeywords), TestState::Passed);
        assert_eq!(short.test(Test::LongTail), TestState::Failed);
        assert_eq!(short.verdict, Verdict::NoGo);

        let lenient = Thresholds {
            enough_long_tail: 9,
            ..Thresholds::default()
        };
        assert_eq!(
            score(rows.clone(), "fr", &checked, TestState::Passed, &lenient).verdict,
            Verdict::Go
        );

        // A stricter difficulty drops what the default kept.
        let strict = Thresholds {
            max_difficulty: 10,
            ..Thresholds::default()
        };
        let rows = rows.into_iter().map(|row| KeywordRow {
            difficulty: Some(11),
            ..row
        });
        let none = score(rows, "fr", &checked, TestState::Passed, &strict);
        assert_eq!((none.main, none.long_tail), (0, 0));
        assert_eq!(none.test(Test::MainKeywords), TestState::Failed);
    }

    /// A specialist found for several main keywords holds the niche; a
    /// site found for one decides nothing, marked or not.
    #[test]
    fn a_specialist_for_several_main_keywords_fails_the_competition() {
        // Nothing searched yet.
        assert_eq!(competition(0, 3, []).state, TestState::NotChecked);
        // amazon for three keywords, a specialist for one, a blog for one.
        let sites = |amazon: Option<bool>, shop: Option<bool>| [(3, amazon), (1, shop), (1, None)];
        let unmarked = competition(3, 0, sites(None, None));
        assert_eq!(unmarked.state, TestState::NotChecked);
        assert_eq!((unmarked.recurring, unmarked.unmarked), (1, 1));
        let generalist = competition(3, 0, sites(Some(false), Some(true)));
        assert_eq!(generalist.state, TestState::Passed);
        assert_eq!((generalist.unmarked, generalist.specialists), (0, 0));
        // A main keyword still without its results keeps it open.
        assert_eq!(
            competition(3, 1, sites(Some(false), None)).state,
            TestState::NotChecked
        );
        // A specialist for two fails it, whatever is left to look at.
        let held = competition(2, 5, [(2, Some(true)), (2, None)]);
        assert_eq!(held.state, TestState::Failed);
        assert_eq!((held.recurring, held.unmarked, held.specialists), (2, 1, 1));
        // Searched, and no site comes back twice: nobody holds it.
        assert_eq!(
            competition(2, 0, [(1, None), (1, None)]).state,
            TestState::Passed
        );
    }

    #[test]
    fn one_more_test_joins_a_verdict() {
        use TestState::{Failed, NotChecked, Passed};
        assert_eq!(Verdict::Go.with(Passed), Verdict::Go);
        assert_eq!(Verdict::Go.with(NotChecked), Verdict::Incomplete);
        assert_eq!(Verdict::Go.with(Failed), Verdict::NoGo);
        assert_eq!(Verdict::Incomplete.with(Passed), Verdict::Incomplete);
        assert_eq!(Verdict::Incomplete.with(Failed), Verdict::NoGo);
        assert_eq!(Verdict::NoGo.with(Passed), Verdict::NoGo);
        for verdict in [Verdict::Go, Verdict::NoGo, Verdict::Incomplete] {
            assert_eq!(Verdict::parse(verdict.as_str()), verdict);
        }
        for state in [Passed, Failed, NotChecked] {
            assert_eq!(TestState::parse(state.as_str()), state);
        }
    }

    #[test]
    fn a_study_with_nothing_fetched_is_incomplete_not_a_no_go() {
        let score = scored(vec![], &StudyInputs::default());
        assert!(
            score
                .tests
                .iter()
                .all(|(_, state)| *state == TestState::NotChecked)
        );
        assert_eq!(score.verdict, Verdict::Incomplete);
    }
}
