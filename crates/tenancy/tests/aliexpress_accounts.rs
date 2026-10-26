mod common;

use common::*;
use timada_tenancy::{
    AuthorizeAliexpress, Command, ConfigureAliexpress, LegalIdentity, TenancyError,
    aliexpress_accounts_due, aliexpress_accounts_subscription,
};

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

async fn catch_up(executor: &evento::Sqlite, pool: &sqlx::SqlitePool) {
    aliexpress_accounts_subscription::<evento::Sqlite>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

/// A company registered by `owner`, with a SIRET of its own.
async fn company(cmd: &Command<evento::Sqlite>, owner: &str, siret: &str) -> String {
    let identity = LegalIdentity {
        legal_name: "Ali Company".to_owned(),
        legal_form: "SAS".to_owned(),
        share_capital_cents: None,
        siret: siret.to_owned(),
        rcs_city: None,
        line1: "1 rue de la Paix".to_owned(),
        line2: None,
        postal_code: "75002".to_owned(),
        city: "Paris".to_owned(),
        country: "FR".to_owned(),
        contact_email: "hello@example.com".to_owned(),
        vat_number: None,
        oss_registered: false,
    };
    cmd.create_company(owner, identity).await.unwrap().id
}

fn market() -> ConfigureAliexpress {
    ConfigureAliexpress {
        ship_to_country: "fr".to_owned(),
        currency: "eur".to_owned(),
        language: "FR".to_owned(),
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
    aliexpress_accounts_due(pool, before, NOW, 100)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_connected_company_is_due_once_its_access_lapses_within_the_margin() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let owner = new_id();
    let company = company(&cmd, &owner, "12002701600092").await;
    catch_up(&executor, &pool).await;
    // Registered but not connected: nothing to renew.
    assert!(due(&pool, NOW + 10 * DAY).await.is_empty());

    cmd.authorize_aliexpress(&owner, &company, tokens(NOW + 2 * DAY, NOW + 30 * DAY))
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    assert!(due(&pool, NOW + DAY).await.is_empty());
    assert_eq!(due(&pool, NOW + 2 * DAY).await, vec![company.clone()]);
    let state = cmd.load_company(&company).await.unwrap().unwrap();
    assert_eq!(state.aliexpress.unwrap().account, "buyer");
}

#[tokio::test]
async fn only_an_owner_connects_or_disconnects_the_account() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let owner = new_id();
    let company = company(&cmd, &owner, "12002701600092").await;
    let stranger = new_id();
    assert!(matches!(
        cmd.authorize_aliexpress(&stranger, &company, tokens(NOW + DAY, 0))
            .await,
        Err(TenancyError::NotCompanyOwner)
    ));
    cmd.authorize_aliexpress(&owner, &company, tokens(NOW + DAY, 0))
        .await
        .unwrap();
    assert!(matches!(
        cmd.disconnect_aliexpress(&stranger, &company).await,
        Err(TenancyError::NotCompanyOwner)
    ));
    catch_up(&executor, &pool).await;
    assert_eq!(due(&pool, NOW + DAY).await, vec![company.clone()]);

    cmd.disconnect_aliexpress(&owner, &company).await.unwrap();
    catch_up(&executor, &pool).await;
    assert!(due(&pool, NOW + DAY).await.is_empty());
    assert!(
        cmd.load_company(&company)
            .await
            .unwrap()
            .unwrap()
            .aliexpress
            .is_none()
    );
    // Disconnected twice is nothing; a host renewal of nothing is nothing.
    cmd.disconnect_aliexpress(&owner, &company).await.unwrap();
    cmd.refresh_aliexpress("host", &company, tokens(NOW + 3 * DAY, 0))
        .await
        .unwrap();
    assert!(
        cmd.load_company(&company)
            .await
            .unwrap()
            .unwrap()
            .aliexpress
            .is_none()
    );
}

#[tokio::test]
async fn the_host_renews_without_owning() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let owner = new_id();
    let company = company(&cmd, &owner, "12002701600092").await;
    cmd.authorize_aliexpress(&owner, &company, tokens(NOW + DAY, NOW + 30 * DAY))
        .await
        .unwrap();
    cmd.refresh_aliexpress("host", &company, tokens(NOW + 3 * DAY, NOW + 40 * DAY))
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    assert!(due(&pool, NOW + 2 * DAY).await.is_empty());
    assert_eq!(due(&pool, NOW + 3 * DAY).await, vec![company.clone()]);
}

#[tokio::test]
async fn a_refresh_token_that_lapsed_is_not_due() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let owner = new_id();
    let lapsed = company(&cmd, &owner, "12002701600092").await;
    let forever = company(&cmd, &owner, "55208131766522").await;
    for (company, refresh_expires_at) in [(&lapsed, NOW - 1), (&forever, 0)] {
        cmd.authorize_aliexpress(&owner, company, tokens(NOW - DAY, refresh_expires_at))
            .await
            .unwrap();
    }
    catch_up(&executor, &pool).await;
    // No refresh lifetime given (0) is renewable.
    assert_eq!(due(&pool, NOW + DAY).await, vec![forever.clone()]);
}

#[tokio::test]
async fn started_on_an_existing_log_it_finds_the_companies_connected_before() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let owner = new_id();
    let company = company(&cmd, &owner, "12002701600092").await;
    cmd.authorize_aliexpress(&owner, &company, tokens(NOW, NOW + 30 * DAY))
        .await
        .unwrap();
    // Other events since: the subscription still starts from the beginning.
    cmd.add_company_owner(&owner, &company, &new_id())
        .await
        .unwrap();

    catch_up(&executor, &pool).await;
    assert_eq!(due(&pool, NOW + DAY).await, vec![company.clone()]);
}

#[tokio::test]
async fn the_market_is_the_shops_and_how_orders_go_is_kept_across_it() {
    let (executor, _pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let owner = new_id();
    let tenant = create_tenant(&cmd, "Ali Orders", &owner).await;
    assert!(matches!(
        cmd.set_aliexpress_forwarding(&owner, &tenant.id, true, true)
            .await,
        Err(TenancyError::AliexpressNotConfigured)
    ));
    assert!(matches!(
        cmd.configure_aliexpress(
            &owner,
            &tenant.id,
            ConfigureAliexpress {
                ship_to_country: "France".to_owned(),
                ..market()
            }
        )
        .await,
        Err(TenancyError::InvalidAliexpressMarket)
    ));
    cmd.configure_aliexpress(&owner, &tenant.id, market())
        .await
        .unwrap();
    let load = async || cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    let settings = load().await.aliexpress.unwrap();
    assert_eq!(
        (
            settings.ship_to_country.as_str(),
            settings.currency.as_str(),
            settings.language.as_str()
        ),
        ("FR", "EUR", "fr")
    );
    assert!(matches!(
        cmd.set_aliexpress_forwarding(&new_id(), &tenant.id, true, true)
            .await,
        Err(TenancyError::Forbidden)
    ));
    cmd.set_aliexpress_forwarding(&owner, &tenant.id, true, false)
        .await
        .unwrap();
    let forwarding = |state: timada_tenancy::TenantState| {
        let settings = state.aliexpress.unwrap();
        (settings.auto_forward, settings.try_to_pay)
    };
    assert_eq!(forwarding(load().await), (true, false));

    // The market set again keeps them; stopping forgets everything.
    cmd.configure_aliexpress(&owner, &tenant.id, market())
        .await
        .unwrap();
    assert_eq!(forwarding(load().await), (true, false));
    cmd.remove_aliexpress(&owner, &tenant.id).await.unwrap();
    assert!(load().await.aliexpress.is_none());
    cmd.remove_aliexpress(&owner, &tenant.id).await.unwrap();
}
