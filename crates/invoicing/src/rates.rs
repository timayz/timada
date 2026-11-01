//! Euro reference rates, for a sale priced in another currency: what it
//! counts for, in euros, against the yearly threshold of EU distance sales,
//! and the VAT a document in another currency also states in euros (CGI
//! art. 289 and directive 2006/112/CE art. 230). The rates are the European
//! Central Bank's daily euro foreign exchange reference rates, which the
//! directive lets a seller convert with (art. 91 §2): one euro buys so many
//! units of the currency, published every working day around 16:00 CET.
//!
//! This module reads the Bank's XML and answers from the days it has seen;
//! fetching the file is the host's (it keeps a [`RateTable`] current). The
//! rate of a day is the latest the Bank published on or before it: before
//! the day's publication, on a weekend or a holiday, that is the working
//! day before, which is what the directive asks (the latest published when
//! the tax becomes chargeable).

use std::collections::BTreeMap;
use std::ops::Bound;

use timada_core::civil::parse_date;

/// The one currency nothing converts.
pub const EURO: &str = "EUR";
/// The source [`ExchangeRate::source`] names for the Bank's reference rates.
pub const ECB_SOURCE: &str = "ecb";
/// How many days before the one asked for a published rate may be and still
/// serve for it: the longest run of days without a publication (Easter,
/// Christmas) is four.
pub const RATE_MAX_AGE_DAYS: i64 = 7;
/// The decimals a rate is kept with; the Bank publishes at most five.
const MICROS: u32 = 6;

/// The euro rate of a currency on a day, as published. Frozen in
/// `events.lock`: the fields never change.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct ExchangeRate {
    /// ISO 4217, uppercase.
    pub currency: String,
    /// How many units of `currency` one euro buys, as the source wrote it
    /// (`1.0850`).
    pub per_euro: String,
    /// The day the source published it, `YYYY-MM-DD`.
    pub date: String,
    /// Who published it ([`ECB_SOURCE`]).
    pub source: String,
}

impl ExchangeRate {
    /// The Bank's rate of `currency` on `date`.
    pub fn ecb(currency: &str, per_euro: &str, date: &str) -> Self {
        Self {
            currency: currency.to_ascii_uppercase(),
            per_euro: per_euro.to_owned(),
            date: date.to_owned(),
            source: ECB_SOURCE.to_owned(),
        }
    }

    /// The rate in millionths of a unit per euro; `None` when `per_euro` is
    /// not a positive decimal of at most six places.
    pub fn per_euro_micros(&self) -> Option<u64> {
        parse_decimal_micros(&self.per_euro)
    }

    /// `cents` of the currency in euro cents, rounded to the nearest, halves
    /// up; `None` when the rate does not parse.
    pub fn to_euro_cents(&self, cents: u64) -> Option<u64> {
        let micros = u128::from(self.per_euro_micros()?);
        let exact = u128::from(cents) * 10u128.pow(MICROS);
        u64::try_from((exact + micros / 2) / micros).ok()
    }

    /// Whether this is the rate of `currency` and it parses.
    pub fn converts(&self, currency: &str) -> bool {
        self.currency.eq_ignore_ascii_case(currency) && self.per_euro_micros().is_some()
    }
}

/// A positive decimal (`1.0850`, `162.08`, `17500`) in millionths; `None`
/// for anything else or more than six decimals.
pub fn parse_decimal_micros(text: &str) -> Option<u64> {
    let text = text.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > MICROS as usize
    {
        return None;
    }
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let mut scaled: u64 = 0;
    for b in fraction.bytes() {
        scaled = scaled * 10 + u64::from(b - b'0');
    }
    scaled *= 10u64.pow(MICROS - fraction.len() as u32);
    let micros = whole.checked_mul(10u64.pow(MICROS))?.checked_add(scaled)?;
    (micros > 0).then_some(micros)
}

/// The days of rates the Bank published, as this process has seen them:
/// `day → currency → rate as published`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateTable {
    days: BTreeMap<String, BTreeMap<String, String>>,
}

impl RateTable {
    /// Reads the Bank's `eurofxref-daily.xml` (one day) or
    /// `eurofxref-hist-90d.xml` (the last ninety days): every `<Cube
    /// time='…'>` and the `<Cube currency='…' rate='…'/>` under it. An
    /// entry that is not a currency, a day or a rate is skipped; a document
    /// with no day at all is an error.
    pub fn from_ecb_xml(xml: &str) -> anyhow::Result<Self> {
        let mut table = Self::default();
        let mut day: Option<String> = None;
        let mut rest = xml;
        while let Some(start) = rest.find("<Cube") {
            let tag = &rest[start..];
            let Some(end) = tag.find('>') else { break };
            let tag = &tag[..end];
            rest = &rest[start + end + 1..];
            // `<Cubes>` or `<CubeX>` would be another element.
            if tag[5..]
                .chars()
                .next()
                .is_some_and(|c| !c.is_whitespace() && c != '/')
            {
                continue;
            }
            if let Some(time) = attribute(tag, "time") {
                if parse_date(time).is_some() {
                    day = Some(time.to_owned());
                } else {
                    tracing::warn!(time, "ECB rates: not a day, skipped");
                    day = None;
                }
                continue;
            }
            if let (Some(currency), Some(rate)) =
                (attribute(tag, "currency"), attribute(tag, "rate"))
            {
                match &day {
                    Some(day) => table.insert(day, currency, rate),
                    None => tracing::warn!(currency, "ECB rates: a rate before any day, skipped"),
                }
            }
        }
        if table.days.is_empty() {
            anyhow::bail!("no day of rates in the document");
        }
        Ok(table)
    }

    /// Records the rate of `currency` on `day` (`YYYY-MM-DD`); a day that is
    /// not one, a currency that is not three letters or a rate that is not a
    /// decimal is skipped with a warning.
    pub fn insert(&mut self, day: &str, currency: &str, per_euro: &str) {
        let valid = parse_date(day).is_some()
            && currency.len() == 3
            && currency.bytes().all(|b| b.is_ascii_alphabetic())
            && parse_decimal_micros(per_euro).is_some();
        if !valid {
            tracing::warn!(day, currency, per_euro, "euro rate skipped: malformed");
            return;
        }
        self.days
            .entry(day.to_owned())
            .or_default()
            .insert(currency.to_ascii_uppercase(), per_euro.trim().to_owned());
    }

    /// Takes in the days of `other`, a newer reading of the same source: a
    /// day both have is `other`'s.
    pub fn merge(&mut self, other: RateTable) {
        for (day, rates) in other.days {
            self.days.insert(day, rates);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.days.is_empty()
    }

    /// The most recent day with rates.
    pub fn latest_day(&self) -> Option<&str> {
        self.days.keys().next_back().map(String::as_str)
    }

    /// Whether the table holds `day`'s rates.
    pub fn has_day(&self, day: &str) -> bool {
        self.days.contains_key(day)
    }

    /// Whether the source published `currency` on any day seen.
    pub fn knows(&self, currency: &str) -> bool {
        let currency = currency.to_ascii_uppercase();
        self.days
            .values()
            .any(|rates| rates.contains_key(&currency))
    }

    /// The rate of `currency` for `day` (`YYYY-MM-DD`): the latest published
    /// on or before it, at most [`RATE_MAX_AGE_DAYS`] before. `None` for a
    /// currency the source does not publish, a day too far past the last
    /// publication seen, or a day that is not one.
    pub fn rate_on(&self, currency: &str, day: &str) -> Option<ExchangeRate> {
        let asked = parse_date(day)? / 86_400;
        let currency = currency.to_ascii_uppercase();
        let (published, rate) = self
            .days
            .range::<str, _>((Bound::Unbounded, Bound::Included(day)))
            .rev()
            .find_map(|(published, rates)| rates.get(&currency).map(|rate| (published, rate)))?;
        let age = asked as i64 - (parse_date(published)? / 86_400) as i64;
        (age <= RATE_MAX_AGE_DAYS).then(|| ExchangeRate::ecb(&currency, rate, published))
    }
}

/// The value of attribute `name` in an XML start tag, whichever quotes it
/// uses.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(pos) = tag[from..].find(name) {
        let at = from + pos;
        let after = &tag[at + name.len()..];
        let bounded = at > 0 && tag.as_bytes()[at - 1].is_ascii_whitespace();
        let value = after.trim_start().strip_prefix('=').map(str::trim_start);
        if let (true, Some(value)) = (bounded, value) {
            let quote = value.chars().next()?;
            if quote != '\'' && quote != '"' {
                return None;
            }
            let value = &value[1..];
            return value.find(quote).map(|end| &value[..end]);
        }
        from = at + name.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Bank's daily file as it reads, one day.
    const DAILY_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gesmes:Envelope xmlns:gesmes="http://www.gesmes.org/xml/2002-08-01" xmlns="http://www.ecb.int/vocabulary/2002-08-01/eurofxref">
	<gesmes:subject>Reference rates</gesmes:subject>
	<gesmes:Sender>
		<gesmes:name>European Central Bank</gesmes:name>
	</gesmes:Sender>
	<Cube>
		<Cube time='2026-10-14'>
			<Cube currency='USD' rate='1.0850'/>
			<Cube currency='JPY' rate='162.08'/>
			<Cube currency='GBP' rate='0.8588'/>
			<Cube currency='CHF' rate='0.9312'/>
			<Cube currency='PLN' rate='4.3285'/>
		</Cube>
	</Cube>
</gesmes:Envelope>
"#;

    /// Three working days of the history file, Friday to Tuesday with the
    /// weekend between, the attributes quoted the other way.
    const HISTORY_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gesmes:Envelope xmlns:gesmes="http://www.gesmes.org/xml/2002-08-01" xmlns="http://www.ecb.int/vocabulary/2002-08-01/eurofxref">
	<Cube>
		<Cube time="2026-10-13">
			<Cube currency="USD" rate="1.0900"/>
			<Cube currency="GBP" rate="0.8600"/>
		</Cube>
		<Cube time="2026-10-12">
			<Cube currency="USD" rate="1.0800"/>
			<Cube currency="GBP" rate="0.8550"/>
		</Cube>
		<Cube time="2026-10-09">
			<Cube currency="USD" rate="1.0700"/>
			<Cube currency="GBP" rate="0.8500"/>
			<Cube rate="n/a" currency="BAD" />
		</Cube>
	</Cube>
</gesmes:Envelope>
"#;

    /// A fixture source: the Bank's daily file and three days of history.
    fn fixture() -> RateTable {
        let mut table = RateTable::from_ecb_xml(HISTORY_XML).unwrap();
        table.merge(RateTable::from_ecb_xml(DAILY_XML).unwrap());
        table
    }

    #[test]
    fn decimals_are_kept_to_the_millionth() {
        assert_eq!(parse_decimal_micros("1.0850"), Some(1_085_000));
        assert_eq!(parse_decimal_micros("162.08"), Some(162_080_000));
        assert_eq!(parse_decimal_micros("17500"), Some(17_500_000_000));
        assert_eq!(parse_decimal_micros(".5"), Some(500_000));
        assert_eq!(parse_decimal_micros("0.123456"), Some(123_456));
        assert_eq!(parse_decimal_micros("0.1234567"), None, "too fine");
        assert_eq!(parse_decimal_micros("0"), None, "nothing is worth nothing");
        assert_eq!(parse_decimal_micros("-1"), None);
        assert_eq!(parse_decimal_micros("1,0850"), None);
        assert_eq!(parse_decimal_micros(""), None);
        assert_eq!(parse_decimal_micros("."), None);
    }

    #[test]
    fn a_rate_converts_to_euro_cents_rounding_halves_up() {
        let usd = ExchangeRate::ecb("usd", "1.0850", "2026-10-14");
        assert_eq!(usd.currency, "USD");
        assert_eq!(usd.source, ECB_SOURCE);
        // 100.00 USD at 1.0850: 92.1659 € → 92.17 €.
        assert_eq!(usd.to_euro_cents(10_000), Some(9_217));
        assert_eq!(usd.to_euro_cents(0), Some(0));
        // 1.085 USD is one euro exactly; 0.5425 is half, up to one cent.
        assert_eq!(
            ExchangeRate::ecb("USD", "1.085", "2026-10-14").to_euro_cents(1085),
            Some(1000)
        );
        assert_eq!(
            ExchangeRate::ecb("USD", "2", "2026-10-14").to_euro_cents(1),
            Some(1)
        );
        assert_eq!(
            ExchangeRate::ecb("USD", "3", "2026-10-14").to_euro_cents(1),
            Some(0)
        );
        // 100.00 JPY at 162.08: 0.6170 € → 0.62 €.
        let jpy = ExchangeRate::ecb("JPY", "162.08", "2026-10-14");
        assert_eq!(jpy.to_euro_cents(10_000), Some(62));
        assert!(usd.converts("USD"));
        assert!(usd.converts("usd"));
        assert!(!usd.converts("GBP"));
        let broken = ExchangeRate::ecb("USD", "n/a", "2026-10-14");
        assert!(!broken.converts("USD"));
        assert_eq!(broken.to_euro_cents(100), None);
    }

    #[test]
    fn the_banks_files_are_read_whichever_way_they_are_quoted() {
        let daily = RateTable::from_ecb_xml(DAILY_XML).unwrap();
        assert_eq!(daily.latest_day(), Some("2026-10-14"));
        assert_eq!(
            daily.rate_on("USD", "2026-10-14"),
            Some(ExchangeRate::ecb("USD", "1.0850", "2026-10-14"))
        );
        assert_eq!(
            daily.rate_on("pln", "2026-10-14").map(|rate| rate.per_euro),
            Some("4.3285".to_owned())
        );
        let history = RateTable::from_ecb_xml(HISTORY_XML).unwrap();
        assert_eq!(history.days.len(), 3);
        assert!(history.knows("GBP"));
        assert!(!history.knows("BAD"), "a malformed rate is skipped");
        assert!(!history.knows("CHF"));
        assert!(RateTable::from_ecb_xml("<Envelope/>").is_err());
        assert!(RateTable::from_ecb_xml("").is_err());
    }

    #[test]
    fn a_day_without_a_publication_takes_the_working_day_before() {
        let table = fixture();
        assert_eq!(table.days.len(), 4);
        // The day itself.
        assert_eq!(
            table.rate_on("USD", "2026-10-13").map(|rate| rate.per_euro),
            Some("1.0900".to_owned())
        );
        // Saturday and Sunday take Friday's.
        let saturday = table.rate_on("USD", "2026-10-10").unwrap();
        assert_eq!(saturday.per_euro, "1.0700");
        assert_eq!(saturday.date, "2026-10-09");
        assert_eq!(
            table.rate_on("GBP", "2026-10-11").map(|rate| rate.date),
            Some("2026-10-09".to_owned())
        );
        // Tomorrow, before its publication, takes today's.
        assert_eq!(
            table.rate_on("USD", "2026-10-15").map(|rate| rate.date),
            Some("2026-10-14".to_owned())
        );
        // A week on, the last publication is too old to stand for the day.
        assert_eq!(
            table.rate_on("USD", "2026-10-21").map(|rate| rate.date),
            Some("2026-10-14".to_owned())
        );
        assert_eq!(table.rate_on("USD", "2026-10-22"), None);
        // Before the first day seen, a currency published only on some
        // days, one never published, a day that is not one.
        assert_eq!(table.rate_on("USD", "2026-10-08"), None);
        assert_eq!(table.rate_on("CHF", "2026-10-13"), None);
        assert_eq!(
            table.rate_on("CHF", "2026-10-14").map(|rate| rate.per_euro),
            Some("0.9312".to_owned())
        );
        assert_eq!(table.rate_on("AED", "2026-10-14"), None);
        assert_eq!(table.rate_on("USD", "2026-13-01"), None);
        assert!(!table.knows("AED"));
    }

    #[test]
    fn a_newer_reading_replaces_the_day() {
        let mut table = RateTable::from_ecb_xml(HISTORY_XML).unwrap();
        let mut newer = RateTable::default();
        newer.insert("2026-10-13", "USD", "1.0950");
        newer.insert("2026-10-13", "usd", "1.0951");
        newer.insert("2026-10-13", "US", "1.0");
        newer.insert("2026-10-13", "USD", "zero");
        newer.insert("2026-10-32", "USD", "1.1");
        table.merge(newer);
        assert_eq!(
            table.rate_on("USD", "2026-10-13").map(|rate| rate.per_euro),
            Some("1.0951".to_owned())
        );
        assert!(
            table
                .rate_on("GBP", "2026-10-13")
                .is_some_and(|rate| rate.date == "2026-10-12")
        );
        assert!(table.has_day("2026-10-13"));
        assert!(!table.has_day("2026-10-32"));
        assert!(RateTable::default().is_empty());
    }

    #[test]
    fn attributes_are_found_by_name_not_by_substring() {
        let tag = "<Cube  currency='USD'   rate = \"1.0850\"";
        assert_eq!(attribute(tag, "currency"), Some("USD"));
        assert_eq!(attribute(tag, "rate"), Some("1.0850"));
        assert_eq!(attribute(tag, "time"), None);
        assert_eq!(attribute("<Cube xrate='1'", "rate"), None);
        assert_eq!(attribute("<Cube rate=1", "rate"), None);
        assert_eq!(attribute("<Cube rate='1", "rate"), None);
    }
}
