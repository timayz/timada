mod common;

use common::*;
use timada_catalog::{CatalogError, MAX_LISTING_FILTERS};

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[tokio::test]
async fn a_shop_names_the_options_its_listings_are_narrowed_by() {
    let cmd = command().await;
    // Never set: the storefront's own choice stands.
    assert_eq!(cmd.load_listing_filters().await.unwrap(), None);
    cmd.set_listing_filters(ALICE, &names(&[" Taille ", "Couleur", "taille", " "]))
        .await
        .unwrap();
    assert_eq!(
        cmd.load_listing_filters().await.unwrap(),
        Some(names(&["Taille", "Couleur"]))
    );
    // None at all is an answer too.
    cmd.set_listing_filters(ALICE, &[]).await.unwrap();
    assert_eq!(cmd.load_listing_filters().await.unwrap(), Some(Vec::new()));

    let many: Vec<String> = (0..=MAX_LISTING_FILTERS)
        .map(|n| format!("Option {n}"))
        .collect();
    let refused = cmd.set_listing_filters(ALICE, &many).await;
    assert!(
        matches!(refused, Err(CatalogError::TooManyListingFilters)),
        "{refused:?}"
    );
    let long = cmd.set_listing_filters(ALICE, &["x".repeat(41)]).await;
    assert!(
        matches!(long, Err(CatalogError::InvalidOptionName)),
        "{long:?}"
    );
}
