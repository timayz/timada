mod common;

use common::*;
use timada_identity::{IdentityError, OAuth2Mode, RegistrationMode};

const NOW: u64 = 1_700_000_000;

#[tokio::test]
async fn admins_grant_and_revoke_roles_but_not_their_own() {
    let cmd = command().await;
    let root = register_via_oauth2(&cmd, "google", "root").await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;

    let err = cmd.set_admin(&alice, &bob, true).await.unwrap_err();
    assert!(
        matches!(err, IdentityError::Forbidden),
        "not an admin: {err}"
    );
    let err = cmd.set_admin("ghost", &bob, true).await.unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");

    cmd.set_admin(&root, &alice, true).await.unwrap();
    let user = cmd.load_user(&alice).await.unwrap().unwrap();
    assert!(user.is_admin && user.role_assigned);
    cmd.set_admin(&root, &alice, true).await.unwrap();
    assert!(
        cmd.load_user(&alice).await.unwrap().unwrap().is_admin,
        "granting twice is a no-op"
    );

    // Alice, now an admin, can demote Root but never herself.
    let err = cmd.set_admin(&alice, &alice, false).await.unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");
    cmd.set_admin(&alice, &root, false).await.unwrap();
    assert!(!cmd.load_user(&root).await.unwrap().unwrap().is_admin);
    let err = cmd.set_admin(&root, &bob, true).await.unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "demoted: {err}");

    let err = cmd.set_admin(&alice, "ghost", true).await.unwrap_err();
    assert!(matches!(err, IdentityError::UserNotFound), "{err}");
}

#[tokio::test]
async fn a_demoted_first_user_is_not_repaired_back_to_admin() {
    let cmd = command().await;
    let root = register_via_oauth2(&cmd, "google", "root").await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    cmd.set_admin(&root, &alice, true).await.unwrap();
    cmd.set_admin(&alice, &root, false).await.unwrap();

    // Signing in runs the first-user repair; the decision must stand.
    cmd.complete_oauth2(oauth2(OAuth2Mode::Login, None, "google", "root"))
        .await
        .unwrap();
    assert!(!cmd.load_user(&root).await.unwrap().unwrap().is_admin);
}

#[tokio::test]
async fn deleting_a_user_voids_sessions_and_frees_identities() {
    let cmd = command().await;
    let root = register_via_oauth2(&cmd, "google", "root").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;
    cmd.register_passkey(register_passkey(
        RegistrationMode::AddToUser,
        Some(&bob),
        &bob,
        "cred-bob",
    ))
    .await
    .unwrap();
    let hash = [1u8; 32];
    cmd.start_session(&hash, &bob, NOW + 600, context())
        .await
        .unwrap();

    let err = cmd.delete_user(&bob, &root).await.unwrap_err();
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
