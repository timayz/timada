mod common;

use common::*;
use timada_identity::{
    AddEmail, CODE_RESEND_AFTER_SECS, CODE_TTL_SECS, ChallengePurpose, ChangePassword,
    IdentityError, RegisterWithPassword, ResetPassword, SignInKind, SignInWithPassword,
    hash_password,
};

fn registration(email: &str, code: &str, hash: &str) -> RegisterWithPassword {
    RegisterWithPassword {
        email: email.to_owned(),
        code: code.to_owned(),
        password_hash: hash.to_owned(),
        context: context(),
    }
}

fn sign_in(email: &str, user_id: &str, code: &str, session: Option<&str>) -> SignInWithPassword {
    SignInWithPassword {
        email: email.to_owned(),
        user_id: user_id.to_owned(),
        code: code.to_owned(),
        session_user_id: session.map(str::to_owned),
        context: context(),
    }
}

#[tokio::test]
async fn registering_proves_the_address_and_signs_in() {
    let cmd = command().await;
    let hash = hash_password(PASSWORD).await.unwrap();
    let issued = cmd
        .issue_email_code(" Alice@Example.COM ", ChallengePurpose::Register, NOW)
        .await
        .unwrap();
    assert_eq!(issued.email, "alice@example.com", "normalized");
    assert_eq!(issued.code.len(), 6);
    assert!(
        !format!("{issued:?}").contains(&issued.code),
        "the code never leaks through Debug"
    );

    let outcome = cmd
        .register_with_password(
            registration("ALICE@example.com", &issued.code, &hash),
            NOW + 1,
        )
        .await
        .unwrap();
    assert_eq!(outcome.label, "alice@example.com");

    let user = cmd.load_user(&outcome.user_id).await.unwrap().unwrap();
    assert_eq!(user.account, "alice@example.com");
    assert_eq!(user.label, "alice@example.com");
    assert_eq!(user.password.as_ref().unwrap().email, "alice@example.com");
    assert_eq!(user.password.as_ref().unwrap().password_hash, hash);
    assert_eq!(user.sign_in_methods(), 1);
    assert_eq!(user.sign_ins.len(), 1);
    assert_eq!(user.last_sign_in().unwrap().method, SignInKind::Password);
    assert!(user.is_admin, "first user");
    assert_eq!(
        cmd.load_email_account("alice@example.com")
            .await
            .unwrap()
            .unwrap()
            .user_id,
        outcome.user_id
    );

    // The code is burnt.
    let err = cmd
        .register_with_password(
            registration("alice@example.com", &issued.code, &hash),
            NOW + 2,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidCode), "{err}");
    // The address is taken, for a new code too.
    let err = cmd
        .issue_email_code("alice@example.com", ChallengePurpose::Register, NOW + 100)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err}");

    let other = register_via_email(&cmd, "bob@example.com").await;
    assert!(!cmd.load_user(&other).await.unwrap().unwrap().is_admin);
}

#[tokio::test]
async fn codes_expire_and_wrong_guesses_are_capped() {
    let cmd = command().await;
    let hash = hash_password(PASSWORD).await.unwrap();
    let issued = cmd
        .issue_email_code("alice@example.com", ChallengePurpose::Register, NOW)
        .await
        .unwrap();
    let wrong = if issued.code == "000000" {
        "000001"
    } else {
        "000000"
    };

    for attempt in 1..5 {
        let err = cmd
            .register_with_password(registration("alice@example.com", wrong, &hash), NOW + 1)
            .await
            .unwrap_err();
        assert!(
            matches!(err, IdentityError::InvalidCode),
            "{attempt}: {err}"
        );
    }
    let err = cmd
        .register_with_password(registration("alice@example.com", wrong, &hash), NOW + 1)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::TooManyAttempts), "{err}");
    // Even the right code is dead now.
    let err = cmd
        .register_with_password(
            registration("alice@example.com", &issued.code, &hash),
            NOW + 1,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::TooManyAttempts), "{err}");
    // And a new one cannot be rushed.
    let err = cmd
        .issue_email_code("alice@example.com", ChallengePurpose::Register, NOW + 30)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::CodeRecentlySent), "{err}");

    let issued = cmd
        .issue_email_code(
            "alice@example.com",
            ChallengePurpose::Register,
            NOW + CODE_RESEND_AFTER_SECS,
        )
        .await
        .unwrap();
    let late = NOW + CODE_RESEND_AFTER_SECS + CODE_TTL_SECS;
    let err = cmd
        .register_with_password(registration("alice@example.com", &issued.code, &hash), late)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::CodeExpired), "{err}");
    // An expired code does not throttle the next one; the old code is gone.
    let fresh = cmd
        .issue_email_code("alice@example.com", ChallengePurpose::Register, late)
        .await
        .unwrap();
    let err = cmd
        .register_with_password(
            registration("alice@example.com", &issued.code, &hash),
            late + 1,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidCode), "{err}");
    cmd.register_with_password(
        registration("alice@example.com", &fresh.code, &hash),
        late + 1,
    )
    .await
    .unwrap();

    // Unknown address: no code was ever issued.
    let err = cmd
        .register_with_password(registration("nobody@example.com", "123456", &hash), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidCode), "{err}");
    let err = cmd
        .issue_email_code("not an address", ChallengePurpose::Register, NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidEmail), "{err}");
}

#[tokio::test]
async fn a_code_only_serves_its_purpose() {
    let cmd = command().await;
    let alice = register_via_email(&cmd, "alice@example.com").await;
    let hash = hash_password("another password").await.unwrap();

    // Signing in right after registering is not throttled: that code is burnt.
    let issued = cmd
        .issue_email_code("alice@example.com", ChallengePurpose::SignIn, NOW + 1)
        .await
        .unwrap();
    let err = cmd
        .reset_password(
            ResetPassword {
                email: "alice@example.com".to_owned(),
                code: issued.code.clone(),
                password_hash: hash,
                context: context(),
            },
            NOW + 2,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidCode), "{err}");
    assert_eq!(
        cmd.load_email_challenge("alice@example.com")
            .await
            .unwrap()
            .unwrap()
            .failed_attempts,
        0,
        "a purpose mismatch is not a guess"
    );
    cmd.sign_in_with_password(
        sign_in("alice@example.com", &alice, &issued.code, None),
        NOW + 3,
    )
    .await
    .unwrap();

    for purpose in [ChallengePurpose::SignIn, ChallengePurpose::ResetPassword] {
        let err = cmd
            .issue_email_code("nobody@example.com", purpose, NOW)
            .await
            .unwrap_err();
        assert!(matches!(err, IdentityError::NotRegistered), "{err}");
    }
}

#[tokio::test]
async fn passwords_are_checked_then_the_code_signs_in() {
    let cmd = command().await;
    let alice = register_via_email(&cmd, "alice@example.com").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;

    let owner = cmd
        .check_password("Alice@example.com", PASSWORD)
        .await
        .unwrap();
    assert_eq!(owner.user_id, alice);
    assert_eq!(owner.label, "alice@example.com");
    for (email, password) in [
        ("alice@example.com", "wrong password"),
        ("nobody@example.com", PASSWORD),
        ("bob@example.com", PASSWORD),
        ("not an address", PASSWORD),
    ] {
        let err = cmd.check_password(email, password).await.unwrap_err();
        assert!(
            matches!(err, IdentityError::InvalidCredentials),
            "{email}: {err}"
        );
    }

    let issued = cmd
        .issue_email_code("alice@example.com", ChallengePurpose::SignIn, NOW + 1)
        .await
        .unwrap();
    let outcome = cmd
        .sign_in_with_password(
            sign_in("alice@example.com", &alice, &issued.code, Some(&bob)),
            NOW + 2,
        )
        .await
        .unwrap();
    assert_eq!(outcome.user_id, alice);
    assert_eq!(outcome.label, "alice@example.com");
    assert_eq!(outcome.switched_from.as_deref(), Some(bob.as_str()));
    let user = cmd.load_user(&alice).await.unwrap().unwrap();
    assert_eq!(user.sign_ins.len(), 2);
    assert_eq!(user.last_sign_in().unwrap().method, SignInKind::Password);
    assert_eq!(
        user.last_sign_in().unwrap().ip.as_deref(),
        Some("127.0.0.1")
    );

    // The code names another user than the address: refused, code burnt.
    let issued = cmd
        .issue_email_code(
            "alice@example.com",
            ChallengePurpose::SignIn,
            NOW + 1 + CODE_RESEND_AFTER_SECS,
        )
        .await
        .unwrap();
    let err = cmd
        .sign_in_with_password(
            sign_in("alice@example.com", &bob, &issued.code, None),
            NOW + 2 + CODE_RESEND_AFTER_SECS,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidState), "{err}");
}

#[tokio::test]
async fn a_reset_replaces_the_password() {
    let cmd = command().await;
    let alice = register_via_email(&cmd, "alice@example.com").await;
    let new_hash = hash_password("a brand new password").await.unwrap();

    let issued = cmd
        .issue_email_code(
            "alice@example.com",
            ChallengePurpose::ResetPassword,
            NOW + 1,
        )
        .await
        .unwrap();
    let outcome = cmd
        .reset_password(
            ResetPassword {
                email: "alice@example.com".to_owned(),
                code: issued.code,
                password_hash: new_hash.clone(),
                context: context(),
            },
            NOW + 2,
        )
        .await
        .unwrap();
    assert_eq!(outcome.user_id, alice);

    let user = cmd.load_user(&alice).await.unwrap().unwrap();
    assert_eq!(user.password.as_ref().unwrap().password_hash, new_hash);
    assert_eq!(user.sign_ins.len(), 2, "a reset signs in");
    let err = cmd
        .check_password("alice@example.com", PASSWORD)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidCredentials), "{err}");
    cmd.check_password("alice@example.com", "a brand new password")
        .await
        .unwrap();
}

#[tokio::test]
async fn the_password_counts_as_a_sign_in_method() {
    let cmd = command().await;
    let alice = register_via_email(&cmd, "alice@example.com").await;
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
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
    // The password stays, so the passkey can go.
    cmd.remove_passkey(&alice, "cred-1").await.unwrap();
    assert_eq!(
        cmd.load_user(&alice)
            .await
            .unwrap()
            .unwrap()
            .sign_in_methods(),
        1
    );
}

#[tokio::test]
async fn two_registrations_for_one_address_make_one_user() {
    let cmd = command().await;
    let hash = hash_password(PASSWORD).await.unwrap();
    let issued = cmd
        .issue_email_code("alice@example.com", ChallengePurpose::Register, NOW)
        .await
        .unwrap();
    // The same code submitted twice at once: one burns it, the other sees it
    // burnt (the race is on the challenge stream, never on the user).
    let (a, b) = tokio::join!(
        cmd.register_with_password(registration("alice@example.com", &issued.code, &hash), NOW),
        cmd.register_with_password(registration("alice@example.com", &issued.code, &hash), NOW),
    );
    assert_eq!(a.is_ok() as u8 + b.is_ok() as u8, 1, "{a:?} / {b:?}");
    assert!(
        cmd.load_email_challenge("alice@example.com")
            .await
            .unwrap()
            .unwrap()
            .consumed
    );
}

#[tokio::test]
async fn the_password_changes_with_the_current_one() {
    let cmd = command().await;
    let alice = register_via_email(&cmd, "alice@example.com").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;
    let new_hash = hash_password("a brand new password").await.unwrap();
    let change = |user: &str, current: &str| ChangePassword {
        user_id: user.to_owned(),
        current_password: current.to_owned(),
        new_password_hash: new_hash.clone(),
    };

    let err = cmd
        .change_password(change(&alice, "not the password"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::WrongPassword), "{err}");
    // No e-mail + password method: nothing to change.
    let err = cmd
        .change_password(change(&bob, PASSWORD))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::NotRegistered), "{err}");
    let err = cmd
        .change_password(change("nobody", PASSWORD))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::UserNotFound), "{err}");

    cmd.change_password(change(&alice, PASSWORD)).await.unwrap();
    let user = cmd.load_user(&alice).await.unwrap().unwrap();
    assert_eq!(user.password.as_ref().unwrap().password_hash, new_hash);
    assert_eq!(user.sign_ins.len(), 1, "a change is not a sign-in");
    let err = cmd
        .check_password("alice@example.com", PASSWORD)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidCredentials), "{err}");
    cmd.check_password("alice@example.com", "a brand new password")
        .await
        .unwrap();
}

#[tokio::test]
async fn a_provider_user_adds_an_address_once() {
    let cmd = command().await;
    let alice = register_via_email(&cmd, "alice@example.com").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;
    let hash = hash_password(PASSWORD).await.unwrap();
    let add = |session: Option<&str>, bound: &str, email: &str, code: &str| AddEmail {
        session_user_id: session.map(str::to_owned),
        bound_user_id: bound.to_owned(),
        email: email.to_owned(),
        code: code.to_owned(),
        password_hash: hash.clone(),
    };

    // A taken address gets no code.
    let err = cmd
        .issue_email_code("alice@example.com", ChallengePurpose::Register, NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err}");

    let issued = cmd
        .issue_email_code("Bob@Example.com", ChallengePurpose::Register, NOW)
        .await
        .unwrap();
    // The browser must still be signed in as the user the flow was for.
    let err = cmd
        .add_email(
            add(Some(&alice), &bob, "bob@example.com", &issued.code),
            NOW + 1,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::SessionMismatch), "{err}");
    let err = cmd
        .add_email(add(None, &bob, "bob@example.com", &issued.code), NOW + 1)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::SessionMismatch), "{err}");
    // A user with an address keeps it, without spending the code.
    let err = cmd
        .add_email(
            add(Some(&alice), &alice, "bob@example.com", &issued.code),
            NOW + 1,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidState), "{err}");

    cmd.add_email(
        add(Some(&bob), &bob, "bob@example.com", &issued.code),
        NOW + 1,
    )
    .await
    .unwrap();
    let user = cmd.load_user(&bob).await.unwrap().unwrap();
    let method = user.password.as_ref().unwrap();
    assert_eq!(method.email, "bob@example.com");
    assert_eq!(method.password_hash, hash);
    assert_eq!(
        user.account, "bob@example.com",
        "the provider's address stays"
    );
    assert_eq!(user.sign_in_methods(), 2);
    assert_eq!(
        cmd.load_email_account("bob@example.com")
            .await
            .unwrap()
            .unwrap()
            .user_id,
        bob
    );
    assert_eq!(
        cmd.check_password("bob@example.com", PASSWORD)
            .await
            .unwrap()
            .user_id,
        bob
    );
    // The code is burnt, and the address is now taken.
    let err = cmd
        .add_email(
            add(Some(&bob), &bob, "bob@example.com", &issued.code),
            NOW + 2,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidState), "{err}");
    let err = cmd
        .issue_email_code("bob@example.com", ChallengePurpose::Register, NOW + 100)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err}");
    // A sign-in code is not proof of a free address.
    let issued = cmd
        .issue_email_code("bob@example.com", ChallengePurpose::SignIn, NOW + 100)
        .await
        .unwrap();
    let carol = register_via_oauth2(&cmd, "google", "carol").await;
    let err = cmd
        .add_email(
            add(Some(&carol), &carol, "carol@example.com", &issued.code),
            NOW + 101,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidCode), "{err}");

    // The provider can go now: the password remains a way in.
    cmd.unlink_oauth2(&bob, "google", "bob").await.unwrap();
    assert_eq!(
        cmd.load_user(&bob)
            .await
            .unwrap()
            .unwrap()
            .sign_in_methods(),
        1
    );
}
