mod common;

use common::*;
use timada_pages::{PageError, TokenChange};

const SYSTEM: &str = "system";

#[tokio::test]
async fn the_pixel_and_its_token_are_set_and_taken_off() {
    let pages = command().await;
    assert!(pages.load_meta_pixel().await.unwrap().is_none());

    pages
        .set_meta_pixel(ALICE, Some(" 1234567890123456 "), TokenChange::Keep, false)
        .await
        .unwrap();
    let pixel = pages.load_meta_pixel().await.unwrap().unwrap();
    assert_eq!(pixel.pixel_id.as_deref(), Some("1234567890123456"));
    assert_eq!(pixel.token_sealed, None);

    // Removing a token that is not there, on the same pixel: nothing written.
    pages
        .set_meta_pixel(ALICE, Some("1234567890123456"), TokenChange::Remove, false)
        .await
        .unwrap();
    let same = pages.load_meta_pixel().await.unwrap().unwrap();
    assert_eq!(same.aggregate_version, pixel.aggregate_version);

    pages
        .set_meta_pixel(
            ALICE,
            Some("1234567890123456"),
            TokenChange::Set(vec![1, 2, 3]),
            false,
        )
        .await
        .unwrap();
    let pixel = pages.load_meta_pixel().await.unwrap().unwrap();
    assert_eq!(pixel.token_sealed.as_deref(), Some(&[1u8, 2, 3][..]));

    // Another pixel keeps the token it is told to keep.
    pages
        .set_meta_pixel(ALICE, Some("99999"), TokenChange::Keep, false)
        .await
        .unwrap();
    let pixel = pages.load_meta_pixel().await.unwrap().unwrap();
    assert_eq!(pixel.pixel_id.as_deref(), Some("99999"));
    assert!(pixel.token_sealed.is_some());

    pages
        .set_meta_pixel(ALICE, Some("99999"), TokenChange::Remove, false)
        .await
        .unwrap();
    let pixel = pages.load_meta_pixel().await.unwrap().unwrap();
    assert_eq!(pixel.token_sealed, None);

    // No pixel, no token: taken off together.
    pages
        .set_meta_pixel(ALICE, Some("99999"), TokenChange::Set(vec![4]), false)
        .await
        .unwrap();
    pages
        .set_meta_pixel(ALICE, Some("  "), TokenChange::Keep, false)
        .await
        .unwrap();
    let pixel = pages.load_meta_pixel().await.unwrap().unwrap();
    assert_eq!(pixel.pixel_id, None);
    assert_eq!(pixel.token_sealed, None);
}

#[tokio::test]
async fn the_id_is_checked_and_a_token_needs_a_pixel() {
    let pages = command().await;
    for id in [
        "1234",
        "12345678901234567890123",
        "12345a",
        "123');alert(1)//",
    ] {
        let refused: PageError = pages
            .set_meta_pixel(ALICE, Some(id), TokenChange::Keep, false)
            .await
            .expect_err(id);
        assert_eq!(refused.code(), Some("meta_pixel_invalid_id"), "{id}");
    }
    let refused = pages
        .set_meta_pixel(ALICE, None, TokenChange::Set(vec![1]), false)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some("meta_pixel_missing"));
    assert!(pages.load_meta_pixel().await.unwrap().is_none());
}

#[tokio::test]
async fn a_refusal_is_kept_once_until_the_token_changes() {
    let pages = command().await;
    // Nothing to refuse without a token.
    pages
        .refuse_meta_conversion(SYSTEM, "order-1", "Invalid OAuth access token")
        .await
        .unwrap();
    pages
        .set_meta_pixel(ALICE, Some("123456"), TokenChange::Keep, false)
        .await
        .unwrap();
    pages
        .refuse_meta_conversion(SYSTEM, "order-1", "Invalid OAuth access token")
        .await
        .unwrap();
    assert_eq!(
        pages.load_meta_pixel().await.unwrap().unwrap().refusal,
        None
    );

    pages
        .set_meta_pixel(ALICE, Some("123456"), TokenChange::Set(vec![1]), false)
        .await
        .unwrap();
    pages
        .refuse_meta_conversion(SYSTEM, "order-1", "Invalid OAuth\naccess token")
        .await
        .unwrap();
    let pixel = pages.load_meta_pixel().await.unwrap().unwrap();
    assert_eq!(pixel.refusal.as_deref(), Some("Invalid OAuth access token"));
    // The same reason for the next order writes nothing.
    pages
        .refuse_meta_conversion(SYSTEM, "order-2", "Invalid OAuth access token")
        .await
        .unwrap();
    let same = pages.load_meta_pixel().await.unwrap().unwrap();
    assert_eq!(same.aggregate_version, pixel.aggregate_version);

    // A new token starts clean.
    pages
        .set_meta_pixel(ALICE, Some("123456"), TokenChange::Set(vec![2]), false)
        .await
        .unwrap();
    assert_eq!(
        pages.load_meta_pixel().await.unwrap().unwrap().refusal,
        None
    );
}

#[tokio::test]
async fn what_a_sale_is_worth_to_meta_is_chosen_with_the_pixel() {
    let pages = command().await;
    // No pixel, no choice kept.
    pages
        .set_meta_pixel(ALICE, Some(" "), TokenChange::Keep, true)
        .await
        .unwrap();
    assert!(pages.load_meta_pixel().await.unwrap().is_none());

    pages
        .set_meta_pixel(ALICE, Some("123456"), TokenChange::Keep, true)
        .await
        .unwrap();
    let pixel = pages.load_meta_pixel().await.unwrap().unwrap();
    assert!(pixel.profit_as_value);
    let version = pixel.aggregate_version;

    // The same again writes nothing.
    pages
        .set_meta_pixel(ALICE, Some("123456"), TokenChange::Keep, true)
        .await
        .unwrap();
    let same = pages.load_meta_pixel().await.unwrap().unwrap();
    assert_eq!(same.aggregate_version, version);

    // The choice alone changes: one event, the pixel as it was.
    pages
        .set_meta_pixel(ALICE, Some("123456"), TokenChange::Keep, false)
        .await
        .unwrap();
    let pixel = pages.load_meta_pixel().await.unwrap().unwrap();
    assert!(!pixel.profit_as_value);
    assert_eq!(pixel.pixel_id.as_deref(), Some("123456"));
    assert_eq!(pixel.aggregate_version, version + 1);
}
