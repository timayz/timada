mod common;

use common::*;
use timada_google::aggregator::CampaignAdGroup;
use timada_google::{
    CAMPAIGN_SHOPPING, CreatedCampaign, GOOGLE_ACTOR, GoogleError, PreparedRound, TUNING_ACTOR,
    TUNING_EVERY_DAYS, TUNING_MIN_CLICKS, TUNING_STEP, TuningSettings,
};

async fn campaign(google: &Google) {
    let settings = [
        ("name", "Shopping · Acme"),
        ("daily_budget", "10"),
        ("bid", "0.30"),
        ("countries", "FR"),
        ("phone_adjustment", "-55"),
        ("start", "live"),
        ("ad_groups", "01CAT Arbres"),
    ];
    google
        .record_campaign(
            ALICE,
            SHOP,
            CreatedCampaign {
                kind: CAMPAIGN_SHOPPING.to_owned(),
                customer_id: "1234567890".to_owned(),
                campaign_id: "11".to_owned(),
                settings: settings
                    .into_iter()
                    .map(|(field, value)| (field.to_owned(), value.to_owned()))
                    .collect(),
                ad_groups: vec![CampaignAdGroup {
                    ad_group_id: "21".into(),
                    category_id: "01CAT".into(),
                    name: "Arbres".into(),
                }],
                proposal_id: "proposal-1".to_owned(),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn the_thresholds_are_the_defaults_until_a_shop_sets_its_own() {
    let google = command().await;
    assert!(google.load_tuning(SHOP).await.unwrap().is_none());
    let defaults = TuningSettings::default();
    assert_eq!(
        (defaults.days, defaults.min_clicks, defaults.drain_clicks),
        (21, 100, 150)
    );
    assert_eq!((defaults.step_cents, defaults.target_return), (2, 300));
    assert_eq!((defaults.rest_days, defaults.every_days), (14, 0));

    let set = |field: &str, value: &str| vec![(field.to_owned(), value.to_owned())];
    google
        .set_tuning(ALICE, SHOP, set(TUNING_MIN_CLICKS, " 60 "))
        .await
        .unwrap();
    let tuning = google.load_tuning(SHOP).await.unwrap().unwrap();
    assert_eq!(tuning.tenant_id, SHOP);
    assert_eq!(tuning.thresholds().min_clicks, 60);
    // The ones left out keep their default.
    assert_eq!(tuning.thresholds().drain_clicks, 150);
    // The same again writes nothing.
    google
        .set_tuning(ALICE, SHOP, set(TUNING_MIN_CLICKS, "60"))
        .await
        .unwrap();
    let again = google.load_tuning(SHOP).await.unwrap().unwrap();
    assert_eq!(again.aggregate_version, tuning.aggregate_version);

    for (field, value) in [
        (TUNING_MIN_CLICKS, "0"),
        (TUNING_MIN_CLICKS, "many"),
        (TUNING_STEP, "0"),
        (TUNING_STEP, "1.50"),
        // By itself, a round comes a week apart at least.
        (TUNING_EVERY_DAYS, "3"),
    ] {
        assert!(
            matches!(
                google.set_tuning(ALICE, SHOP, set(field, value)).await,
                Err(GoogleError::InvalidTuning)
            ),
            "{field} {value}"
        );
    }
}

#[tokio::test]
async fn a_round_is_asked_for_once_and_says_what_it_prepared() {
    let google = command().await;
    assert!(google.request_round(ALICE, SHOP).await.unwrap());
    assert!(!google.request_round(TUNING_ACTOR, SHOP).await.unwrap());
    let asked = google.load_tuning(SHOP).await.unwrap().unwrap();
    assert!(asked.round_requested);
    assert_eq!(asked.round_requested_by, ALICE);
    assert!(!asked.round_due(u64::MAX));

    let round = PreparedRound {
        first: "2026-11-01".into(),
        last: "2026-11-21".into(),
        proposals: vec!["p1".into(), "p2".into()],
        notes: vec![
            " Jouets: 12 clicks of the 100 a decision takes. ".into(),
            " ".into(),
        ],
        flags: vec![],
    };
    assert!(
        google
            .record_round(GOOGLE_ACTOR, SHOP, round.clone())
            .await
            .unwrap()
    );
    // Prepared by another node too: recorded once.
    assert!(
        !google
            .record_round(GOOGLE_ACTOR, SHOP, round)
            .await
            .unwrap()
    );
    assert!(
        !google
            .record_round_failure(GOOGLE_ACTOR, SHOP, "late")
            .await
            .unwrap()
    );
    let tuning = google.load_tuning(SHOP).await.unwrap().unwrap();
    assert!(!tuning.round_requested);
    let last = tuning.last_round.clone().unwrap();
    assert_eq!(last.proposals, vec!["p1", "p2"]);
    assert_eq!(
        last.notes,
        vec!["Jouets: 12 clicks of the 100 a decision takes."]
    );
    assert_eq!(last.error, None);
    // Only when asked, until the shop says every how many days.
    assert!(!tuning.round_due(last.at + 365 * 86_400));
    google
        .set_tuning(
            ALICE,
            SHOP,
            vec![(TUNING_EVERY_DAYS.to_owned(), "14".to_owned())],
        )
        .await
        .unwrap();
    let tuning = google.load_tuning(SHOP).await.unwrap().unwrap();
    assert!(!tuning.round_due(last.at + 14 * 86_400 - 1));
    assert!(tuning.round_due(last.at + 14 * 86_400));

    google.request_round(ALICE, SHOP).await.unwrap();
    google
        .record_round_failure(GOOGLE_ACTOR, SHOP, "Google Ads was not read yet.")
        .await
        .unwrap();
    let failed = google.load_tuning(SHOP).await.unwrap().unwrap();
    assert!(!failed.round_requested);
    assert_eq!(
        failed.last_round.unwrap().error.as_deref(),
        Some("Google Ads was not read yet.")
    );
}

#[tokio::test]
async fn what_was_changed_at_google_ads_is_what_the_campaign_holds() {
    let google = command().await;
    assert!(matches!(
        google
            .record_bid(ALICE, SHOP, CAMPAIGN_SHOPPING, "21", "0.32", "p1")
            .await,
        Err(GoogleError::NoCampaign)
    ));
    campaign(&google).await;
    let load = async || {
        google
            .load_campaign(SHOP, CAMPAIGN_SHOPPING)
            .await
            .unwrap()
            .unwrap()
    };
    // As created: the starting bid everywhere, phones as approved.
    let created = load().await;
    assert_eq!(created.bid_of("21"), "0.30");
    assert_eq!(created.adjustment_of("mobile"), "-55");
    assert_eq!(created.adjustment_of("tablet"), "");
    assert_eq!(created.changed_at("21"), None);

    assert!(
        google
            .record_bid(ALICE, SHOP, CAMPAIGN_SHOPPING, "21", "0,34", "p1")
            .await
            .unwrap()
    );
    assert!(
        !google
            .record_bid(ALICE, SHOP, CAMPAIGN_SHOPPING, "21", "0.34", "p1")
            .await
            .unwrap()
    );
    assert!(matches!(
        google
            .record_bid(ALICE, SHOP, CAMPAIGN_SHOPPING, "99", "0.34", "p1")
            .await,
        Err(GoogleError::InvalidTuned)
    ));
    assert!(
        google
            .record_adjustment(ALICE, SHOP, CAMPAIGN_SHOPPING, "mobile", "-65", "p2")
            .await
            .unwrap()
    );
    // Back to none is no adjustment at all.
    google
        .record_adjustment(ALICE, SHOP, CAMPAIGN_SHOPPING, "desktop", "10", "p3")
        .await
        .unwrap();
    google
        .record_adjustment(ALICE, SHOP, CAMPAIGN_SHOPPING, "desktop", "0", "p4")
        .await
        .unwrap();
    assert!(matches!(
        google
            .record_adjustment(ALICE, SHOP, CAMPAIGN_SHOPPING, "tv", "10", "p3")
            .await,
        Err(GoogleError::InvalidTuned)
    ));
    assert!(
        google
            .record_excluded_term(ALICE, SHOP, CAMPAIGN_SHOPPING, " niche chien ", "p5")
            .await
            .unwrap()
    );
    assert!(
        !google
            .record_excluded_term(ALICE, SHOP, CAMPAIGN_SHOPPING, "niche chien", "p5")
            .await
            .unwrap()
    );
    let tuned = load().await;
    assert_eq!(tuned.bid_of("21"), "0.34");
    assert!(tuned.changed_at("21").is_some());
    assert_eq!(tuned.adjustment_of("mobile"), "-65");
    assert_eq!(tuned.adjustment_of("desktop"), "");
    assert!(tuned.changed_at("mobile").is_some());
    assert!(tuned.excludes("niche chien"));
    assert!(!tuned.excludes("niche"));
    // What was approved at creation stays what it was.
    assert_eq!(tuned.setting("bid"), "0.30");
}
