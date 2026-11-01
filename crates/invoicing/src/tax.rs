//! Which VAT a sale carries, for a seller established in France: the regime
//! (domestic, distance sale taxed in France or through the One-Stop Shop,
//! reverse charge, export, exemption) and the rate of each line.
//!
//! Prices include VAT and the shopper always pays the same price: the
//! regime only decides how that price splits into base and tax. The rate of
//! a level (standard, reduced…) in each country comes from the `world-tax`
//! crate; which level a kind of goods falls under in each country is ours
//! (`data/tax_categories.json`), as are the few rates that table lacks (its
//! `_overrides`, in basis points) and the yearly threshold of EU distance
//! sales (the crate checks a threshold per sale). The threshold is counted
//! in euros: a sale in another currency counts for what it is worth at the
//! euro rate of the day ([`crate::rates`]).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;
use world_tax::TaxDatabase;

use crate::rates::{EURO, ExchangeRate};
use crate::value_object::vat_breakdown;

/// The seller's country: every rule here is French law.
pub const SELLER_COUNTRY: &str = "FR";
/// Sales to consumers in other EU countries, all together, excluding VAT,
/// above which their VAT applies instead of France's (art. 258 A du CGI).
pub const EU_DISTANCE_THRESHOLD_CENTS: u64 = 1_000_000;
/// The category of goods with no particular rate.
const STANDARD: &str = "standard";
const CATEGORIES_JSON: &str = include_str!("../data/tax_categories.json");
/// The key of `tax_categories.json` that holds the rates the `world-tax`
/// table lacks.
const OVERRIDES_KEY: &str = "_overrides";
/// A rate the `world-tax` table lacks, kept in `tax_categories.json` under
/// a name of its own that a category maps a country to instead of a level.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct RateOverride {
    /// The one country the rate holds in: a category of another country
    /// pointing at it is a mistake, taxed at that country's standard rate.
    country: String,
    rate_bps: u32,
}

/// How a sale is taxed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Regime {
    /// Sold in France, French VAT.
    #[default]
    Domestic,
    /// The seller is exempt (franchise en base, art. 293 B du CGI).
    Franchise,
    /// To a consumer in another EU country, under the yearly threshold:
    /// French VAT.
    DistanceOrigin,
    /// To a consumer in another EU country, past the threshold or under the
    /// One-Stop Shop: that country's VAT.
    Oss,
    /// To a business in another EU country with a valid VAT number: no VAT,
    /// the buyer accounts for it (art. 262 ter I du CGI).
    ReverseCharge,
    /// Leaves the EU VAT area: no VAT (art. 262 I du CGI).
    Export,
}

impl Regime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Domestic => "domestic",
            Self::Franchise => "franchise",
            Self::DistanceOrigin => "distance_origin",
            Self::Oss => "oss",
            Self::ReverseCharge => "reverse_charge",
            Self::Export => "export",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "domestic" => Self::Domestic,
            "franchise" => Self::Franchise,
            "distance_origin" => Self::DistanceOrigin,
            "oss" => Self::Oss,
            "reverse_charge" => Self::ReverseCharge,
            "export" => Self::Export,
            _ => return None,
        })
    }

    /// What a document taxed this way must say, if anything.
    pub fn mention(self) -> Option<&'static str> {
        match self {
            Self::Franchise => Some("TVA non applicable, art. 293 B du CGI"),
            Self::ReverseCharge => {
                Some("Autoliquidation : exonération de TVA, art. 262 ter I du CGI")
            }
            Self::Export => Some("Exonération de TVA, art. 262 I du CGI (exportation)"),
            Self::Domestic | Self::DistanceOrigin | Self::Oss => None,
        }
    }
}

/// The seller as far as VAT goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TaxProfile {
    /// Has a VAT number; without one, the exemption for small businesses.
    pub vat_registered: bool,
    /// Registered for the EU One-Stop Shop: destination VAT from the first
    /// euro of EU distance sales.
    pub oss_registered: bool,
}

/// Where the goods go and who buys them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Destination {
    /// ISO 3166-1 alpha-2.
    pub country: String,
    pub postal_code: String,
    /// The buyer's EU VAT number, only once VIES said it is valid.
    pub vat_number: Option<String>,
}

/// The year's EU distance sales, excluding VAT, and whether destination VAT
/// applies for the rest of it (the threshold was passed this year or the
/// year before).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DistanceSales {
    pub sales_cents: u64,
    pub passed: bool,
}

/// How one sale is taxed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decision {
    pub regime: Regime,
    /// The country whose VAT applies (or where the goods go, when none does).
    pub tax_country: String,
    /// The rate of each line, in basis points; `None` when no VAT applies.
    pub rates: Vec<Option<u32>>,
    /// The year's distance sales, this one included.
    pub distance: DistanceSales,
    /// The euro rate this sale was counted at, when it was priced in another
    /// currency and could be.
    pub euro_rate: Option<ExchangeRate>,
}

/// Where a destination stands for VAT.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Territory {
    France,
    /// Another EU country's VAT area.
    Eu(String),
    /// Outside the EU VAT area.
    Outside,
}

/// Places in an EU country that are outside its VAT area (or, for Monaco,
/// inside France's), by country and postal code.
fn territory(country: &str, postal_code: &str) -> Territory {
    let postal: String = postal_code
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| postal.starts_with(p));
    match country {
        "MC" => return Territory::France,
        // Overseas departments and collectivities; 980xx is Monaco.
        "FR" if starts(&["97", "986", "987", "988"]) => return Territory::Outside,
        "FR" => return Territory::France,
        // Canary Islands, Ceuta, Melilla.
        "ES" if starts(&["35", "38", "51", "52"]) => return Territory::Outside,
        // Livigno, Campione d'Italia.
        "IT" if starts(&["23041", "22061"]) => return Territory::Outside,
        // Heligoland, Büsingen am Hochrhein.
        "DE" if starts(&["27498", "78266"]) => return Territory::Outside,
        // Mount Athos.
        "GR" if starts(&["63086", "63087"]) => return Territory::Outside,
        // Åland.
        "FI" if starts(&["22"]) => return Territory::Outside,
        _ => {}
    }
    if eu_members().iter().any(|member| member == country) {
        Territory::Eu(country.to_owned())
    } else {
        Territory::Outside
    }
}

fn database() -> &'static TaxDatabase {
    static DATABASE: OnceLock<TaxDatabase> = OnceLock::new();
    DATABASE.get_or_init(|| TaxDatabase::new().expect("world-tax ships a valid database"))
}

fn eu_members() -> &'static [String] {
    database()
        .trade_agreements
        .get("EU")
        .map_or(&[], |eu| eu.members.as_slice())
}

/// `category → country → level (or override name)`.
fn categories() -> &'static HashMap<String, HashMap<String, String>> {
    static CATEGORIES: OnceLock<HashMap<String, HashMap<String, String>>> = OnceLock::new();
    CATEGORIES.get_or_init(|| {
        let raw: HashMap<String, serde_json::Value> =
            serde_json::from_str(CATEGORIES_JSON).expect("tax_categories.json is valid");
        raw.into_iter()
            .filter(|(key, _)| !key.starts_with('_'))
            .map(|(key, value)| {
                let levels = serde_json::from_value(value).expect("a map of country to level");
                (key, levels)
            })
            .collect()
    })
}

/// `override name → rate`, from the file's `_overrides`.
fn overrides() -> &'static HashMap<String, RateOverride> {
    static OVERRIDES: OnceLock<HashMap<String, RateOverride>> = OnceLock::new();
    OVERRIDES.get_or_init(|| {
        let mut raw: HashMap<String, serde_json::Value> =
            serde_json::from_str(CATEGORIES_JSON).expect("tax_categories.json is valid");
        raw.remove(OVERRIDES_KEY)
            .map_or_else(HashMap::new, |value| {
                serde_json::from_value(value).expect("_overrides is a map of name to rate")
            })
    })
}

/// The rate of a kind of goods in a country, in basis points. A level the
/// country does not have, or a kind it does not list, is its standard rate.
/// A kind mapped to one of the file's `_overrides` carries that rate, when
/// the override is the country's own.
pub fn rate_bps(country: &str, category: &str) -> u32 {
    let Ok(rates) = database().get_country(country) else {
        tracing::warn!(country, "no VAT rates for this country: 0 %");
        return 0;
    };
    let level = categories()
        .get(category)
        .and_then(|levels| levels.get(country))
        .map_or(STANDARD, String::as_str);
    match overrides().get(level) {
        Some(rate) if rate.country == country => return rate.rate_bps,
        Some(rate) => tracing::warn!(
            country,
            category,
            level,
            holds_in = rate.country,
            "rate override of another country: standard rate"
        ),
        None => {}
    }
    let rate = match level {
        "zero" => Some(0.0),
        "reduced" => rates.reduced_rate,
        "reduced_alt" => rates.reduced_rate_alt,
        "super_reduced" => rates.super_reduced_rate,
        _ => Some(rates.standard_rate),
    };
    let rate = match rate {
        Some(rate) if rate > 0.0 || level == "zero" => rate,
        _ => {
            if category != STANDARD {
                tracing::warn!(
                    country,
                    category,
                    level,
                    "no such rate level: standard rate"
                );
            }
            rates.standard_rate
        }
    };
    (rate * 10_000.0).round() as u32
}

/// Why a sale cannot be taken at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaxRefusal {
    /// An EU distance sale, from a seller not registered for the One-Stop
    /// Shop, in a currency the European Central Bank publishes no euro rate
    /// for. The sale cannot be counted against the yearly threshold, so it
    /// would have to be taxed at destination, which only the One-Stop Shop
    /// lets a French seller remit.
    OssRequiredForCurrency,
}

/// Whether a sale may go ahead at all, before anything is charged: a
/// VAT-registered seller without the One-Stop Shop sells to consumers in
/// other EU countries in euros, or in a currency with a published euro rate
/// (`rate_published`), and in no other
/// ([`TaxRefusal::OssRequiredForCurrency`]). Everything else is taxed one
/// way or another by [`decide`].
pub fn accepts(
    profile: TaxProfile,
    destination: &Destination,
    currency: &str,
    rate_published: bool,
) -> Result<(), TaxRefusal> {
    if needs_euro_rate(profile, destination, currency) && !rate_published {
        Err(TaxRefusal::OssRequiredForCurrency)
    } else {
        Ok(())
    }
}

/// Whether the tax of a sale hangs on the euro rate of its currency: an EU
/// distance sale in another currency than the euro, from a VAT-registered
/// seller without the One-Stop Shop, is taxed in France or at destination
/// according to what it counts for, in euros, against the yearly threshold.
/// Every other sale in another currency is taxed the same with or without
/// the rate (the rate then only serves the count and the VAT in euros a
/// document states).
pub fn needs_euro_rate(profile: TaxProfile, destination: &Destination, currency: &str) -> bool {
    profile.vat_registered
        && !profile.oss_registered
        && !currency.eq_ignore_ascii_case(EURO)
        && is_distance_sale(profile, destination)
}

/// Whether a sale is an EU distance sale (to a consumer in another EU
/// country): the only kind that counts towards the yearly threshold, and so
/// the only kind whose decision needs the company's total.
pub fn is_distance_sale(profile: TaxProfile, destination: &Destination) -> bool {
    matches!(
        territory(&destination.country, &destination.postal_code),
        Territory::Eu(_)
    ) && !(profile.vat_registered && destination.vat_number.is_some())
}

/// The tax of a sale whose lines are `(tax category, price VAT included)`,
/// in `currency`, given the year's distance sales before it. A sale in
/// another currency than the euro counts towards the threshold for what it
/// is worth at `euro_rate` (the rate of that currency on the day); without
/// one it cannot be counted and is taxed at destination, which [`accepts`]
/// lets through for a seller registered for the One-Stop Shop alone.
pub fn decide(
    profile: TaxProfile,
    destination: &Destination,
    lines: &[(&str, u64)],
    currency: &str,
    euro_rate: Option<&ExchangeRate>,
    before: DistanceSales,
) -> Decision {
    let in_euros = currency.eq_ignore_ascii_case(EURO);
    let euro_rate = euro_rate.filter(|rate| {
        if in_euros {
            return false;
        }
        let converts = rate.converts(currency);
        if !converts {
            tracing::warn!(
                currency,
                rate_currency = rate.currency,
                per_euro = rate.per_euro,
                "not the euro rate of this currency: ignored"
            );
        }
        converts
    });
    let rates_in = |country: &str| -> Vec<Option<u32>> {
        lines
            .iter()
            .map(|(category, _)| Some(rate_bps(country, category)))
            .collect()
    };
    let excl_vat = |rates: &[Option<u32>]| -> u64 {
        let total: u64 = lines.iter().map(|(_, amount)| amount).sum();
        let vat: u64 = vat_breakdown(
            lines
                .iter()
                .zip(rates)
                .filter_map(|((_, amount), rate)| rate.map(|rate| (rate, *amount))),
        )
        .iter()
        .map(|subtotal| subtotal.vat_cents)
        .sum();
        total - vat
    };
    // What a sum excluding VAT counts for, in euro cents; `None` when it
    // cannot be counted.
    let euro_cents = |cents: u64| -> Option<u64> {
        if in_euros {
            Some(cents)
        } else {
            euro_rate.and_then(|rate| rate.to_euro_cents(cents))
        }
    };
    let count = |sales: u64| -> DistanceSales {
        let sales_cents = before.sales_cents + euro_cents(sales).unwrap_or(0);
        DistanceSales {
            sales_cents,
            passed: before.passed || sales_cents > EU_DISTANCE_THRESHOLD_CENTS,
        }
    };
    let counted_at = || euro_rate.cloned();
    let none = || vec![None; lines.len()];
    match territory(&destination.country, &destination.postal_code) {
        Territory::Outside => Decision {
            regime: Regime::Export,
            tax_country: destination.country.clone(),
            rates: none(),
            distance: before,
            euro_rate: None,
        },
        Territory::France if !profile.vat_registered => Decision {
            regime: Regime::Franchise,
            tax_country: SELLER_COUNTRY.to_owned(),
            rates: none(),
            distance: before,
            euro_rate: None,
        },
        Territory::France => Decision {
            regime: Regime::Domestic,
            tax_country: SELLER_COUNTRY.to_owned(),
            rates: rates_in(SELLER_COUNTRY),
            distance: before,
            euro_rate: None,
        },
        Territory::Eu(country) if profile.vat_registered && destination.vat_number.is_some() => {
            Decision {
                regime: Regime::ReverseCharge,
                tax_country: country,
                rates: none(),
                distance: before,
                euro_rate: None,
            }
        }
        Territory::Eu(_) if !profile.vat_registered => {
            let rates = none();
            let distance = count(excl_vat(&rates));
            Decision {
                regime: Regime::Franchise,
                tax_country: SELLER_COUNTRY.to_owned(),
                rates,
                distance,
                euro_rate: counted_at(),
            }
        }
        Territory::Eu(country) => {
            let origin = rates_in(SELLER_COUNTRY);
            // In euros, or converted: whether this sale passes the
            // threshold. A sale that cannot be counted is taxed at
            // destination (see `accepts`).
            let crosses = euro_cents(excl_vat(&origin))
                .map(|sale| before.sales_cents + sale > EU_DISTANCE_THRESHOLD_CENTS);
            if profile.oss_registered || before.passed || crosses.unwrap_or(true) {
                let rates = rates_in(&country);
                let distance = count(excl_vat(&rates));
                Decision {
                    regime: Regime::Oss,
                    tax_country: country,
                    rates,
                    distance,
                    euro_rate: counted_at(),
                }
            } else {
                let distance = count(excl_vat(&origin));
                Decision {
                    regime: Regime::DistanceOrigin,
                    tax_country: SELLER_COUNTRY.to_owned(),
                    rates: origin,
                    distance,
                    euro_rate: counted_at(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rate levels the `world-tax` table has.
    const LEVELS: [&str; 5] = [
        "standard",
        "reduced",
        "reduced_alt",
        "super_reduced",
        "zero",
    ];

    const REGISTERED: TaxProfile = TaxProfile {
        vat_registered: true,
        oss_registered: false,
    };

    fn to(country: &str, postal_code: &str) -> Destination {
        Destination {
            country: country.into(),
            postal_code: postal_code.into(),
            vat_number: None,
        }
    }

    fn below() -> DistanceSales {
        DistanceSales::default()
    }

    #[test]
    fn rates_follow_the_kind_of_goods_and_the_country() {
        assert_eq!(rate_bps("FR", "standard"), 2000);
        assert_eq!(rate_bps("FR", "books"), 550);
        assert_eq!(rate_bps("FR", "food"), 550);
        assert_eq!(rate_bps("FR", "medicines"), 1000);
        assert_eq!(rate_bps("FR", "press"), 210);
        assert_eq!(rate_bps("DE", "books"), 700);
        assert_eq!(rate_bps("DE", "standard"), 1900);
        assert_eq!(rate_bps("IE", "books"), 0);
        assert_eq!(rate_bps("IT", "books"), 400);
        assert_eq!(rate_bps("DK", "books"), 2500);
        // Reviewed on 2026-10-17: Estonia's books are 9 %, not the 13 % of
        // accommodation; Czechia zero-rates books but not periodicals.
        assert_eq!(rate_bps("EE", "books"), 900);
        assert_eq!(rate_bps("CZ", "books"), 0);
        assert_eq!(rate_bps("CZ", "press"), 1200);
        // An unknown kind is standard.
        assert_eq!(rate_bps("DE", "cars"), 1900);
    }

    /// Latvia's 5 % on books and press (law of 2025-12-18) is a rate the
    /// `world-tax` table lacks: it comes from the file's `_overrides`. The
    /// other kinds keep the table's rates.
    #[test]
    fn a_rate_the_table_lacks_comes_from_an_override() {
        assert_eq!(rate_bps("LV", "books"), 500);
        assert_eq!(rate_bps("LV", "press"), 500);
        assert_eq!(rate_bps("LV", "standard"), 2100);
        assert_eq!(rate_bps("LV", "food"), 2100);
        assert_eq!(rate_bps("LV", "medicines"), 1200);
        // The override is Latvia's alone.
        assert_eq!(overrides()["lv_publications"].country, "LV");
    }

    /// An override is named after no level, holds in an EU country, and some
    /// category of that country uses it; no other country points at it, and
    /// every entry of a category is a level or an override.
    #[test]
    fn overrides_are_well_formed_and_used() {
        for (name, rate) in overrides() {
            assert!(!LEVELS.contains(&name.as_str()), "{name} shadows a level");
            assert!(
                eu_members().contains(&rate.country),
                "{name} holds in {}, not an EU country",
                rate.country
            );
            let pointing: Vec<(&str, &str)> = categories()
                .iter()
                .flat_map(|(category, levels)| {
                    levels
                        .iter()
                        .filter(|(_, level)| *level == name)
                        .map(move |(country, _)| (category.as_str(), country.as_str()))
                })
                .collect();
            assert!(!pointing.is_empty(), "{name} is used by no category");
            for (category, country) in pointing {
                assert_eq!(
                    country, rate.country,
                    "{category} points {country} at {name}"
                );
            }
        }
        for levels in categories().values() {
            for level in levels.values() {
                assert!(
                    LEVELS.contains(&level.as_str()) || overrides().contains_key(level),
                    "{level} is neither a level nor an override"
                );
            }
        }
    }

    /// The table was last checked against the published rates on the date it
    /// records. The Commission publishes its schedule yearly; past eighteen
    /// months the check is due again.
    #[test]
    fn the_table_was_reviewed_recently() {
        let raw: serde_json::Value = serde_json::from_str(CATEGORIES_JSON).unwrap();
        let reviewed_on = raw["_review"]["reviewed_on"]
            .as_str()
            .expect("_review.reviewed_on is a YYYY-MM-DD date");
        let mut parts = reviewed_on
            .split('-')
            .map(|part| part.parse::<i64>().unwrap());
        let (year, month, day) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        let reviewed = timada_core::civil::days_since_epoch(year, month as u32, day as u32)
            .expect("reviewed_on is a calendar date");
        let today = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            / 86_400;
        let age_days = today - reviewed;
        assert!(age_days >= 0, "reviewed_on {reviewed_on} is in the future");
        assert!(
            age_days <= 548,
            "tax_categories.json was last reviewed on {reviewed_on}, {age_days} days ago: \
             check it against the Commission's current 'VAT rates applied in the Member \
             States' and bump _review.reviewed_on"
        );
    }

    /// The effective rate of every kind of goods in every EU country, as
    /// `docs/invoicing.md` shows it to whoever reviews the table.
    fn rates_table() -> String {
        let mut kinds: Vec<&str> = categories().keys().map(String::as_str).collect();
        kinds.sort_unstable();
        kinds.insert(0, STANDARD);
        let percent = |bps: u32| -> String {
            if bps.is_multiple_of(100) {
                format!("{} %", bps / 100)
            } else {
                format!("{}.{} %", bps / 100, bps % 100 / 10)
            }
        };
        let mut members: Vec<&str> = eu_members().iter().map(String::as_str).collect();
        members.sort_unstable();
        let mut table = format!(
            "| Country | {} |\n|---|{}\n",
            kinds
                .iter()
                .map(|kind| {
                    let mut chars = kind.chars();
                    chars
                        .next()
                        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" | "),
            "---|".repeat(kinds.len())
        );
        for member in members {
            let rates: Vec<String> = kinds
                .iter()
                .map(|kind| percent(rate_bps(member, kind)))
                .collect();
            table.push_str(&format!("| {member} | {} |\n", rates.join(" | ")));
        }
        table
    }

    #[test]
    fn the_docs_table_matches_the_data() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/invoicing.md");
        let docs = std::fs::read_to_string(path).unwrap();
        let (start, end) = (
            "<!-- tax-categories:start -->",
            "<!-- tax-categories:end -->",
        );
        let from = docs
            .find(start)
            .expect("docs/invoicing.md has the start marker")
            + start.len();
        let to = docs
            .find(end)
            .expect("docs/invoicing.md has the end marker");
        let expected = rates_table();
        assert_eq!(
            docs[from..to].trim(),
            expected.trim(),
            "the rates table in docs/invoicing.md is stale; replace it with:\n\n{expected}"
        );
    }

    #[test]
    fn every_eu_country_has_every_kind() {
        for (category, levels) in categories() {
            for member in eu_members() {
                assert!(
                    levels.contains_key(member),
                    "{category} has no level for {member}"
                );
            }
        }
    }

    #[test]
    fn a_sale_in_france_is_taxed_in_france() {
        let decision = decide(
            REGISTERED,
            &to("FR", "75002"),
            &[("books", 1000)],
            "EUR",
            None,
            below(),
        );
        assert_eq!(decision.regime, Regime::Domestic);
        assert_eq!(decision.rates, vec![Some(550)]);
        assert_eq!(decision.distance, below());
        // Monaco is France for VAT.
        let monaco = decide(
            REGISTERED,
            &to("MC", "98000"),
            &[("standard", 1000)],
            "EUR",
            None,
            below(),
        );
        assert_eq!(monaco.regime, Regime::Domestic);
        let exempt = TaxProfile::default();
        let franchise = decide(
            exempt,
            &to("FR", "75002"),
            &[("standard", 1000)],
            "EUR",
            None,
            below(),
        );
        assert_eq!(franchise.regime, Regime::Franchise);
        assert_eq!(franchise.rates, vec![None]);
    }

    #[test]
    fn leaving_the_eu_vat_area_is_an_export() {
        for (country, postal) in [
            ("US", "10001"),
            ("CA", "H2X 1Y4"),
            ("AE", ""),
            ("GB", "SW1A 1AA"),
            ("FR", "97100"),
            ("ES", "35001"),
            ("FI", "22100"),
        ] {
            let decision = decide(
                REGISTERED,
                &to(country, postal),
                &[("standard", 1000)],
                "EUR",
                None,
                below(),
            );
            assert_eq!(decision.regime, Regime::Export, "{country} {postal}");
            assert_eq!(decision.rates, vec![None]);
        }
        let exempt = decide(
            TaxProfile::default(),
            &to("US", "10001"),
            &[("standard", 1000)],
            "EUR",
            None,
            below(),
        );
        assert_eq!(exempt.regime, Regime::Export);
    }

    #[test]
    fn a_business_with_a_valid_number_reverse_charges() {
        let business = Destination {
            vat_number: Some("DE123456789".into()),
            ..to("DE", "10115")
        };
        let decision = decide(
            REGISTERED,
            &business,
            &[("standard", 1000)],
            "EUR",
            None,
            below(),
        );
        assert_eq!(decision.regime, Regime::ReverseCharge);
        assert_eq!(decision.tax_country, "DE");
        assert_eq!(decision.rates, vec![None]);
        assert_eq!(
            decision.distance,
            below(),
            "a business sale is no distance sale"
        );
        // A French business pays French VAT.
        let french = Destination {
            vat_number: Some("FR85120027016".into()),
            ..to("FR", "75002")
        };
        assert_eq!(
            decide(
                REGISTERED,
                &french,
                &[("standard", 1000)],
                "EUR",
                None,
                below()
            )
            .regime,
            Regime::Domestic
        );
    }

    #[test]
    fn eu_consumers_pay_french_vat_until_the_threshold() {
        let lines = [("standard", 12_000)];
        // 100.00 excluding VAT at 20 %.
        let first = decide(REGISTERED, &to("DE", "10115"), &lines, "EUR", None, below());
        assert_eq!(first.regime, Regime::DistanceOrigin);
        assert_eq!(first.tax_country, "FR");
        assert_eq!(first.rates, vec![Some(2000)]);
        assert_eq!(first.distance.sales_cents, 10_000);
        assert!(!first.distance.passed);

        // The sale that passes 10,000 € is taxed at destination already.
        let near = DistanceSales {
            sales_cents: 995_000,
            passed: false,
        };
        let crossing = decide(REGISTERED, &to("DE", "10115"), &lines, "EUR", None, near);
        assert_eq!(crossing.regime, Regime::Oss);
        assert_eq!(crossing.tax_country, "DE");
        assert_eq!(crossing.rates, vec![Some(1900)]);
        assert!(crossing.distance.passed);
        // 120.00 at 19 %: 100.84 excluding VAT.
        assert_eq!(crossing.distance.sales_cents, 995_000 + 10_084);

        // Passed (this year or last): destination for every sale.
        let passed = DistanceSales {
            sales_cents: 0,
            passed: true,
        };
        assert_eq!(
            decide(
                REGISTERED,
                &to("IT", "00100"),
                &[("books", 1000)],
                "EUR",
                None,
                passed
            )
            .rates,
            vec![Some(400)]
        );
        // Registered for the One-Stop Shop: destination from the first euro.
        let oss = TaxProfile {
            vat_registered: true,
            oss_registered: true,
        };
        assert_eq!(
            decide(oss, &to("DE", "10115"), &lines, "EUR", None, below()).regime,
            Regime::Oss
        );
        // A book and a lamp to Latvia: 5 % through the override, then 21 %.
        let latvia = decide(
            oss,
            &to("LV", "LV-1050"),
            &[("books", 1000), ("standard", 1000)],
            "EUR",
            None,
            below(),
        );
        assert_eq!(latvia.regime, Regime::Oss);
        assert_eq!(latvia.tax_country, "LV");
        assert_eq!(latvia.rates, vec![Some(500), Some(2100)]);
        // Another currency is taxed at destination and not counted (and
        // only taken from a seller registered for the One-Stop Shop, see
        // `accepts`).
        let usd = decide(oss, &to("DE", "10115"), &lines, "USD", None, below());
        assert_eq!(usd.regime, Regime::Oss);
        assert_eq!(usd.distance, below());
    }

    /// Without the One-Stop Shop, a sale to an EU consumer in another
    /// currency needs that currency's euro rate to be counted: a currency
    /// the Bank publishes no rate for is refused.
    #[test]
    fn a_distance_sale_in_a_currency_without_a_euro_rate_needs_the_one_stop_shop() {
        let oss = TaxProfile {
            vat_registered: true,
            oss_registered: true,
        };
        let germany = to("DE", "10115");
        assert!(needs_euro_rate(REGISTERED, &germany, "USD"));
        assert!(
            !needs_euro_rate(REGISTERED, &germany, "eur"),
            "case apart, euros"
        );
        assert!(!needs_euro_rate(oss, &germany, "USD"));
        assert_eq!(
            accepts(REGISTERED, &germany, "AED", false),
            Err(TaxRefusal::OssRequiredForCurrency)
        );
        assert_eq!(accepts(REGISTERED, &germany, "USD", true), Ok(()));
        assert_eq!(accepts(REGISTERED, &germany, "EUR", false), Ok(()));
        assert_eq!(accepts(oss, &germany, "AED", false), Ok(()));
        // Not distance sales: France, outside the EU, a business buyer, a
        // seller under the exemption.
        assert_eq!(
            accepts(REGISTERED, &to("FR", "75002"), "AED", false),
            Ok(())
        );
        assert_eq!(
            accepts(REGISTERED, &to("US", "10001"), "AED", false),
            Ok(())
        );
        let business = Destination {
            vat_number: Some("DE123456789".to_owned()),
            ..to("DE", "10115")
        };
        assert_eq!(accepts(REGISTERED, &business, "AED", false), Ok(()));
        assert!(!needs_euro_rate(REGISTERED, &business, "AED"));
        assert_eq!(
            accepts(TaxProfile::default(), &germany, "AED", false),
            Ok(())
        );
    }

    /// A sale in dollars counts for its worth in euros at the day's rate,
    /// which is what decides whether it passes the threshold.
    #[test]
    fn a_sale_in_another_currency_counts_at_the_euro_rate() {
        // 1 € = 1.25 $: 120.00 $ at 20 % is 100.00 $ excluding VAT, 80.00 €.
        let usd = ExchangeRate::ecb("USD", "1.25", "2026-10-14");
        let lines = [("standard", 12_000)];
        let first = decide(
            REGISTERED,
            &to("DE", "10115"),
            &lines,
            "USD",
            Some(&usd),
            below(),
        );
        assert_eq!(first.regime, Regime::DistanceOrigin);
        assert_eq!(first.rates, vec![Some(2000)]);
        assert_eq!(first.distance.sales_cents, 8_000);
        assert_eq!(first.euro_rate.as_ref(), Some(&usd));

        // 9,930 € so far: 80 € more passes 10,000 €, so the sale is German
        // already, and counted at the German rate (120 $ at 19 %: 100.84 $,
        // 80.67 €).
        let near = DistanceSales {
            sales_cents: 993_000,
            passed: false,
        };
        let crossing = decide(
            REGISTERED,
            &to("DE", "10115"),
            &lines,
            "USD",
            Some(&usd),
            near,
        );
        assert_eq!(crossing.regime, Regime::Oss);
        assert_eq!(crossing.tax_country, "DE");
        assert_eq!(crossing.rates, vec![Some(1900)]);
        assert!(crossing.distance.passed);
        assert_eq!(crossing.distance.sales_cents, 993_000 + 8_067);
        // Just short: 9,910 € and 80 € is 9,990 €, French VAT.
        let short = DistanceSales {
            sales_cents: 991_000,
            passed: false,
        };
        let under = decide(
            REGISTERED,
            &to("DE", "10115"),
            &lines,
            "USD",
            Some(&usd),
            short,
        );
        assert_eq!(under.regime, Regime::DistanceOrigin);
        assert_eq!(under.distance.sales_cents, 999_000);
        assert!(!under.distance.passed);

        // The rate of another currency is no use: not counted, taxed at
        // destination (what `accepts` refuses without the One-Stop Shop).
        let gbp = ExchangeRate::ecb("GBP", "0.86", "2026-10-14");
        let unconverted = decide(
            REGISTERED,
            &to("DE", "10115"),
            &lines,
            "USD",
            Some(&gbp),
            below(),
        );
        assert_eq!(unconverted.regime, Regime::Oss);
        assert_eq!(unconverted.distance, below());
        assert_eq!(unconverted.euro_rate, None);
        let without = decide(REGISTERED, &to("DE", "10115"), &lines, "USD", None, below());
        assert_eq!(without.regime, Regime::Oss);
        assert_eq!(without.distance, below());
        // A rate given with euros changes nothing and is not recorded.
        let euros = decide(
            REGISTERED,
            &to("DE", "10115"),
            &lines,
            "EUR",
            Some(&usd),
            below(),
        );
        assert_eq!(euros.distance.sales_cents, 10_000);
        assert_eq!(euros.euro_rate, None);
        // Registered for the One-Stop Shop: German VAT as always, and the
        // sale counted now that it can be.
        let oss = TaxProfile {
            vat_registered: true,
            oss_registered: true,
        };
        let joined = decide(oss, &to("DE", "10115"), &lines, "USD", Some(&usd), below());
        assert_eq!(joined.regime, Regime::Oss);
        assert_eq!(joined.distance.sales_cents, 8_067);
        assert_eq!(joined.euro_rate.as_ref(), Some(&usd));
        // An exempt seller's sale counts the same way.
        let exempt = decide(
            TaxProfile::default(),
            &to("DE", "10115"),
            &lines,
            "USD",
            Some(&usd),
            below(),
        );
        assert_eq!(exempt.regime, Regime::Franchise);
        assert_eq!(exempt.distance.sales_cents, 9_600);
        // A French sale in dollars: neither counted nor recorded.
        let french = decide(
            REGISTERED,
            &to("FR", "75002"),
            &lines,
            "USD",
            Some(&usd),
            below(),
        );
        assert_eq!(french.regime, Regime::Domestic);
        assert_eq!(french.euro_rate, None);
    }

    #[test]
    fn an_exempt_seller_still_counts_its_eu_sales() {
        let decision = decide(
            TaxProfile::default(),
            &to("BE", "1000"),
            &[("standard", 5_000)],
            "EUR",
            None,
            DistanceSales {
                sales_cents: 998_000,
                passed: false,
            },
        );
        assert_eq!(decision.regime, Regime::Franchise);
        assert_eq!(decision.distance.sales_cents, 1_003_000);
        assert!(
            decision.distance.passed,
            "past the threshold: it must register"
        );
    }
}
