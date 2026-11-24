mod common;

use common::*;
use timada_google::{
    ACCESS_ADS, ACCESS_MERCHANT_CENTER, ACCESS_SEARCH_CONSOLE, Connect, GOOGLE_ACTOR, GoogleError,
};

const SITE: &str = "https://shop.example/";

#[tokio::test]
async fn a_shop_connects_chooses_its_site_and_disconnects() {
    let google = command().await;
    assert!(google.load_connection(SHOP).await.unwrap().is_none());

    google
        .connect(ALICE, SHOP, grant("alice@example.com", b"sealed-1"))
        .await
        .unwrap();
    let connection = google.load_connection(SHOP).await.unwrap().unwrap();
    assert!(connection.reads(ACCESS_SEARCH_CONSOLE));
    assert!(!connection.covers(ACCESS_ADS));
    assert_eq!(connection.account, "alice@example.com");
    assert_eq!(connection.refresh_token_sealed, b"sealed-1");
    assert_eq!(connection.site, None);

    google.choose_site(ALICE, SHOP, SITE).await.unwrap();
    // The same again writes nothing.
    google.choose_site(ALICE, SHOP, SITE).await.unwrap();
    let connection = google.load_connection(SHOP).await.unwrap().unwrap();
    assert_eq!(connection.site.as_deref(), Some(SITE));
    assert_eq!(connection.aggregate_version, 2);

    google.disconnect(ALICE, SHOP).await.unwrap();
    google.disconnect(ALICE, SHOP).await.unwrap();
    let connection = google.load_connection(SHOP).await.unwrap().unwrap();
    assert!(!connection.connected);
    assert!(connection.refresh_token_sealed.is_empty());
    assert_eq!(connection.site, None);
    assert!(!connection.reads(ACCESS_SEARCH_CONSOLE));
    assert_eq!(connection.aggregate_version, 3);
    assert!(matches!(
        google.choose_site(ALICE, SHOP, SITE).await,
        Err(GoogleError::NotConnected)
    ));
}

#[tokio::test]
async fn a_wider_grant_keeps_the_site_and_another_account_drops_it() {
    let google = command().await;
    google
        .connect(ALICE, SHOP, grant("alice@example.com", b"sealed-1"))
        .await
        .unwrap();
    google.choose_site(ALICE, SHOP, SITE).await.unwrap();

    // One more kind of access, asked for later: listed in a fixed order.
    google
        .connect(
            ALICE,
            SHOP,
            Connect {
                refresh_token_sealed: b"sealed-2".to_vec(),
                account: "alice@example.com".to_owned(),
                accesses: vec![
                    ACCESS_MERCHANT_CENTER.to_owned(),
                    ACCESS_SEARCH_CONSOLE.to_owned(),
                ],
            },
        )
        .await
        .unwrap();
    let connection = google.load_connection(SHOP).await.unwrap().unwrap();
    assert_eq!(
        connection.accesses,
        vec![ACCESS_SEARCH_CONSOLE, ACCESS_MERCHANT_CENTER]
    );
    assert_eq!(connection.refresh_token_sealed, b"sealed-2");
    assert_eq!(connection.site.as_deref(), Some(SITE));

    google
        .connect(ALICE, SHOP, grant("bob@example.com", b"sealed-3"))
        .await
        .unwrap();
    let connection = google.load_connection(SHOP).await.unwrap().unwrap();
    assert_eq!(connection.account, "bob@example.com");
    assert_eq!(connection.site, None);
}

#[tokio::test]
async fn a_grant_without_search_console_has_no_site() {
    let google = command().await;
    google
        .connect(ALICE, SHOP, grant("alice@example.com", b"sealed-1"))
        .await
        .unwrap();
    google.choose_site(ALICE, SHOP, SITE).await.unwrap();
    let ads_only = Connect {
        refresh_token_sealed: b"sealed-2".to_vec(),
        account: "alice@example.com".to_owned(),
        accesses: vec![ACCESS_ADS.to_owned()],
    };
    google.connect(ALICE, SHOP, ads_only).await.unwrap();
    let connection = google.load_connection(SHOP).await.unwrap().unwrap();
    assert_eq!(connection.site, None);
    assert!(matches!(
        google.choose_site(ALICE, SHOP, SITE).await,
        Err(GoogleError::NoSearchConsole)
    ));
}

#[tokio::test]
async fn a_grant_is_refused_without_a_known_access() {
    let google = command().await;
    let mut nothing = grant("alice@example.com", b"sealed-1");
    nothing.accesses.clear();
    assert!(matches!(
        google.connect(ALICE, SHOP, nothing).await,
        Err(GoogleError::NothingGranted)
    ));
    let mut unknown = grant("alice@example.com", b"sealed-1");
    unknown.accesses.push("drive".to_owned());
    assert!(matches!(
        google.connect(ALICE, SHOP, unknown).await,
        Err(GoogleError::UnknownAccess)
    ));
    assert!(matches!(
        google.choose_site(ALICE, SHOP, "shop.example").await,
        Err(GoogleError::InvalidSite)
    ));
    assert!(google.load_connection(SHOP).await.unwrap().is_none());
}

#[tokio::test]
async fn a_lapse_is_noted_once_for_the_grant_it_was_seen_on() {
    let google = command().await;
    // Nothing to lapse yet.
    assert!(
        !google
            .note_lapsed(GOOGLE_ACTOR, SHOP, b"sealed-1")
            .await
            .unwrap()
    );
    google
        .connect(ALICE, SHOP, grant("alice@example.com", b"sealed-1"))
        .await
        .unwrap();
    google.choose_site(ALICE, SHOP, SITE).await.unwrap();

    // A refusal met with a grant replaced since says nothing of this one.
    assert!(
        !google
            .note_lapsed(GOOGLE_ACTOR, SHOP, b"sealed-0")
            .await
            .unwrap()
    );
    assert!(
        google
            .note_lapsed(GOOGLE_ACTOR, SHOP, b"sealed-1")
            .await
            .unwrap()
    );
    assert!(
        !google
            .note_lapsed(GOOGLE_ACTOR, SHOP, b"sealed-1")
            .await
            .unwrap()
    );
    let connection = google.load_connection(SHOP).await.unwrap().unwrap();
    assert!(connection.lapsed);
    assert!(connection.covers(ACCESS_SEARCH_CONSOLE));
    assert!(!connection.reads(ACCESS_SEARCH_CONSOLE));
    assert!(matches!(
        google
            .choose_site(ALICE, SHOP, "sc-domain:shop.example")
            .await,
        Err(GoogleError::Lapsed)
    ));

    // Connecting again clears it and keeps the site.
    google
        .connect(ALICE, SHOP, grant("alice@example.com", b"sealed-2"))
        .await
        .unwrap();
    let connection = google.load_connection(SHOP).await.unwrap().unwrap();
    assert!(connection.reads(ACCESS_SEARCH_CONSOLE));
    assert_eq!(connection.site.as_deref(), Some(SITE));
}
