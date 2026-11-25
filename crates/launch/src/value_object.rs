//! The vocabulary of a launch: its checkpoints, the thresholds they are
//! judged on, and the judging itself.

use crate::error::LaunchError;

/// The days, counted from the first campaign's start, the numbers are
/// read at. The last opens a window: day 120 to 180.
pub const CHECKPOINTS: [u32; 4] = [21, 45, 90, 120];

/// The numbers say to carry on.
pub const GO_ON: &str = "go_on";
/// They say something has to be put right first.
pub const FIX: &str = "fix";
/// They say to stop, or to change niche.
pub const STOP: &str = "stop";

/// What a checkpoint may say, and what a merchant may decide.
pub const ADVICES: [&str; 3] = [GO_ON, FIX, STOP];

/// Who the host's own writes say made them.
pub const LAUNCH_ACTOR: &str = "timada-launch";

/// The longest note a decision carries.
pub const MAX_NOTE_LEN: usize = 1_000;

/// How long the notice of a checkpoint is worth sending: a week.
pub const NOTICE_WINDOW_SECS: u64 = 7 * 24 * 60 * 60;

/// The stream of a shop's launch. A tenant's stream ids are one space
/// whatever the aggregate.
pub fn launch_id() -> String {
    evento::hash_ids(vec!["launch"])
}

/// The stream of the notice of one checkpoint.
pub fn notice_id(checkpoint: u32) -> String {
    evento::hash_ids(vec!["launch-notice", &checkpoint.to_string()])
}

/// Unix seconds at which the checkpoint of day `checkpoint` comes for a
/// campaign that started at `started_at`.
pub fn checkpoint_at(started_at: u64, checkpoint: u32) -> u64 {
    started_at + u64::from(checkpoint) * 86_400
}

/// Day 21: the return of the first campaign at or above which to go on.
pub const D21_GO: &str = "d21_go";
/// Day 21: the return under which, with enough clicks, to stop.
pub const D21_STOP: &str = "d21_stop";
/// Day 21: how many clicks make a poor return a verdict.
pub const D21_STOP_CLICKS: &str = "d21_stop_clicks";
/// Day 45: how far under its break-even, in hundredths of it, the return
/// may be and still be within reach.
pub const D45_WITHIN: &str = "d45_within";
/// Day 90: how many categories have to stand in the first positions.
pub const D90_CATEGORIES: &str = "d90_categories";
/// Day 90: the last of those positions.
pub const D90_POSITION: &str = "d90_position";
/// Day 120: the margin after ads, in hundredths, at or above which to go
/// on.
pub const D120_GO: &str = "d120_go";
/// Day 120: the margin under which, when it is not improving, to stop.
pub const D120_STOP: &str = "d120_stop";

/// The thresholds, in the order a page shows them, each with its default.
/// They come from one case study: a shop is expected to tune them.
pub const THRESHOLDS: [(&str, &str); 8] = [
    (D21_GO, "0.70"),
    (D21_STOP, "0.40"),
    (D21_STOP_CLICKS, "1000"),
    (D45_WITHIN, "30"),
    (D90_CATEGORIES, "2"),
    (D90_POSITION, "20"),
    (D120_GO, "15"),
    (D120_STOP, "5"),
];

/// An amount as a person types it (`0.7`, `12,50`), in hundredths.
pub fn parse_hundredths(value: &str) -> Option<u64> {
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

/// Hundredths as a person writes them: `0.70`.
pub fn hundredths(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let value = value.unsigned_abs();
    format!("{sign}{}.{:02}", value / 100, value % 100)
}

/// `value` as the threshold `name` is kept.
pub fn normalize_threshold(name: &str, value: &str) -> Result<String, LaunchError> {
    let value = value.trim().trim_end_matches('%').trim();
    let whole = |range: std::ops::RangeInclusive<u64>| {
        value
            .parse::<u64>()
            .ok()
            .filter(|n| range.contains(n))
            .map(|n| n.to_string())
            .ok_or(LaunchError::InvalidThreshold)
    };
    match name {
        D21_GO | D21_STOP => parse_hundredths(value)
            .filter(|hundredths| (1..=10_000).contains(hundredths))
            .map(|value| hundredths(value as i64))
            .ok_or(LaunchError::InvalidThreshold),
        D21_STOP_CLICKS => whole(1..=1_000_000),
        D45_WITHIN => whole(1..=99),
        D90_CATEGORIES => whole(1..=100),
        D90_POSITION => whole(1..=100),
        D120_GO | D120_STOP => whole(0..=100),
        _ => Err(LaunchError::InvalidThreshold),
    }
}

/// The thresholds the checkpoints are judged on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    /// In hundredths: 70 is 0.70 back for 1 spent.
    pub d21_go: i64,
    pub d21_stop: i64,
    pub d21_stop_clicks: u64,
    /// In hundredths of the break-even.
    pub d45_within: i64,
    pub d90_categories: u64,
    pub d90_position: u64,
    /// In hundredths of a percent: 1,500 is 15 %.
    pub d120_go_bps: i64,
    pub d120_stop_bps: i64,
}

impl Default for Thresholds {
    fn default() -> Self {
        thresholds_of(std::iter::empty()).expect("the defaults are thresholds")
    }
}

impl Thresholds {
    /// What `name` is set at, as a person reads it.
    pub fn value(&self, name: &str) -> String {
        match name {
            D21_GO => hundredths(self.d21_go),
            D21_STOP => hundredths(self.d21_stop),
            D21_STOP_CLICKS => self.d21_stop_clicks.to_string(),
            D45_WITHIN => self.d45_within.to_string(),
            D90_CATEGORIES => self.d90_categories.to_string(),
            D90_POSITION => self.d90_position.to_string(),
            D120_GO => (self.d120_go_bps / 100).to_string(),
            D120_STOP => (self.d120_stop_bps / 100).to_string(),
            _ => String::new(),
        }
    }
}

/// The thresholds that `set` (name, value) say, the default for each one
/// left out. The bar to go on is never under the bar to stop.
pub fn thresholds_of<'a>(
    set: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<Thresholds, LaunchError> {
    let given: Vec<(&str, &str)> = set.into_iter().collect();
    let kept = |name: &str| -> Result<String, LaunchError> {
        let default = THRESHOLDS
            .iter()
            .find(|(named, _)| *named == name)
            .map_or("", |(_, default)| *default);
        let value = given
            .iter()
            .find(|(named, _)| *named == name)
            .map_or(default, |(_, value)| *value);
        normalize_threshold(name, value)
    };
    let whole = |name: &str| -> Result<u64, LaunchError> {
        kept(name)?
            .parse()
            .map_err(|_| LaunchError::InvalidThreshold)
    };
    let amount = |name: &str| -> Result<i64, LaunchError> {
        parse_hundredths(&kept(name)?)
            .map(|value| value as i64)
            .ok_or(LaunchError::InvalidThreshold)
    };
    let thresholds = Thresholds {
        d21_go: amount(D21_GO)?,
        d21_stop: amount(D21_STOP)?,
        d21_stop_clicks: whole(D21_STOP_CLICKS)?,
        d45_within: whole(D45_WITHIN)? as i64,
        d90_categories: whole(D90_CATEGORIES)?,
        d90_position: whole(D90_POSITION)?,
        d120_go_bps: whole(D120_GO)? as i64 * 100,
        d120_stop_bps: whole(D120_STOP)? as i64 * 100,
    };
    if thresholds.d21_stop > thresholds.d21_go || thresholds.d120_stop_bps > thresholds.d120_go_bps
    {
        return Err(LaunchError::InvalidThreshold);
    }
    Ok(thresholds)
}

/// What a checkpoint reads, as the host found it on its day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// The first campaign's first three weeks, as Google Ads counts them.
    Day21 {
        /// What its conversions were worth for 1 spent, in hundredths;
        /// `None` with nothing spent.
        return_hundredths: Option<i64>,
        clicks: u64,
    },
    /// The shop's own orders that came from an ad, over the weeks before
    /// day 45.
    Day45 {
        /// What they sold for 1 spent, in hundredths.
        return_hundredths: Option<i64>,
        /// What has to be sold for 1 left before ads; `None` while the
        /// sales do not say what they left.
        break_even: Option<i64>,
        /// Whether a tuning round was prepared since the campaign started.
        tuned: bool,
    },
    /// Search Console, over the four weeks before day 90 and the four
    /// before those.
    Day90 {
        /// Whether Search Console is read for the shop at all.
        read: bool,
        /// How many categories stand in the first positions.
        placed: u64,
        impressions: u64,
        impressions_before: u64,
    },
    /// Every channel together, over the thirty days before day 120 and the
    /// thirty before those.
    Day120 {
        /// What is left of the revenue once goods, costs and ads are paid,
        /// in hundredths of a percent; `None` while the sales do not say
        /// what they left, or nothing was sold.
        margin_bps: Option<i64>,
        margin_before_bps: Option<i64>,
    },
}

/// A percentage from hundredths of a percent: `12.5 %`.
fn percent(bps: i64) -> String {
    let sign = if bps < 0 { "-" } else { "" };
    let bps = bps.unsigned_abs();
    if bps.is_multiple_of(100) {
        format!("{sign}{} %", bps / 100)
    } else {
        format!("{sign}{}.{} %", bps / 100, (bps % 100) / 10)
    }
}

impl Reading {
    /// The day it is read at.
    pub fn checkpoint(&self) -> u32 {
        match self {
            Self::Day21 { .. } => 21,
            Self::Day45 { .. } => 45,
            Self::Day90 { .. } => 90,
            Self::Day120 { .. } => 120,
        }
    }

    /// The numbers, each named for a person.
    pub fn figures(&self) -> Vec<(String, String)> {
        let known = |value: Option<i64>, shown: fn(i64) -> String| {
            value.map_or_else(|| "Not known".to_owned(), shown)
        };
        let figure = |name: &str, value: String| (name.to_owned(), value);
        match *self {
            Self::Day21 {
                return_hundredths,
                clicks,
            } => vec![
                figure(
                    "Return on ad spend, as Google Ads counts it",
                    known(return_hundredths, hundredths),
                ),
                figure("Clicks", clicks.to_string()),
            ],
            Self::Day45 {
                return_hundredths,
                break_even,
                tuned,
            } => vec![
                figure(
                    "Return on ad spend, from the shop's orders",
                    known(return_hundredths, hundredths),
                ),
                figure("Break-even", known(break_even, hundredths)),
                figure(
                    "Tuning round",
                    if tuned { "Made" } else { "None yet" }.to_owned(),
                ),
            ],
            Self::Day90 { read: false, .. } => {
                vec![figure("Search Console", "Not read".to_owned())]
            }
            Self::Day90 {
                placed,
                impressions,
                impressions_before,
                ..
            } => vec![
                figure("Categories in the first positions", placed.to_string()),
                figure("Impressions, last four weeks", impressions.to_string()),
                figure(
                    "Impressions, the four weeks before",
                    impressions_before.to_string(),
                ),
            ],
            Self::Day120 {
                margin_bps,
                margin_before_bps,
            } => vec![
                figure(
                    "Margin after ads, last thirty days",
                    known(margin_bps, percent),
                ),
                figure(
                    "Margin after ads, the thirty days before",
                    known(margin_before_bps, percent),
                ),
            ],
        }
    }
}

/// What `reading` says against `thresholds`: one of [`ADVICES`], and why.
pub fn judge(thresholds: &Thresholds, reading: &Reading) -> (&'static str, String) {
    match *reading {
        Reading::Day21 {
            return_hundredths: None,
            ..
        } => (
            FIX,
            "The campaign spent nothing in its first three weeks: it does not serve.".to_owned(),
        ),
        Reading::Day21 {
            return_hundredths: Some(back),
            clicks,
        } => {
            if back >= thresholds.d21_go {
                (
                    GO_ON,
                    format!(
                        "A return of {} is at or above {}: on course for a first campaign.",
                        hundredths(back),
                        hundredths(thresholds.d21_go)
                    ),
                )
            } else if back < thresholds.d21_stop && clicks > thresholds.d21_stop_clicks {
                (
                    STOP,
                    format!(
                        "A return of {} is under {} after {clicks} clicks: the niche does not pay for its clicks.",
                        hundredths(back),
                        hundredths(thresholds.d21_stop)
                    ),
                )
            } else if back < thresholds.d21_stop {
                (
                    FIX,
                    format!(
                        "A return of {} is under {}, on {clicks} clicks only: too few to call it, look at the products and the prices.",
                        hundredths(back),
                        hundredths(thresholds.d21_stop)
                    ),
                )
            } else {
                (
                    FIX,
                    format!(
                        "A return of {} is between {} and {}: the products, the prices or the bids need work.",
                        hundredths(back),
                        hundredths(thresholds.d21_stop),
                        hundredths(thresholds.d21_go)
                    ),
                )
            }
        }
        Reading::Day45 {
            return_hundredths,
            break_even,
            tuned,
        } => {
            let (Some(back), Some(even)) = (return_hundredths, break_even.filter(|even| *even > 0))
            else {
                return (
                    FIX,
                    "The return cannot be held against a break-even: no order came from an ad, or the sales do not say what they left. Say what the goods cost.".to_owned(),
                );
            };
            let within = even * (100 - thresholds.d45_within) / 100;
            if back >= even {
                (
                    GO_ON,
                    format!(
                        "A return of {} is at or above the break-even of {}: the ads pay for themselves.",
                        hundredths(back),
                        hundredths(even)
                    ),
                )
            } else if back >= within {
                (
                    FIX,
                    format!(
                        "A return of {} is within {} % of the break-even of {}: a tuning round may close it.",
                        hundredths(back),
                        thresholds.d45_within,
                        hundredths(even)
                    ),
                )
            } else if back * 2 < even && tuned {
                (
                    STOP,
                    format!(
                        "A return of {} is under half the break-even of {}, after a tuning round.",
                        hundredths(back),
                        hundredths(even)
                    ),
                )
            } else if back * 2 < even {
                (
                    FIX,
                    format!(
                        "A return of {} is under half the break-even of {}, and no tuning round was made yet: make one.",
                        hundredths(back),
                        hundredths(even)
                    ),
                )
            } else {
                (
                    FIX,
                    format!(
                        "A return of {} is more than {} % under the break-even of {}.",
                        hundredths(back),
                        thresholds.d45_within,
                        hundredths(even)
                    ),
                )
            }
        }
        Reading::Day90 { read: false, .. } => (
            FIX,
            "Search Console is not read for the shop: connect it, and choose the shop's site, to know where its categories stand.".to_owned(),
        ),
        Reading::Day90 {
            placed,
            impressions,
            impressions_before,
            ..
        } => {
            if placed >= thresholds.d90_categories {
                (
                    GO_ON,
                    format!(
                        "{placed} categories stand in positions 1 to {}.",
                        thresholds.d90_position
                    ),
                )
            } else if impressions > impressions_before {
                (
                    FIX,
                    format!(
                        "{placed} categories in positions 1 to {}, but the impressions are rising ({impressions_before} to {impressions}): work the categories that show beyond.",
                        thresholds.d90_position
                    ),
                )
            } else {
                (
                    STOP,
                    format!(
                        "{placed} categories in positions 1 to {}, and the impressions are not rising ({impressions_before} to {impressions}).",
                        thresholds.d90_position
                    ),
                )
            }
        }
        Reading::Day120 {
            margin_bps: None, ..
        } => (
            FIX,
            "The margin after ads is not known: nothing was sold, or the sales do not say what they left.".to_owned(),
        ),
        Reading::Day120 {
            margin_bps: Some(margin),
            margin_before_bps,
        } => {
            if margin >= thresholds.d120_go_bps {
                (
                    GO_ON,
                    format!(
                        "A margin after ads of {} is at or above {}.",
                        percent(margin),
                        percent(thresholds.d120_go_bps)
                    ),
                )
            } else if margin >= thresholds.d120_stop_bps {
                (
                    FIX,
                    format!(
                        "A margin after ads of {} is between {} and {}.",
                        percent(margin),
                        percent(thresholds.d120_stop_bps),
                        percent(thresholds.d120_go_bps)
                    ),
                )
            } else if margin_before_bps.is_some_and(|before| margin > before) {
                (
                    FIX,
                    format!(
                        "A margin after ads of {} is under {}, but improving.",
                        percent(margin),
                        percent(thresholds.d120_stop_bps)
                    ),
                )
            } else {
                (
                    STOP,
                    format!(
                        "A margin after ads of {} is under {} and not improving.",
                        percent(margin),
                        percent(thresholds.d120_stop_bps)
                    ),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advice(reading: Reading) -> &'static str {
        judge(&Thresholds::default(), &reading).0
    }

    #[test]
    fn day_21_reads_the_first_campaigns_return() {
        let day = |back: Option<i64>, clicks| Reading::Day21 {
            return_hundredths: back,
            clicks,
        };
        assert_eq!(advice(day(Some(70), 300)), GO_ON);
        assert_eq!(advice(day(Some(69), 300)), FIX);
        assert_eq!(advice(day(Some(40), 5_000)), FIX);
        // A poor return is a verdict only on many clicks.
        assert_eq!(advice(day(Some(39), 1_001)), STOP);
        assert_eq!(advice(day(Some(39), 1_000)), FIX);
        assert_eq!(advice(day(None, 0)), FIX);
        let (_, why) = judge(&Thresholds::default(), &day(Some(39), 1_001));
        assert_eq!(
            why,
            "A return of 0.39 is under 0.40 after 1001 clicks: the niche does not pay for its clicks."
        );
    }

    #[test]
    fn day_45_holds_the_return_against_the_break_even() {
        let day = |back: Option<i64>, even: Option<i64>, tuned| Reading::Day45 {
            return_hundredths: back,
            break_even: even,
            tuned,
        };
        assert_eq!(advice(day(Some(300), Some(300), false)), GO_ON);
        // Within 30 %.
        assert_eq!(advice(day(Some(210), Some(300), true)), FIX);
        assert_eq!(advice(day(Some(209), Some(300), true)), FIX);
        // Under half: a stop only once a tuning round was tried.
        assert_eq!(advice(day(Some(149), Some(300), true)), STOP);
        assert_eq!(advice(day(Some(149), Some(300), false)), FIX);
        assert_eq!(advice(day(Some(149), None, true)), FIX);
        assert_eq!(advice(day(None, Some(300), true)), FIX);
    }

    #[test]
    fn day_90_reads_search_and_day_120_the_margin() {
        let search = |placed, now, before| Reading::Day90 {
            read: true,
            placed,
            impressions: now,
            impressions_before: before,
        };
        assert_eq!(advice(search(2, 10, 100)), GO_ON);
        assert_eq!(advice(search(1, 101, 100)), FIX);
        assert_eq!(advice(search(1, 100, 100)), STOP);
        // Nothing read of search is not a verdict on it.
        let unread = Reading::Day90 {
            read: false,
            placed: 0,
            impressions: 0,
            impressions_before: 0,
        };
        assert_eq!(advice(unread), FIX);
        let margin = |now: Option<i64>, before: Option<i64>| Reading::Day120 {
            margin_bps: now,
            margin_before_bps: before,
        };
        assert_eq!(advice(margin(Some(1_500), None)), GO_ON);
        assert_eq!(advice(margin(Some(500), None)), FIX);
        assert_eq!(advice(margin(Some(499), Some(100))), FIX);
        assert_eq!(advice(margin(Some(499), Some(499))), STOP);
        assert_eq!(advice(margin(Some(-200), None)), STOP);
        assert_eq!(advice(margin(None, Some(900))), FIX);
        assert_eq!(
            judge(&Thresholds::default(), &margin(Some(1_250), None)).1,
            "A margin after ads of 12.5 % is between 5 % and 15 %."
        );
    }

    #[test]
    fn thresholds_are_the_defaults_until_set_and_keep_their_order() {
        let defaults = Thresholds::default();
        assert_eq!((defaults.d21_go, defaults.d21_stop), (70, 40));
        assert_eq!(defaults.d21_stop_clicks, 1_000);
        assert_eq!((defaults.d120_go_bps, defaults.d120_stop_bps), (1_500, 500));
        for (name, default) in THRESHOLDS {
            assert_eq!(defaults.value(name), default, "{name}");
        }
        let set = thresholds_of([(D21_GO, "1"), (D120_GO, "20 %")]).unwrap();
        assert_eq!((set.d21_go, set.d120_go_bps), (100, 2_000));
        // The bar to go on is never under the bar to stop.
        assert!(thresholds_of([(D21_GO, "0.30")]).is_err());
        assert!(thresholds_of([(D120_STOP, "16")]).is_err());
        assert!(thresholds_of([(D21_STOP_CLICKS, "many")]).is_err());
        assert!(normalize_threshold("d200_go", "1").is_err());
        assert_eq!(checkpoint_at(1_000, 21), 1_000 + 21 * 86_400);
        assert_ne!(notice_id(21), notice_id(45));
        assert_ne!(notice_id(21), launch_id());
    }
}
