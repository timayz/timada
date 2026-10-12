mod common;

use common::*;
use timada_identity::{IdentityError, SignInKind};

#[tokio::test]
async fn an_added_passkey_signs_the_user_in() {
    let cmd = command().await;
    let alice = register_via_email(&cmd, "alice@example.com").await;
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap();

    let user = cmd.load_user(&alice).await.unwrap().unwrap();
    assert_eq!(user.passkeys.len(), 1);
    assert_eq!(user.passkeys[0].credential_id, "cred-1");
    assert_eq!(user.sign_in_methods(), 2);
    assert_eq!(user.sign_ins.len(), 1, "adding a passkey is not a sign-in");
    let credential = cmd.load_passkey("cred-1").await.unwrap().unwrap();
    assert_eq!(credential.user_id, alice);

    let outcome = cmd
        .sign_in_passkey(sign_in_passkey("cred-1", Some("{\"counter\":1}"), None))
        .await
        .unwrap();
    assert_eq!(outcome.user_id, alice);
    assert_eq!(outcome.label, "alice@example.com");
    assert_eq!(outcome.switched_from, None);
    assert_eq!(
        cmd.load_passkey("cred-1")
            .await
            .unwrap()
            .unwrap()
            .passkey_json,
        "{\"counter\":1}"
    );
    cmd.sign_in_passkey(sign_in_passkey("cred-1", None, None))
        .await
        .unwrap();
    let user = cmd.load_user(&alice).await.unwrap().unwrap();
    assert_eq!(user.sign_ins.len(), 3);
    assert_eq!(user.last_sign_in().unwrap().method, SignInKind::Passkey);

    let err = cmd
        .sign_in_passkey(sign_in_passkey("unknown", None, None))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::UnknownCredential), "{err}");
}

#[tokio::test]
async fn adding_checks_the_session_it_was_started_in() {
    let cmd = command().await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;

    let err = cmd
        .add_passkey(add_passkey(None, &alice, "cred-1"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::Unauthorized), "{err}");

    // Started as Alice, finished while signed in as Bob.
    let err = cmd
        .add_passkey(add_passkey(Some(&bob), &alice, "cred-1"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::SessionMismatch), "{err}");

    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap();
    let user = cmd.load_user(&alice).await.unwrap().unwrap();
    assert_eq!(user.sign_in_methods(), 2);

    // Bob cannot register Alice's credential.
    let err = cmd
        .add_passkey(add_passkey(Some(&bob), &bob, "cred-1"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err}");

    // Adding the same credential twice is idempotent for its owner.
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap();
    assert_eq!(
        cmd.load_user(&alice).await.unwrap().unwrap().passkeys.len(),
        1
    );
}

#[tokio::test]
async fn rename_and_remove_respect_ownership_and_the_last_method() {
    let cmd = command().await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap();

    cmd.rename_passkey(&alice, "cred-1", "Laptop")
        .await
        .unwrap();
    assert_eq!(
        cmd.load_user(&alice).await.unwrap().unwrap().passkeys[0].name,
        "Laptop"
    );
    let err = cmd
        .rename_passkey(&bob, "cred-1", "Mine")
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");

    // The provider account goes; the passkey is now the only method.
    cmd.unlink_oauth2(&alice, "google", "alice").await.unwrap();
    let err = cmd.remove_passkey(&alice, "cred-1").await.unwrap_err();
    assert!(matches!(err, IdentityError::LastSignInMethod), "{err}");

    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-2"))
        .await
        .unwrap();
    let err = cmd.remove_passkey(&bob, "cred-1").await.unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");
    cmd.remove_passkey(&alice, "cred-1").await.unwrap();

    let user = cmd.load_user(&alice).await.unwrap().unwrap();
    assert_eq!(user.passkeys.len(), 1);
    assert!(cmd.load_passkey("cred-1").await.unwrap().is_none());
    let err = cmd
        .sign_in_passkey(sign_in_passkey("cred-1", None, None))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::UnknownCredential), "{err}");

    // The removed credential id can be registered again.
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap();
    assert_eq!(
        cmd.load_user(&alice).await.unwrap().unwrap().passkeys.len(),
        2
    );
}

#[tokio::test]
async fn passkeys_per_user_are_capped() {
    let cmd = command().await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    for i in 0..timada_identity::MAX_PASSKEYS_PER_USER {
        cmd.add_passkey(add_passkey(Some(&alice), &alice, &format!("cred-{i}")))
            .await
            .unwrap();
    }
    let err = cmd
        .add_passkey(add_passkey(Some(&alice), &alice, "one-too-many"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::TooManyPasskeys), "{err}");
}

#[tokio::test]
async fn signing_in_with_another_users_passkey_switches_accounts() {
    let cmd = command().await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-alice"))
        .await
        .unwrap();
    let bob = register_via_oauth2(&cmd, "google", "bob").await;

    let outcome = cmd
        .sign_in_passkey(sign_in_passkey("cred-alice", None, Some(&bob)))
        .await
        .unwrap();
    assert_eq!(outcome.user_id, alice);
    assert_eq!(outcome.switched_from.as_deref(), Some(bob.as_str()));

    let outcome = cmd
        .sign_in_passkey(sign_in_passkey("cred-alice", None, Some(&alice)))
        .await
        .unwrap();
    assert_eq!(outcome.switched_from, None, "own passkey: plain re-sign-in");
}
