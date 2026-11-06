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
async fn stopped_payments_keep_their_keys_until_forgotten() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    cmd.configure_payments(&alice, &tenant.id, stripe("pk_test_123"))
        .await
        .unwrap();
    let payments_of = async |id: &str| cmd.load_tenant(id).await.unwrap().unwrap().payments;

    // Not the merchant's to keep taking: only a member allowed to.
    let err = cmd.stop_payments(&new_id(), &tenant.id).await.unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
    // Stopped, twice: the keys stay, to settle what is held or owed.
    cmd.stop_payments(&alice, &tenant.id).await.unwrap();
    cmd.stop_payments(&alice, &tenant.id).await.unwrap();
    let stopped = payments_of(&tenant.id).await.unwrap();
    assert!(stopped.is_stopped());
    assert_eq!(stopped.secret_key_sealed, vec![1, 2, 3]);

    // Saving keys again takes payments again: nothing to forget then.
    cmd.configure_payments(&alice, &tenant.id, stripe("pk_test_123"))
        .await
        .unwrap();
    assert!(!payments_of(&tenant.id).await.unwrap().is_stopped());
    assert!(
        !cmd.forget_payments("timada-dashboard", &tenant.id)
            .await
            .unwrap()
    );
    assert!(payments_of(&tenant.id).await.is_some());

    // Stopped again, then forgotten once nothing is held: once.
    cmd.stop_payments(&alice, &tenant.id).await.unwrap();
    assert!(
        cmd.forget_payments("timada-dashboard", &tenant.id)
            .await
            .unwrap()
    );
    assert!(payments_of(&tenant.id).await.is_none());
    assert!(
        !cmd.forget_payments("timada-dashboard", &tenant.id)
            .await
            .unwrap()
    );
    // Nothing set up: stopping does nothing.
    cmd.stop_payments(&alice, &tenant.id).await.unwrap();
    assert!(payments_of(&tenant.id).await.is_none());
}

#[tokio::test]
async fn an_owner_sets_up_payments() {
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
}
