mod common;

use common::*;
use timada_tenancy::{
    DefineRole, PERMISSION_MEMBERS_MANAGE, PERMISSION_MEMBERS_READ, PERMISSION_ROLES_MANAGE,
    PERMISSION_TENANT_MANAGE, PERMISSIONS, TenancyError,
};

fn role(name: &str, perms: &[&str]) -> DefineRole {
    DefineRole {
        role: name.to_owned(),
        label: name.to_owned(),
        permissions: perms.iter().map(|p| (*p).to_owned()).collect(),
    }
}

#[tokio::test]
async fn roles_are_defined_updated_and_removed_by_roles_managers() {
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

    // A role manager that is not an owner can manage roles.
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
    cmd.update_role(
        &bob,
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
        &bob,
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
        .update_role(&bob, &tenant.id, "ghost", "Ghost", vec![])
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::UnknownRole), "{err}");
    let err = cmd
        .update_role(&bob, &tenant.id, "owner", "Owner", vec![])
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::ReservedRole), "{err}");

    // A held role cannot be removed; an unheld one can.
    let err = cmd
        .remove_role(&bob, &tenant.id, "admin")
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::RoleInUse), "{err}");
    cmd.remove_role(&bob, &tenant.id, "support").await.unwrap();
    let state = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(!state.roles.contains_key("support"));
    let err = cmd
        .remove_role(&bob, &tenant.id, "support")
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
