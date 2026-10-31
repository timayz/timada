mod common;

use common::*;
use timada_automations::{
    AutomationError, catalog, normalize_email, opt_out_id, opt_out_id_of_token, opt_out_token,
};

const KEY: [u8; 32] = [7; 32];

#[test]
fn an_opt_out_link_checks_out_for_its_shop_only() {
    let token = opt_out_token(&KEY, TENANT, "ada@example.com");
    assert!(!token.contains("ada"));
    assert_eq!(
        opt_out_id_of_token(&KEY, TENANT, &token),
        Some(opt_out_id("ada@example.com"))
    );
    assert_eq!(opt_out_id_of_token(&KEY, "another-shop", &token), None);
    assert_eq!(opt_out_id_of_token(&[8; 32], TENANT, &token), None);
    let tampered = format!("{}x", token);
    assert_eq!(opt_out_id_of_token(&KEY, TENANT, &tampered), None);
    assert_eq!(opt_out_id_of_token(&KEY, TENANT, "nothing"), None);
}

#[test]
fn addresses_are_kept_trimmed_and_lowercased() {
    assert_eq!(
        normalize_email("  Ada@Example.COM "),
        Some("ada@example.com".into())
    );
    assert_eq!(normalize_email("ada"), None);
    // The one rule every context shares: a domain needs no dot.
    assert_eq!(
        normalize_email("ada@example").as_deref(),
        Some("ada@example")
    );
    assert_eq!(normalize_email("a da@example.com"), None);
}

#[tokio::test]
async fn an_address_leaves_and_rejoins_the_automated_mails() {
    let automations = command().await;
    assert!(!automations.is_opted_out("ada@example.com").await.unwrap());
    let token = opt_out_token(&KEY, TENANT, "ada@example.com");
    automations
        .opt_out_with_link(&KEY, "storefront", &token)
        .await
        .unwrap();
    // Twice is harmless.
    automations
        .opt_out_with_link(&KEY, "storefront", &token)
        .await
        .unwrap();
    assert!(automations.is_opted_out(" ADA@example.com").await.unwrap());
    assert!(matches!(
        automations
            .opt_out_with_link(&KEY, "storefront", "forged.token")
            .await,
        Err(AutomationError::InvalidLink)
    ));

    assert!(
        automations
            .withdraw_opt_out(ALICE, "ada@example.com")
            .await
            .unwrap()
    );
    assert!(!automations.is_opted_out("ada@example.com").await.unwrap());
    assert!(
        !automations
            .withdraw_opt_out(ALICE, "ada@example.com")
            .await
            .unwrap()
    );
    automations
        .opt_out_address(ALICE, "ada@example.com")
        .await
        .unwrap();
    assert!(automations.is_opted_out("ada@example.com").await.unwrap());
    assert!(matches!(
        automations.opt_out_address(ALICE, "not an address").await,
        Err(AutomationError::InvalidEmail)
    ));
}

#[tokio::test]
async fn a_scenario_mails_an_address_once_a_day() {
    let automations = command().await;
    assert!(
        automations
            .claim_mail(ALICE, "sc1", "ada@example.com", 20_000)
            .await
            .unwrap()
    );
    assert!(
        !automations
            .claim_mail(ALICE, "sc1", "Ada@Example.com", 20_000)
            .await
            .unwrap()
    );
    // The next day, or another scenario: another mail.
    assert!(
        automations
            .claim_mail(ALICE, "sc1", "ada@example.com", 20_001)
            .await
            .unwrap()
    );
    assert!(
        automations
            .claim_mail(ALICE, "sc2", "ada@example.com", 20_000)
            .await
            .unwrap()
    );
}

#[test]
fn a_mail_step_needs_a_subject_and_a_body() {
    let settings = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    };
    let read =
        catalog::parse_email(&settings(&[("subject", "Thanks"), ("html", "<p>Hi</p>")])).unwrap();
    assert_eq!(read.text, None);
    let issues = catalog::parse_email(&settings(&[("text", " ")])).unwrap_err();
    assert_eq!(issues.len(), 2);
}
