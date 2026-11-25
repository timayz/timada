//! What a tuning round decides, from the numbers of a campaign's last
//! weeks: pure arithmetic, the same for every shop, on thresholds the shop
//! sets ([`TuningSettings`]).
//!
//! - **Bids.** An ad group with enough clicks has its bid moved by one
//!   step, or two: up when what it brings is at or above its target, down
//!   when it is under half of it. The target is the category's break-even
//!   when the shop's sales say it, the shop's own target otherwise. One
//!   that barely serves is raised a step whatever its clicks. With too few
//!   clicks nothing is proposed, and the round says so.
//! - **Draining products.** Many clicks and not one sale: kept out of ads,
//!   never out of the shop. One that sells below its break-even is flagged
//!   for its price, not excluded.
//! - **Winners.** Enough clicks and a return well above break-even: named
//!   for a campaign of their own.
//! - **Searches.** Clicks, no sale, and not a word in common with the
//!   products they showed: excluded.
//! - **Devices.** A kind of device with enough clicks, whose return is
//!   well off the campaign's, is paid five or ten points more or less.
//!
//! Each suggestion carries the numbers it was decided on. What a round
//! suggests is prepared as proposals by the host: nothing here changes
//! anything.

use crate::value_object::{MAX_ADJUSTMENT, MIN_ADJUSTMENT, TUNED_DEVICES, TuningSettings};

/// An ad group's bid.
pub const TUNED_BID: &str = "max_cpc";
/// A device's adjustment, in hundredths.
pub const TUNED_ADJUSTMENT: &str = "adjustment";
/// Whether campaigns show a product.
pub const TUNED_ADS: &str = "ads";
/// Whether the campaign shows for a search.
pub const TUNED_SEARCH: &str = "search";

/// What [`TUNED_ADS`] and [`TUNED_SEARCH`] hold.
pub const SHOWN: &str = "shown";
pub const KEPT_OUT: &str = "kept out";
pub const EXCLUDED: &str = "excluded";

/// What the subject of a bid's proposal starts with for an ad group, and
/// for a kind of device; the ad group's number or the device follows.
pub const GROUP_PREFIX: &str = "group:";
pub const DEVICE_PREFIX: &str = "device:";

/// Under how many impressions over a round's days an ad group barely
/// serves.
pub const BARELY_IMPRESSIONS: u64 = 200;

/// The most a round suggests of each kind.
pub const MAX_SUGGESTIONS: usize = 50;

/// An ad group over a round's days.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupFigures {
    pub ad_group_id: String,
    pub name: String,
    /// What a click is paid at most now, in hundredths.
    pub bid_cents: u64,
    pub impressions: u64,
    pub clicks: u64,
    pub cost_micros: u64,
    /// What Google Ads says its conversions were worth.
    pub value_micros: u64,
    /// What has to be sold for 1 left before ads, in hundredths, when the
    /// shop's sales of the category say it.
    pub break_even: Option<i64>,
    /// Its bid was changed too lately to be looked at again.
    pub resting: bool,
}

/// A product over a round's days.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductFigures {
    pub product_id: String,
    pub name: String,
    pub clicks: u64,
    pub cost_micros: u64,
    pub conversions_milli: u64,
    pub value_micros: u64,
    /// How many of it the orders that came from an ad bought.
    pub sold: i64,
    pub break_even: Option<i64>,
    /// Kept out of ads already, itself or through its category.
    pub out_of_ads: bool,
}

/// A search over a round's days.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TermFigures {
    pub term: String,
    /// The ad group it showed in, by name.
    pub group: String,
    pub clicks: u64,
    pub cost_micros: u64,
    pub conversions_milli: u64,
    /// It shares a word with the ad group's category or its products.
    pub related: bool,
    /// The campaign no longer shows for it.
    pub excluded: bool,
}

/// A kind of device over a round's days.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceFigures {
    /// One of [`TUNED_DEVICES`].
    pub device: String,
    pub clicks: u64,
    pub cost_micros: u64,
    pub value_micros: u64,
    /// How much more or less its clicks are paid now, in hundredths.
    pub percent: i32,
    pub resting: bool,
}

/// One change a round suggests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// What it is about: `group:{ad group}`, `device:{device}`, a
    /// product's id or a search.
    pub subject_id: String,
    pub subject_name: String,
    /// One of [`TUNED_BID`], [`TUNED_ADJUSTMENT`], [`TUNED_ADS`],
    /// [`TUNED_SEARCH`].
    pub field: &'static str,
    /// The value the field would take.
    pub after: String,
    /// The numbers it was decided on.
    pub reason: String,
}

/// What a round decided.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Round {
    pub suggestions: Vec<Suggestion>,
    /// What was looked at and left alone, and why.
    pub notes: Vec<String>,
    /// What a person should look at, and no proposal changes.
    pub flags: Vec<String>,
    /// The products worth a campaign of their own: id, then name.
    pub winners: Vec<(String, String)>,
}

/// `value` over `cost`, in hundredths; `None` with nothing spent.
pub fn return_of(value_micros: u64, cost_micros: u64) -> Option<i64> {
    (cost_micros > 0).then(|| (value_micros as i128 * 100 / cost_micros as i128) as i64)
}

/// Hundredths as a person writes them: `3.45`.
fn hundredths(value: i64) -> String {
    format!("{}.{:02}", value / 100, value % 100)
}

/// Millionths of the account's currency, as an amount.
fn money(micros: u64, currency: &str) -> String {
    timada_core::format_money(micros / 10_000, currency)
}

/// The words of a text worth comparing: in small letters, without
/// accents, three letters at least.
fn words(text: &str) -> Vec<String> {
    let folded: String = text
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
            'ç' => 'c',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' => 'i',
            'ô' | 'ö' | 'ó' | 'õ' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ñ' => 'n',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    folded
        .split_whitespace()
        .filter(|word| word.chars().count() >= 3)
        .map(|word| {
            // A plural reads as its singular.
            word.strip_suffix('s')
                .filter(|stem| stem.chars().count() >= 3)
                .unwrap_or(word)
                .to_owned()
        })
        .collect()
}

/// Whether the search `term` shares a word with one of `names` (a
/// category's, its products'): whatever the capitals, the accents and a
/// closing `s`.
pub fn shares_a_word<'a>(term: &str, names: impl IntoIterator<Item = &'a str>) -> bool {
    let typed = words(term);
    names
        .into_iter()
        .any(|name| words(name).iter().any(|word| typed.contains(word)))
}

/// What a round suggests from the numbers of the `settings.days` days
/// before it, in the account's `currency`.
pub fn tune(
    settings: &TuningSettings,
    currency: &str,
    groups: &[GroupFigures],
    products: &[ProductFigures],
    terms: &[TermFigures],
    devices: &[DeviceFigures],
) -> Round {
    let mut round = Round::default();
    let days = settings.days;
    let step = settings.step_cents.max(1);

    let mut bids = 0;
    for group in groups {
        if bids == MAX_SUGGESTIONS {
            break;
        }
        if group.resting {
            round.notes.push(format!(
                "{}: its bid changed less than {} days ago, left to show.",
                group.name, settings.rest_days
            ));
            continue;
        }
        let spent = money(group.cost_micros, currency);
        let back = return_of(group.value_micros, group.cost_micros);
        let (target, whose) = match group.break_even {
            Some(even) if even > 0 => (even, "its break-even"),
            _ => (settings.target_return, "the shop's target"),
        };
        let (steps, why): (i64, String) = if group.clicks < settings.min_clicks {
            if group.impressions >= BARELY_IMPRESSIONS {
                round.notes.push(format!(
                    "{}: {} clicks of the {} a decision takes.",
                    group.name, group.clicks, settings.min_clicks
                ));
                continue;
            }
            (
                1,
                format!(
                    "It barely serves: {} impressions and {} clicks in {days} days.",
                    group.impressions, group.clicks
                ),
            )
        } else {
            let back = back.unwrap_or(0);
            let steps = if back * 2 >= target * 3 {
                2
            } else if back >= target {
                1
            } else if back * 4 < target {
                -2
            } else if back * 2 < target {
                -1
            } else {
                round.notes.push(format!(
                    "{}: a return of {} against {whose} of {}, within reach: left as it is.",
                    group.name,
                    hundredths(back),
                    hundredths(target)
                ));
                continue;
            };
            (
                steps,
                format!(
                    "A return of {} against {whose} of {}: {} clicks and {spent} spent in {days} days.",
                    hundredths(back),
                    hundredths(target),
                    group.clicks
                ),
            )
        };
        let bid = (group.bid_cents as i64 + steps * step as i64).max(1) as u64;
        if bid == group.bid_cents {
            continue;
        }
        bids += 1;
        round.suggestions.push(Suggestion {
            subject_id: format!("{GROUP_PREFIX}{}", group.ad_group_id),
            subject_name: group.name.clone(),
            field: TUNED_BID,
            after: timada_core::money::format_amount(bid),
            reason: why,
        });
    }

    let mut exclusions = 0;
    for product in products.iter().filter(|product| !product.out_of_ads) {
        let spent = money(product.cost_micros, currency);
        let back = return_of(product.value_micros, product.cost_micros);
        if product.clicks >= settings.drain_clicks
            && product.conversions_milli == 0
            && product.sold == 0
        {
            if exclusions < MAX_SUGGESTIONS {
                exclusions += 1;
                round.suggestions.push(Suggestion {
                    subject_id: product.product_id.clone(),
                    subject_name: product.name.clone(),
                    field: TUNED_ADS,
                    after: KEPT_OUT.to_owned(),
                    reason: format!(
                        "{} clicks and {spent} spent in {days} days, and not one sale. It stays in the shop.",
                        product.clicks
                    ),
                });
            }
            continue;
        }
        let (Some(back), Some(even)) = (back, product.break_even.filter(|even| *even > 0)) else {
            continue;
        };
        if product.clicks >= settings.winner_clicks && back * 2 >= even * 3 {
            round
                .winners
                .push((product.product_id.clone(), product.name.clone()));
        } else if back < even && product.sold > 0 {
            round.flags.push(format!(
                "{} sells below its break-even (a return of {} against {}, {spent} spent): look at its price.",
                product.name,
                hundredths(back),
                hundredths(even)
            ));
        }
    }

    let mut searches = 0;
    for term in terms {
        if searches == MAX_SUGGESTIONS {
            break;
        }
        if term.excluded
            || term.related
            || term.conversions_milli > 0
            || term.clicks < settings.term_clicks
        {
            continue;
        }
        searches += 1;
        round.suggestions.push(Suggestion {
            subject_id: term.term.clone(),
            subject_name: term.term.clone(),
            field: TUNED_SEARCH,
            after: EXCLUDED.to_owned(),
            reason: format!(
                "{} clicks and {} spent in {days} days, no sale, and not a word in common with the products of « {} ».",
                term.clicks,
                money(term.cost_micros, currency),
                term.group
            ),
        });
    }

    let all = return_of(
        devices.iter().map(|device| device.value_micros).sum(),
        devices.iter().map(|device| device.cost_micros).sum(),
    )
    .filter(|all| *all > 0);
    for device in devices
        .iter()
        .filter(|device| TUNED_DEVICES.contains(&device.device.as_str()))
    {
        let Some(all) = all else {
            break;
        };
        if device.clicks < settings.min_clicks {
            continue;
        }
        if device.resting {
            round.notes.push(format!(
                "{}: its adjustment changed less than {} days ago, left to show.",
                device.device, settings.rest_days
            ));
            continue;
        }
        let back = return_of(device.value_micros, device.cost_micros).unwrap_or(0);
        let points = if back * 2 >= all * 3 {
            10
        } else if back * 4 >= all * 5 {
            5
        } else if back * 2 <= all {
            -10
        } else if back * 4 <= all * 3 {
            -5
        } else {
            continue;
        };
        let percent = (device.percent + points).clamp(MIN_ADJUSTMENT, MAX_ADJUSTMENT);
        if percent == device.percent {
            continue;
        }
        round.suggestions.push(Suggestion {
            subject_id: format!("{DEVICE_PREFIX}{}", device.device),
            subject_name: device.device.clone(),
            field: TUNED_ADJUSTMENT,
            after: if percent == 0 {
                String::new()
            } else {
                percent.to_string()
            },
            reason: format!(
                "A return of {} there against {} for the campaign: {} clicks and {} spent in {days} days.",
                hundredths(back),
                hundredths(all),
                device.clicks,
                money(device.cost_micros, currency)
            ),
        });
    }
    round
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> TuningSettings {
        TuningSettings::default()
    }

    /// An ad group at 0.30 a click, with a return of `back` hundredths
    /// over `clicks` clicks.
    fn group(name: &str, clicks: u64, back: u64) -> GroupFigures {
        GroupFigures {
            ad_group_id: name.to_lowercase(),
            name: name.to_owned(),
            bid_cents: 30,
            impressions: clicks * 40,
            clicks,
            cost_micros: 10_000_000,
            value_micros: back * 100_000,
            break_even: None,
            resting: false,
        }
    }

    fn bids(round: &Round) -> Vec<(&str, &str)> {
        round
            .suggestions
            .iter()
            .filter(|suggestion| suggestion.field == TUNED_BID)
            .map(|suggestion| (suggestion.subject_name.as_str(), suggestion.after.as_str()))
            .collect()
    }

    #[test]
    fn a_bid_moves_by_a_step_or_two_with_what_the_ad_group_brings() {
        let groups = [
            group("Well", 120, 450),
            group("Good", 120, 300),
            group("Near", 120, 200),
            group("Poor", 120, 140),
            group("Bad", 120, 70),
            group("Few", 99, 900),
        ];
        let round = tune(&settings(), "EUR", &groups, &[], &[], &[]);
        assert_eq!(
            bids(&round),
            vec![
                ("Well", "0.34"),
                ("Good", "0.32"),
                ("Poor", "0.28"),
                ("Bad", "0.26")
            ]
        );
        // The numbers it was decided on.
        assert_eq!(
            round.suggestions[0].reason,
            "A return of 4.50 against the shop's target of 3.00: 120 clicks and 10.00 € spent in 21 days."
        );
        assert_eq!(round.suggestions[0].subject_id, "group:well");
        // Too few clicks proposes nothing and says so; so does a return
        // within reach of its target.
        assert_eq!(
            round.notes,
            vec![
                "Near: a return of 2.00 against the shop's target of 3.00, within reach: left as it is.",
                "Few: 99 clicks of the 100 a decision takes.",
            ]
        );
    }

    #[test]
    fn the_target_is_the_categorys_break_even_when_the_sales_say_it() {
        let mut thin = group("Thin", 120, 300);
        thin.break_even = Some(450);
        let mut fat = group("Fat", 120, 300);
        fat.break_even = Some(150);
        let round = tune(&settings(), "EUR", &[thin, fat], &[], &[], &[]);
        // 3.00 is under a break-even of 4.50, within reach: left; and twice
        // one of 1.50: two steps up.
        assert_eq!(bids(&round), vec![("Fat", "0.34")]);
        assert!(
            round.suggestions[0]
                .reason
                .contains("its break-even of 1.50")
        );
    }

    #[test]
    fn an_ad_group_that_barely_serves_is_raised_and_one_just_changed_is_left() {
        let mut quiet = group("Quiet", 2, 0);
        quiet.impressions = 80;
        let mut resting = group("Resting", 500, 900);
        resting.resting = true;
        let mut floor = group("Floor", 500, 10);
        floor.bid_cents = 3;
        let round = tune(&settings(), "EUR", &[quiet, resting, floor], &[], &[], &[]);
        // Never under a hundredth.
        assert_eq!(bids(&round), vec![("Quiet", "0.32"), ("Floor", "0.01")]);
        assert_eq!(
            round.suggestions[0].reason,
            "It barely serves: 80 impressions and 2 clicks in 21 days."
        );
        assert_eq!(
            round.notes,
            vec!["Resting: its bid changed less than 14 days ago, left to show."]
        );
    }

    fn product(name: &str, clicks: u64, sold: i64, back: u64) -> ProductFigures {
        ProductFigures {
            product_id: name.to_lowercase(),
            name: name.to_owned(),
            clicks,
            cost_micros: 20_000_000,
            conversions_milli: sold as u64 * 1_000,
            value_micros: back * 200_000,
            sold,
            break_even: Some(200),
            out_of_ads: false,
        }
    }

    #[test]
    fn a_product_drains_wins_or_is_flagged_for_its_price() {
        let mut out = product("Out", 400, 0, 0);
        out.out_of_ads = true;
        let mut unknown = product("Unknown", 400, 3, 900);
        unknown.break_even = None;
        let products = [
            product("Drain", 150, 0, 0),
            product("Almost", 149, 0, 0),
            product("Winner", 50, 4, 300),
            product("Young", 49, 4, 300),
            product("Cheap", 80, 2, 150),
            out,
            unknown,
        ];
        let round = tune(&settings(), "EUR", &[], &products, &[], &[]);
        assert_eq!(round.suggestions.len(), 1);
        let drained = &round.suggestions[0];
        assert_eq!(
            (
                drained.subject_id.as_str(),
                drained.field,
                drained.after.as_str()
            ),
            ("drain", TUNED_ADS, KEPT_OUT)
        );
        assert_eq!(
            drained.reason,
            "150 clicks and 20.00 € spent in 21 days, and not one sale. It stays in the shop."
        );
        assert_eq!(
            round.winners,
            vec![("winner".to_owned(), "Winner".to_owned())]
        );
        // Sold below its break-even: its price is the matter, not its ads.
        assert_eq!(
            round.flags,
            vec![
                "Cheap sells below its break-even (a return of 1.50 against 2.00, 20.00 € spent): look at its price."
            ]
        );
    }

    #[test]
    fn a_search_with_clicks_and_nothing_in_common_is_excluded() {
        let term = |term: &str, clicks: u64, related: bool| TermFigures {
            term: term.to_owned(),
            group: "Arbres à chat".to_owned(),
            clicks,
            cost_micros: 1_500_000,
            conversions_milli: 0,
            related,
            excluded: false,
        };
        let mut sold = term("niche chien", 9, false);
        sold.conversions_milli = 1_000;
        let mut gone = term("croquettes", 9, false);
        gone.excluded = true;
        let terms = [
            term("niche chien", 3, false),
            term("arbre a chat", 9, true),
            term("litière", 2, false),
            sold,
            gone,
        ];
        let round = tune(&settings(), "EUR", &[], &[], &terms, &[]);
        assert_eq!(round.suggestions.len(), 1);
        assert_eq!(round.suggestions[0].subject_id, "niche chien");
        assert_eq!(round.suggestions[0].after, EXCLUDED);
        assert!(
            round.suggestions[0]
                .reason
                .ends_with("not a word in common with the products of « Arbres à chat »."),
            "{}",
            round.suggestions[0].reason
        );

        assert!(shares_a_word(
            "Arbres à chats pas cher",
            ["Arbre à chat XXL"]
        ));
        assert!(shares_a_word("GRIFFOIR", ["Arbres", "Griffoirs en carton"]));
        assert!(!shares_a_word("niche chien", ["Arbre à chat", "Griffoir"]));
        // Short words say nothing.
        assert!(!shares_a_word("un si bel os", ["Os à mâcher si bon"]));
    }

    #[test]
    fn a_device_well_off_the_campaigns_return_is_adjusted() {
        let device = |device: &str, clicks: u64, value: u64, percent: i32| DeviceFigures {
            device: device.to_owned(),
            clicks,
            cost_micros: 10_000_000,
            value_micros: value * 1_000_000,
            percent,
            resting: false,
        };
        // 60 back for 30 spent: the campaign returns 2.00.
        let devices = [
            device("mobile", 200, 5, -55),
            device("desktop", 200, 40, 0),
            device("tablet", 20, 15, -40),
        ];
        let round = tune(&settings(), "EUR", &[], &[], &[], &devices);
        let adjusted: Vec<_> = round
            .suggestions
            .iter()
            .map(|suggestion| (suggestion.subject_id.as_str(), suggestion.after.as_str()))
            .collect();
        // Phones bring a quarter of it: ten points less. Computers twice:
        // ten more. Tablets have too few clicks.
        assert_eq!(
            adjusted,
            vec![("device:mobile", "-65"), ("device:desktop", "10")]
        );
        assert_eq!(round.suggestions[0].field, TUNED_ADJUSTMENT);

        // Back to none reads as nothing, and the floor is the floor.
        let devices = [
            device("mobile", 200, 40, -5),
            device("desktop", 200, 5, -90),
        ];
        let round = tune(&settings(), "EUR", &[], &[], &[], &devices);
        assert_eq!(round.suggestions.len(), 1);
        assert_eq!(round.suggestions[0].after, "5");
    }
}
