mod common;

use common::*;
use timada_pages::{NewsletterPopupSettings, PageError};

fn offer() -> NewsletterPopupSettings {
    NewsletterPopupSettings {
        shown: true,
        title: "  -10 % sur votre\npremière commande ".to_owned(),
        text: "Inscrivez-vous : le code arrive par e-mail.".to_owned(),
        delay_secs: 8,
        on_leave: true,
    }
}

#[tokio::test]
async fn the_popup_is_not_shown_until_the_merchant_sets_it() {
    let pages = command().await;
    assert_eq!(
        pages.newsletter_popup().await.unwrap(),
        NewsletterPopupSettings::default()
    );
    pages.set_newsletter_popup(ALICE, offer()).await.unwrap();
    let set = pages.newsletter_popup().await.unwrap();
    assert!(set.shown && set.on_leave);
    assert_eq!(set.title, "-10 % sur votre première commande");
    assert_eq!(set.delay_secs, 8);

    // Taken off, it keeps its words for the next time.
    pages
        .set_newsletter_popup(
            ALICE,
            NewsletterPopupSettings {
                shown: false,
                ..offer()
            },
        )
        .await
        .unwrap();
    let off = pages.newsletter_popup().await.unwrap();
    assert!(!off.shown);
    assert_eq!(off.title, "-10 % sur votre première commande");
}

#[tokio::test]
async fn a_popup_shown_needs_a_title_and_opens_within_ten_minutes() {
    let pages = command().await;
    for refused in [
        NewsletterPopupSettings {
            title: " ".to_owned(),
            ..offer()
        },
        NewsletterPopupSettings {
            title: "x".repeat(81),
            ..offer()
        },
        NewsletterPopupSettings {
            text: "x".repeat(301),
            ..offer()
        },
        NewsletterPopupSettings {
            delay_secs: 601,
            ..offer()
        },
    ] {
        let set = pages.set_newsletter_popup(ALICE, refused).await;
        assert!(matches!(set, Err(PageError::InvalidPopup)), "{set:?}");
    }
    // One not shown may have no title yet.
    pages
        .set_newsletter_popup(
            ALICE,
            NewsletterPopupSettings {
                delay_secs: 5,
                ..NewsletterPopupSettings::default()
            },
        )
        .await
        .unwrap();
}
