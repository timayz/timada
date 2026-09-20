//! The ECB source against a local stand-in for the bank's file.
#![cfg(feature = "ecb")]

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use axum::{Router, extract::State, http::StatusCode, routing::get};
use timada_core::Money;
use timada_tax::{EcbRates, ExchangeRates, RateError};

const FILE: &str = r#"<gesmes:Envelope><Cube><Cube time='2026-09-18'>
<Cube currency='GBP' rate='0.85380'/><Cube currency='CHF' rate='0.9412'/>
</Cube></Cube></gesmes:Envelope>"#;

#[derive(Default)]
struct Bank {
    down: AtomicBool,
    asked: AtomicU32,
}

async fn daily(State(bank): State<Arc<Bank>>) -> Result<&'static str, StatusCode> {
    bank.asked.fetch_add(1, Ordering::SeqCst);
    if bank.down.load(Ordering::SeqCst) {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    } else {
        Ok(FILE)
    }
}

async fn bank() -> anyhow::Result<(String, Arc<Bank>)> {
    let bank = Arc::new(Bank::default());
    let app = Router::new()
        .route("/eurofxref-daily.xml", get(daily))
        .with_state(bank.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://{address}/eurofxref-daily.xml"), bank))
}

#[tokio::test]
async fn the_latest_published_rate_answers_even_when_the_bank_does_not() -> anyhow::Result<()> {
    let (url, bank) = bank().await?;
    let rates = EcbRates::new()?.with_url(&url);

    let pounds = rates.rate("EUR", "GBP", 0).await?;
    assert_eq!(pounds.per_base_micros, 853_800);
    assert_eq!(pounds.source, "ECB");
    // The day the bank says, not the day it was asked.
    assert_eq!(pounds.as_of, 1_789_689_600);
    assert_eq!(
        pounds.to_base(&Money::new(10_900, "GBP"))?,
        Money::eur(12_766)
    );
    // One file answers every currency for an hour.
    rates.rate("EUR", "CHF", 0).await?;
    assert_eq!(bank.asked.load(Ordering::SeqCst), 1);
    assert!(matches!(
        rates.rate("EUR", "XXX", 0).await,
        Err(RateError::Unknown(_))
    ));
    // The bank quotes against the euro only.
    assert!(matches!(
        rates.rate("GBP", "CHF", 0).await,
        Err(RateError::Unknown(_))
    ));

    // Down, and the table is stale: the last one seen is still the latest
    // rate published.
    let stale = rates.with_refresh_after(Duration::ZERO);
    bank.down.store(true, Ordering::SeqCst);
    assert_eq!(stale.rate("EUR", "GBP", 0).await?.per_base_micros, 853_800);
    assert!(bank.asked.load(Ordering::SeqCst) >= 2);

    // Nothing ever seen, and nobody answers: no rate is made up.
    let cold = EcbRates::new()?.with_url(&url);
    assert!(matches!(
        cold.rate("EUR", "GBP", 0).await,
        Err(RateError::Unavailable(_))
    ));
    Ok(())
}
