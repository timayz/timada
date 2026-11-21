mod common;

use std::sync::Arc;

use common::*;
use timada_core::{Claimant, HmacCodeVerifier, MailStatus, Sending, TokenDeriver};
use timada_identity::{
    ACCOUNT_INVITATION_TTL_SECS, Admission, Command, CompleteOAuth2, IdentityError, OAuth2Mode,
    RegisterInvited, hash_password, open_account_invitations, read_model_subscription,
};

fn tokens() -> Arc<dyn TokenDeriver> {
    Arc::new(HmacCodeVerifier::new([9; 32]))
}

/// Root, signed in, invites `email`; the link's token.
async fn invite(cmd: &Command<evento::Turso>, email: &str) -> String {
    cmd.invite_account(&*tokens(), ROOT_USER_ID, email, NOW)
        .await
        .unwrap()
        .token
}

async fn invited(token: &str) -> RegisterInvited {
    RegisterInvited {
        token: token.to_owned(),
        password_hash: hash_password(PASSWORD).await.unwrap(),
        context: context(),
    }
}

/// A provider callback for `sub`, whose address is `{sub}@example.com`.
fn callback(mode: OAuth2Mode, sub: &str, admission: Admission) -> CompleteOAuth2 {
    CompleteOAuth2 {
        admission,
        ..oauth2(mode, None, "google", sub)
    }
}

fn with_link(token: &str) -> Admission {
    Admission::Invited {
        token: token.to_owned(),
        now: NOW + 1,
    }
}

const ROOT_USER_ID: &str = timada_identity::ROOT_USER_ID;

#[tokio::test]
async fn root_alone_invites_and_only_an_address_without_an_account() {
    let cmd = command().await;
    sign_in_root(&cmd).await;
    let alice = register_via_email(&cmd, "alice@example.com").await;
    let admin = register_via_oauth2(&cmd, "google", "admin").await;
    cmd.set_admin(ROOT_USER_ID, &admin, true).await.unwrap();

    for actor in [&alice, &admin] {
        let err = cmd
            .invite_account(&*tokens(), actor, "bob@example.com", NOW)
            .await
            .unwrap_err();
        assert!(matches!(err, IdentityError::Forbidden), "{err:?}");
    }
    for taken in ["Alice@Example.com", ROOT_EMAIL] {
        let err = cmd
            .invite_account(&*tokens(), ROOT_USER_ID, taken, NOW)
            .await
            .unwrap_err();
        assert!(matches!(err, IdentityError::AlreadyRegistered), "{err:?}");
    }
    let err = cmd
        .invite_account(&*tokens(), ROOT_USER_ID, "not an address", NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidEmail), "{err:?}");

    let sent = cmd
        .invite_account(&*tokens(), ROOT_USER_ID, " Bob@Example.com ", NOW)
        .await
        .unwrap();
    assert_eq!(sent.invitation.email, "bob@example.com");
    assert_eq!(sent.invitation.invited_by, ROOT_USER_ID);
    assert_eq!(
        sent.invitation.expires_at,
        NOW + ACCOUNT_INVITATION_TTL_SECS
    );
    assert_eq!(sent.invitation.mail, Sending::Pending);
    // The stream holds the nonce, never the token.
    assert!(!format!("{sent:?}").contains(&sent.token));
    let loaded = cmd
        .load_account_invitation_by_token(&sent.token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded, sent.invitation);
}

#[tokio::test]
async fn the_link_creates_the_account_of_the_invited_address_once() {
    let cmd = command().await;
    sign_in_root(&cmd).await;
    let token = invite(&cmd, "bob@example.com").await;

    let done = cmd
        .register_invited(invited(&token).await, NOW + 1)
        .await
        .unwrap();
    assert_eq!(done.label, "bob@example.com");
    let user = cmd.load_user(&done.user_id).await.unwrap().unwrap();
    assert_eq!(user.account, "bob@example.com");
    assert!(user.has_email("bob@example.com"));
    assert!(!user.is_admin);
    assert_eq!(user.sign_ins.len(), 1, "signed in by the registration");
    // The address signs in from now on.
    let holder = cmd
        .load_email_account("bob@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(holder.user_id, done.user_id);
    let invitation = cmd
        .load_account_invitation_by_token(&token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        invitation.accepted_by.as_deref(),
        Some(done.user_id.as_str())
    );
    assert_eq!(invitation.accepted_at, Some(NOW + 1));

    // Once.
    let err = cmd
        .register_invited(invited(&token).await, NOW + 2)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvitationInvalid), "{err:?}");
}

#[tokio::test]
async fn a_link_unknown_expired_or_withdrawn_creates_nothing() {
    let cmd = command().await;
    sign_in_root(&cmd).await;

    let err = cmd
        .register_invited(invited("no-such-token").await, NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvitationInvalid), "{err:?}");

    let expired = invite(&cmd, "bob@example.com").await;
    let err = cmd
        .register_invited(invited(&expired).await, NOW + ACCOUNT_INVITATION_TTL_SECS)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvitationInvalid), "{err:?}");

    let withdrawn = cmd
        .invite_account(&*tokens(), ROOT_USER_ID, "carol@example.com", NOW)
        .await
        .unwrap();
    let alice = register_via_email(&cmd, "alice@example.com").await;
    let err = cmd
        .revoke_account_invitation(&alice, &withdrawn.invitation.id, NOW + 1)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err:?}");
    cmd.revoke_account_invitation(ROOT_USER_ID, &withdrawn.invitation.id, NOW + 1)
        .await
        .unwrap();
    let err = cmd
        .register_invited(invited(&withdrawn.token).await, NOW + 2)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvitationInvalid), "{err:?}");
    // Withdrawn already.
    let err = cmd
        .revoke_account_invitation(ROOT_USER_ID, &withdrawn.invitation.id, NOW + 3)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvitationInvalid), "{err:?}");
    assert!(
        cmd.load_email_account("carol@example.com")
            .await
            .unwrap()
            .is_none()
    );
}

/// The address got its account another way while the invitation waited
/// (sign-up was open then): the link creates no second one, and stays open.
#[tokio::test]
async fn an_address_registered_meanwhile_is_not_registered_twice() {
    let cmd = command().await;
    sign_in_root(&cmd).await;
    let token = invite(&cmd, "bob@example.com").await;
    register_via_email(&cmd, "bob@example.com").await;

    let err = cmd
        .register_invited(invited(&token).await, NOW + 1)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err:?}");
    let err = cmd
        .complete_oauth2(callback(OAuth2Mode::CreateUser, "bob", with_link(&token)))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err:?}");
}

#[tokio::test]
async fn two_browsers_with_one_link_create_one_account() {
    let cmd = command().await;
    sign_in_root(&cmd).await;
    let token = invite(&cmd, "bob@example.com").await;

    let (first, second) = tokio::join!(
        cmd.register_invited(invited(&token).await, NOW + 1),
        cmd.complete_oauth2(callback(OAuth2Mode::CreateUser, "bob", with_link(&token))),
    );
    assert_eq!(
        usize::from(first.is_ok()) + usize::from(second.is_ok()),
        1,
        "{first:?} {second:?}"
    );
}

#[tokio::test]
async fn closed_sign_up_lets_a_provider_sign_in_but_create_nothing() {
    let cmd = command().await;
    let known = register_via_oauth2(&cmd, "google", "known").await;

    for mode in [OAuth2Mode::CreateUser, OAuth2Mode::CreateUserOrLogin] {
        let err = cmd
            .complete_oauth2(callback(mode, "stranger", Admission::Closed))
            .await
            .unwrap_err();
        assert!(matches!(err, IdentityError::SignupClosed), "{err:?}");
    }
    assert!(
        cmd.load_oauth2_account("google", "stranger")
            .await
            .unwrap()
            .is_none()
    );
    // An account that exists signs in as before.
    for mode in [OAuth2Mode::Login, OAuth2Mode::CreateUserOrLogin] {
        let outcome = cmd
            .complete_oauth2(callback(mode, "known", Admission::Closed))
            .await
            .unwrap();
        assert_eq!(outcome.user_id, known);
        assert!(!outcome.created);
    }
}

#[tokio::test]
async fn a_provider_creates_the_invited_account_only_for_the_address_it_verified() {
    let cmd = command().await;
    sign_in_root(&cmd).await;
    let token = invite(&cmd, "bob@example.com").await;

    // Another address.
    let err = cmd
        .complete_oauth2(callback(OAuth2Mode::CreateUser, "eve", with_link(&token)))
        .await
        .unwrap_err();
    assert!(
        matches!(err, IdentityError::InvitationForOtherAddress),
        "{err:?}"
    );
    // The right address, which the provider did not verify.
    let mut unverified = callback(OAuth2Mode::CreateUser, "bob", with_link(&token));
    unverified.identity.email_verified = false;
    let err = cmd.complete_oauth2(unverified).await.unwrap_err();
    assert!(
        matches!(err, IdentityError::InvitationForOtherAddress),
        "{err:?}"
    );
    // Neither used the link up.
    let mut cased = callback(OAuth2Mode::CreateUserOrLogin, "bob", with_link(&token));
    cased.identity.email = Some("Bob@Example.com".to_owned());
    let outcome = cmd.complete_oauth2(cased).await.unwrap();
    assert!(outcome.created);
    let user = cmd.load_user(&outcome.user_id).await.unwrap().unwrap();
    assert!(user.has_oauth2_account("google", "bob"));
    let invitation = cmd
        .load_account_invitation_by_token(&token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        invitation.accepted_by.as_deref(),
        Some(outcome.user_id.as_str())
    );

    // The link is spent: a second identity with the same address gets none.
    let mut other = callback(OAuth2Mode::CreateUser, "bob2", with_link(&token));
    other.identity.email = Some("bob@example.com".to_owned());
    let err = cmd.complete_oauth2(other).await.unwrap_err();
    assert!(matches!(err, IdentityError::InvitationInvalid), "{err:?}");
    // Signing in afterwards needs no invitation.
    let again = cmd
        .complete_oauth2(callback(OAuth2Mode::Login, "bob", Admission::Closed))
        .await
        .unwrap();
    assert_eq!(again.user_id, outcome.user_id);
}

#[tokio::test]
async fn the_link_is_mailed_by_one_node_and_its_outcome_recorded() {
    let cmd = command().await;
    sign_in_root(&cmd).await;
    let sent = cmd
        .invite_account(&*tokens(), ROOT_USER_ID, "bob@example.com", NOW)
        .await
        .unwrap();
    let id = sent.invitation.id.as_str();
    let node = |node, now| Claimant { node, now };

    let mail = cmd
        .claim_account_invitation_mail(&*tokens(), id, node("node-a", NOW))
        .await
        .unwrap()
        .expect("the first node takes the send");
    assert_eq!(mail.token, sent.token, "derived again from the nonce");
    assert_eq!(mail.invitation.email, "bob@example.com");
    assert!(
        cmd.claim_account_invitation_mail(&*tokens(), id, node("node-b", NOW))
            .await
            .unwrap()
            .is_none(),
        "another node is sending"
    );

    cmd.record_account_invitation_mail_not_sent(id, "node-a", "no answer", NOW)
        .await
        .unwrap();
    let state = cmd.load_account_invitation(id).await.unwrap().unwrap();
    let Sending::Deferred {
        retry_at,
        attempts: 1,
        ..
    } = state.mail
    else {
        panic!("{:?}", state.mail);
    };
    assert!(
        cmd.claim_account_invitation_mail(&*tokens(), id, node("node-b", retry_at))
            .await
            .unwrap()
            .is_some(),
        "any node takes the retry once due"
    );
    cmd.record_account_invitation_mail_sent(id).await.unwrap();
    assert_eq!(
        cmd.load_account_invitation(id).await.unwrap().unwrap().mail,
        Sending::Sent
    );
    // The mail's writes leave the link usable.
    cmd.register_invited(invited(&sent.token).await, retry_at + 1)
        .await
        .unwrap();
    // Used: nothing more to mail.
    assert!(
        cmd.claim_account_invitation_mail(&*tokens(), id, node("node-a", retry_at + 2))
            .await
            .unwrap()
            .is_none()
    );

    let refused = cmd
        .invite_account(&*tokens(), ROOT_USER_ID, "nobody@example.com", NOW)
        .await
        .unwrap();
    cmd.record_account_invitation_mail_undeliverable(&refused.invitation.id, "no such user")
        .await
        .unwrap();
    assert!(
        cmd.load_account_invitation(&refused.invitation.id)
            .await
            .unwrap()
            .unwrap()
            .mail
            .is_undeliverable()
    );
}

#[tokio::test]
async fn the_page_lists_the_invitations_still_waiting() {
    let (executor, db) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), codes());
    sign_in_root(&cmd).await;
    let used = invite(&cmd, "used@example.com").await;
    let withdrawn = cmd
        .invite_account(&*tokens(), ROOT_USER_ID, "withdrawn@example.com", NOW + 1)
        .await
        .unwrap();
    let waiting = cmd
        .invite_account(&*tokens(), ROOT_USER_ID, "waiting@example.com", NOW + 2)
        .await
        .unwrap();
    let latest = cmd
        .invite_account(&*tokens(), ROOT_USER_ID, "latest@example.com", NOW + 3)
        .await
        .unwrap();
    cmd.register_invited(invited(&used).await, NOW + 4)
        .await
        .unwrap();
    cmd.revoke_account_invitation(ROOT_USER_ID, &withdrawn.invitation.id, NOW + 4)
        .await
        .unwrap();
    cmd.record_account_invitation_mail_sent(&waiting.invitation.id)
        .await
        .unwrap();

    for _ in 0..2 {
        read_model_subscription::<evento::Turso>(db.clone())
            .run_once(&executor)
            .await
            .unwrap();
        let rows = open_account_invitations(&db, NOW + 5).await.unwrap();
        let emails: Vec<&str> = rows.iter().map(|row| row.email.as_str()).collect();
        assert_eq!(emails, ["latest@example.com", "waiting@example.com"]);
        assert_eq!(rows[0].id, latest.invitation.id);
        assert_eq!(rows[0].invited_by, ROOT_USER_ID);
        assert_eq!(rows[0].mail(), MailStatus::Sending);
        assert_eq!(rows[1].mail(), MailStatus::Sent);
    }
    // Past their seven days, none waits.
    assert!(
        open_account_invitations(&db, NOW + 3 + ACCOUNT_INVITATION_TTL_SECS)
            .await
            .unwrap()
            .is_empty()
    );
}
