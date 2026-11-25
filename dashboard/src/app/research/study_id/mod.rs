//! `/research/{study_id}`: one study, for its owner: its seeds and where
//! the fetch of each stands, the verdict and the tests behind it, what one
//! sale leaves, the numbers the score reads, and the keywords with what the
//! score made of each, and who holds the first results of its main
//! keywords. Where a fetch stands, what the owner said and how they marked
//! a site are read from the streams; the keywords, what they come to and
//! the sites found are the shared read model's, a moment behind.

pub mod competition;
pub mod delete;
pub mod drop;
pub mod export;
pub mod fetch;
pub mod inputs;
pub mod mark;
pub mod rename;
pub mod restore;
pub mod seeds;
pub mod tree;

use timada_core::page::{Cursor, PageFrom};
use timada_research::{
    CPC_CURRENCY, Channel, Command, Competition, DUPLICATE, DomainRow, DropReason, ENOUGH_PRODUCTS,
    FetchStatus, Judgement, KeywordRecord, MAX_CHECKED_KEYWORDS, MAX_SEEDS, ResearchError,
    SEVERAL_KEYWORDS, SeedState, SpendCap, StudyState, Tally, Test, TestState, Thresholds, Tier,
    VARIANT, Verdict, competition as judge_competition, domains_of_study, judge,
    keywords_of_outcome, keywords_of_study, main_keywords, outcome_counts, spend_day,
    spend_day_ends, study_row,
};
use topcoat::{
    Result,
    context::{Cx, context},
    icon::icon,
    router::{error::not_found, href, module_param, page, query_params},
    view::{View, attributes, view},
};

use super::research;
use crate::auth::require_user;
use crate::cluster::LocalReads;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::pagination::{
    pagination, pagination_content, pagination_item, pagination_next, pagination_previous,
};
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::components::textarea::textarea;
use crate::config::Config;
use crate::db::Db;
use crate::keywords::Keywords;
use crate::research::{Spending, cap_reached, format_click, format_ratio, format_usd};
use crate::ui::{
    LINK, PageTab, Shell, Tone, document, empty_row, fact, facts, format_date, icons, notice,
    page_header, page_tabs, section_card, signed_money, status_badge,
};

module_param!(pub study_id);

/// The page shows this many keywords at once.
const PAGE_SIZE: i64 = 100;

#[topcoat::router::query_params(error = bad_request)]
struct StudyQuery {
    /// Which of the keyword lists is shown ([`LISTS`]); all of them
    /// without one.
    list: Option<String>,
    /// The cursor of the row the page starts after (less searched keywords).
    after: Option<String>,
    /// The cursor of the row the page ends before (more searched keywords).
    before: Option<String>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct StudyLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    list: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
}

pub fn study_url(cx: &Cx, study_id: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, study_id: study_id.to_owned())
        .query(StudyLink {
            list: None,
            after: None,
            before: None,
            error,
            message,
        })
        .resolve(cx)
}

/// The study's page at who holds the first results.
pub(crate) fn competition_url(
    cx: &Cx,
    study_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    format!("{}#competition", study_url(cx, study_id, error, message))
}

/// The study's page on one of its keyword lists, at the keywords.
pub(crate) fn list_url(
    cx: &Cx,
    study_id: &str,
    list: Option<&str>,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    let url = href!(index, study_id: study_id.to_owned())
        .query(StudyLink {
            list: list_of(list).filter(|list| *list != ALL),
            after: None,
            before: None,
            error,
            message,
        })
        .resolve(cx);
    format!("{url}#keywords")
}

/// A keyword's row asks to drop it or to bring it back.
#[derive(serde::Deserialize)]
pub struct KeywordForm {
    keyword: String,
    /// The list the row was on, to come back to.
    #[serde(default)]
    list: Option<String>,
}

/// The list of every keyword, whatever the score made of it.
const ALL: &str = "all";

/// The lists a study's keywords are read by: all of them, the kept ones by
/// tier, the spellings that count under another, then the dropped ones by
/// reason. Each key but the first is an outcome of the read model.
const LISTS: [(&str, &str); 9] = [
    (ALL, "All"),
    ("main", "Main"),
    ("long_tail", "Long tail"),
    (VARIANT, "Other spellings"),
    ("excluded", "Excluded"),
    ("brand", "Brand"),
    ("informational", "Informational"),
    ("too_difficult", "Too difficult"),
    ("low_volume", "Volume too low"),
];

/// The key of the list a parameter names, when it names one.
fn list_of(list: Option<&str>) -> Option<&'static str> {
    LISTS
        .iter()
        .map(|(key, _)| *key)
        .find(|key| Some(*key) == list)
}

/// How a keyword's outcome reads in the list of all of them.
fn outcome_label(outcome: &str) -> (&'static str, Tone) {
    match outcome {
        "main" => ("Main", Tone::Success),
        "long_tail" => ("Long tail", Tone::Success),
        VARIANT => ("Other spelling", Tone::Neutral),
        "excluded" => ("Excluded", Tone::Warning),
        "brand" => ("Brand", Tone::Warning),
        "informational" => ("Informational", Tone::Warning),
        "too_difficult" => ("Too difficult", Tone::Warning),
        "low_volume" => ("Volume too low", Tone::Warning),
        DUPLICATE => ("Under another seed", Tone::Neutral),
        _ => ("Not scored yet", Tone::Neutral),
    }
}

/// One test of the verdict, as the page words it.
struct TestLine {
    /// The test's key, which a test of the page reads.
    key: &'static str,
    name: &'static str,
    /// `passed`, `failed` or `not_checked`.
    state: &'static str,
    label: &'static str,
    tone: Tone,
    /// The numbers behind it.
    numbers: String,
}

/// Who holds the first results, as the page counts it.
struct Held {
    /// How many main keywords have their first results looked at.
    mains: usize,
    /// How many of them have theirs, and how many are still without, as
    /// the sites found were last worked out.
    searched: i64,
    owed: i64,
    stands: Competition,
}

/// The six tests with the numbers behind each.
fn test_lines(
    study: &StudyState,
    tally: &Tally,
    held: &Held,
    judgement: &Judgement,
    thresholds: &Thresholds,
) -> Vec<TestLine> {
    let fetched = tally.keywords > 0;
    let currency = study.inputs.sale.currency.as_str();
    Test::ALL
        .into_iter()
        .map(|test| {
            let state = judgement.test(test);
            let (label, tone) = match state {
                TestState::Passed => ("Passed", Tone::Success),
                TestState::Failed => ("Failed", Tone::Danger),
                TestState::NotChecked => ("Not checked", Tone::Neutral),
            };
            let (name, numbers) = match test {
                Test::MainKeywords => (
                    "Main keywords",
                    if fetched {
                        format!(
                            "{} kept with 1,000 searches a month or more; {} are needed.",
                            tally.main, thresholds.enough_main
                        )
                    } else {
                        "No keyword fetched yet.".to_owned()
                    },
                ),
                Test::LongTail => (
                    "Long-tail keywords",
                    if fetched {
                        format!(
                            "{} kept with 150 to 999 searches a month; {} are needed.",
                            tally.long_tail, thresholds.enough_long_tail
                        )
                    } else {
                        "No keyword fetched yet.".to_owned()
                    },
                ),
                Test::Margin => (
                    "Margin",
                    match judgement.sale {
                        Some(sale) => format!(
                            "A sale at {} leaves {}.",
                            timada_core::format_money(sale.margin.price_cents, currency),
                            signed_money(sale.margin.margin_cents(), currency)
                        ),
                        None => "Say the price and the supplier's cost below.".to_owned(),
                    },
                ),
                Test::PricePerClick => (
                    "Price / CPC",
                    match (judgement.sale, judgement.price_per_click) {
                        (None, _) => "Say the price and the supplier's cost below.".to_owned(),
                        (Some(sale), _) if sale.needed_price_per_click.is_none() => {
                            "The sale leaves nothing: no ad pays for itself.".to_owned()
                        }
                        (Some(sale), Some(ratio)) => format!(
                            "{} over the kept keywords, by volume; {} is needed.",
                            format_ratio(ratio),
                            sale.needed_price_per_click
                                .map(format_ratio)
                                .unwrap_or_default()
                        ),
                        (Some(_), None) if tally.avg_cpc_millis.is_some() => {
                            "A click on the kept keywords costs nothing.".to_owned()
                        }
                        (Some(_), None) => {
                            "No kept keyword has both a volume and a cost per click.".to_owned()
                        }
                    },
                ),
                Test::SupplierProducts => (
                    "Products at the supplier",
                    match study.inputs.supplier_products {
                        Some(products) => {
                            format!("{products} found; {ENOUGH_PRODUCTS} are needed.")
                        }
                        None => "Count the good products at the supplier and say below.".to_owned(),
                    },
                ),
                Test::NoSpecialist => (
                    "No specialist ahead",
                    if held.stands.specialists > 0 {
                        format!(
                            "{} found in the first results of {SEVERAL_KEYWORDS} main keywords or more.",
                            counted(held.stands.specialists as usize, "specialist shop")
                        )
                    } else if held.mains == 0 {
                        "No main keyword yet.".to_owned()
                    } else if held.searched == 0 {
                        "Search the first results of the main keywords below.".to_owned()
                    } else if held.owed > 0 {
                        format!(
                            "{} still without {} first results.",
                            counted(held.owed as usize, "main keyword"),
                            if held.owed == 1 { "its" } else { "their" }
                        )
                    } else if held.stands.unmarked > 0 {
                        format!(
                            "{} found for {SEVERAL_KEYWORDS} main keywords or more to mark below.",
                            counted(held.stands.unmarked as usize, "site")
                        )
                    } else {
                        format!(
                            "No specialist shop in the first results of {SEVERAL_KEYWORDS} main keywords or more."
                        )
                    },
                ),
            };
            TestLine {
                key: test.as_str(),
                name,
                state: state.as_str(),
                label,
                tone,
                numbers,
            }
        })
        .collect()
}

/// `1 site`, `3 sites`.
fn counted(count: usize, what: &str) -> String {
    format!("{count} {what}{}", if count == 1 { "" } else { "s" })
}

/// A site found in the first results, as the page words it.
struct SiteLine {
    domain: String,
    keywords: i64,
    positions: String,
    /// `specialist`, `generalist`, or empty until marked.
    mark: &'static str,
    label: &'static str,
    tone: Tone,
    /// Whether it was found for enough keywords to decide.
    decides: bool,
}

impl SiteLine {
    fn of(row: &DomainRow, specialist: Option<bool>) -> Self {
        let (mark, label, tone) = match specialist {
            Some(true) => ("specialist", "Specialist", Tone::Danger),
            Some(false) => ("generalist", "Generalist", Tone::Success),
            None => ("", "Not marked", Tone::Neutral),
        };
        Self {
            domain: row.domain.clone(),
            keywords: row.keywords,
            positions: row.positions.clone(),
            mark,
            label,
            tone,
            decides: row.keywords >= i64::from(SEVERAL_KEYWORDS),
        }
    }
}

/// The economics of one sale, line by line; nothing until the price and
/// the supplier's cost are both said.
fn sale_lines(
    study: &StudyState,
    tally: &Tally,
    judgement: &Judgement,
    thresholds: &Thresholds,
) -> Vec<(&'static str, &'static str, String)> {
    let Some(sale) = judgement.sale else {
        return Vec::new();
    };
    let currency = study.inputs.sale.currency.as_str();
    // What the costs per click are in: the sale's currency once converted.
    let click_currency = if judgement.cpc_converted {
        currency
    } else {
        CPC_CURRENCY
    };
    let money = |cents: u64| timada_core::format_money(cents, currency);
    let margin = sale.margin;
    let mut lines = vec![
        ("price", "Price, VAT included", money(margin.price_cents)),
        ("vat", "VAT", money(margin.vat_cents)),
        ("cost", "Supplier", money(margin.cost_cents)),
        ("shipping", "Sending", money(margin.shipping_cents)),
        ("import", "At the border", money(margin.import_cents)),
        ("fixed", "Fixed costs", money(margin.fixed_cents)),
        ("fee", "Payment fee", money(margin.payment_fee_cents)),
        (
            "margin",
            "What the sale leaves",
            match margin.rate_bps() {
                Some(rate) => format!(
                    "{} ({}{} % of the price)",
                    signed_money(margin.margin_cents(), currency),
                    if rate < 0 { "-" } else { "" },
                    timada_core::format_amount(rate.unsigned_abs())
                ),
                None => signed_money(margin.margin_cents(), currency),
            },
        ),
        (
            "break-even",
            "Break-even return on ad spend",
            margin
                .break_even_roas()
                .map_or_else(|| "None: no ad pays for itself".to_owned(), format_ratio),
        ),
    ];
    if let Some(needed) = sale.needed_price_per_click {
        lines.push((
            "needed",
            "Price / CPC needed",
            format!(
                "{} (break-even + {}, at a conversion rate of {} %)",
                format_ratio(needed),
                format_ratio(u64::from(thresholds.safety)),
                timada_core::format_amount(u64::from(thresholds.conversion_bps))
            ),
        ));
    }
    if let Some(highest) = sale.highest_cpc_millis {
        lines.push((
            "highest-cpc",
            "Highest cost per click it allows",
            format_click(highest, currency),
        ));
    }
    if let Some(average) = tally.avg_cpc_millis {
        lines.push((
            "average-cpc",
            "Cost per click of the kept keywords, by volume",
            format_click(average, click_currency),
        ));
    }
    if let Some(ratio) = judgement.price_per_click {
        lines.push(("ratio", "Price / CPC of the study", format_ratio(ratio)));
    }
    lines
}

/// One keyword's row, as the page words it.
struct KeywordLine {
    keyword: String,
    volume: String,
    difficulty: String,
    /// As the source gives it, in US dollars.
    cpc: Option<String>,
    /// The sale's price over its cost per click.
    ratio: Option<String>,
    /// `ads` or `seo_only`, for a kept keyword once the sale is known.
    channel: Option<Channel>,
    intent: String,
    outcome: String,
    status: &'static str,
    tone: Tone,
    /// The keyword it counts under.
    variant_of: Option<String>,
    /// Whether its row offers to drop it (`Some(true)`), to bring it back
    /// (`Some(false)`), or neither.
    drops: Option<bool>,
}

impl KeywordLine {
    fn of(row: &KeywordRecord, study: &StudyState, judgement: &Judgement) -> Self {
        let click = row
            .cpc_usd_cents
            .map(|cents| study.inputs.cpc_millis(cents as u64));
        let kept = [Tier::Main.as_str(), Tier::LongTail.as_str()].contains(&row.outcome.as_str());
        let dropped = DropReason::ALL
            .iter()
            .any(|reason| reason.as_str() == row.outcome);
        let (status, tone) = outcome_label(&row.outcome);
        Self {
            keyword: row.keyword.clone(),
            volume: figure(row.volume),
            difficulty: figure(row.difficulty),
            cpc: row
                .cpc_usd_cents
                .map(|cents| format!("${}", timada_core::format_amount(cents as u64))),
            ratio: judgement
                .sale
                .zip(click)
                .and_then(|(sale, click)| sale.price_per_click(click))
                .map(format_ratio),
            channel: judgement
                .sale
                .filter(|_| kept)
                .and_then(|sale| sale.channel(click)),
            intent: row.intent.clone().unwrap_or_else(|| "–".to_owned()),
            outcome: row.outcome.clone(),
            status,
            tone,
            variant_of: row.variant_of.clone(),
            drops: if kept || row.outcome == VARIANT {
                Some(true)
            } else if dropped {
                Some(false)
            } else {
                None
            },
        }
    }
}

/// The study in the URL and the signed-in user, who must own it (else 404:
/// a study is nobody else's business).
pub(crate) async fn require_owned(cx: &Cx) -> Result<(StudyState, String)> {
    let user = require_user(cx).await?;
    match research(cx).require_owned(study_id(cx), &user.id).await {
        Ok(study) => Ok((study, user.id.clone())),
        Err(ResearchError::NotFound) => Err(not_found().into()),
        Err(err) => Err(err.into()),
    }
}

/// A seed's row, as the page words it.
struct SeedLine {
    seed: String,
    /// `fetching`, `fetched`, `failed`, `stalled` or `never`: what a test
    /// reads.
    state: &'static str,
    label: &'static str,
    tone: Tone,
    keywords: u32,
    /// The day of the last fetch that gave its rows.
    fetched: Option<String>,
    error: Option<String>,
    /// What the button that asks for its keywords says; none while they are
    /// on their way, or so recent that asking again would give them back.
    action: Option<&'static str>,
}

impl SeedLine {
    /// `reuse_secs`: for how long a seed's keywords answer a new fetch of
    /// it.
    fn of(seed: &SeedState, now: u64, reuse_secs: u64) -> Self {
        let fresh = seed
            .fetched_at
            .is_some_and(|at| now.saturating_sub(at) < reuse_secs);
        let (state, label, tone, action) = if seed.is_fetching(now) {
            ("fetching", "Fetching…", Tone::Info, None)
        } else {
            match seed.status {
                FetchStatus::Fetched => (
                    "fetched",
                    "Fetched",
                    Tone::Success,
                    (!fresh).then_some("Fetch again"),
                ),
                FetchStatus::Failed => ("failed", "Failed", Tone::Danger, Some("Try again")),
                FetchStatus::Never => ("never", "Not fetched", Tone::Neutral, Some("Fetch")),
                // Asked for long ago and never answered: the node that took
                // it on stopped.
                FetchStatus::Requested | FetchStatus::Fetching => {
                    ("stalled", "No answer", Tone::Warning, Some("Try again"))
                }
            }
        };
        Self {
            seed: seed.seed.clone(),
            state,
            label,
            tone,
            keywords: seed.keywords,
            fetched: seed.fetched_at.map(format_date),
            error: seed.last_error.clone(),
            action,
        }
    }
}

/// A figure the source may have nothing for: never shown as a zero.
fn figure(value: Option<i64>) -> String {
    value.map_or_else(|| "–".to_owned(), |value| value.to_string())
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let (study, user_id) = require_owned(cx).await?;
    let query = query_params::<StudyQuery>(cx)?;
    let after = query.after.as_deref().and_then(Cursor::parse);
    let before = query.before.as_deref().and_then(Cursor::parse);
    let db = context::<Db>(cx);
    let list = list_of(query.list.as_deref()).unwrap_or(ALL);
    let from = PageFrom::of(after.as_ref(), before.as_ref());
    let listed = if list == ALL {
        keywords_of_study(db, &study.id, from, PAGE_SIZE).await
    } else {
        keywords_of_outcome(db, &study.id, list, from, PAGE_SIZE).await
    }
    .map_err(topcoat::Error::from_anyhow)?;
    let counts = outcome_counts(db, &study.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    // What the keywords come to is the read model's; the tests are run
    // again here over what the owner says now, so a save shows at once.
    let scored = study_row(db, &study.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let tally = scored.as_ref().map(|row| row.tally()).unwrap_or_default();
    let now = crate::auth::now();
    // Who holds the first results: the sites found are the read model's,
    // and how the owner marked them is read from their own stream, so a
    // mark shows at once.
    let mains = main_keywords(db, &study.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let found = domains_of_study(db, &study.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let marks = research(cx)
        .load_marks(&user_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let sites: Vec<SiteLine> = found
        .iter()
        .map(|row| SiteLine::of(row, marks.specialist(&row.domain)))
        .collect();
    let (searched, still_owed) = scored.as_ref().map_or((0, mains.len() as i64), |row| {
        (row.searched_count, row.owed_count)
    });
    let held = Held {
        mains: mains.len(),
        searched,
        owed: still_owed,
        stands: judge_competition(
            searched as u32,
            still_owed as u32,
            found
                .iter()
                .map(|row| (row.keywords as u32, marks.specialist(&row.domain))),
        ),
    };
    // Where each search stands is the study's own to say.
    let searching = mains
        .iter()
        .filter(|keyword| {
            study
                .check(keyword)
                .is_some_and(|check| check.is_fetching(now))
        })
        .count();
    let to_search = mains
        .iter()
        .filter(|keyword| study.check(keyword).is_none_or(|check| check.is_owed(now)))
        .count();
    let mut search_errors: Vec<String> = mains
        .iter()
        .filter_map(|keyword| study.check(keyword)?.last_error.clone())
        .collect();
    search_errors.dedup();
    search_errors.truncate(3);
    let thresholds = Thresholds::default();
    let judgement = judge(&tally, held.stands.state, &study.inputs, &thresholds);
    let (verdict, verdict_tone, verdict_text) = match judgement.verdict {
        Verdict::Go => ("Go", Tone::Success, "Every test passes."),
        Verdict::NoGo => ("No-go", Tone::Danger, "One test at least fails."),
        Verdict::Incomplete => (
            "Incomplete",
            Tone::Neutral,
            "No test fails, and one at least is not checked yet.",
        ),
    };
    let tests = test_lines(&study, &tally, &held, &judgement, &thresholds);
    let economics = sale_lines(&study, &tally, &judgement, &thresholds);
    let said = &study.inputs;
    let amount = timada_core::format_amount;
    let optional = |cents: Option<u64>| cents.map(amount).unwrap_or_default();
    let never_said = study.inputs_set_at.is_none();
    // A study nothing was said of yet shows empty fields, not zeros.
    let or_empty = |value: String| if never_said { String::new() } else { value };
    let currency = if said.sale.currency.is_empty() {
        timada_core::CURRENCIES[0].to_owned()
    } else {
        said.sale.currency.clone()
    };
    let form_price = optional(said.sale.price_cents);
    let form_cost = optional(said.sale.cost_cents);
    let form_shipping = or_empty(amount(said.sale.shipping_cents));
    let form_import = or_empty(amount(said.sale.import_cents));
    let form_vat = or_empty(amount(u64::from(said.sale.vat_bps)));
    let form_fixed = or_empty(amount(said.sale.fixed_cents));
    let form_fee_percent = or_empty(amount(u64::from(said.sale.payment_fee_bps)));
    let form_fee_fixed = or_empty(amount(said.sale.payment_fee_cents));
    let form_brands = said.brands.join("\n");
    let form_excluded = said.excluded_words.join("\n");
    let form_products = said
        .supplier_products
        .map(|products| products.to_string())
        .unwrap_or_default();
    // How the costs per click, which the source gives in US dollars, stand
    // against the sale's price.
    let rate_text = if never_said || said.sale.currency == CPC_CURRENCY {
        None
    } else {
        Some(match said.usd_micros {
            Some(micros) => format!(
                "A cost per click is in US dollars: it is read at 1 {} = ${}.{:04}, the euro reference rate of {}, when you last saved.",
                said.sale.currency,
                micros / 1_000_000,
                (micros % 1_000_000) / 100,
                study.inputs_set_at.map(format_date).unwrap_or_default()
            ),
            None => format!(
                "No exchange rate could be read when you last saved: a cost per click in US dollars stands against a price in {} as it is. Save again later to convert it.",
                said.sale.currency
            ),
        })
    };
    let unconverted = !never_said && !judgement.cpc_converted;
    let source = &context::<Keywords>(cx).0;
    let enabled = source.is_some();
    // What the searches asked for now would cost at the most.
    let search_cost = |keywords: usize| {
        source.as_ref().map_or_else(String::new, |source| {
            format_usd(source.most_results_usd_micros() * keywords as u64)
        })
    };
    let search_owed = format!(
        "Search the first results of {}",
        counted(to_search, "main keyword")
    );
    let search_owed_cost = format!(
        "One paid search each: {} at the most.",
        search_cost(to_search)
    );
    let search_again_cost = format!(
        "Every main keyword has its first results. Searching the {} again costs {} at the most.",
        mains.len(),
        search_cost(mains.len())
    );
    let competition_description = format!(
        "The sites in the first two pages of a search for each main keyword (the {MAX_CHECKED_KEYWORDS} most searched at the most), in the study's country and language. A niche whose first results are marketplaces and department stores can be taken by a specialist shop; one a specialist already holds is hard. Say of each site found for {SEVERAL_KEYWORDS} keywords or more which it is: a mark is kept for your other studies."
    );
    let config = &context::<Config>(cx).research;
    let spending = Spending::new(config);
    // What the day's calls cost the user so far, and the cap the next fetch
    // would pass: read from this node's file, as the saga that decides
    // reads it again through the cluster.
    let (spent, capped) = match source {
        Some(source) => {
            let day = Command(context::<LocalReads>(cx).0.clone())
                .load_spend_day(&spend_day(now))
                .await
                .map_err(topcoat::Error::from_anyhow)?;
            let capped = day.refusal(spending.caps, &user_id, source.most_usd_micros(), now);
            (Some(day.charged_to(&user_id)), capped)
        }
        None => (None, None),
    };
    let capped_text = capped.map(|cap| {
        let reused = match config.reuse_days {
            0 => String::new(),
            1 => " A seed fetched in the last day is still answered.".to_owned(),
            days => format!(" A seed fetched in the last {days} days is still answered."),
        };
        format!("{}{reused}", cap_reached(cap, spend_day_ends(now)))
    });
    let capped_by = capped.map(|cap| match cap {
        SpendCap::User => "user",
        SpendCap::Platform => "platform",
    });
    let spent_text = spent.map(|spent| {
        format!(
            "Your fetches and searches cost {} today (UTC), of the {} a day may cost.",
            format_usd(spent),
            format_usd(spending.caps.user_usd_micros)
        )
    });
    let seeds_description = match config.reuse_days {
        0 => "Each seed is fetched with the keywords that contain its words, up to 1,000 of them. A fetch is one paid call to the keyword source, and replaces what an earlier fetch of the seed gave.".to_owned(),
        days => format!("Each seed is fetched with the keywords that contain its words, up to 1,000 of them. A fetch is one paid call to the keyword source, unless the seed was fetched in the last {days} day{} for the same country and language, by you or anyone: its keywords are then those, at no cost.", if days == 1 { "" } else { "s" }),
    };
    let id = study.id.clone();
    let page_link = |after: Option<&str>, before: Option<&str>| {
        href!(index, study_id: id.clone())
            .query(StudyLink {
                list: Some(list).filter(|list| *list != ALL),
                after,
                before,
                error: None,
                message: None,
            })
            .resolve(cx)
    };
    let more_searched = listed
        .newer
        .as_ref()
        .map(|cursor| page_link(None, Some(&cursor.param())));
    let less_searched = listed
        .older
        .as_ref()
        .map(|cursor| page_link(Some(&cursor.param()), None));
    let paged = more_searched.is_some() || less_searched.is_some();
    let rows: Vec<KeywordLine> = listed
        .rows
        .iter()
        .map(|row| KeywordLine::of(row, &study, &judgement))
        .collect();
    let tabs: Vec<PageTab> = LISTS
        .iter()
        .map(|(key, label)| PageTab {
            label,
            href: list_url(cx, &study.id, Some(key), None, None),
            count: Some(if *key == ALL {
                study.keywords() as usize
            } else {
                counts
                    .iter()
                    .find(|(outcome, _)| outcome == key)
                    .map_or(0, |(_, count)| *count as usize)
            }),
            current: *key == list,
        })
        .collect();
    let empty_list = if list == ALL {
        "No keyword yet: fetch the keywords of a seed."
    } else {
        "No keyword in this list."
    };
    let seeds: Vec<SeedLine> = study
        .seeds
        .iter()
        .map(|seed| SeedLine::of(seed, now, spending.reuse_secs))
        .collect();
    let fetching = seeds.iter().any(|seed| seed.state == "fetching");
    let owed = study.seeds.iter().filter(|seed| seed.is_owed(now)).count();
    let fetch_owed = match owed {
        1 => "Fetch the keywords of 1 seed".to_owned(),
        n => format!("Fetch the keywords of {n} seeds"),
    };
    let can_add = study.seeds.len() < MAX_SEEDS;
    let total = study.keywords();
    let keywords_title = match total {
        1 => "1 keyword".to_owned(),
        n => format!("{n} keywords"),
    };
    let tree_size = study.tree.categories.len();
    let tree_text = match tree_size {
        0 => "No category yet: the kept keywords are what a shop's categories are named after."
            .to_owned(),
        1 => "1 category built from the kept keywords.".to_owned(),
        n => format!("{n} categories built from the kept keywords."),
    };
    let title = format!("{} · Research", study.name);
    let place = format!("Searched in {} · {}", study.country, study.language);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::page("research"),
            page_header(
                title: &study.name,
                subtitle: Some(&place),
                h1_attrs: attributes! { data-study-name=(&study.name) },
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            if !enabled {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" class="mb-4" data-research-disabled="" },
                    icon(data: icons::WARNING)
                    alert_description("Keyword research is not set up on this platform: the operator has to give it a keyword source before keywords can be fetched.")
                )
            }
            if let (Some(text), Some(by)) = (&capped_text, capped_by) {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" class="mb-4" data-spend-capped=(by) },
                    icon(data: icons::WARNING)
                    alert_description((text))
                )
            }
            if fetching {
                alert(
                    variant: AlertVariant::Info,
                    attrs: attributes! { class="mb-4" data-study-fetching="" },
                    icon(data: icons::INFO)
                    alert_description(
                        "Keywords are being fetched, which takes a few seconds. "
                        <a class="font-medium underline underline-offset-4" href=(href!(index, study_id: id.clone()))>"Look again"</a>
                    )
                )
            }

            section_card(
                title: Some("Seed keywords"),
                description: Some(&seeds_description),
                table(
                    table_header(
                        table_row(
                            table_head("Seed")
                            table_head("Keywords")
                            table_head("Fetched")
                            table_head("Status")
                            table_head(<span class="sr-only">"Fetch"</span>)
                        )
                    )
                    table_body(
                        for seed in &seeds {
                            table_row(
                                attrs: attributes! { data-seed=(&seed.seed) data-seed-status=(seed.state) },
                                table_cell(attrs: attributes! { class="font-medium" }, (&seed.seed))
                                table_cell(attrs: attributes! { class="tabular-nums" data-seed-keywords=(seed.keywords) },
                                    if seed.fetched.is_some() { (seed.keywords) } else { "–" }
                                )
                                table_cell(
                                    if let Some(day) = &seed.fetched {
                                        <time datetime=(day)>(day)</time>
                                    } else {
                                        <span class="text-muted-foreground">"Never"</span>
                                    }
                                )
                                table_cell(
                                    status_badge(tone: seed.tone, (seed.label))
                                    if let Some(error) = &seed.error {
                                        <p class="mt-1 max-w-md text-sm whitespace-normal text-destructive-text" data-seed-error="">(error)</p>
                                    }
                                )
                                table_cell(
                                    attrs: attributes! { class="text-right" },
                                    if let Some(action) = seed.action.filter(|_| enabled) {
                                        <form method="post" action=(href!(fetch::request, study_id: id.clone()))>
                                            <input type="hidden" name="seed" value=(&seed.seed)>
                                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, (action))
                                        </form>
                                    }
                                )
                            )
                        }
                    )
                )
                if let (Some(text), Some(spent)) = (&spent_text, spent) {
                    <p class="mt-3 text-sm text-muted-foreground" data-spent-today=(spent)>(text)</p>
                }
                <div class="mt-4 flex flex-wrap items-start justify-between gap-3">
                    if can_add {
                        <form method="post" action=(href!(seeds::add_seed, study_id: id.clone())) class="flex flex-wrap gap-2">
                            <label class="sr-only" for="new-seed">"A seed keyword to add"</label>
                            input(attrs: attributes! { id="new-seed" name="seed" required="" maxlength="80" placeholder="Another seed keyword" class="sm:w-72" })
                            button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, icon(data: icons::PLUS) "Add the seed")
                        </form>
                    }
                    if enabled && owed > 0 {
                        <form method="post" action=(href!(fetch::request, study_id: id.clone()))>
                            button(attrs: attributes! { type="submit" data-fetch-owed=(owed) }, (&fetch_owed))
                        </form>
                    }
                </div>
            )

            section_card(
                title: Some("Verdict"),
                description: Some("A niche is a go when every test passes, a no-go when one fails. The thresholds are the method's own."),
                attrs: attributes! { id="score" data-verdict=(judgement.verdict.as_str()) },
                <p class="flex flex-wrap items-center gap-3 px-6 text-sm">
                    status_badge(tone: verdict_tone, (verdict))
                    <span class="text-muted-foreground">(verdict_text)</span>
                </p>
                table(
                    table_header(
                        table_row(
                            table_head("Test")
                            table_head("Result")
                            table_head("The numbers behind it")
                        )
                    )
                    table_body(
                        for test in &tests {
                            table_row(
                                attrs: attributes! { data-test=(test.key) data-test-state=(test.state) },
                                table_cell(attrs: attributes! { class="font-medium" }, (test.name))
                                table_cell(status_badge(tone: test.tone, (test.label)))
                                table_cell(attrs: attributes! { class="whitespace-normal" data-test-numbers="" }, (&test.numbers))
                            )
                        }
                    )
                )
            )

            section_card(
                title: Some("One sale"),
                description: Some("What one typical sale of the niche leaves, the return on ad spend at which an ad pays for itself, and the most a click may cost."),
                attrs: attributes! { id="sale" },
                if economics.is_empty() {
                    <p class="px-6 text-sm text-muted-foreground" data-sale-unknown="">"Say the price and the supplier's cost below to work it out."</p>
                } else {
                    <div class="px-6">
                        facts(
                            for (key, term, value) in &economics {
                                fact(term: term, attrs: attributes! { class="tabular-nums" data-sale-line=(*key) }, (value))
                            }
                        )
                    </div>
                }
                if let Some(text) = &rate_text {
                    <p class="px-6 text-sm text-muted-foreground" data-cpc-converted=(if unconverted { "no" } else { "yes" })>(text)</p>
                }
            )

            section_card(
                title: Some("What the score reads"),
                description: Some("Saving scores the study again from the keywords it holds: nothing is fetched, and nothing is paid."),
                attrs: attributes! { id="inputs" },
                <form method="post" action=(href!(inputs::set, study_id: id.clone())) class="grid gap-4 px-6 sm:grid-cols-2 lg:grid-cols-3" data-study-inputs="">
                    field(
                        field_label(attrs: attributes! { for="sale-currency" }, "Currency")
                        select(
                            attrs: attributes! { id="sale-currency" name="currency" },
                            for code in timada_core::CURRENCIES {
                                <option value=(*code) selected=(*code == currency)>(*code)</option>
                            }
                        )
                    )
                    field(
                        field_label(attrs: attributes! { for="sale-price" }, "Average price, VAT included")
                        input(attrs: attributes! { id="sale-price" name="price" type="text" inputmode="decimal" value=(&form_price) })
                    )
                    field(
                        field_label(attrs: attributes! { for="sale-cost" }, "Supplier's cost")
                        input(attrs: attributes! { id="sale-cost" name="cost" type="text" inputmode="decimal" value=(&form_cost) })
                    )
                    field(
                        field_label(attrs: attributes! { for="sale-shipping" }, "Sending a parcel")
                        input(attrs: attributes! { id="sale-shipping" name="shipping" type="text" inputmode="decimal" value=(&form_shipping) })
                    )
                    field(
                        field_label(attrs: attributes! { for="sale-import" }, "At the border")
                        input(attrs: attributes! { id="sale-import" name="import" type="text" inputmode="decimal" value=(&form_import) })
                    )
                    field(
                        field_label(attrs: attributes! { for="sale-vat" }, "VAT in the price, in %")
                        input(attrs: attributes! { id="sale-vat" name="vat_percent" type="text" inputmode="decimal" placeholder="20" value=(&form_vat) })
                    )
                    field(
                        field_label(attrs: attributes! { for="sale-fixed" }, "Fixed costs, for each sale")
                        input(attrs: attributes! { id="sale-fixed" name="fixed" type="text" inputmode="decimal" value=(&form_fixed) })
                    )
                    field(
                        field_label(attrs: attributes! { for="sale-fee-percent" }, "Payment fee, in % of the price")
                        input(attrs: attributes! { id="sale-fee-percent" name="fee_percent" type="text" inputmode="decimal" value=(&form_fee_percent) })
                    )
                    field(
                        field_label(attrs: attributes! { for="sale-fee-fixed" }, "Payment fee, for each payment")
                        input(attrs: attributes! { id="sale-fee-fixed" name="fee_fixed" type="text" inputmode="decimal" value=(&form_fee_fixed) })
                    )
                    field(
                        field_label(attrs: attributes! { for="study-brands" }, "Brands to drop, one a line")
                        textarea(attrs: attributes! { id="study-brands" name="brands" rows="4" aria-describedby="study-brands-hint" }, (&form_brands))
                        field_description(attrs: attributes! { id="study-brands-hint" }, "A keyword that holds one, as whole words, is another shop's.")
                    )
                    field(
                        field_label(attrs: attributes! { for="study-excluded" }, "Words to exclude, one a line")
                        textarea(attrs: attributes! { id="study-excluded" name="excluded_words" rows="4" aria-describedby="study-excluded-hint" }, (&form_excluded))
                        field_description(attrs: attributes! { id="study-excluded-hint" }, "A keyword that holds one is none of the niche's.")
                    )
                    field(
                        field_label(attrs: attributes! { for="study-products" }, "Good products found at the supplier")
                        input(attrs: attributes! { id="study-products" name="supplier_products" type="text" inputmode="numeric" aria-describedby="study-products-hint" value=(&form_products) })
                        field_description(attrs: attributes! { id="study-products-hint" }, "Empty until you counted them.")
                    )
                    <div class="sm:col-span-2 lg:col-span-3">
                        button(attrs: attributes! { type="submit" }, "Save and score again")
                    </div>
                </form>
            )

            section_card(
                title: Some("Who holds the first results"),
                description: Some(&competition_description),
                attrs: attributes! { id="competition" data-competition=(held.stands.state.as_str()) data-competition-searched=(held.searched) data-competition-searching=(searching) },
                if searching > 0 {
                    <p class="px-6 text-sm text-muted-foreground" data-competition-on-its-way="">
                        "The first results of " (counted(searching, "main keyword")) " are being searched, a few seconds each. "
                        <a class=(LINK) href=(competition_url(cx, &id, None, None))>"Look again"</a>
                    </p>
                }
                for error in &search_errors {
                    <p class="px-6 text-sm whitespace-normal text-destructive-text" data-competition-error="">(error)</p>
                }
                if mains.is_empty() {
                    <p class="px-6 text-sm text-muted-foreground" data-competition-empty="">"No main keyword yet: the first results are searched for the kept keywords with 1,000 searches a month or more."</p>
                } else {
                    table(
                        table_header(
                            table_row(
                                table_head("Site")
                                table_head("Main keywords")
                                table_head("Places")
                                table_head("What it is")
                                table_head(<span class="sr-only">"Mark"</span>)
                            )
                        )
                        table_body(
                            if sites.is_empty() {
                                empty_row(colspan: 5, text: "No first results yet.")
                            }
                            for site in &sites {
                                table_row(
                                    attrs: attributes! { data-site=(&site.domain) data-site-mark=(site.mark) data-site-decides=(if site.decides { "yes" } else { "no" }) },
                                    table_cell(attrs: attributes! { class="font-medium" },
                                        <a class=(LINK) href=(format!("https://{}", site.domain)) rel="noopener noreferrer" target="_blank">(&site.domain)</a>
                                    )
                                    table_cell(attrs: attributes! { class="tabular-nums" data-site-keywords=(site.keywords) }, (site.keywords) " of " (held.searched))
                                    table_cell(attrs: attributes! { class="tabular-nums whitespace-normal" data-site-places="" }, (&site.positions))
                                    table_cell(status_badge(tone: site.tone, (site.label)))
                                    table_cell(
                                        attrs: attributes! { class="text-right" },
                                        <div class="flex justify-end gap-2">
                                            for (value, label) in [("yes", "Specialist"), ("no", "Generalist")] {
                                                if site.mark != label.to_lowercase() {
                                                    <form method="post" action=(href!(mark::mark, study_id: id.clone()))>
                                                        <input type="hidden" name="domain" value=(&site.domain)>
                                                        <input type="hidden" name="specialist" value=(value)>
                                                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" },
                                                            (label)
                                                            <span class="sr-only">": " (&site.domain)</span>
                                                        )
                                                    </form>
                                                }
                                            }
                                        </div>
                                    )
                                )
                            }
                        )
                    )
                    <p class="px-6 text-sm text-muted-foreground">"A site found for one main keyword decides nothing, marked or not."</p>
                    if enabled && to_search > 0 {
                        <form method="post" action=(href!(competition::check, study_id: id.clone())) class="flex flex-wrap items-center gap-3 px-6">
                            button(attrs: attributes! { type="submit" data-search-owed=(to_search) }, (&search_owed))
                            <span class="text-sm text-muted-foreground" data-search-cost="">(&search_owed_cost)</span>
                        </form>
                    } else if enabled && searching == 0 {
                        <form method="post" action=(href!(competition::check, study_id: id.clone())) class="flex flex-wrap items-center gap-3 px-6">
                            <input type="hidden" name="again" value="yes">
                            button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" data-search-again="" }, "Search them all again")
                            <span class="text-sm text-muted-foreground" data-search-cost="">(&search_again_cost)</span>
                        </form>
                    }
                }
            )

            section_card(
                title: Some(&keywords_title),
                description: Some("As the last fetch of each seed gave them, the most searched first, with what the score made of each. A figure the source has nothing for stays empty. The cost per click is the source's, in US dollars; the price / CPC is the sale's price over it."),
                attrs: attributes! { id="keywords" data-study-keywords=(total) },
                <div class="px-6">
                    page_tabs(label: "Keyword lists", tabs: &tabs)
                </div>
                table(
                    table_header(
                        table_row(
                            table_head("Keyword")
                            table_head("Searches a month")
                            table_head("Difficulty")
                            table_head("Cost per click")
                            table_head("Price / CPC")
                            table_head("Ads")
                            table_head("Intent")
                            table_head("Score")
                            table_head(<span class="sr-only">"Drop or bring back"</span>)
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 9, text: empty_list)
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-keyword=(&row.keyword) data-outcome=(&row.outcome) },
                                table_cell(attrs: attributes! { class="font-medium" },
                                    (&row.keyword)
                                    if let Some(of) = &row.variant_of {
                                        <p class="text-xs font-normal text-muted-foreground" data-variant-of=(of)>"Counts under " (of)</p>
                                    }
                                )
                                table_cell(attrs: attributes! { class="tabular-nums" data-volume="" }, (&row.volume))
                                table_cell(attrs: attributes! { class="tabular-nums" data-difficulty="" }, (&row.difficulty))
                                table_cell(attrs: attributes! { class="tabular-nums" data-cpc="" }, (row.cpc.as_deref().unwrap_or("–")))
                                table_cell(attrs: attributes! { class="tabular-nums" data-ratio="" }, (row.ratio.as_deref().unwrap_or("–")))
                                table_cell(attrs: attributes! { data-channel=(row.channel.map_or("", Channel::as_str)) },
                                    (match row.channel {
                                        Some(Channel::Ads) => "Ads",
                                        Some(Channel::SeoOnly) => "SEO only",
                                        None => "–",
                                    })
                                )
                                table_cell(attrs: attributes! { data-intent="" }, (&row.intent))
                                table_cell(status_badge(tone: row.tone, (row.status)))
                                table_cell(
                                    attrs: attributes! { class="text-right" },
                                    if let Some(drops) = row.drops {
                                        <form method="post" action=(if drops { href!(drop::drop, study_id: id.clone()).resolve(cx) } else { href!(restore::restore, study_id: id.clone()).resolve(cx) })>
                                            <input type="hidden" name="keyword" value=(&row.keyword)>
                                            <input type="hidden" name="list" value=(list)>
                                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" },
                                                (if drops { "Drop" } else { "Bring back" })
                                                <span class="sr-only">" " (&row.keyword)</span>
                                            )
                                        </form>
                                    }
                                )
                            )
                        }
                    )
                )
                if paged {
                    pagination(
                        attrs: attributes! { class="mt-4" },
                        pagination_content(
                            if let Some(more_searched) = more_searched {
                                pagination_item(pagination_previous(label: "More searched", attrs: attributes! { href=(more_searched) rel="prev" }))
                            }
                            if let Some(less_searched) = less_searched {
                                pagination_item(pagination_next(label: "Less searched", attrs: attributes! { href=(less_searched) rel="next" }))
                            }
                        )
                    )
                }
                if total > 0 {
                    <p class="px-6 text-sm">
                        <a class=(LINK) href=(href!(export::export, study_id: id.clone())) data-study-export="">"Export the keywords with their score (CSV)"</a>
                    </p>
                }
            )

            section_card(
                title: Some("Categories"),
                description: Some("The tree of a shop to come, built by hand from the kept keywords: parent categories, their children, and the keywords parked for later."),
                attrs: attributes! { id="categories" data-study-categories=(tree_size) },
                <p class="px-6 text-sm">
                    <span class="text-muted-foreground">(&tree_text)</span>
                    " "
                    <a class=(LINK) href=(href!(tree::index, study_id: id.clone())) data-study-tree="">"Build the category tree"</a>
                </p>
            )

            section_card(
                title: Some("The study"),
                <form method="post" action=(href!(rename::rename, study_id: id.clone())) class="flex flex-wrap gap-2">
                    <label class="sr-only" for="study-name">"Name"</label>
                    input(attrs: attributes! { id="study-name" name="name" required="" maxlength="80" value=(&study.name) class="sm:w-72" })
                    button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, "Rename")
                </form>
                <form method="post" action=(href!(delete::delete, study_id: id.clone())) class="mt-4">
                    <p class="mb-2 text-sm text-muted-foreground">"Deleting the study removes it and its keywords from your list for good."</p>
                    button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Delete the study")
                </form>
            )
        )
    })
}
