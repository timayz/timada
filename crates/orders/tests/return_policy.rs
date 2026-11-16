mod common;

use common::*;
use timada_orders::{MAX_RETURN_DAYS, ReturnTerms};

#[tokio::test]
async fn a_shop_states_its_return_terms_and_takes_them_back() {
    let (orders, _catalog) = store().await;
    assert_eq!(orders.return_policy().await.unwrap(), None);

    let terms = ReturnTerms {
        days: 30,
        free: true,
    };
    orders.set_return_policy(ALICE, Some(terms)).await.unwrap();
    assert_eq!(orders.return_policy().await.unwrap(), Some(terms));

    // The same again writes nothing.
    let version = orders
        .load_return_policy()
        .await
        .unwrap()
        .unwrap()
        .aggregate_version;
    orders.set_return_policy(ALICE, Some(terms)).await.unwrap();
    assert_eq!(
        orders
            .load_return_policy()
            .await
            .unwrap()
            .unwrap()
            .aggregate_version,
        version
    );

    orders.set_return_policy(ALICE, None).await.unwrap();
    assert_eq!(orders.return_policy().await.unwrap(), None);
}

#[tokio::test]
async fn a_return_window_is_a_day_to_a_year() {
    let (orders, _catalog) = store().await;
    for days in [0, MAX_RETURN_DAYS + 1] {
        let refused = orders
            .set_return_policy(ALICE, Some(ReturnTerms { days, free: false }))
            .await
            .unwrap_err();
        assert_eq!(refused.code(), Some("invalid_return_days"), "{days}");
    }
    assert!(orders.load_return_policy().await.unwrap().is_none());
    orders
        .set_return_policy(
            ALICE,
            Some(ReturnTerms {
                days: MAX_RETURN_DAYS,
                free: false,
            }),
        )
        .await
        .unwrap();
}
