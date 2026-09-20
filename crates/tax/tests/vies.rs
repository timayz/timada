//! The VIES validator against a local stand-in for the EU's service.
#![cfg(feature = "vies")]

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use timada_tax::{VatNumber, VatNumberValidator, ViesValidator};

/// What was asked, and what to answer next.
#[derive(Default)]
struct Fake {
    asked: Vec<Value>,
    answers: Vec<Value>,
}

type Shared = Arc<Mutex<Fake>>;

async fn check(State(fake): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let mut fake = fake.lock().unwrap_or_else(|e| e.into_inner());
    fake.asked.push(body);
    Json(fake.answers.remove(0))
}

async fn vies(requester: Option<&str>) -> anyhow::Result<(ViesValidator, Shared)> {
    let fake = Shared::default();
    let app = Router::new()
        .route("/check-vat-number", post(check))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let requester = requester.map(VatNumber::parse).transpose()?;
    let validator = ViesValidator::new(requester)?.with_api_base(format!("http://{address}"));
    Ok((validator, fake))
}

#[tokio::test]
async fn vies_answers_are_told_apart() -> anyhow::Result<()> {
    let (validator, fake) = vies(Some("FR40303265045")).await?;
    let number = VatNumber::parse("EL123456789")?;
    let answer = |value: Value| {
        fake.lock()
            .unwrap_or_else(|e| e.into_inner())
            .answers
            .push(value)
    };

    // Valid, with the proof a named requester gets.
    answer(json!({
        "countryCode": "EL", "vatNumber": "123456789", "valid": true,
        "requestIdentifier": "WAPIAAAAY1X2Z3", "name": " HELLAS TRADING AE ", "address": "ATHENS"
    }));
    let check = validator.check(&number).await?;
    assert!(check.valid);
    assert_eq!(check.consultation_ref.as_deref(), Some("WAPIAAAAY1X2Z3"));
    assert_eq!(check.registered_name.as_deref(), Some("HELLAS TRADING AE"));
    {
        let fake = fake.lock().unwrap_or_else(|e| e.into_inner());
        // Greece is asked about as EL; the shop names itself.
        assert_eq!(
            fake.asked[0],
            json!({
                "countryCode": "EL", "vatNumber": "123456789",
                "requesterMemberStateCode": "FR", "requesterNumber": "40303265045"
            })
        );
    }

    // Not registered; a member state that discloses no name.
    answer(json!({ "valid": false, "requestIdentifier": "", "name": "---" }));
    let check = validator.check(&number).await?;
    assert!(!check.valid);
    assert_eq!(
        (check.consultation_ref, check.registered_name),
        (None, None)
    );
    // A number VIES cannot even read is a verdict too.
    answer(json!({ "actionSucceed": false, "errorWrappers": [{ "error": "INVALID_INPUT" }] }));
    assert!(!validator.check(&number).await?.valid);

    // The member state is down: no verdict.
    answer(json!({ "actionSucceed": false, "errorWrappers": [{ "error": "MS_UNAVAILABLE" }] }));
    let down = validator.check(&number).await;
    assert_eq!(down.map_err(|e| e.0), Err("MS_UNAVAILABLE".to_owned()));
    answer(json!({ "unexpected": true }));
    assert!(validator.check(&number).await.is_err());

    // Nobody answers at all.
    let nowhere = ViesValidator::new(None)?.with_api_base("http://127.0.0.1:9");
    assert!(nowhere.check(&number).await.is_err());
    Ok(())
}

#[tokio::test]
async fn an_anonymous_requester_is_not_named() -> anyhow::Result<()> {
    let (validator, fake) = vies(None).await?;
    fake.lock()
        .unwrap_or_else(|e| e.into_inner())
        .answers
        .push(json!({ "valid": true }));
    let check = validator.check(&VatNumber::parse("DE123456789")?).await?;
    assert!(check.valid && check.consultation_ref.is_none());
    let asked = fake.lock().unwrap_or_else(|e| e.into_inner()).asked[0].clone();
    assert_eq!(
        asked,
        json!({ "countryCode": "DE", "vatNumber": "123456789" })
    );
    Ok(())
}
