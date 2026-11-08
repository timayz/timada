mod common;

use common::*;
use evento::Executor;
use timada_tenancy::{DefineRole, PERMISSION_MEMBERS_READ};

/// Every back-office page loads the tenant for the member's permissions, and
/// a tenant stream grows with every member, role, key and mail server: a load
/// leaves a snapshot behind, and the next load folds only what came after it.
#[tokio::test]
async fn a_load_of_the_tenant_leaves_a_snapshot_the_next_load_builds_on() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    cmd.define_role(
        &alice,
        &tenant.id,
        DefineRole {
            role: "support".to_owned(),
            label: "Support".to_owned(),
            permissions: vec![PERMISSION_MEMBERS_READ.to_owned()],
        },
    )
    .await
    .unwrap();

    let loaded = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert!(loaded.roles.contains_key("support"));

    let snapshot = cmd
        .0
        .get_snapshot(
            String::new(),
            "timada-tenancy/Tenant".to_owned(),
            "timada-tenancy/TenantState".to_owned(),
            "0".to_owned(),
            tenant.id.clone(),
        )
        .await
        .unwrap();
    assert!(
        snapshot.is_some(),
        "the load stored a snapshot of the tenant"
    );

    // What comes after the snapshot is folded on top of it, and the version
    // the write model observes stays exact.
    cmd.rename_tenant(&alice, &tenant.id, "Acme Ltd")
        .await
        .unwrap();
    let again = cmd.load_tenant(&tenant.id).await.unwrap().unwrap();
    assert_eq!(again.name, "Acme Ltd");
    assert!(again.roles.contains_key("support"));
    assert_eq!(again.aggregate_version, loaded.aggregate_version + 1);
}
