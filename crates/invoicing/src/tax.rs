//! Which VAT a sale carries, for a seller established in France: the regime
//! (domestic, distance sale taxed in France or through the One-Stop Shop,
//! reverse charge, export, exemption) and the rate of each line.
//!
//! Prices include VAT and the shopper always pays the same price: the
//! regime only decides how that price splits into base and tax. The rate of
//! a level (standard, reduced…) in each country comes from the `world-tax`
//! crate; which level a kind of goods falls under in each country is ours
//! (`data/tax_categories.json`), and so is the yearly threshold of EU
//! distance sales (the crate checks a threshold per sale).

use std::collections::HashMap;
use std::sync::OnceLock;

use world_tax::TaxDatabase;

use crate::value_object::vat_breakdown;

/// The seller's country: every rule here is French law.
pub const SELLER_COUNTRY: &str = "FR";
/// Sales to consumers in other EU countries, all together, excluding VAT,
/// above which their VAT applies instead of France's (art. 258 A du CGI).
pub const EU_DISTANCE_THRESHOLD_CENTS: u64 = 1_000_000;
/// The category of goods with no particular rate.
const STANDARD: &str = "standard";
const CATEGORIES_JSON: &str = include_str!("../data/tax_categories.json");

/// How a sale is taxed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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

/// `category → country → level`.
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

/// The rate of a kind of goods in a country, in basis points. A level the
/// country does not have, or a kind it does not list, is its standard rate.
pub fn rate_bps(country: &str, category: &str) -> u32 {
    let Ok(rates) = database().get_country(country) else {
        tracing::warn!(country, "no VAT rates for this country: 0 %");
        return 0;
    };
    let level = categories()
        .get(category)
        .and_then(|levels| levels.get(country))
        .map_or(STANDARD, String::as_str);
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
/// in `currency`, given the year's distance sales before it. Only sales in
/// euros count towards the threshold; a distance sale in another currency
/// is taxed at destination, the safe side.
pub fn decide(
    profile: TaxProfile,
    destination: &Destination,
    lines: &[(&str, u64)],
    currency: &str,
    before: DistanceSales,
) -> Decision {
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
    let count = |sales: u64| -> DistanceSales {
        let sales_cents = if currency == "EUR" {
            before.sales_cents + sales
        } else {
            before.sales_cents
        };
        DistanceSales {
            sales_cents,
            passed: before.passed || sales_cents > EU_DISTANCE_THRESHOLD_CENTS,
        }
    };
    let none = || vec![None; lines.len()];
    match territory(&destination.country, &destination.postal_code) {
        Territory::Outside => Decision {
            regime: Regime::Export,
            tax_country: destination.country.clone(),
            rates: none(),
            distance: before,
        },
        Territory::France if !profile.vat_registered => Decision {
            regime: Regime::Franchise,
            tax_country: SELLER_COUNTRY.to_owned(),
            rates: none(),
            distance: before,
        },
        Territory::France => Decision {
            regime: Regime::Domestic,
            tax_country: SELLER_COUNTRY.to_owned(),
            rates: rates_in(SELLER_COUNTRY),
            distance: before,
        },
        Territory::Eu(country) if profile.vat_registered && destination.vat_number.is_some() => {
            Decision {
                regime: Regime::ReverseCharge,
                tax_country: country,
                rates: none(),
                distance: before,
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
            }
        }
        Territory::Eu(country) => {
            let origin = rates_in(SELLER_COUNTRY);
            let crosses = before.sales_cents + excl_vat(&origin) > EU_DISTANCE_THRESHOLD_CENTS;
            if profile.oss_registered || before.passed || currency != "EUR" || crosses {
                let rates = rates_in(&country);
                let distance = count(excl_vat(&rates));
                Decision {
                    regime: Regime::Oss,
                    tax_country: country,
                    rates,
                    distance,
                }
            } else {
                let distance = count(excl_vat(&origin));
                Decision {
                    regime: Regime::DistanceOrigin,
                    tax_country: SELLER_COUNTRY.to_owned(),
                    rates: origin,
                    distance,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
        let year = if month <= 2 { year - 1 } else { year };
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        let reviewed = era * 146_097 + day_of_era - 719_468;
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
            below(),
        );
        assert_eq!(monaco.regime, Regime::Domestic);
        let exempt = TaxProfile::default();
        let franchise = decide(
            exempt,
            &to("FR", "75002"),
            &[("standard", 1000)],
            "EUR",
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
        let decision = decide(REGISTERED, &business, &[("standard", 1000)], "EUR", below());
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
            decide(REGISTERED, &french, &[("standard", 1000)], "EUR", below()).regime,
            Regime::Domestic
        );
    }

    #[test]
    fn eu_consumers_pay_french_vat_until_the_threshold() {
        let lines = [("standard", 12_000)];
        // 100.00 excluding VAT at 20 %.
        let first = decide(REGISTERED, &to("DE", "10115"), &lines, "EUR", below());
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
        let crossing = decide(REGISTERED, &to("DE", "10115"), &lines, "EUR", near);
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
            decide(oss, &to("DE", "10115"), &lines, "EUR", below()).regime,
            Regime::Oss
        );
        // Another currency is taxed at destination and not counted.
        let usd = decide(REGISTERED, &to("DE", "10115"), &lines, "USD", below());
        assert_eq!(usd.regime, Regime::Oss);
        assert_eq!(usd.distance, below());
    }

    #[test]
    fn an_exempt_seller_still_counts_its_eu_sales() {
        let decision = decide(
            TaxProfile::default(),
            &to("BE", "1000"),
            &[("standard", 5_000)],
            "EUR",
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
