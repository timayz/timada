mod common;

use timada_core::db::Db;

use common::*;
use timada_discounts::{
    Command, CreateDiscount, DiscountValue, discounts_of_tenant, read_model_subscription,
};

async fn catch_up(executor: &evento::Turso, pool: &Db) {
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn discounts_are_listed_and_stay_idempotent() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let ten = create(&cmd, percent("TEN", 1000)).await;
    let five = create(
        &cmd,
        CreateDiscount {
            code: "FIVE".to_owned(),
            value: DiscountValue::Amount {
                cents: 500,
                currency: "EUR".to_owned(),
            },
            max_uses: Some(3),
            once_per_email: true,
            ..Default::default()
        },
    )
    .await;
    cmd.deactivate_discount(ALICE, &ten.id).await.unwrap();

    catch_up(&executor, &pool).await;
    catch_up(&executor, &pool).await;

    let rows = discounts_of_tenant(&pool, TENANT, false).await.unwrap();
    assert_eq!(rows.len(), 2);
    let ten_row = rows.iter().find(|row| row.id == ten.id).unwrap();
    assert_eq!(ten_row.status, "inactive");
    assert_eq!(ten_row.value(), DiscountValue::Percent(1000));
    let five_row = rows.iter().find(|row| row.id == five.id).unwrap();
    assert_eq!(five_row.value(), five.value);
    assert_eq!(five_row.max_uses, Some(3));
    assert!(five_row.once_per_email);
    assert!(
        discounts_of_tenant(&pool, "other", true)
            .await
            .unwrap()
            .is_empty()
    );

    cmd.archive_discount(ALICE, &ten.id).await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(
        discounts_of_tenant(&pool, TENANT, false)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        discounts_of_tenant(&pool, TENANT, true)
            .await
            .unwrap()
            .len(),
        2
    );
}
