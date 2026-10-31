mod common;

use common::*;
use timada_tenancy::{
    DefineRole, PERMISSION_CUSTOMERS_MANAGE, PERMISSION_CUSTOMERS_READ, PERMISSION_MEMBERS_MANAGE,
    PERMISSION_MEMBERS_READ, PERMISSION_ROLES_MANAGE, PERMISSION_TENANT_MANAGE, PERMISSIONS,
    TenancyError, implies,
};

fn role(name: &str, perms: &[&str]) -> DefineRole {
    DefineRole {
        role: name.to_owned(),
        label: name.to_owned(),
        permissions: perms.iter().map(|p| (*p).to_owned()).collect(),
    }
}

#[tokio::test]
async fn roles_are_defined_updated_and_removed_by_owners_alone() {
    let cmd = command().await;
    let alice = new_id();
    let bob = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;

    let err = cmd
        .define_role(
            &bob,
            &tenant.id,
            role("support", &[PERMISSION_MEMBERS_READ]),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");

    cmd.define_role(
        &alice,
        &tenant.id,
        DefineRole {
            role: " Support ".to_owned(),
            label: " Support team ".to_owned(),
            permissions: vec![
                PERMISSION_MEMBERS_READ.to_owned(),
                PERMISSION_MEMBERS_READ.to_owned(),
            ],
        },
    )
    .await
    .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    let def = &state.roles["support"];
    assert_eq!(def.label, "Support team");
    assert_eq!(def.permissions.len(), 1);
    assert!(state.has_role("support") && state.has_role("owner") && !state.has_role("sales"));

    for (bad, expected) in [
        (role("support", &[]), "RoleExists"),
        (role("owner", &[]), "ReservedRole"),
        (role("x", &[]), "InvalidRole"),
        (role("sales", &["members:delete"]), "InvalidPermission"),
    ] {
        let err = cmd.define_role(&alice, &tenant.id, bad).await.unwrap_err();
        assert!(format!("{err:?}").starts_with(expected), "{err:?}");
    }
    let err = cmd
        .define_role(
            &alice,
            &tenant.id,
            DefineRole {
                role: "sales".to_owned(),
                label: "   ".to_owned(),
                permissions: vec![],
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvalidRoleLabel), "{err}");

    // `roles:manage` lets a member see the roles; shaping them is the
    // owners' alone, whatever a role grants: a member who could edit roles
    // could widen their own.
    cmd.define_role(
        &alice,
        &tenant.id,
        role("admin", &[PERMISSION_ROLES_MANAGE]),
    )
    .await
    .unwrap();
    cmd.grant_member(&alice, &tenant.id, &bob, "admin")
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(state.can(&bob, PERMISSION_ROLES_MANAGE));
    let err = cmd
        .define_role(&bob, &tenant.id, role("sales", &[]))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "defining: {err}");
    let err = cmd
        .update_role(
            &bob,
            &tenant.id,
            "admin",
            "Admin",
            PERMISSIONS.iter().map(|p| (*p).to_owned()).collect(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, TenancyError::Forbidden),
        "widening one's own role: {err}"
    );
    let err = cmd
        .remove_role(&bob, &tenant.id, "support")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "removing: {err}");
    assert_eq!(cmd.load_tenant(&tenant.id).await.unwrap().unwrap(), state);

    cmd.update_role(
        &alice,
        &tenant.id,
        "support",
        "Support",
        vec![
            PERMISSION_MEMBERS_READ.to_owned(),
            PERMISSION_MEMBERS_MANAGE.to_owned(),
        ],
    )
    .await
    .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(state.roles["support"].permissions.len(), 2);
    assert_eq!(state.roles["support"].label, "Support");
    // Unchanged → no-op (same version).
    cmd.update_role(
        &alice,
        &tenant.id,
        "support",
        "Support",
        vec![
            PERMISSION_MEMBERS_MANAGE.to_owned(),
            PERMISSION_MEMBERS_READ.to_owned(),
        ],
    )
    .await
    .unwrap();
    assert_eq!(cmd.load_tenant(&tenant.id).await.unwrap().unwrap(), state);

    let err = cmd
        .update_role(&alice, &tenant.id, "ghost", "Ghost", vec![])
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::UnknownRole), "{err}");
    let err = cmd
        .update_role(&alice, &tenant.id, "owner", "Owner", vec![])
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::ReservedRole), "{err}");

    // A held role cannot be removed; an unheld one can.
    let err = cmd
        .remove_role(&alice, &tenant.id, "admin")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::RoleInUse), "{err}");
    cmd.remove_role(&alice, &tenant.id, "support")
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(!state.roles.contains_key("support"));
    let err = cmd
        .remove_role(&alice, &tenant.id, "support")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::UnknownRole), "{err}");
}

#[tokio::test]
async fn permissions_follow_roles_and_owners_hold_everything() {
    let cmd = command().await;
    let alice = new_id();
    let bob = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    cmd.define_role(
        &alice,
        &tenant.id,
        role("support", &[PERMISSION_MEMBERS_READ]),
    )
    .await
    .unwrap();
    cmd.grant_member(&alice, &tenant.id, &bob, "support")
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();

    assert!(PERMISSIONS.iter().all(|p| state.can(&alice, p)));
    assert!(state.can(&bob, PERMISSION_MEMBERS_READ));
    assert!(!state.can(&bob, PERMISSION_MEMBERS_MANAGE));
    // `manage` implies `read` on the same resource.
    cmd.define_role(
        &alice,
        &tenant.id,
        role("manager", &[PERMISSION_MEMBERS_MANAGE]),
    )
    .await
    .unwrap();
    cmd.grant_member(&alice, &tenant.id, &new_id(), "manager")
        .await
        .unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    let manager = state
        .members
        .iter()
        .find(|(_, r)| r.as_str() == "manager")
        .map(|(u, _)| u.clone())
        .unwrap();
    assert!(state.can(&manager, PERMISSION_MEMBERS_READ));
    assert!(state.can(&manager, PERMISSION_MEMBERS_MANAGE));
    assert!(!state.can(&manager, PERMISSION_ROLES_MANAGE));
    // Deleting a shopper account includes seeing the customers, not the other
    // way round.
    assert!(PERMISSIONS.contains(&PERMISSION_CUSTOMERS_MANAGE));
    assert!(implies(
        PERMISSION_CUSTOMERS_MANAGE,
        PERMISSION_CUSTOMERS_READ
    ));
    assert!(!implies(
        PERMISSION_CUSTOMERS_READ,
        PERMISSION_CUSTOMERS_MANAGE
    ));
    assert!(!state.can(&bob, PERMISSION_TENANT_MANAGE));
    assert!(!state.can(&new_id(), PERMISSION_MEMBERS_READ));

    // Renaming is `tenant:manage`, not "owner".
    let err = cmd
        .rename_tenant(&bob, &tenant.id, "Evil")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Forbidden), "{err}");
    cmd.update_role(
        &alice,
        &tenant.id,
        "support",
        "Support",
        vec![PERMISSION_TENANT_MANAGE.to_owned()],
    )
    .await
    .unwrap();
    cmd.rename_tenant(&bob, &tenant.id, "Acme Corp")
        .await
        .unwrap();
    assert_eq!(
        cmd.load_tenant(&tenant.id).await.unwrap().unwrap().name,
        "Acme Corp"
    );
}

#[tokio::test]
async fn a_suspended_tenant_is_read_only() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    cmd.suspend_tenant(&tenant.id, "unpaid").await.unwrap();

    let err = cmd
        .define_role(&alice, &tenant.id, role("support", &[]))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Suspended), "{err}");
    let err = cmd
        .grant_member(&alice, &tenant.id, &new_id(), "owner")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Suspended), "{err}");
    let err = cmd
        .rename_tenant(&alice, &tenant.id, "Other")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::Suspended), "{err}");
}
