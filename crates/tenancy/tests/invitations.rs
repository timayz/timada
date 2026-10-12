mod common;

use common::*;
use timada_tenancy::{
    AcceptInvitation, DefineRole, INVITATION_TTL_SECS, InviteMember, PERMISSION_MEMBERS_MANAGE,
    ROLE_OWNER, TenancyError, invitation_id,
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
