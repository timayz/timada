mod common;

use common::*;
use timada_identity::{IdentityError, RegistrationMode, SignInMethod};

#[tokio::test]
async fn create_user_registers_signs_in_and_grants_admin() {
    let cmd = command().await;
    let bound = new_id();
    let outcome = cmd
        .register_passkey(register_passkey(
            RegistrationMode::CreateUser,
            None,
            &bound,
            "cred-1",
        ))
        .await
        .unwrap();
    assert!(outcome.created);
    assert_eq!(outcome.user_id, bound);

    let user = cmd.load_user(&bound).await.unwrap().unwrap();
    assert_eq!(user.account, "alice");
    assert_eq!(user.passkeys.len(), 1);
    assert_eq!(user.passkeys[0].credential_id, "cred-1");
    assert_eq!(user.last_sign_in().unwrap().method, SignInMethod::Passkey);
    assert!(user.is_admin);

    let credential = cmd.load_passkey("cred-1").await.unwrap().unwrap();
    assert_eq!(credential.user_id, bound);
}

#[tokio::test]
async fn sign_in_updates_the_credential_when_asked() {
    let cmd = command().await;
    let bound = new_id();
    cmd.register_passkey(register_passkey(
        RegistrationMode::CreateUser,
        None,
        &bound,
        "cred-1",
    ))
    .await
    .unwrap();

    let outcome = cmd
        .sign_in_passkey(sign_in_passkey("cred-1", Some("{\"counter\":1}"), None))
        .await
        .unwrap();
    assert_eq!(outcome.user_id, bound);
    assert_eq!(outcome.label, "Alice");
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
    assert_eq!(
        cmd.load_user(&bound).await.unwrap().unwrap().sign_ins.len(),
        3
    );

    let err = cmd
        .sign_in_passkey(sign_in_passkey("unknown", None, None))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::UnknownCredential), "{err}");
}

#[tokio::test]
async fn create_user_refuses_sessions_duplicates_and_replays() {
    let cmd = command().await;
    let bound = new_id();
    cmd.register_passkey(register_passkey(
        RegistrationMode::CreateUser,
        None,
        &bound,
        "cred-1",
    ))
    .await
    .unwrap();

    let err = cmd
        .register_passkey(register_passkey(
            RegistrationMode::CreateUser,
            Some(&bound),
            &new_id(),
            "cred-2",
        ))
        .await
        .unwrap_err();
    assert!(
        matches!(err, IdentityError::UnexpectedlyAuthorized),
        "{err}"
    );

    let err = cmd
        .register_passkey(register_passkey(
            RegistrationMode::CreateUser,
            None,
            &new_id(),
            "cred-1",
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err}");

    // Replaying the finish with the same bound id is not a second user.
    let err = cmd
        .register_passkey(register_passkey(
            RegistrationMode::CreateUser,
            None,
            &bound,
            "cred-3",
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidState), "{err}");
}

#[tokio::test]
async fn add_to_user_checks_the_session_it_was_started_in() {
    let cmd = command().await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;

    let err = cmd
        .register_passkey(register_passkey(
            RegistrationMode::AddToUser,
            None,
            &alice,
            "cred-1",
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::Unauthorized), "{err}");

    // Started as Alice, finished while signed in as Bob.
    let err = cmd
        .register_passkey(register_passkey(
            RegistrationMode::AddToUser,
            Some(&bob),
            &alice,
            "cred-1",
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::SessionMismatch), "{err}");

    let outcome = cmd
        .register_passkey(register_passkey(
            RegistrationMode::AddToUser,
            Some(&alice),
            &alice,
            "cred-1",
        ))
        .await
        .unwrap();
    assert!(!outcome.created);
    let user = cmd.load_user(&alice).await.unwrap().unwrap();
    assert_eq!(user.sign_in_methods(), 2);
    assert_eq!(user.sign_ins.len(), 1, "adding a passkey is not a sign-in");

    // Bob cannot register Alice's credential.
    let err = cmd
        .register_passkey(register_passkey(
            RegistrationMode::AddToUser,
            Some(&bob),
            &bob,
            "cred-1",
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err}");
}

#[tokio::test]
async fn rename_and_remove_respect_ownership_and_the_last_method() {
    let cmd = command().await;
    let bound = new_id();
    cmd.register_passkey(register_passkey(
        RegistrationMode::CreateUser,
        None,
        &bound,
        "cred-1",
    ))
    .await
    .unwrap();
    let bob = register_via_oauth2(&cmd, "google", "bob").await;

    cmd.rename_passkey(&bound, "cred-1", "Laptop")
        .await
        .unwrap();
    assert_eq!(
        cmd.load_user(&bound).await.unwrap().unwrap().passkeys[0].name,
        "Laptop"
    );
    let err = cmd
        .rename_passkey(&bob, "cred-1", "Mine")
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");

    let err = cmd.remove_passkey(&bound, "cred-1").await.unwrap_err();
    assert!(matches!(err, IdentityError::LastSignInMethod), "{err}");

    cmd.register_passkey(register_passkey(
        RegistrationMode::AddToUser,
        Some(&bound),
        &bound,
        "cred-2",
    ))
    .await
    .unwrap();
    let err = cmd.remove_passkey(&bob, "cred-1").await.unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");
    cmd.remove_passkey(&bound, "cred-1").await.unwrap();

    let user = cmd.load_user(&bound).await.unwrap().unwrap();
    assert_eq!(user.passkeys.len(), 1);
    assert!(cmd.load_passkey("cred-1").await.unwrap().is_none());
    let err = cmd
        .sign_in_passkey(sign_in_passkey("cred-1", None, None))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::UnknownCredential), "{err}");

    // The removed credential id can be registered again.
    cmd.register_passkey(register_passkey(
        RegistrationMode::AddToUser,
        Some(&bound),
        &bound,
        "cred-1",
    ))
    .await
    .unwrap();
    assert_eq!(
        cmd.load_user(&bound).await.unwrap().unwrap().passkeys.len(),
        2
    );
}

#[tokio::test]
async fn passkeys_per_user_are_capped() {
    let cmd = command().await;
    let bound = new_id();
    cmd.register_passkey(register_passkey(
        RegistrationMode::CreateUser,
        None,
        &bound,
        "cred-0",
    ))
    .await
    .unwrap();
    for i in 1..timada_identity::MAX_PASSKEYS_PER_USER {
        cmd.register_passkey(register_passkey(
            RegistrationMode::AddToUser,
            Some(&bound),
            &bound,
            &format!("cred-{i}"),
        ))
        .await
        .unwrap();
    }
    let err = cmd
        .register_passkey(register_passkey(
            RegistrationMode::AddToUser,
            Some(&bound),
            &bound,
            "one-too-many",
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::TooManyPasskeys), "{err}");
}

#[tokio::test]
async fn signing_in_with_another_users_passkey_switches_accounts() {
    let cmd = command().await;
    let alice = new_id();
    cmd.register_passkey(register_passkey(
        RegistrationMode::CreateUser,
        None,
        &alice,
        "cred-alice",
    ))
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
