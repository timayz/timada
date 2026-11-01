mod common;

use common::*;
use timada_tenancy::{
    DefineRole, PERMISSION_MEMBERS_MANAGE, PERMISSION_MEMBERS_READ, PERMISSION_TENANT_MANAGE,
    ROLE_OWNER, TenancyError,
};

async fn tenant_with_roles(
    cmd: &timada_tenancy::Command<evento::Turso>,
    owner: &str,
) -> timada_tenancy::TenantState {
    let tenant = create_tenant(cmd, "Acme", owner).await;
    for (role, perms) in [
        ("manager", vec![PERMISSION_MEMBERS_MANAGE]),
        ("support", vec![PERMISSION_MEMBERS_READ]),
        // Wider than a manager: nothing a manager may hand out.
        (
            "admin",
            vec![PERMISSION_MEMBERS_MANAGE, PERMISSION_TENANT_MANAGE],
        ),
    ] {
        cmd.define_role(
            owner,
            &tenant.id,
            DefineRole {
                role: role.to_owned(),
                label: role.to_owned(),
                permissions: perms.into_iter().map(str::to_owned).collect(),
            },
        )
        .await
        .unwrap();
    }
    tenant
}

#[tokio::test]
async fn members_are_granted_changed_and_revoked_under_members_manage() {
    let cmd = command().await;
    let alice = new_id();
    let bob = new_id();
    let carol = new_id();
    let tenant = tenant_with_roles(&cmd, &alice).await;

    let err = cmd
        .grant_member(&bob, &tenant.id, &carol, "support")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "non-member: {err}");
    let err = cmd
        .grant_member(&alice, &tenant.id, &bob, "ghost")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::UnknownRole), "{err}");

    cmd.grant_member(&alice, &tenant.id, &bob, "manager")
        .await
        .unwrap();
    let err = cmd
        .grant_member(&alice, &tenant.id, &bob, "support")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::AlreadyMember), "{err}");

    // A manager grants roles no wider than their own (support's one
    // permission is implied by members:manage; a manager's own role too),
    // never `owner`, never more than they hold.
    cmd.grant_member(&bob, &tenant.id, &carol, "support")
        .await
        .unwrap();
    let err = cmd
        .grant_member(&bob, &tenant.id, &new_id(), ROLE_OWNER)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
    let err = cmd
        .grant_member(&bob, &tenant.id, &new_id(), "admin")
        .await
        .unwrap_err();
    assert!(
        matches!(err, TenancyError::Forbidden),
        "a role wider than the manager's: {err}"
    );
    let dave = new_id();
    cmd.grant_member(&bob, &tenant.id, &dave, "manager")
        .await
        .unwrap();
    // The owner hands out anything.
    cmd.grant_member(&alice, &tenant.id, &new_id(), "admin")
        .await
        .unwrap();

    // Moving members between roles is the owners' alone: a manager could
    // otherwise move themselves, or a friend, up.
    for (user, role) in [
        (&carol, "manager"),
        (&carol, ROLE_OWNER),
        (&bob, "admin"),
        (&alice, "support"),
    ] {
        let err = cmd
            .change_member_role(&bob, &tenant.id, user, role)
            .await
            .unwrap_err();
        assert!(matches!(err, TenancyError::Forbidden), "{role}: {err}");
    }
    let err = cmd
        .revoke_member(&bob, &tenant.id, &alice)
        .await
        .unwrap_err();
    assert!(
        matches!(err, TenancyError::Forbidden),
        "removing an owner: {err}"
    );

    cmd.change_member_role(&alice, &tenant.id, &carol, "manager")
        .await
        .unwrap();
    // Same role → no-op.
    let before = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    cmd.change_member_role(&alice, &tenant.id, &carol, "manager")
        .await
        .unwrap();
    assert_eq!(cmd.load_tenant(&tenant.id).await.unwrap().unwrap(), before);
    assert_eq!(before.role_of(&carol), Some("manager"));
    let err = cmd
        .change_member_role(&alice, &tenant.id, &new_id(), "manager")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::NotMember), "{err}");
    cmd.revoke_member(&bob, &tenant.id, &dave).await.unwrap();

    cmd.revoke_member(&bob, &tenant.id, &carol).await.unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.role_of(&carol).is_none());
    let err = cmd
        .revoke_member(&bob, &tenant.id, &carol)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::NotMember), "{err}");
}

#[tokio::test]
async fn a_tenant_keeps_its_last_owner_and_members_may_leave() {
    let cmd = command().await;
    let alice = new_id();
    let bob = new_id();
    let tenant = tenant_with_roles(&cmd, &alice).await;
    cmd.grant_member(&alice, &tenant.id, &bob, "support")
        .await
        .unwrap();

    let err = cmd
        .change_member_role(&alice, &tenant.id, &alice, "support")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::LastOwner), "{err}");
    let err = cmd
        .revoke_member(&alice, &tenant.id, &alice)
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::LastOwner), "{err}");

    // A second owner frees the first.
    cmd.change_member_role(&alice, &tenant.id, &bob, ROLE_OWNER)
        .await
        .unwrap();
    cmd.change_member_role(&alice, &tenant.id, &alice, "support")
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.owners(), 1);
    assert!(state.is_owner(&bob) && !state.is_owner(&alice));

    // Anyone leaves their own seat, no permission needed; an owner leaving
    // is still an owner removal.
    cmd.revoke_member(&alice, &tenant.id, &alice).await.unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.role_of(&alice).is_none());
    let err = cmd.revoke_member(&bob, &tenant.id, &bob).await.unwrap_err();
    assert!(matches!(err, TenancyError::LastOwner), "{err}");
}
