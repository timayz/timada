//! Reads the European Central Bank's daily euro reference rates, for a sale
//! priced in another currency than the euro: what it counts for against the
//! EU distance-sales threshold and the VAT in euros its invoice states (see
//! `timada_invoicing::rates`). The Bank publishes one file with the day's
//! rates and one with the last ninety days; this client reads the history
//! once, then the day's file when a day it has not seen is asked for, at most
//! once an hour, and answers from what it has.

use std::time::{Duration, Instant};

use openidconnect::reqwest;
use timada_invoicing::rates::{ExchangeRate, RateTable};
use tokio::sync::Mutex;

/// Where the Bank publishes its reference rates.
pub const ECB_RATES_URL: &str = "https://www.ecb.europa.eu/stats/eurofxref";
/// The day's rates, under [`ECB_RATES_URL`].
const DAILY_FILE: &str = "eurofxref-daily.xml";
/// The last ninety days' rates, under [`ECB_RATES_URL`].
const HISTORY_FILE: &str = "eurofxref-hist-90d.xml";
/// How long a reading that lacks the day asked for stands before the file
/// is read again (the Bank publishes once a day, around 16:00 CET).
const REREAD_AFTER: Duration = Duration::from_secs(3600);

/// What the rates say of a currency on a day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateAnswer {
    /// The rate of the day, or of the last day published before it.
    Rate(ExchangeRate),
    /// The Bank publishes no euro rate for this currency.
    NoSuchCurrency,
    /// The rates could not be read (or are too old to stand for the day).
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait EuroRates: Send + Sync {
    /// The euro rate of `currency` (ISO 4217) for `day` (`YYYY-MM-DD`).
    async fn rate_on(&self, currency: &str, day: &str) -> RateAnswer;
}

/// A fixed table answers as it stands: the tests' source.
#[async_trait::async_trait]
impl EuroRates for RateTable {
    async fn rate_on(&self, currency: &str, day: &str) -> RateAnswer {
        answer_from(self, currency, day)
    }
}

/// What a table says of a currency on a day.
fn answer_from(table: &RateTable, currency: &str, day: &str) -> RateAnswer {
    match table.rate_on(currency, day) {
        Some(rate) => RateAnswer::Rate(rate),
        None if table.is_empty() => RateAnswer::Unavailable("no rates read yet".to_owned()),
        None if !table.knows(currency) => RateAnswer::NoSuchCurrency,
        None => RateAnswer::Unavailable(format!(
            "no rate of {currency} for {day}; the last day read is {}",
            table.latest_day().unwrap_or("none")
        )),
    }
}

struct Cache {
    table: RateTable,
    /// When the Bank's files were last read, whatever came of it.
    read_at: Option<Instant>,
}

pub struct EcbClient {
    url: String,
    client: reqwest::Client,
    cache: Mutex<Cache>,
}

impl EcbClient {
    /// `url`: where the Bank's files are; the Bank's own address when
    /// `None`.
    pub fn new(url: Option<String>) -> Self {
        Self {
            url: url
                .unwrap_or_else(|| ECB_RATES_URL.to_owned())
                .trim_end_matches('/')
                .to_owned(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            cache: Mutex::new(Cache {
                table: RateTable::default(),
                read_at: None,
            }),
        }
    }

    /// Reads one of the Bank's files into a table.
    async fn read(&self, file: &str) -> anyhow::Result<RateTable> {
        let response = self
            .client
            .get(format!("{}/{file}", self.url))
            .send()
            .await?;
        if !response.status().is_success() {
            anyhow::bail!("HTTP {}", response.status());
        }
        let body = response.text().await?;
        RateTable::from_ecb_xml(&body)
    }
}

#[async_trait::async_trait]
impl EuroRates for EcbClient {
    async fn rate_on(&self, currency: &str, day: &str) -> RateAnswer {
        let mut cache = self.cache.lock().await;
        let stale = cache
            .read_at
            .is_none_or(|read_at| read_at.elapsed() >= REREAD_AFTER);
        if !cache.table.has_day(day) && stale {
            // The history the first time, then the day's file.
            let file = if cache.table.is_empty() {
                HISTORY_FILE
            } else {
                DAILY_FILE
            };
            cache.read_at = Some(Instant::now());
            match self.read(file).await {
                Ok(table) => {
                    tracing::debug!(file, latest = table.latest_day(), "euro rates read");
                    cache.table.merge(table);
                }
                Err(err) => {
                    tracing::warn!(file, %err, "the euro reference rates could not be read");
                    if cache.table.is_empty() {
                        return RateAnswer::Unavailable(err.to_string());
                    }
                }
            }
        }
        answer_from(&cache.table, currency, day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<Envelope><Cube>
        <Cube time='2026-10-14'><Cube currency='USD' rate='1.0850'/></Cube>
        <Cube time='2026-10-13'><Cube currency='USD' rate='1.0900'/><Cube currency='GBP' rate='0.86'/></Cube>
    </Cube></Envelope>"#;

    /// A fixed table is a rate source: the day's rate, the day before's for
    /// a day without one, and what it cannot say.
    #[tokio::test]
    async fn a_table_answers_as_a_rate_source() {
        let table = RateTable::from_ecb_xml(XML).unwrap();
        let source: &dyn EuroRates = &table;
        assert_eq!(
            source.rate_on("USD", "2026-10-14").await,
            RateAnswer::Rate(ExchangeRate::ecb("USD", "1.0850", "2026-10-14"))
        );
        assert_eq!(
            source.rate_on("GBP", "2026-10-15").await,
            RateAnswer::Rate(ExchangeRate::ecb("GBP", "0.86", "2026-10-13"))
        );
        assert_eq!(
            source.rate_on("AED", "2026-10-14").await,
            RateAnswer::NoSuchCurrency
        );
        assert!(matches!(
            source.rate_on("USD", "2026-11-14").await,
            RateAnswer::Unavailable(_)
        ));
        assert!(matches!(
            EuroRates::rate_on(&RateTable::default(), "USD", "2026-10-14").await,
            RateAnswer::Unavailable(_)
        ));
    }

    /// Nothing listens there: the client says so and never panics.
    #[tokio::test]
    async fn an_unreachable_bank_is_unavailable() {
        let client = EcbClient::new(Some("http://127.0.0.1:9/rates".to_owned()));
        assert!(matches!(
            client.rate_on("USD", "2026-10-14").await,
            RateAnswer::Unavailable(_)
        ));
    }
}
