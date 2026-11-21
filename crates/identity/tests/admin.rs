mod common;

use common::*;
use timada_identity::{
    AddEmail, ChallengePurpose, IdentityError, OAuth2Mode, ROOT_USER_ID, RootAccount, SignInKind,
    hash_password,
};

#[tokio::test]
async fn root_signs_in_with_the_configured_pair_and_no_code() {
    let cmd = command().await;
    let root = root_account();

    for (email, password) in [
        (ROOT_EMAIL, "not the password"),
        ("someone@example.com", ROOT_PASSWORD),
        ("not an address", ROOT_PASSWORD),
    ] {
        let err = cmd
            .sign_in_root(&root, root_sign_in(email, password, None))
            .await
            .unwrap_err();
        assert!(
            matches!(err, IdentityError::InvalidCredentials),
            "{email}: {err}"
        );
    }
    assert!(
        cmd.load_user(ROOT_USER_ID).await.unwrap().is_none(),
        "a refused pair writes nothing"
    );

    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let done = cmd
        .sign_in_root(
            &root,
            root_sign_in(" Root@Example.com ", ROOT_PASSWORD, Some(&alice)),
        )
        .await
        .unwrap();
    assert_eq!(done.user_id, ROOT_USER_ID);
    assert_eq!(done.label, ROOT_EMAIL);
    assert_eq!(done.switched_from.as_deref(), Some(alice.as_str()));
    let user = cmd.load_user(ROOT_USER_ID).await.unwrap().unwrap();
    assert!(user.is_root() && user.is_admin);
    assert_eq!(user.account, ROOT_EMAIL);
    assert_eq!(user.label, ROOT_EMAIL);
    assert!(
        user.password.is_none(),
        "the hash stays in the configuration"
    );
    assert_eq!(user.sign_in_methods(), 0);
    assert_eq!(user.last_sign_in().unwrap().method, SignInKind::Password);
    assert!(!cmd.load_user(&alice).await.unwrap().unwrap().is_root());

    let again = cmd
        .sign_in_root(
            &root,
            root_sign_in(ROOT_EMAIL, ROOT_PASSWORD, Some(ROOT_USER_ID)),
        )
        .await
        .unwrap();
    assert_eq!(again.switched_from, None);
    let user = cmd.load_user(ROOT_USER_ID).await.unwrap().unwrap();
    assert_eq!(user.sign_ins.len(), 2);
    assert_eq!(user.aggregate_version, 3, "established once");
}

#[tokio::test]
async fn root_follows_the_configured_address_and_password() {
    let cmd = command().await;
    sign_in_root(&cmd).await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    cmd.set_admin(ROOT_USER_ID, &alice, true).await.unwrap();

    let moved = RootAccount::new(
        "owner@example.com",
        &hash_password("another password").await.unwrap(),
    )
    .unwrap();
    for (email, password) in [
        (ROOT_EMAIL, ROOT_PASSWORD),
        ("owner@example.com", ROOT_PASSWORD),
        (ROOT_EMAIL, "another password"),
    ] {
        let err = cmd
            .sign_in_root(&moved, root_sign_in(email, password, None))
            .await
            .unwrap_err();
        assert!(
            matches!(err, IdentityError::InvalidCredentials),
            "{email}: {err}"
        );
    }
    cmd.sign_in_root(
        &moved,
        root_sign_in("owner@example.com", "another password", None),
    )
    .await
    .unwrap();
    let user = cmd.load_user(ROOT_USER_ID).await.unwrap().unwrap();
    assert_eq!(user.account, "owner@example.com");
    assert_eq!(user.sign_ins.len(), 2, "the same account");
    assert!(
        cmd.load_user(&alice).await.unwrap().unwrap().is_admin,
        "what root decided stands"
    );
}

#[tokio::test]
async fn only_root_makes_and_unmakes_administrators() {
    let cmd = command().await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;

    // Nobody administers before root says so, root's account written or not.
    let err = cmd.set_admin(&alice, &bob, true).await.unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");
    let err = cmd.set_admin(ROOT_USER_ID, &bob, true).await.unwrap_err();
    assert!(
        matches!(err, IdentityError::Forbidden),
        "root never signed in: {err}"
    );
    let root = sign_in_root(&cmd).await;
    let err = cmd.set_admin("ghost", &bob, true).await.unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");

    cmd.set_admin(&root, &alice, true).await.unwrap();
    assert!(cmd.load_user(&alice).await.unwrap().unwrap().is_admin);
    cmd.set_admin(&root, &alice, true).await.unwrap();
    assert!(
        cmd.load_user(&alice).await.unwrap().unwrap().is_admin,
        "granting twice is a no-op"
    );

    // An administrator makes none, unmakes none, and never root.
    for (target, admin) in [(&bob, true), (&alice, false), (&root, false)] {
        let err = cmd.set_admin(&alice, target, admin).await.unwrap_err();
        assert!(matches!(err, IdentityError::Forbidden), "{target}: {err}");
    }
    // Root's own role is not one to change.
    for admin in [false, true] {
        let err = cmd.set_admin(&root, &root, admin).await.unwrap_err();
        assert!(matches!(err, IdentityError::Forbidden), "{err}");
    }
    assert!(cmd.load_user(&root).await.unwrap().unwrap().is_admin);

    cmd.set_admin(&root, &alice, false).await.unwrap();
    assert!(!cmd.load_user(&alice).await.unwrap().unwrap().is_admin);
    let err = cmd.set_admin(&root, "ghost", true).await.unwrap_err();
    assert!(matches!(err, IdentityError::UserNotFound), "{err}");
}

#[tokio::test]
async fn administrators_delete_members_and_root_deletes_administrators() {
    let cmd = command().await;
    let root = sign_in_root(&cmd).await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;
    let carol = register_via_oauth2(&cmd, "google", "carol").await;
    cmd.set_admin(&root, &alice, true).await.unwrap();
    cmd.set_admin(&root, &bob, true).await.unwrap();

    for target in [&root, &bob, &alice] {
        let err = cmd.delete_user(&alice, target).await.unwrap_err();
        assert!(matches!(err, IdentityError::Forbidden), "{target}: {err}");
    }
    assert!(cmd.load_user(&bob).await.unwrap().is_some());
    cmd.delete_user(&alice, &carol).await.unwrap();
    assert!(cmd.load_user(&carol).await.unwrap().is_none());

    cmd.delete_user(&root, &bob).await.unwrap();
    assert!(cmd.load_user(&bob).await.unwrap().is_none());
    assert!(cmd.load_user(&root).await.unwrap().is_some());
}

#[tokio::test]
async fn root_takes_no_other_way_to_sign_in() {
    let cmd = command().await;
    let root = sign_in_root(&cmd).await;

    let err = cmd
        .add_passkey(add_passkey(Some(&root), &root, "cred-root"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");
    assert!(cmd.load_passkey("cred-root").await.unwrap().is_none());

    let err = cmd
        .complete_oauth2(oauth2(OAuth2Mode::AddToUser, Some(&root), "google", "r"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");
    assert!(
        cmd.load_oauth2_account("google", "r")
            .await
            .unwrap()
            .is_none()
    );

    let issued = cmd
        .issue_email_code("other@example.com", ChallengePurpose::Register, NOW)
        .await
        .unwrap();
    let err = cmd
        .add_email(
            AddEmail {
                session_user_id: Some(root.clone()),
                bound_user_id: root.clone(),
                email: "other@example.com".to_owned(),
                code: issued.code,
                password_hash: hash_password(PASSWORD).await.unwrap(),
            },
            NOW,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");
    assert_eq!(
        cmd.load_user(&root)
            .await
            .unwrap()
            .unwrap()
            .sign_in_methods(),
        0
    );
}

#[tokio::test]
async fn deleting_a_user_voids_sessions_and_frees_identities() {
    let cmd = command().await;
    let root = sign_in_root(&cmd).await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;
    cmd.add_passkey(add_passkey(Some(&bob), &bob, "cred-bob"))
        .await
        .unwrap();
    let hash = [1u8; 32];
    cmd.start_session(&hash, &bob, NOW + 600, context())
        .await
        .unwrap();

    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let err = cmd.delete_user(&bob, &alice).await.unwrap_err();
    assert!(
        matches!(err, IdentityError::Forbidden),
        "not an admin: {err}"
    );
    let err = cmd.delete_user(&root, &root).await.unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "self: {err}");
    let err = cmd.delete_user(&root, "ghost").await.unwrap_err();
    assert!(matches!(err, IdentityError::UserNotFound), "{err}");

    cmd.delete_user(&root, &bob).await.unwrap();
    assert!(cmd.load_user(&bob).await.unwrap().is_none());
    assert!(cmd.load_user_any(&bob).await.unwrap().unwrap().deleted);
    assert!(cmd.resolve_session(&hash, NOW).await.unwrap().is_none());
    assert!(
        cmd.load_oauth2_account("google", "bob")
            .await
            .unwrap()
            .is_none()
    );
    assert!(cmd.load_passkey("cred-bob").await.unwrap().is_none());

    // The person registers again with the same identity: a new user.
    let outcome = cmd
        .complete_oauth2(oauth2(OAuth2Mode::CreateUserOrLogin, None, "google", "bob"))
        .await
        .unwrap();
    assert!(outcome.created);
    assert_ne!(outcome.user_id, bob);

    // Deleting again only retries freeing the index entries.
    cmd.delete_user(&root, &bob).await.unwrap();
    assert_eq!(
        cmd.load_oauth2_account("google", "bob")
            .await
            .unwrap()
            .unwrap()
            .user_id,
        outcome.user_id,
        "the new owner keeps the identity"
    );
}

#[tokio::test]
async fn deleting_an_email_user_frees_the_address() {
    let cmd = command().await;
    let root = sign_in_root(&cmd).await;
    let carol = register_via_email(&cmd, "carol@example.com").await;

    cmd.delete_user(&root, &carol).await.unwrap();
    assert!(
        cmd.load_email_account("carol@example.com")
            .await
            .unwrap()
            .is_none()
    );
    let err = cmd
        .check_password("carol@example.com", PASSWORD)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidCredentials), "{err}");

    // The person registers again with the same address: a new user.
    let again = register_via_email(&cmd, "carol@example.com").await;
    assert_ne!(again, carol);
    assert_eq!(
        cmd.load_email_account("carol@example.com")
            .await
            .unwrap()
            .unwrap()
            .user_id,
        again
    );
    // Deleting the old record again leaves the new owner alone.
    cmd.delete_user(&root, &carol).await.unwrap();
    assert_eq!(
        cmd.load_email_account("carol@example.com")
            .await
            .unwrap()
            .unwrap()
            .user_id,
        again
    );
}
