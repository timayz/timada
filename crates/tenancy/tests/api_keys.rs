mod common;

use common::*;
use timada_tenancy::{
    API_KEY_PREFIX, Command, CreateApiKey, DefineRole, PERMISSION_API_KEYS_MANAGE, TenancyError,
    api_key_hint, api_key_id, api_keys_of_tenant, read_model_subscription,
};

const NOW: u64 = 1_800_000_000;

fn create(tenant_id: &str, label: &str) -> CreateApiKey {
    CreateApiKey {
        tenant_id: tenant_id.to_owned(),
        label: label.to_owned(),
    }
}

#[tokio::test]
async fn a_key_is_created_by_a_manager_shown_once_and_authenticates() {
    let cmd = command().await;
    let alice = new_id();
    let bob = new_id();
    let carol = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    cmd.define_role(
        &alice,
        &tenant.id,
        DefineRole {
            role: "ops".to_owned(),
            label: "Operations".to_owned(),
            permissions: vec![PERMISSION_API_KEYS_MANAGE.to_owned()],
        },
    )
    .await
    .unwrap();
    cmd.define_role(
        &alice,
        &tenant.id,
        DefineRole {
            role: "viewer".to_owned(),
            label: "Viewer".to_owned(),
            permissions: vec![],
        },
    )
    .await
    .unwrap();
    cmd.grant_member(&alice, &tenant.id, &bob, "ops")
        .await
        .unwrap();
    cmd.grant_member(&alice, &tenant.id, &carol, "viewer")
        .await
        .unwrap();

    let err = cmd
        .create_api_key(&alice, create(&tenant.id, "  "), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvalidApiKeyLabel), "{err}");
    let err = cmd
        .create_api_key(&carol, create(&tenant.id, "Shop"), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
    let err = cmd
        .create_api_key(&new_id(), create(&tenant.id, "Shop"), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");

    let created = cmd
        .create_api_key(&bob, create(&tenant.id, " Shop "), NOW)
        .await
        .unwrap();
    assert!(created.token.starts_with(API_KEY_PREFIX));
    assert_eq!(created.token.len(), API_KEY_PREFIX.len() + 43);
    assert_eq!(created.key.id, api_key_id(&created.token));
    assert_eq!(created.key.tenant_id, tenant.id);
    assert_eq!(created.key.label, "Shop");
    assert_eq!(created.key.hint, api_key_hint(&created.token));
    assert_eq!(created.key.created_by, bob);
    assert_eq!(created.key.created_at, NOW);
    assert!(created.key.is_active());
    assert!(!format!("{created:?}").contains(&created.token));

    let access = cmd.authenticate_api_key(&created.token).await.unwrap();
    assert_eq!(access.key, created.key);
    assert_eq!(access.tenant.id, tenant.id);

    let err = cmd
        .authenticate_api_key("tk_nobody-issued-this")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::ApiKeyInvalid), "{err}");
    let other = cmd
        .create_api_key(&alice, create(&tenant.id, "Other"), NOW)
        .await
        .unwrap();
    assert_ne!(other.token, created.token);
}

#[tokio::test]
async fn a_revoked_key_no_longer_authenticates() {
    let cmd = command().await;
    let alice = new_id();
    let stranger = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    let created = cmd
        .create_api_key(&alice, create(&tenant.id, "Shop"), NOW)
        .await
        .unwrap();

    let err = cmd
        .revoke_api_key(&stranger, &created.key.id, NOW + 1)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
    let err = cmd
        .revoke_api_key(&alice, &api_key_id("tk_unknown"), NOW + 1)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::ApiKeyInvalid), "{err}");

    cmd.revoke_api_key(&alice, &created.key.id, NOW + 1)
        .await
        .unwrap();
    let key = cmd.load_api_key(&created.key.id).await.unwrap().unwrap();
    assert!(!key.is_active());
    assert_eq!(key.revoked_by.as_deref(), Some(alice.as_str()));
    assert_eq!(key.revoked_at, Some(NOW + 1));

    let err = cmd.authenticate_api_key(&created.token).await.unwrap_err();
    assert!(matches!(err, TenancyError::ApiKeyInvalid), "{err}");
    let err = cmd
        .revoke_api_key(&alice, &created.key.id, NOW + 2)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::ApiKeyInvalid), "{err}");
}

#[tokio::test]
async fn a_suspended_tenant_refuses_its_keys() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    let created = cmd
        .create_api_key(&alice, create(&tenant.id, "Shop"), NOW)
        .await
        .unwrap();
    cmd.suspend_tenant(&tenant.id, "unpaid").await.unwrap();

    let err = cmd.authenticate_api_key(&created.token).await.unwrap_err();
    assert!(matches!(err, TenancyError::Suspended), "{err}");
    let err = cmd
        .create_api_key(&alice, create(&tenant.id, "More"), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Suspended), "{err}");
    let err = cmd
        .revoke_api_key(&alice, &created.key.id, NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Suspended), "{err}");

    cmd.resume_tenant(&tenant.id).await.unwrap();
    assert!(cmd.authenticate_api_key(&created.token).await.is_ok());
}

#[tokio::test]
async fn keys_are_listed_newest_first_with_their_status() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    let first = cmd
        .create_api_key(&alice, create(&tenant.id, "First"), NOW)
        .await
        .unwrap();
    let second = cmd
        .create_api_key(&alice, create(&tenant.id, "Second"), NOW + 10)
        .await
        .unwrap();
    cmd.revoke_api_key(&alice, &first.key.id, NOW + 20)
        .await
        .unwrap();

    let catch_up = || async {
        read_model_subscription::<evento::Sqlite>(pool.clone())
            .run_once(&executor)
            .await
            .unwrap();
    };
    catch_up().await;
    catch_up().await; // redelivery leaves the rows as they are

    let rows = api_keys_of_tenant(&pool, &tenant.id).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, second.key.id);
    assert_eq!(rows[0].label, "Second");
    assert_eq!(rows[0].hint, second.key.hint);
    assert_eq!(rows[0].created_by, alice);
    assert_eq!(rows[0].created_at, (NOW + 10) as i64);
    assert!(rows[0].is_active());
    assert_eq!(rows[1].id, first.key.id);
    assert!(!rows[1].is_active());
    assert_eq!(rows[1].revoked_by.as_deref(), Some(alice.as_str()));
    assert_eq!(rows[1].revoked_at, Some((NOW + 20) as i64));
    assert!(
        api_keys_of_tenant(&pool, "nope").await.unwrap().is_empty(),
        "keys are scoped to their tenant"
    );
}
