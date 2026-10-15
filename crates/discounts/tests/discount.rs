mod common;

use common::*;
use timada_discounts::{
    CreateDiscount, DiscountError, DiscountStatus, DiscountValue, MIN_CHARGE_CENTS,
};

#[tokio::test]
async fn a_code_is_created_found_in_any_case_and_unique() {
    let cmd = command().await;
    let discount = create(&cmd, percent("summer-10", 1000)).await;
    assert_eq!(discount.code, "SUMMER-10");
    assert_eq!(discount.value, DiscountValue::Percent(1000));
    assert_eq!(discount.status, DiscountStatus::Active);

    let found = cmd
        .load_discount_by_code(" Summer-10")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, discount.id);
    assert!(cmd.load_discount_by_code("WINTER").await.unwrap().is_none());
    assert!(cmd.load_discount_by_code("no way").await.unwrap().is_none());

    let taken = cmd.create_discount(ALICE, percent("SUMMER-10", 500)).await;
    assert!(matches!(taken, Err(DiscountError::CodeTaken)));
}

#[tokio::test]
async fn what_is_refused_at_creation() {
    let cmd = command().await;
    let cases = [
        (percent("A", 1000), "invalid_code"),
        (percent("ZERO", 0), "invalid_percent"),
        (percent("MORE", 10_001), "invalid_percent"),
        (
            CreateDiscount {
                code: "NOAMOUNT".to_owned(),
                value: DiscountValue::Amount {
                    cents: 0,
                    currency: "EUR".to_owned(),
                },
                ..Default::default()
            },
            "invalid_amount",
        ),
        (
            CreateDiscount {
                code: "YEN".to_owned(),
                value: DiscountValue::Amount {
                    cents: 500,
                    currency: "JPY".to_owned(),
                },
                ..Default::default()
            },
            "invalid_currency",
        ),
        (
            CreateDiscount {
                starts_at: Some(200),
                ends_at: Some(200),
                ..percent("WINDOW", 1000)
            },
            "invalid_window",
        ),
        (
            CreateDiscount {
                max_uses: Some(0),
                ..percent("NONE", 1000)
            },
            "invalid_max_uses",
        ),
        (
            CreateDiscount {
                max_uses: Some(10_001),
                ..percent("MANY", 1000)
            },
            "invalid_max_uses",
        ),
    ];
    for (discount, code) in cases {
        let err = cmd.create_discount(ALICE, discount).await.unwrap_err();
        assert_eq!(err.code(), Some(code));
    }
}

#[tokio::test]
async fn what_a_code_takes_off() {
    let cmd = command().await;
    let ten = create(&cmd, percent("TEN", 1000)).await;
    assert_eq!(ten.amount_off(1995, "EUR", 0).unwrap(), 200);

    let five = create(
        &cmd,
        CreateDiscount {
            code: "FIVE".to_owned(),
            value: DiscountValue::Amount {
                cents: 500,
                currency: "eur".to_owned(),
            },
            ..Default::default()
        },
    )
    .await;
    assert_eq!(five.amount_off(2000, "EUR", 0).unwrap(), 500);
    assert!(matches!(
        five.amount_off(2000, "USD", 0),
        Err(DiscountError::Currency)
    ));
    // Never below what a card payment takes.
    assert_eq!(
        five.amount_off(300, "EUR", 0).unwrap(),
        300 - MIN_CHARGE_CENTS
    );
    assert_eq!(five.amount_off(40, "EUR", 0).unwrap(), 0);
    let all = create(&cmd, percent("FREE", 10_000)).await;
    assert_eq!(
        all.amount_off(1000, "EUR", 0).unwrap(),
        1000 - MIN_CHARGE_CENTS
    );
}

#[tokio::test]
async fn a_code_works_inside_its_window_while_active() {
    let cmd = command().await;
    let discount = create(
        &cmd,
        CreateDiscount {
            starts_at: Some(100),
            ends_at: Some(200),
            ..percent("WINDOW", 1000)
        },
    )
    .await;
    assert!(matches!(
        discount.amount_off(1000, "EUR", 99),
        Err(DiscountError::Inactive)
    ));
    assert_eq!(discount.amount_off(1000, "EUR", 100).unwrap(), 100);
    assert!(discount.amount_off(1000, "EUR", 200).is_err());

    cmd.deactivate_discount(ALICE, &discount.id).await.unwrap();
    cmd.deactivate_discount(ALICE, &discount.id).await.unwrap();
    let off = cmd.load_discount(&discount.id).await.unwrap().unwrap();
    assert_eq!(off.status, DiscountStatus::Inactive);
    assert!(off.amount_off(1000, "EUR", 150).is_err());

    cmd.reactivate_discount(ALICE, &discount.id).await.unwrap();
    let on = cmd.load_discount(&discount.id).await.unwrap().unwrap();
    assert_eq!(on.amount_off(1000, "EUR", 150).unwrap(), 100);
}

#[tokio::test]
async fn archiving_frees_the_code() {
    let cmd = command().await;
    let first = create(&cmd, percent("REUSE", 1000)).await;
    cmd.archive_discount(ALICE, &first.id).await.unwrap();
    cmd.archive_discount(ALICE, &first.id).await.unwrap();
    assert!(cmd.load_discount_by_code("REUSE").await.unwrap().is_none());
    assert!(matches!(
        cmd.deactivate_discount(ALICE, &first.id).await,
        Err(DiscountError::Archived)
    ));

    let second = create(&cmd, percent("REUSE", 2000)).await;
    let found = cmd.load_discount_by_code("REUSE").await.unwrap().unwrap();
    assert_eq!(found.id, second.id);
}
