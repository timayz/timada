mod common;

use common::*;
use timada_tenancy::TenancyError;

#[tokio::test]
async fn only_an_owner_renames_a_tenant() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "acme", &alice).await;

    cmd.rename_tenant(&alice, &tenant.id, " Acme Corp ")
        .await
        .unwrap();
    let renamed = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(renamed.name, "Acme Corp");

    // The same name is a no-op (no new event, same version).
    cmd.rename_tenant(&alice, &tenant.id, "Acme Corp")
        .await
        .unwrap();
    assert_eq!(cmd.load_tenant(&tenant.id).await.unwrap().unwrap(), renamed);

    let err = cmd
        .rename_tenant(&new_id(), &tenant.id, "Evil")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
    let err = cmd.rename_tenant(&alice, &tenant.id, "").await.unwrap_err();
    assert!(matches!(err, TenancyError::InvalidName), "{err}");
    let err = cmd
        .rename_tenant(&alice, &new_id(), "Ghost")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::NotFound), "{err}");
}

#[tokio::test]
async fn suspension_is_idempotent_and_reversible() {
    let cmd = command().await;
    let tenant = create_tenant(&cmd, "acme", &new_id()).await;

    let err = cmd.resume_tenant(&tenant.id).await.unwrap_err();
    assert!(matches!(err, TenancyError::NotSuspended), "{err}");

    cmd.suspend_tenant(&tenant.id, " unpaid invoices ")
        .await
        .unwrap();
    cmd.suspend_tenant(&tenant.id, "again").await.unwrap();
    let suspended = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(suspended.suspended.as_deref(), Some("unpaid invoices"));

    cmd.resume_tenant(&tenant.id).await.unwrap();
    let resumed = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(!resumed.is_suspended());
    assert_eq!(resumed.name, tenant.name);
}
