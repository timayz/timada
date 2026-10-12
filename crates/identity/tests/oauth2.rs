mod common;

use common::*;
use timada_identity::{IdentityError, OAuth2Mode, SignInKind};

#[tokio::test]
async fn create_user_or_login_registers_then_signs_in() {
    let cmd = command().await;

    let first = cmd
        .complete_oauth2(oauth2(OAuth2Mode::CreateUserOrLogin, None, "google", "1"))
        .await
        .unwrap();
    assert!(first.created);
    assert!(!first.linked);

    let user = cmd.load_user(&first.user_id).await.unwrap().unwrap();
    assert_eq!(user.account, "1@example.com");
    assert_eq!(user.label, "User 1");
    assert_eq!(user.oauth2_accounts.len(), 1);
    assert_eq!(user.sign_in_methods(), 1);
    assert_eq!(user.sign_ins.len(), 1);
    assert_eq!(user.last_sign_in().unwrap().method, SignInKind::OAuth2);
    assert!(user.is_admin, "the first user administers the instance");

    let again = cmd
        .complete_oauth2(oauth2(OAuth2Mode::CreateUserOrLogin, None, "google", "1"))
        .await
        .unwrap();
    assert_eq!(again.user_id, first.user_id);
    assert!(!again.created);
    let user = cmd.load_user(&first.user_id).await.unwrap().unwrap();
    assert_eq!(user.sign_ins.len(), 2);

    let index = cmd
        .load_oauth2_account("google", "1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(index.user_id, first.user_id);
}

#[tokio::test]
async fn second_user_is_not_admin() {
    let cmd = command().await;
    let first = register_via_oauth2(&cmd, "google", "1").await;
    let second = register_via_oauth2(&cmd, "google", "2").await;
    assert!(cmd.load_user(&first).await.unwrap().unwrap().is_admin);
    assert!(!cmd.load_user(&second).await.unwrap().unwrap().is_admin);
}

#[tokio::test]
async fn login_requires_a_registered_identity() {
    let cmd = command().await;
    let err = cmd
        .complete_oauth2(oauth2(OAuth2Mode::Login, None, "google", "ghost"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::NotRegistered), "{err}");

    let user_id = register_via_oauth2(&cmd, "google", "1").await;
    let outcome = cmd
        .complete_oauth2(oauth2(OAuth2Mode::Login, None, "google", "1"))
        .await
        .unwrap();
    assert_eq!(outcome.user_id, user_id);
}

#[tokio::test]
async fn create_user_rejects_a_registered_identity() {
    let cmd = command().await;
    register_via_oauth2(&cmd, "google", "1").await;
    let err = cmd
        .complete_oauth2(oauth2(OAuth2Mode::CreateUser, None, "google", "1"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err}");
}

#[tokio::test]
async fn signing_in_while_signed_in_switches_or_re_signs_in() {
    let cmd = command().await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;

    // Bob's identity from Alice's browser: Bob signs in, Alice's session goes.
    for mode in [OAuth2Mode::Login, OAuth2Mode::CreateUserOrLogin] {
        let outcome = cmd
            .complete_oauth2(oauth2(mode, Some(&alice), "google", "bob"))
            .await
            .unwrap();
        assert_eq!(outcome.user_id, bob, "{mode:?}");
        assert_eq!(outcome.label, "User bob");
        assert_eq!(outcome.switched_from.as_deref(), Some(alice.as_str()));
        assert!(!outcome.created && !outcome.linked);
    }
    assert_eq!(
        cmd.load_user(&bob).await.unwrap().unwrap().sign_ins.len(),
        3
    );

    // Her own identity: a plain re-sign-in.
    let outcome = cmd
        .complete_oauth2(oauth2(OAuth2Mode::Login, Some(&alice), "google", "alice"))
        .await
        .unwrap();
    assert_eq!(outcome.user_id, alice);
    assert_eq!(outcome.switched_from, None);
    assert_eq!(
        cmd.load_user(&alice).await.unwrap().unwrap().sign_ins.len(),
        2
    );
}

#[tokio::test]
async fn creating_an_account_while_signed_in_is_refused() {
    let cmd = command().await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    for mode in [OAuth2Mode::CreateUser, OAuth2Mode::CreateUserOrLogin] {
        let err = cmd
            .complete_oauth2(oauth2(mode, Some(&alice), "google", "new"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, IdentityError::UnexpectedlyAuthorized),
            "{mode:?}: {err}"
        );
    }
    assert!(
        cmd.load_oauth2_account("google", "new")
            .await
            .unwrap()
            .is_none()
    );

    let err = cmd
        .complete_oauth2(oauth2(
            OAuth2Mode::CreateUser,
            Some(&alice),
            "google",
            "alice",
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err}");
    let err = cmd
        .complete_oauth2(oauth2(OAuth2Mode::Login, Some(&alice), "google", "ghost"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::NotRegistered), "{err}");
    let err = cmd
        .complete_oauth2(oauth2(OAuth2Mode::AddToUser, None, "google", "2"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::Unauthorized), "{err}");
}

#[tokio::test]
async fn add_to_user_links_idempotently_and_refuses_other_users_identities() {
    let cmd = command().await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;

    let linked = cmd
        .complete_oauth2(oauth2(
            OAuth2Mode::AddToUser,
            Some(&alice),
            "auth0",
            "alice",
        ))
        .await
        .unwrap();
    assert!(linked.linked);
    assert_eq!(linked.user_id, alice);
    let user = cmd.load_user(&alice).await.unwrap().unwrap();
    assert_eq!(user.sign_in_methods(), 2);
    assert_eq!(user.sign_ins.len(), 1, "linking is not a sign-in");

    // Same identity again: nothing changes.
    cmd.complete_oauth2(oauth2(
        OAuth2Mode::AddToUser,
        Some(&alice),
        "auth0",
        "alice",
    ))
    .await
    .unwrap();
    assert_eq!(
        cmd.load_user(&alice)
            .await
            .unwrap()
            .unwrap()
            .sign_in_methods(),
        2
    );

    // Bob cannot take Alice's identity.
    let err = cmd
        .complete_oauth2(oauth2(OAuth2Mode::AddToUser, Some(&bob), "auth0", "alice"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, IdentityError::AlreadyLinkedToOtherUser),
        "{err}"
    );

    // Signing in with the linked identity lands on Alice.
    let outcome = cmd
        .complete_oauth2(oauth2(OAuth2Mode::Login, None, "auth0", "alice"))
        .await
        .unwrap();
    assert_eq!(outcome.user_id, alice);
}

#[tokio::test]
async fn unlink_keeps_one_sign_in_method_and_frees_the_identity() {
    let cmd = command().await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;

    let err = cmd
        .unlink_oauth2(&alice, "google", "alice")
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::LastSignInMethod), "{err}");

    cmd.complete_oauth2(oauth2(
        OAuth2Mode::AddToUser,
        Some(&alice),
        "auth0",
        "alice",
    ))
    .await
    .unwrap();
    let err = cmd.unlink_oauth2(&bob, "auth0", "alice").await.unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");

    cmd.unlink_oauth2(&alice, "auth0", "alice").await.unwrap();
    let user = cmd.load_user(&alice).await.unwrap().unwrap();
    assert_eq!(user.sign_in_methods(), 1);
    assert!(
        cmd.load_oauth2_account("auth0", "alice")
            .await
            .unwrap()
            .is_none()
    );

    // Bob can now link it, and the index points at him.
    cmd.complete_oauth2(oauth2(OAuth2Mode::AddToUser, Some(&bob), "auth0", "alice"))
        .await
        .unwrap();
    let index = cmd
        .load_oauth2_account("auth0", "alice")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(index.user_id, bob);
}

#[tokio::test]
async fn concurrent_registrations_of_one_identity_create_one_user() {
    let cmd = command().await;
    let (a, b) = tokio::join!(
        cmd.complete_oauth2(oauth2(OAuth2Mode::CreateUser, None, "google", "1")),
        cmd.complete_oauth2(oauth2(OAuth2Mode::CreateUser, None, "google", "1")),
    );
    let (ok, err) = match (a, b) {
        (Ok(ok), Err(err)) | (Err(err), Ok(ok)) => (ok, err),
        other => panic!("exactly one registration must win: {other:?}"),
    };
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err}");
    let index = cmd
        .load_oauth2_account("google", "1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(index.user_id, ok.user_id);
}

#[tokio::test]
async fn deterministic_ids_ignore_the_default_routing_key() {
    let cmd = timada_identity::Command(
        evento::Evento::new(executor().await).default_routing_key("eu-west-1"),
    );
    let user_id = register_via_oauth2(&cmd, "google", "1").await;
    let outcome = cmd
        .complete_oauth2(oauth2(OAuth2Mode::Login, None, "google", "1"))
        .await
        .unwrap();
    assert_eq!(outcome.user_id, user_id);
    assert!(cmd.load_user(&user_id).await.unwrap().unwrap().is_admin);
}

#[tokio::test]
async fn update_profile_changes_account_and_label() {
    let cmd = command().await;
    let user_id = register_via_oauth2(&cmd, "google", "1").await;
    cmd.update_profile(&user_id, "alice", "Alice")
        .await
        .unwrap();
    cmd.update_profile(&user_id, "alice", "Alice")
        .await
        .unwrap();
    let user = cmd.load_user(&user_id).await.unwrap().unwrap();
    assert_eq!(
        (user.account.as_str(), user.label.as_str()),
        ("alice", "Alice")
    );
    assert!(matches!(
        cmd.update_profile(&user_id, "", "x").await.unwrap_err(),
        IdentityError::InvalidState
    ));
}
