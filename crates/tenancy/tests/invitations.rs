mod common;

use common::*;
use timada_tenancy::aggregator::{Invitation, MemberGranted};
use timada_tenancy::{
    AcceptInvitation, Command, DefineRole, INVITATION_TTL_SECS, InviteMember,
    PERMISSION_MEMBERS_MANAGE, ROLE_OWNER, SentInvitation, TenancyError, TenantState,
    invitation_id,
};

const NOW: u64 = 1_800_000_000;

fn invite(tenant_id: &str, email: &str, role: &str) -> InviteMember {
    InviteMember {
        tenant_id: tenant_id.to_owned(),
        email: email.to_owned(),
        role: role.to_owned(),
    }
}

fn accept(token: &str, user_id: &str) -> AcceptInvitation {
    AcceptInvitation {
        token: token.to_owned(),
        user_id: user_id.to_owned(),
    }
}

#[tokio::test]
async fn an_invitation_is_sent_accepted_once_and_grants_its_role() {
    let cmd = command().await;
    let alice = new_id();
    let bob = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    cmd.define_role(
        &alice,
        &tenant.id,
        DefineRole {
            role: "support".to_owned(),
            label: "Support".to_owned(),
            permissions: vec![],
        },
    )
    .await
    .unwrap();

    let err = cmd
        .invite_member(&alice, invite(&tenant.id, "not an address", "support"), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvalidEmail), "{err}");
    let err = cmd
        .invite_member(&alice, invite(&tenant.id, "bob@example.com", "ghost"), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::UnknownRole), "{err}");
    let err = cmd
        .invite_member(&bob, invite(&tenant.id, "bob@example.com", "support"), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");

    let sent = cmd
        .invite_member(
            &alice,
            invite(&tenant.id, " Bob@Example.com ", "support"),
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(sent.invitation.email, "bob@example.com");
    assert_eq!(sent.invitation.id, invitation_id(&sent.token));
    assert_eq!(sent.invitation.expires_at, NOW + INVITATION_TTL_SECS);
    assert_eq!(sent.invitation.invited_by, alice);
    assert!(sent.invitation.is_open_at(NOW));
    assert!(
        !format!("{sent:?}").contains(&sent.token),
        "token is redacted"
    );
    assert_eq!(
        cmd.load_invitation_by_token(&sent.token)
            .await
            .unwrap()
            .unwrap(),
        sent.invitation
    );

    let joined = cmd
        .accept_invitation(accept(&sent.token, &bob), NOW + 60)
        .await
        .unwrap();
    assert_eq!(joined.id, tenant.id);
    assert_eq!(joined.role_of(&bob), Some("support"));
    let state = cmd
        .load_invitation(&sent.invitation.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.accepted_by.as_deref(), Some(bob.as_str()));
    assert_eq!(state.accepted_at, Some(NOW + 60));
    assert!(!state.is_open_at(NOW + 61));

    // Used, unknown or expired links are all "invalid".
    let err = cmd
        .accept_invitation(accept(&sent.token, &new_id()), NOW + 120)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvitationInvalid), "{err}");
    let err = cmd
        .accept_invitation(accept("nope", &bob), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvitationInvalid), "{err}");
    let late = cmd
        .invite_member(
            &alice,
            invite(&tenant.id, "late@example.com", "support"),
            NOW,
        )
        .await
        .unwrap();
    let err = cmd
        .accept_invitation(accept(&late.token, &new_id()), NOW + INVITATION_TTL_SECS)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvitationInvalid), "{err}");
}

#[tokio::test]
async fn owner_invitations_need_an_owner_and_revocation_closes_the_link() {
    let cmd = command().await;
    let alice = new_id();
    let bob = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    cmd.define_role(
        &alice,
        &tenant.id,
        DefineRole {
            role: "manager".to_owned(),
            label: "Manager".to_owned(),
            permissions: vec![PERMISSION_MEMBERS_MANAGE.to_owned()],
        },
    )
    .await
    .unwrap();
    cmd.grant_member(&alice, &tenant.id, &bob, "manager")
        .await
        .unwrap();

    let err = cmd
        .invite_member(&bob, invite(&tenant.id, "x@example.com", ROLE_OWNER), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
    let sent = cmd
        .invite_member(&bob, invite(&tenant.id, "x@example.com", "manager"), NOW)
        .await
        .unwrap();

    let err = cmd
        .revoke_invitation(&new_id(), &sent.invitation.id, NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
    cmd.revoke_invitation(&bob, &sent.invitation.id, NOW)
        .await
        .unwrap();
    let err = cmd
        .revoke_invitation(&bob, &sent.invitation.id, NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvitationInvalid), "{err}");
    let err = cmd
        .accept_invitation(accept(&sent.token, &new_id()), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvitationInvalid), "{err}");
    let err = cmd
        .revoke_invitation(&bob, "unknown", NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvitationInvalid), "{err}");

    // An owner may invite another owner.
    let sent = cmd
        .invite_member(
            &alice,
            invite(&tenant.id, "co@example.com", ROLE_OWNER),
            NOW,
        )
        .await
        .unwrap();
    let carol = new_id();
    let joined = cmd
        .accept_invitation(accept(&sent.token, &carol), NOW)
        .await
        .unwrap();
    assert!(joined.is_owner(&carol));
    assert_eq!(joined.owners(), 2);
}

#[tokio::test]
async fn accepting_converges_when_the_role_vanished_or_the_user_is_already_in() {
    let cmd = command().await;
    let alice = new_id();
    let bob = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    cmd.define_role(
        &alice,
        &tenant.id,
        DefineRole {
            role: "support".to_owned(),
            label: "Support".to_owned(),
            permissions: vec![],
        },
    )
    .await
    .unwrap();
    let sent = cmd
        .invite_member(
            &alice,
            invite(&tenant.id, "bob@example.com", "support"),
            NOW,
        )
        .await
        .unwrap();

    // The role was removed in between: the link stays open, the grant fails.
    cmd.remove_role(&alice, &tenant.id, "support")
        .await
        .unwrap();
    let err = cmd
        .accept_invitation(accept(&sent.token, &bob), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::UnknownRole), "{err}");
    assert!(
        cmd.load_invitation(&sent.invitation.id)
            .await
            .unwrap()
            .unwrap()
            .is_open_at(NOW)
    );

    // Already a member (granted some other way): the link is consumed, the
    // existing seat untouched.
    cmd.grant_member(&alice, &tenant.id, &bob, ROLE_OWNER)
        .await
        .unwrap();
    cmd.define_role(
        &alice,
        &tenant.id,
        DefineRole {
            role: "support".to_owned(),
            label: "Support".to_owned(),
            permissions: vec![],
        },
    )
    .await
    .unwrap();
    let joined = cmd
        .accept_invitation(accept(&sent.token, &bob), NOW)
        .await
        .unwrap();
    assert!(joined.is_owner(&bob));
    assert!(
        !cmd.load_invitation(&sent.invitation.id)
            .await
            .unwrap()
            .unwrap()
            .is_open_at(NOW)
    );

    // A suspended tenant refuses the link without consuming it.
    let sent = cmd
        .invite_member(&alice, invite(&tenant.id, "c@example.com", "support"), NOW)
        .await
        .unwrap();
    cmd.suspend_tenant(&tenant.id, "unpaid").await.unwrap();
    let err = cmd
        .accept_invitation(accept(&sent.token, &new_id()), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Suspended), "{err}");
}

/// Acme, its `support` role, and an invitation to it.
async fn acme_with_invitation(
    cmd: &Command<evento::Turso>,
    owner: &str,
) -> (TenantState, SentInvitation) {
    let tenant = create_tenant(cmd, "Acme", owner).await;
    cmd.define_role(
        owner,
        &tenant.id,
        DefineRole {
            role: "support".to_owned(),
            label: "Support".to_owned(),
            permissions: vec![],
        },
    )
    .await
    .unwrap();
    let sent = cmd
        .invite_member(owner, invite(&tenant.id, "bob@example.com", "support"), NOW)
        .await
        .unwrap();
    (tenant, sent)
}

#[tokio::test]
async fn two_people_with_one_link_get_one_seat() {
    let (store, _) = executor_with_pool().await;
    let cmd = Command(store.clone());
    let alice = new_id();
    let (tenant, sent) = acme_with_invitation(&cmd, &alice).await;
    let bob = new_id();
    let eve = new_id();

    // Bob's acceptance is held at the invitation; Eve, with the link
    // forwarded to her, accepts in between.
    let rigged = Rigged::new(store.clone()).hold_first::<Invitation>();
    let held = Command(rigged.clone());
    let (token, user) = (sent.token.clone(), bob.clone());
    let first =
        tokio::spawn(async move { held.accept_invitation(accept(&token, &user), NOW + 1).await });
    rigged.parked().await;
    Command(rigged)
        .accept_invitation(accept(&sent.token, &eve), NOW + 1)
        .await
        .unwrap();
    let err = first.await.unwrap().unwrap_err();
    assert!(matches!(err, TenancyError::InvitationInvalid), "{err}");

    let tenant = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(tenant.role_of(&eve), Some("support"));
    assert_eq!(
        tenant.role_of(&bob),
        None,
        "the losing acceptance grants nothing"
    );
    assert_eq!(
        cmd.load_invitation(&sent.invitation.id)
            .await
            .unwrap()
            .unwrap()
            .accepted_by,
        Some(eve)
    );
}

#[tokio::test]
async fn a_revocation_racing_an_acceptance_lets_one_of_them_win() {
    let (store, _) = executor_with_pool().await;
    let cmd = Command(store.clone());
    let alice = new_id();
    let (tenant, sent) = acme_with_invitation(&cmd, &alice).await;
    let bob = new_id();

    // The revocation lands first: the acceptance grants nothing.
    let rigged = Rigged::new(store.clone()).hold_first::<Invitation>();
    let held = Command(rigged.clone());
    let (token, user) = (sent.token.clone(), bob.clone());
    let accepting =
        tokio::spawn(async move { held.accept_invitation(accept(&token, &user), NOW + 1).await });
    rigged.parked().await;
    Command(rigged)
        .revoke_invitation(&alice, &sent.invitation.id, NOW + 1)
        .await
        .unwrap();
    let err = accepting.await.unwrap().unwrap_err();
    assert!(matches!(err, TenancyError::InvitationInvalid), "{err}");
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.role_of(&bob), None);

    // The acceptance lands first: the revocation is refused, the seat stays.
    let carol = new_id();
    let sent = cmd
        .invite_member(
            &alice,
            invite(&tenant.id, "carol@example.com", "support"),
            NOW,
        )
        .await
        .unwrap();
    let rigged = Rigged::new(store.clone()).hold_first::<Invitation>();
    let held = Command(rigged.clone());
    let (actor, invitation) = (alice.clone(), sent.invitation.id.clone());
    let revoking =
        tokio::spawn(async move { held.revoke_invitation(&actor, &invitation, NOW + 1).await });
    rigged.parked().await;
    Command(rigged)
        .accept_invitation(accept(&sent.token, &carol), NOW + 1)
        .await
        .unwrap();
    let err = revoking.await.unwrap().unwrap_err();
    assert!(matches!(err, TenancyError::InvitationInvalid), "{err}");
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.role_of(&carol), Some("support"));
}

#[tokio::test]
async fn the_subscription_grants_the_seat_a_crash_left_owed() {
    let (store, _) = executor_with_pool().await;
    let cmd = Command(store.clone());
    let alice = new_id();
    let (tenant, sent) = acme_with_invitation(&cmd, &alice).await;
    let bob = new_id();

    // The node stops between the acceptance and the seat.
    let err = Command(Rigged::new(store.clone()).refuse::<MemberGranted>())
        .accept_invitation(accept(&sent.token, &bob), NOW + 1)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Write(_)), "{err}");
    let invitation = cmd
        .load_invitation(&sent.invitation.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(invitation.accepted_by.as_deref(), Some(bob.as_str()));
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.role_of(&bob), None);

    grant_owed_seats(&store).await;
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.role_of(&bob), Some("support"));
}

#[tokio::test]
async fn an_old_acceptance_never_lets_a_removed_member_back() {
    let (store, _) = executor_with_pool().await;
    let cmd = Command(store.clone());
    let alice = new_id();
    let (tenant, sent) = acme_with_invitation(&cmd, &alice).await;
    let bob = new_id();
    let carol = new_id();

    // Bob joins through the link, Carol accepts one while already in; both
    // are removed afterwards.
    cmd.accept_invitation(accept(&sent.token, &bob), NOW + 1)
        .await
        .unwrap();
    cmd.grant_member(&alice, &tenant.id, &carol, "support")
        .await
        .unwrap();
    let again = cmd
        .invite_member(
            &alice,
            invite(&tenant.id, "carol@example.com", "support"),
            NOW,
        )
        .await
        .unwrap();
    cmd.accept_invitation(accept(&again.token, &carol), NOW + 1)
        .await
        .unwrap();
    cmd.revoke_member(&alice, &tenant.id, &bob).await.unwrap();
    cmd.revoke_member(&alice, &tenant.id, &carol).await.unwrap();

    // A node that comes by the acceptances only now grants nothing.
    grant_owed_seats(&store).await;
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.role_of(&bob), None);
    assert_eq!(state.role_of(&carol), None);
}

#[tokio::test]
async fn a_member_removed_before_accepting_again_is_owed_the_seat() {
    let (store, _) = executor_with_pool().await;
    let cmd = Command(store.clone());
    let alice = new_id();
    let (tenant, sent) = acme_with_invitation(&cmd, &alice).await;
    let bob = new_id();
    cmd.grant_member(&alice, &tenant.id, &bob, "support")
        .await
        .unwrap();
    cmd.revoke_member(&alice, &tenant.id, &bob).await.unwrap();

    Command(Rigged::new(store.clone()).refuse::<MemberGranted>())
        .accept_invitation(accept(&sent.token, &bob), NOW + 1)
        .await
        .unwrap_err();
    grant_owed_seats(&store).await;
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.role_of(&bob), Some("support"));
}
