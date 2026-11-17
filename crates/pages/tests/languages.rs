mod common;

use common::*;
use timada_pages::{MAX_OTHER_LANGUAGES, PageError, ShopLanguages};

fn codes(codes: &[&str]) -> Vec<String> {
    codes.iter().map(|code| (*code).to_owned()).collect()
}

#[tokio::test]
async fn a_shop_names_the_languages_its_storefront_is_read_in() {
    let pages = command().await;
    // Never set: the storefront's own language stands.
    assert_eq!(pages.languages().await.unwrap(), None);
    pages
        .set_languages(ALICE, " FR ", &codes(&["de", "en", "fr", "DE"]))
        .await
        .unwrap();
    let set = pages.languages().await.unwrap().unwrap();
    assert_eq!(
        set,
        ShopLanguages {
            main: "fr".to_owned(),
            // In the list's order, the main one and the repeat dropped.
            others: codes(&["en", "de"]),
        }
    );
    assert!(set.has("fr") && set.has("de") && !set.has("es"));

    // Another main language: the old one is a translation like any other.
    pages
        .set_languages(ALICE, "en", &codes(&["fr"]))
        .await
        .unwrap();
    assert_eq!(
        pages.languages().await.unwrap(),
        Some(ShopLanguages {
            main: "en".to_owned(),
            others: codes(&["fr"]),
        })
    );
}

#[tokio::test]
async fn a_language_off_the_list_and_too_many_of_them_are_refused() {
    let pages = command().await;
    for (main, others) in [
        ("zz", codes(&[])),
        ("fr", codes(&["english"])),
        ("", codes(&[])),
    ] {
        let set = pages.set_languages(ALICE, main, &others).await;
        assert!(matches!(set, Err(PageError::InvalidLanguage)), "{set:?}");
    }
    let many: Vec<String> = timada_core::languages::LANGUAGES
        .iter()
        .map(|(code, _)| (*code).to_owned())
        .filter(|code| code != "fr")
        .take(MAX_OTHER_LANGUAGES + 1)
        .collect();
    let set = pages.set_languages(ALICE, "fr", &many).await;
    assert!(matches!(set, Err(PageError::TooManyLanguages)), "{set:?}");
    pages
        .set_languages(ALICE, "fr", &many[..MAX_OTHER_LANGUAGES])
        .await
        .unwrap();
}

#[tokio::test]
async fn the_same_languages_write_nothing() {
    let pages = command().await;
    pages
        .set_languages(ALICE, "fr", &codes(&["en"]))
        .await
        .unwrap();
    pages
        .set_languages(ALICE, "fr", &codes(&["en", "fr"]))
        .await
        .unwrap();
    let state = timada_pages::languages_projection()
        .load(timada_pages::languages_id())
        .tenant(TENANT)
        .execute(pages.executor())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.aggregate_version, 1);
}
