mod common;

use common::*;
use timada_tenancy::{ConfigurePayments, TenancyError};

fn stripe(publishable_key: &str) -> ConfigurePayments {
    ConfigurePayments {
        provider: "stripe".into(),
        publishable_key: publishable_key.into(),
        secret_key_sealed: vec![1, 2, 3],
        webhook_secret_sealed: vec![4, 5, 6],
    }
}

#[tokio::test]
async fn an_owner_sets_up_and_removes_payments() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;

    cmd.configure_payments(&alice, &tenant.id, stripe(" pk_test_123 "))
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    let payments = state.payments.unwrap();
    assert_eq!(payments.publishable_key, "pk_test_123");
    assert_eq!(payments.secret_key_sealed, vec![1, 2, 3]);
    assert_eq!(payments.configured_by, alice);

    let err = cmd
        .configure_payments(&alice, &tenant.id, stripe("sk_live_oops"))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvalidPublishableKey), "{err}");
    let err = cmd
        .configure_payments(&new_id(), &tenant.id, stripe("pk_test_x"))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");

    cmd.remove_payments(&alice, &tenant.id).await.unwrap();
    cmd.remove_payments(&alice, &tenant.id).await.unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.payments.is_none());
}
