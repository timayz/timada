mod common;

use common::*;
use evento::ProjectionAggregate;
use timada_core::Claimant;
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
async fn a_reset_ends_every_session_in_the_same_write() {
    let cmd = command().await;
    let alice = register_via_email(&cmd, "alice@example.com").await;
    let laptop = [1u8; 32];
    let phone = [2u8; 32];
    cmd.start_session(&laptop, &alice, NOW + 600, context())
        .await
        .unwrap();
    cmd.start_session(&phone, &alice, NOW + 600, context())
        .await
        .unwrap();
    // The millisecond cutoff must be strictly after both starts.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let issued = cmd
        .issue_email_code(
            "alice@example.com",
            ChallengePurpose::ResetPassword,
            NOW + 1,
        )
        .await
        .unwrap();
    cmd.reset_password(
        ResetPassword {
            email: "alice@example.com".to_owned(),
            code: issued.code,
            password_hash: hash_password("a brand new password").await.unwrap(),
            context: context(),
        },
        NOW + 2,
    )
    .await
    .unwrap();
    // Nothing else asked for: whoever held the old password is out.
    assert!(
        cmd.resolve_session(&laptop, NOW + 2)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        cmd.resolve_session(&phone, NOW + 2)
            .await
            .unwrap()
            .is_none()
    );

    // The session the reset signs in with starts afterwards and stays.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let fresh = [3u8; 32];
    cmd.start_session(&fresh, &alice, NOW + 600, context())
        .await
        .unwrap();
    assert!(
        cmd.resolve_session(&fresh, NOW + 2)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_session_a_reset_ended_cannot_spare_itself() {
    let cmd = command().await;
    let alice = register_via_email(&cmd, "alice@example.com").await;
    let stolen = [1u8; 32];
    cmd.start_session(&stolen, &alice, NOW + 600, context())
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let issued = cmd
        .issue_email_code(
            "alice@example.com",
            ChallengePurpose::ResetPassword,
            NOW + 1,
        )
        .await
        .unwrap();
    cmd.reset_password(
        ResetPassword {
            email: "alice@example.com".to_owned(),
            code: issued.code,
            password_hash: hash_password("a brand new password").await.unwrap(),
            context: context(),
        },
        NOW + 2,
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let fresh = [2u8; 32];
    cmd.start_session(&fresh, &alice, NOW + 600, context())
        .await
        .unwrap();

    // The stolen session asks to sign the others out after the reset (its
    // page loaded before): it is ended, and spares nobody.
    let err = cmd
        .revoke_sessions(&alice, &alice, Some(&stolen))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::Unauthorized), "{err}");
    assert!(
        cmd.resolve_session(&fresh, NOW + 2)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        cmd.resolve_session(&stolen, NOW + 2)
            .await
            .unwrap()
            .is_none()
    );

    // The browser the reset signed in may still sign the others out.
    cmd.revoke_sessions(&alice, &alice, Some(&fresh))
        .await
        .unwrap();
    assert!(
        cmd.resolve_session(&fresh, NOW + 2)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_change_ends_the_other_sessions_in_the_same_write() {
    let cmd = command().await;
    let alice = register_via_email(&cmd, "alice@example.com").await;
    let laptop = [1u8; 32];
    let phone = [2u8; 32];
    cmd.start_session(&laptop, &alice, NOW + 600, context())
        .await
        .unwrap();
    cmd.start_session(&phone, &alice, NOW + 600, context())
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    cmd.change_password(ChangePassword {
        user_id: alice.clone(),
        current_password: PASSWORD.to_owned(),
        new_password_hash: hash_password("a brand new password").await.unwrap(),
        session_token_hash: Some(laptop),
    })
    .await
    .unwrap();
    assert!(
        cmd.resolve_session(&laptop, NOW).await.unwrap().is_some(),
        "the asking browser stays"
    );
    assert!(cmd.resolve_session(&phone, NOW).await.unwrap().is_none());
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
        session_token_hash: None,
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

#[tokio::test]
async fn a_code_checks_only_under_the_key_it_was_issued_with() {
    let executor = executor().await;
    let cmd = timada_identity::Command::new(executor.clone(), codes());
    let other = timada_identity::Command::new(
        executor,
        std::sync::Arc::new(timada_identity::HmacCodeVerifier::new([9; 32])),
    );
    let hash = hash_password(PASSWORD).await.unwrap();
    let issued = cmd
        .issue_email_code("alice@example.com", ChallengePurpose::Register, NOW)
        .await
        .unwrap();
    let challenge = cmd
        .load_email_challenge("alice@example.com")
        .await
        .unwrap()
        .unwrap();
    // Neither the code nor a plain hash of it is in the stream: a nonce the
    // code derives from under the key, and the address the mail goes to.
    assert_eq!(challenge.email, "alice@example.com");
    assert_eq!(challenge.nonce.len(), timada_core::codes::NONCE_LEN);
    assert!(
        !challenge
            .nonce
            .windows(issued.code.len())
            .any(|window| window == issued.code.as_bytes())
    );
    assert_ne!(
        challenge.nonce,
        evento::hash_ids(vec!["email-code", &challenge.id, &issued.code]).into_bytes()
    );
    assert_eq!(
        codes().derive_code("", &challenge.id, &challenge.nonce),
        issued.code,
        "the node that mails derives the same code from the nonce"
    );
    // Without the key, the right code is a wrong guess, and counts as one.
    assert!(matches!(
        other
            .register_with_password(registration("alice@example.com", &issued.code, &hash), NOW)
            .await,
        Err(IdentityError::InvalidCode)
    ));
    let challenge = cmd
        .load_email_challenge("alice@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(challenge.failed_attempts, 1);
    cmd.register_with_password(registration("alice@example.com", &issued.code, &hash), NOW)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_codes_mail_is_taken_by_one_node_and_recorded_once() {
    let executor = executor().await;
    let node_a = timada_identity::Command::new(executor.clone(), codes());
    let node_b = timada_identity::Command::new(executor.clone(), codes());
    let issued = node_a
        .issue_email_code("alice@example.com", ChallengePurpose::Register, NOW)
        .await
        .unwrap();
    let challenge = node_a
        .load_email_challenge("alice@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(challenge.mail, timada_core::Sending::Pending);
    let version = challenge.issued_version;

    // Both nodes see the event; the first claim wins and gets the mail.
    let mail = node_a
        .claim_code_mail(
            &challenge.id,
            version,
            Claimant {
                node: "node-a",
                now: NOW,
            },
        )
        .await
        .unwrap()
        .expect("node-a takes the send");
    assert_eq!(mail.email, "alice@example.com");
    assert_eq!(mail.purpose, ChallengePurpose::Register);
    assert_eq!(mail.code, issued.code);
    assert!(
        node_b
            .claim_code_mail(
                &challenge.id,
                version,
                Claimant {
                    node: "node-b",
                    now: NOW + 1
                }
            )
            .await
            .unwrap()
            .is_none(),
        "node-b leaves it"
    );
    // The holder, retrying a failed send, gets the mail again.
    assert!(
        node_a
            .claim_code_mail(
                &challenge.id,
                version,
                Claimant {
                    node: "node-a",
                    now: NOW + 2
                }
            )
            .await
            .unwrap()
            .is_some()
    );
    // Once the lease lapsed, node-b takes over.
    let late = NOW + timada_core::LEASE_SECS;
    assert!(
        node_b
            .claim_code_mail(
                &challenge.id,
                version,
                Claimant {
                    node: "node-b",
                    now: late
                }
            )
            .await
            .unwrap()
            .is_some()
    );
    node_b
        .record_code_mail_sent(&challenge.id, version)
        .await
        .unwrap();
    let challenge = node_a
        .load_email_challenge("alice@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(challenge.mail, timada_core::Sending::Sent);
    // Replayed: nothing to send, nothing to record.
    assert!(
        node_a
            .claim_code_mail(
                &challenge.id,
                version,
                Claimant {
                    node: "node-a",
                    now: late + 1
                }
            )
            .await
            .unwrap()
            .is_none()
    );
    node_a
        .record_code_mail_sent(&challenge.id, version)
        .await
        .unwrap();
    assert_eq!(
        node_a
            .load_email_challenge("alice@example.com")
            .await
            .unwrap()
            .unwrap()
            .aggregate_version()
            .unwrap(),
        4,
        "issued, claimed twice, sent: nothing more"
    );
    // The code still signs the person up.
    let hash = hash_password(PASSWORD).await.unwrap();
    node_a
        .register_with_password(registration("alice@example.com", &issued.code, &hash), NOW)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_undeliverable_code_frees_the_resend_window_and_a_stale_event_mails_nothing() {
    let cmd = command().await;
    cmd.issue_email_code("alice@example.com", ChallengePurpose::Register, NOW)
        .await
        .unwrap();
    let first = cmd
        .load_email_challenge("alice@example.com")
        .await
        .unwrap()
        .unwrap();
    cmd.claim_code_mail(
        &first.id,
        first.issued_version,
        Claimant {
            node: "node-a",
            now: NOW,
        },
    )
    .await
    .unwrap()
    .expect("taken");
    // Within the minute, another code is refused.
    assert!(matches!(
        cmd.issue_email_code("alice@example.com", ChallengePurpose::Register, NOW + 10)
            .await,
        Err(IdentityError::CodeRecentlySent)
    ));
    cmd.record_code_mail_undeliverable(&first.id, first.issued_version, "550 no such user")
        .await
        .unwrap();
    let refused = cmd
        .load_email_challenge("alice@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        refused.mail,
        timada_core::Sending::Undeliverable {
            reason: "550 no such user".to_owned()
        }
    );
    // Refused for good: the person may try again at once.
    let second = cmd
        .issue_email_code("alice@example.com", ChallengePurpose::Register, NOW + 10)
        .await
        .unwrap();
    let state = cmd
        .load_email_challenge("alice@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.mail, timada_core::Sending::Pending);
    assert_ne!(state.issued_version, first.issued_version);
    // The first code's event, replayed, mails nothing: the second owns the
    // stream now.
    assert!(
        cmd.claim_code_mail(
            &first.id,
            first.issued_version,
            Claimant {
                node: "node-a",
                now: NOW + 11
            }
        )
        .await
        .unwrap()
        .is_none()
    );
    cmd.record_code_mail_sent(&first.id, first.issued_version)
        .await
        .unwrap();
    assert_eq!(
        cmd.load_email_challenge("alice@example.com")
            .await
            .unwrap()
            .unwrap()
            .mail,
        timada_core::Sending::Pending,
        "the stale record is dropped"
    );
    let mail = cmd
        .claim_code_mail(
            &state.id,
            state.issued_version,
            Claimant {
                node: "node-a",
                now: NOW + 11,
            },
        )
        .await
        .unwrap()
        .expect("the second code is mailed");
    assert_eq!(mail.code, second.code);
    // An expired code is not mailed any more.
    assert!(
        cmd.claim_code_mail(
            &state.id,
            state.issued_version,
            Claimant {
                node: "node-b",
                now: NOW + 10 + CODE_TTL_SECS,
            },
        )
        .await
        .unwrap()
        .is_none()
    );
}
