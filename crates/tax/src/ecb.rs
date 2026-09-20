//! The European Central Bank's euro reference rates as an [`ExchangeRates`]
//! (feature `ecb`): one small XML file, published around 16:00 CET on working
//! days. It is the rate French law points at for a VAT amount stated in
//! euros on a foreign-currency invoice.
//!
//! The table is fetched at most once an hour and kept: when the bank cannot
//! be reached, the last table known still answers — "the latest rate
//! published" is what the law asks for, and a weekend's orders use Friday's.
//! Only a cold start without the bank fails.

use std::{
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::exchange::{ExchangeRates, PinnedRate, RateError, RateFuture};

const DAILY: &str = "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml";
const REFRESH_AFTER_SECS: u64 = 3_600;

#[derive(Debug, Clone)]
struct Table {
    fetched_at: u64,
    /// Unix seconds of the day the rates are of.
    as_of: u64,
    /// `(currency, millionths of it per euro)`.
    rates: Vec<(String, u64)>,
}

#[derive(Debug)]
pub struct EcbRates {
    http: reqwest::Client,
    url: String,
    refresh_after: Duration,
    table: Mutex<Option<Table>>,
}

impl EcbRates {
    pub fn new() -> Result<Self, RateError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|err| RateError::Unavailable(err.to_string()))?;
        Ok(Self {
            http,
            url: DAILY.to_owned(),
            refresh_after: Duration::from_secs(REFRESH_AFTER_SECS),
            table: Mutex::new(None),
        })
    }

    /// Points the source at another address — a stand-in, in tests.
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// How long a fetched table answers before the bank is asked again (an
    /// hour by default).
    pub fn with_refresh_after(mut self, refresh_after: Duration) -> Self {
        self.refresh_after = refresh_after;
        self
    }

    fn known(&self) -> Option<Table> {
        self.table.lock().ok().and_then(|table| table.clone())
    }

    async fn fetch(&self) -> Result<Table, RateError> {
        let unavailable = |err: reqwest::Error| RateError::Unavailable(err.to_string());
        let body = self
            .http
            .get(&self.url)
            .send()
            .await
            .map_err(unavailable)?
            .error_for_status()
            .map_err(unavailable)?
            .text()
            .await
            .map_err(unavailable)?;
        let table = parse(&body, now())
            .ok_or_else(|| RateError::Unavailable("unreadable ECB rates file".to_owned()))?;
        if let Ok(mut known) = self.table.lock() {
            *known = Some(table.clone());
        }
        Ok(table)
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The value of `name='…'` (or `name="…"`) in a tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.find(&format!("{name}="))? + name.len() + 1;
    let quote = tag[at..]
        .chars()
        .next()
        .filter(|q| matches!(q, '\'' | '"'))?;
    let value = &tag[at + 1..];
    Some(&value[..value.find(quote)?])
}

/// `1.0852` → `1_085_200`, without going through a float.
fn micros(rate: &str) -> Option<u64> {
    let (units, fraction) = rate.split_once('.').unwrap_or((rate, ""));
    if units.is_empty()
        || !units
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let mut fraction = fraction.to_owned();
    fraction.truncate(6);
    while fraction.len() < 6 {
        fraction.push('0');
    }
    units
        .parse::<u64>()
        .ok()?
        .checked_mul(1_000_000)?
        .checked_add(fraction.parse().ok()?)
}

/// Unix seconds of midnight UTC on `YYYY-MM-DD` (days-from-civil).
fn day(date: &str) -> Option<u64> {
    let mut parts = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    u64::try_from((era * 146_097 + day_of_era - 719_468) * 86_400).ok()
}

fn parse(xml: &str, fetched_at: u64) -> Option<Table> {
    let mut as_of = None;
    let mut rates = Vec::new();
    for tag in xml.split('<').filter(|tag| tag.starts_with("Cube")) {
        if let Some(time) = attribute(tag, "time") {
            as_of = day(time);
        }
        if let (Some(currency), Some(rate)) = (attribute(tag, "currency"), attribute(tag, "rate"))
            && let Some(per_euro) = micros(rate).filter(|per_euro| *per_euro > 0)
        {
            rates.push((currency.to_owned(), per_euro));
        }
    }
    (!rates.is_empty()).then_some(Table {
        fetched_at,
        as_of: as_of?,
        rates,
    })
}

impl ExchangeRates for EcbRates {
    fn rate<'a>(&'a self, base: &'a str, currency: &'a str, _at: u64) -> RateFuture<'a> {
        Box::pin(async move {
            // The bank quotes against the euro, and nothing else.
            if base != "EUR" {
                return Err(RateError::Unknown(format!("{base}/{currency}")));
            }
            let known = self.known();
            let fresh = known.as_ref().is_some_and(|table| {
                now().saturating_sub(table.fetched_at) < self.refresh_after.as_secs()
            });
            let table = match (fresh, known) {
                (true, Some(table)) => table,
                (_, known) => match (self.fetch().await, known) {
                    (Ok(table), _) => table,
                    // The latest rate published is still the last one seen.
                    (Err(_), Some(table)) => table,
                    (Err(err), None) => return Err(err),
                },
            };
            let per_base_micros = table
                .rates
                .iter()
                .find(|(known, _)| known == currency)
                .map(|(_, per_euro)| *per_euro)
                .ok_or_else(|| RateError::Unknown(format!("EUR/{currency}")))?;
            Ok(PinnedRate {
                base: "EUR".to_owned(),
                currency: currency.to_owned(),
                per_base_micros,
                as_of: table.as_of,
                source: "ECB".to_owned(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gesmes:Envelope xmlns:gesmes="http://www.gesmes.org/xml/2002-08-01">
  <gesmes:subject>Reference rates</gesmes:subject>
  <Cube>
    <Cube time='2026-09-18'>
      <Cube currency='USD' rate='1.0852'/>
      <Cube currency='GBP' rate='0.85380'/>
      <Cube currency='CHF' rate='0.9412'/>
      <Cube currency='JPY' rate='161.37'/>
      <Cube currency='XXX' rate='n/a'/>
    </Cube>
  </Cube>
</gesmes:Envelope>"#;

    #[test]
    fn the_banks_file_is_read_without_a_float() {
        let table = parse(FILE, 42);
        let table = table.as_ref();
        assert_eq!(table.map(|t| t.as_of), Some(1_789_689_600));
        let rate = |currency: &str| {
            table.and_then(|t| {
                t.rates
                    .iter()
                    .find(|(known, _)| known == currency)
                    .map(|(_, per_euro)| *per_euro)
            })
        };
        assert_eq!(rate("GBP"), Some(853_800));
        assert_eq!(rate("USD"), Some(1_085_200));
        assert_eq!(rate("JPY"), Some(161_370_000));
        assert_eq!(rate("XXX"), None);
        assert!(parse("<html>maintenance</html>", 0).is_none());
        assert_eq!(day("1970-01-01"), Some(0));
        assert_eq!(day("2026-13-01"), None);
        assert_eq!(micros("1"), Some(1_000_000));
        assert_eq!(micros("0.1234567"), Some(123_456));
        assert_eq!(micros("-1.0"), None);
    }
}
