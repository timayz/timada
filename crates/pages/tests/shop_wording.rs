mod common;

use common::*;
use timada_pages::{NewAnnouncement, PageError};

#[tokio::test]
async fn the_bar_and_the_popup_are_worded_in_the_shops_other_languages() {
    let pages = command().await;
    let id = pages
        .add_announcement(
            ALICE,
            NewAnnouncement {
                text: "**-10 %** avec le code BIENVENUE10".to_owned(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let english = pages.shop_wording("en").await.unwrap();
    assert!(english.announcements.is_empty() && english.popup_title.is_empty());

    pages
        .word_announcement(ALICE, "EN", &id, "  **10% off** with the\ncode WELCOME10 ")
        .await
        .unwrap();
    pages
        .word_newsletter_popup(ALICE, "en", " 10% off your\nfirst order ", "")
        .await
        .unwrap();
    let english = pages.shop_wording("en").await.unwrap();
    assert_eq!(
        english.announcements.get(&id).map(String::as_str),
        Some("**10% off** with the code WELCOME10")
    );
    assert_eq!(english.popup_title, "10% off your first order");
    assert_eq!(english.popup_text, "");
    // Each language its own.
    assert!(
        pages
            .shop_wording("de")
            .await
            .unwrap()
            .announcements
            .is_empty()
    );

    // An empty text goes back to the main language's.
    pages
        .word_announcement(ALICE, "en", &id, " ")
        .await
        .unwrap();
    pages
        .word_newsletter_popup(ALICE, "en", "", "")
        .await
        .unwrap();
    let english = pages.shop_wording("en").await.unwrap();
    assert!(english.announcements.is_empty() && english.popup_title.is_empty());
}

#[tokio::test]
async fn a_wording_is_checked_as_the_main_text_is() {
    let pages = command().await;
    let id = pages
        .add_announcement(
            ALICE,
            NewAnnouncement {
                text: "Livraison offerte".to_owned(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let unknown = pages.word_announcement(ALICE, "en", "nope", "Free").await;
    assert!(
        matches!(unknown, Err(PageError::AnnouncementNotFound)),
        "{unknown:?}"
    );
    let language = pages.word_announcement(ALICE, "xx", &id, "Free").await;
    assert!(
        matches!(language, Err(PageError::InvalidLanguage)),
        "{language:?}"
    );
    let long = pages
        .word_announcement(ALICE, "en", &id, &"x".repeat(201))
        .await;
    assert!(
        matches!(long, Err(PageError::InvalidAnnouncement)),
        "{long:?}"
    );
    let title = pages
        .word_newsletter_popup(ALICE, "en", &"x".repeat(81), "")
        .await;
    assert!(matches!(title, Err(PageError::InvalidPopup)), "{title:?}");
}
