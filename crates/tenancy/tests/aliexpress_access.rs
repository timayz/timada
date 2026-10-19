mod common;

use common::*;
use timada_tenancy::{
    AuthorizeAliexpress, Command, ConfigureAliexpress, aliexpress_access_due,
    aliexpress_access_subscription,
};

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

async fn catch_up(executor: &evento::Sqlite, pool: &sqlx::SqlitePool) {
    aliexpress_access_subscription::<evento::Sqlite>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

fn app(app_key: &str) -> ConfigureAliexpress {
    ConfigureAliexpress {
        app_key: app_key.to_owned(),
        app_secret_sealed: vec![1, 2, 3],
        ship_to_country: "FR".to_owned(),
        currency: "EUR".to_owned(),
        language: "fr".to_owned(),
        pricing: None,
    }
}

fn tokens(expires_at: i64, refresh_expires_at: i64) -> AuthorizeAliexpress {
    AuthorizeAliexpress {
        access_token_sealed: vec![4],
        refresh_token_sealed: vec![5],
        expires_at: expires_at as u64,
        refresh_expires_at: refresh_expires_at as u64,
        account: "buyer".to_owned(),
    }
}

async fn due(pool: &sqlx::SqlitePool, before: i64) -> Vec<String> {
    aliexpress_access_due(pool, before, NOW, 100).await.unwrap()
}

#[tokio::test]
async fn a_connected_shop_is_due_once_its_access_lapses_within_the_margin() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let owner = new_id();
    let tenant = create_tenant(&cmd, "Ali Idle", &owner).await;

    cmd.configure_aliexpress(&owner, &tenant.id, app("12345"))
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    // Set up but not connected: nothing to renew.
    assert!(due(&pool, NOW + 10 * DAY).await.is_empty());

    cmd.authorize_aliexpress(&owner, &tenant.id, tokens(NOW + 2 * DAY, NOW + 30 * DAY))
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    assert!(due(&pool, NOW + DAY).await.is_empty());
    assert_eq!(due(&pool, NOW + 2 * DAY).await, vec![tenant.id.clone()]);

    // Saving the same app again keeps the account.
    cmd.configure_aliexpress(&owner, &tenant.id, app("12345"))
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(due(&pool, NOW + 2 * DAY).await, vec![tenant.id.clone()]);
}

#[tokio::test]
async fn a_refresh_token_that_lapsed_is_not_due() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let owner = new_id();
    let lapsed = create_tenant(&cmd, "Ali Lapsed", &owner).await;
    let forever = create_tenant(&cmd, "Ali Forever", &owner).await;
    for (tenant, refresh_expires_at) in [(&lapsed, NOW - 1), (&forever, 0)] {
        cmd.configure_aliexpress(&owner, &tenant.id, app("12345"))
            .await
            .unwrap();
        cmd.authorize_aliexpress(&owner, &tenant.id, tokens(NOW - DAY, refresh_expires_at))
            .await
            .unwrap();
    }
    catch_up(&executor, &pool).await;
    // No refresh lifetime given (0) is renewable.
    assert_eq!(due(&pool, NOW + DAY).await, vec![forever.id.clone()]);
}

#[tokio::test]
async fn another_app_or_removing_it_drops_the_shop() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let owner = new_id();
    let switched = create_tenant(&cmd, "Ali Switched", &owner).await;
    let removed = create_tenant(&cmd, "Ali Removed", &owner).await;
    for tenant in [&switched, &removed] {
        cmd.configure_aliexpress(&owner, &tenant.id, app("12345"))
            .await
            .unwrap();
        cmd.authorize_aliexpress(&owner, &tenant.id, tokens(NOW, NOW + 30 * DAY))
            .await
            .unwrap();
    }
    catch_up(&executor, &pool).await;
    assert_eq!(due(&pool, NOW + DAY).await.len(), 2);

    cmd.configure_aliexpress(&owner, &switched.id, app("67890"))
        .await
        .unwrap();
    cmd.remove_aliexpress(&owner, &removed.id).await.unwrap();
    catch_up(&executor, &pool).await;
    assert!(due(&pool, NOW + DAY).await.is_empty());
}

#[tokio::test]
async fn started_on_an_existing_log_it_finds_the_shops_connected_before() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let owner = new_id();
    let tenant = create_tenant(&cmd, "Ali Before", &owner).await;
    cmd.configure_aliexpress(&owner, &tenant.id, app("12345"))
        .await
        .unwrap();
    cmd.authorize_aliexpress(&owner, &tenant.id, tokens(NOW, NOW + 30 * DAY))
        .await
        .unwrap();
    // Other events since: the subscription still starts from the beginning.
    cmd.rename_tenant(&owner, &tenant.id, "Ali Renamed")
        .await
        .unwrap();

    catch_up(&executor, &pool).await;
    assert_eq!(due(&pool, NOW + DAY).await, vec![tenant.id.clone()]);
}

#[tokio::test]
async fn how_orders_go_to_aliexpress_is_set_and_kept_with_the_same_app() {
    let (executor, _pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let owner = new_id();
    let tenant = create_tenant(&cmd, "Ali Orders", &owner).await;
    assert!(matches!(
        cmd.set_aliexpress_forwarding(&owner, &tenant.id, true, true)
            .await,
        Err(timada_tenancy::TenancyError::AliexpressNotConfigured)
    ));
    cmd.configure_aliexpress(&owner, &tenant.id, app("12345"))
        .await
        .unwrap();
    assert!(matches!(
        cmd.set_aliexpress_forwarding(&new_id(), &tenant.id, true, true)
            .await,
        Err(timada_tenancy::TenancyError::Forbidden)
    ));
    cmd.set_aliexpress_forwarding(&owner, &tenant.id, true, false)
        .await
        .unwrap();
    let settings = |state: timada_tenancy::TenantState| {
        let settings = state.aliexpress.unwrap();
        (settings.auto_forward, settings.try_to_pay)
    };
    let load = async || cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(settings(load().await), (true, false));

    // The same app set up again keeps them; another starts over.
    cmd.configure_aliexpress(&owner, &tenant.id, app("12345"))
        .await
        .unwrap();
    assert_eq!(settings(load().await), (true, false));
    cmd.configure_aliexpress(&owner, &tenant.id, app("67890"))
        .await
        .unwrap();
    assert_eq!(settings(load().await), (false, false));
}
