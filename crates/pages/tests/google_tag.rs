mod common;

use common::*;
use timada_pages::{GoogleTagSettings, PageError};

fn some(text: &str) -> Option<String> {
    Some(text.to_owned())
}

/// How many events the tag's stream holds.
async fn version(pages: &Pages) -> u16 {
    pages
        .load_google_tag()
        .await
        .unwrap()
        .unwrap()
        .aggregate_version
}

#[tokio::test]
async fn the_tag_is_set_up_whole_and_taken_off_whole() {
    let pages = command().await;
    assert!(!pages.google_tag().await.unwrap().is_set());

    pages
        .set_google_tag(
            ALICE,
            GoogleTagSettings {
                analytics_id: some(" g-abc123def4 "),
                ads_id: some("aw-123456789"),
                // The whole `send_to` pasted from Google Ads.
                purchase_label: some("AW-123456789/AbC-D_efG"),
                add_to_cart_label: some("  "),
                begin_checkout_label: None,
                enhanced_conversions: true,
                profit_as_value: false,
            },
        )
        .await
        .unwrap();
    let tag = pages.google_tag().await.unwrap();
    assert!(tag.is_set());
    assert_eq!(tag.analytics_id.as_deref(), Some("G-ABC123DEF4"));
    assert_eq!(tag.ads_id.as_deref(), Some("AW-123456789"));
    assert_eq!(tag.purchase_label.as_deref(), Some("AbC-D_efG"));
    assert_eq!(tag.add_to_cart_label, None);
    assert!(tag.enhanced_conversions);

    // The same again writes nothing.
    let version = pages
        .load_google_tag()
        .await
        .unwrap()
        .unwrap()
        .aggregate_version;
    pages.set_google_tag(ALICE, tag).await.unwrap();
    assert_eq!(
        pages
            .load_google_tag()
            .await
            .unwrap()
            .unwrap()
            .aggregate_version,
        version
    );

    pages
        .set_google_tag(ALICE, GoogleTagSettings::default())
        .await
        .unwrap();
    assert_eq!(
        pages.google_tag().await.unwrap(),
        GoogleTagSettings::default()
    );
}

#[tokio::test]
async fn the_ids_and_labels_are_checked() {
    let pages = command().await;
    let with = |change: fn(&mut GoogleTagSettings)| {
        let mut settings = GoogleTagSettings {
            ads_id: some("AW-123456789"),
            ..Default::default()
        };
        change(&mut settings);
        settings
    };
    for (settings, expected) in [
        (
            with(|s| s.analytics_id = some("UA-12345-1")),
            "google_tag_invalid_id",
        ),
        (
            with(|s| s.analytics_id = some("G-ABC');alert(1)//")),
            "google_tag_invalid_id",
        ),
        (
            with(|s| s.analytics_id = some("AW-123456789")),
            "google_tag_invalid_id",
        ),
        (
            with(|s| s.ads_id = some("G-ABC123DEF4")),
            "google_tag_invalid_id",
        ),
        (
            with(|s| s.ads_id = some("AW-12AB")),
            "google_tag_invalid_id",
        ),
        (
            with(|s| s.purchase_label = some("a b")),
            "google_tag_invalid_label",
        ),
        (
            with(|s| s.add_to_cart_label = some("x'y")),
            "google_tag_invalid_label",
        ),
        (
            with(|s| s.begin_checkout_label = some("AW-1/")),
            "google_tag_invalid_label",
        ),
        (
            with(|s| {
                s.ads_id = None;
                s.purchase_label = some("AbC");
            }),
            "google_ads_id_missing",
        ),
        (
            with(|s| {
                s.ads_id = None;
                s.analytics_id = some("G-ABC123DEF4");
                s.enhanced_conversions = true;
            }),
            "google_ads_id_missing",
        ),
        (
            with(|s| {
                s.ads_id = None;
                s.analytics_id = some("G-ABC123DEF4");
                s.profit_as_value = true;
            }),
            "google_ads_id_missing",
        ),
    ] {
        let refused: PageError = pages
            .set_google_tag(ALICE, settings.clone())
            .await
            .expect_err(&format!("{settings:?}"));
        assert_eq!(refused.code(), Some(expected), "{settings:?}");
    }
    assert!(pages.load_google_tag().await.unwrap().is_none());
}

#[tokio::test]
async fn what_a_sale_is_worth_is_chosen_apart_from_the_tag() {
    let pages = command().await;
    let tag = GoogleTagSettings {
        ads_id: some("AW-123456789"),
        purchase_label: some("SaLe"),
        ..Default::default()
    };
    pages.set_google_tag(ALICE, tag.clone()).await.unwrap();
    assert!(!pages.google_tag().await.unwrap().profit_as_value);
    let before = version(&pages).await;

    // The profit chosen: one event, the tag as it was.
    let valued = GoogleTagSettings {
        profit_as_value: true,
        ..tag.clone()
    };
    pages.set_google_tag(ALICE, valued.clone()).await.unwrap();
    assert_eq!(version(&pages).await, before + 1);
    let set = pages.google_tag().await.unwrap();
    assert!(set.profit_as_value);
    assert_eq!(set.purchase_label.as_deref(), Some("SaLe"));

    // The tag changed: the choice stands.
    pages
        .set_google_tag(
            ALICE,
            GoogleTagSettings {
                purchase_label: some("OtHeR"),
                ..valued
            },
        )
        .await
        .unwrap();
    assert_eq!(version(&pages).await, before + 2);
    let set = pages.google_tag().await.unwrap();
    assert!(set.profit_as_value);
    assert_eq!(set.purchase_label.as_deref(), Some("OtHeR"));

    // Back to the total.
    pages
        .set_google_tag(
            ALICE,
            GoogleTagSettings {
                purchase_label: some("OtHeR"),
                ..tag
            },
        )
        .await
        .unwrap();
    assert!(!pages.google_tag().await.unwrap().profit_as_value);
}
