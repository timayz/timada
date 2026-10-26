mod common;

use std::sync::{Arc, LazyLock};

use common::{MemoryKeys, TENANT, ring};
use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_orders::aggregator::Placed;
use timada_orders::{Payment, ShippingAddress};
use timada_shoppers::{Command, count_shoppers, read_model_subscription, shoppers_of_tenant};

const NOW: u64 = 1_700_000_000;

/// The shoppers' keys, for every test: key ids are random, so they never
/// collide.
static KEYS: LazyLock<Arc<MemoryKeys>> = LazyLock::new(Default::default);

/// A fresh in-memory event store with the orders and shoppers read models.
async fn executor_with_pool() -> (evento::Sqlite, sqlx::SqlitePool) {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let mut migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
    migrator
        .add_migrations(timada_orders::migrations())
        .unwrap();
    migrator
        .add_migrations(timada_shoppers::migrations())
        .unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    (pool.clone().into(), pool)
}

async fn catch_up(executor: &evento::Sqlite, pool: &sqlx::SqlitePool) {
    timada_orders::read_model_subscription::<evento::Sqlite>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
    read_model_subscription::<evento::Sqlite>(pool.clone(), ring(&KEYS))
        .run_once(executor)
        .await
        .unwrap();
}

async fn sign_in(shoppers: &Command<evento::Sqlite>, email: &str, now: u64) -> String {
    let issued = shoppers.issue_code(email, now).await.unwrap();
    shoppers
        .sign_in(&**KEYS, email, &issued.code, now)
        .await
        .unwrap()
        .shopper_id
}

fn address(city: &str) -> ShippingAddress {
    ShippingAddress {
        name: "Ada Lovelace".to_owned(),
        line1: "1 rue de la Paix".to_owned(),
        postal_code: "75002".to_owned(),
        city: city.to_owned(),
        country: "FR".to_owned(),
        ..Default::default()
    }
}

/// A paid order placed with `email`, signed in or not.
async fn placed(executor: &evento::Sqlite, order_id: &str, email: &str) {
    evento::append(order_id)
        .original_version(0)
        .event(&Placed {
            reference: format!("REF-{order_id}"),
            cart_id: format!("cart-{order_id}"),
            email: email.to_owned(),
            shipping: address("Paris"),
            note: None,
            lines: vec![],
            currency: "EUR".to_owned(),
            total_cents: 1000,
            payment: Payment {
                provider: "stripe".to_owned(),
                payment_id: format!("pi-{order_id}"),
                amount_cents: 1000,
                currency: "EUR".to_owned(),
            },
            placed_by: "storefront".to_owned(),
        })
        .tenant(TENANT)
        .commit(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn shoppers_are_listed_with_their_orders_and_addresses() {
    let (executor, pool) = executor_with_pool().await;
    let shoppers = Command::new(executor.clone(), TENANT);

    // Ada ordered as a guest before she had an account.
    placed(&executor, "order-1", "ada@example.com").await;
    let ada = sign_in(&shoppers, "Ada@Example.com", NOW).await;
    let paris = shoppers
        .save_address(&**KEYS, &ada, address("Paris"))
        .await
        .unwrap();
    shoppers
        .save_address(&**KEYS, &ada, address("Lyon"))
        .await
        .unwrap();
    shoppers.remove_address(&ada, &paris).await.unwrap();
    placed(&executor, "order-2", "ada@example.com").await;
    let bob = sign_in(&shoppers, "bob@example.com", NOW + 60).await;
    placed(&executor, "order-3", "carol@example.com").await;

    // Twice: delivery may repeat without the rows drifting.
    for _ in 0..2 {
        catch_up(&executor, &pool).await;
    }
    assert_eq!(count_shoppers(&pool, TENANT).await.unwrap(), 2);
    let rows = shoppers_of_tenant(&pool, TENANT, 10, 0).await.unwrap();
    assert_eq!(rows.len(), 2);

    let row = |id: &str| rows.iter().find(|row| row.id == id).unwrap();
    assert_eq!(row(&bob).order_count, 0);
    assert_eq!(row(&bob).address_count, 0);
    assert_eq!(row(&ada).email, "ada@example.com");
    assert_eq!(row(&ada).order_count, 2, "guest orders count too");
    assert_eq!(row(&ada).address_count, 1);
    assert!(row(&ada).registered_at > 0);
    assert!(
        rows[0].registered_at >= rows[1].registered_at,
        "newest first"
    );

    let page_two = shoppers_of_tenant(&pool, TENANT, 1, 1).await.unwrap();
    assert_eq!(page_two.len(), 1);
    assert_eq!(page_two[0].id, rows[1].id);

    assert_eq!(count_shoppers(&pool, "tenant-other").await.unwrap(), 0);
    assert!(
        shoppers_of_tenant(&pool, "tenant-other", 10, 0)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_forgotten_shopper_leaves_the_list_until_signing_in_again() {
    let (executor, pool) = executor_with_pool().await;
    let shoppers = Command::new(executor.clone(), TENANT);
    let ada = sign_in(&shoppers, "ada@example.com", NOW).await;
    shoppers
        .save_address(&**KEYS, &ada, address("Paris"))
        .await
        .unwrap();
    sign_in(&shoppers, "bob@example.com", NOW).await;
    catch_up(&executor, &pool).await;
    assert_eq!(count_shoppers(&pool, TENANT).await.unwrap(), 2);

    shoppers.forget(&**KEYS, &ada).await.unwrap();
    catch_up(&executor, &pool).await;
    let rows = shoppers_of_tenant(&pool, TENANT, 10, 0).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].email, "bob@example.com");

    sign_in(&shoppers, "ada@example.com", NOW + 60).await;
    catch_up(&executor, &pool).await;
    let rows = shoppers_of_tenant(&pool, TENANT, 10, 0).await.unwrap();
    let row = rows.iter().find(|row| row.id == ada).unwrap();
    assert_eq!(row.email, "ada@example.com");
    assert_eq!(row.address_count, 0, "a fresh account");
}
