mod common;

use common::*;
use timada_tenancy::{
    AcceptInvitation, Command, InviteMember, count_open_invitations_of_tenant, count_tenants,
    group_of, invitation_by_id, list_tenants, open_invitations_of_tenant, read_model_subscription,
    tenant_by_group, tenants_of_user,
};

async fn catch_up(executor: &evento::Sqlite, pool: &sqlx::SqlitePool) {
    read_model_subscription::<evento::Sqlite>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn tenants_and_members_are_listed_and_stay_idempotent() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let alice = new_id();
    let bob = new_id();
    let acme = create_tenant(&cmd, "Acme", &alice).await;
    let zeta = create_tenant(&cmd, "Zeta", &bob).await;
    cmd.rename_tenant(&bob, &zeta.id, "Aardvark").await.unwrap();
    cmd.suspend_tenant(&zeta.id, "unpaid").await.unwrap();

    catch_up(&executor, &pool).await;

    assert_eq!(count_tenants(&pool, None).await.unwrap(), 2);
    let rows = list_tenants(&pool, None, 0, 50).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].name, "Aardvark", "sorted by name");
    assert_eq!(rows[0].id, zeta.id);
    assert!(rows[0].suspended);
    assert_eq!(rows[0].suspended_reason.as_deref(), Some("unpaid"));
    assert_eq!(rows[0].group(), group_of(&zeta.id));
    assert_eq!(rows[1].slug, "acme");
    assert_eq!(rows[1].name, "Acme");
    assert_eq!(rows[1].created_by, alice);
    assert_eq!(rows[1].member_count, 1);
    assert!(!rows[1].suspended);
    assert_eq!(
        list_tenants(&pool, Some("ZET"), 0, 50).await.unwrap().len(),
        1,
        "search is case-insensitive on slug and name"
    );
    assert_eq!(count_tenants(&pool, Some("%")).await.unwrap(), 0);
    assert_eq!(
        list_tenants(&pool, Some("dvar"), 0, 50).await.unwrap()[0].id,
        zeta.id,
        "a renamed tenant is found by its new name"
    );
    assert_eq!(
        count_tenants(&pool, Some("cm")).await.unwrap(),
        1,
        "shorter than the index cuts, still found"
    );

    let mine = tenants_of_user(&pool, &alice).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].tenant_id, acme.id);
    assert_eq!(mine[0].slug, "acme");
    assert_eq!(mine[0].role, "owner");
    assert!(!mine[0].suspended);
    assert!(tenants_of_user(&pool, &bob).await.unwrap()[0].suspended);
    assert!(tenants_of_user(&pool, "nobody").await.unwrap().is_empty());

    let found = tenant_by_group(&pool, group_of(&acme.id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, acme.id);
    assert!(tenant_by_group(&pool, 0xdead_beef).await.unwrap().is_none());

    // Delivering everything again changes nothing.
    read_model_subscription::<evento::Sqlite>(pool.clone())
        .run_once(&executor)
        .await
        .unwrap();
    assert_eq!(list_tenants(&pool, None, 0, 50).await.unwrap(), rows);
    assert_eq!(tenants_of_user(&pool, &alice).await.unwrap(), mine);
}

#[tokio::test]
async fn invitations_are_listed_while_open() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let alice = new_id();
    let bob = new_id();
    let now = 1_800_000_000;
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    let invite = |email: &str, at: u64| {
        cmd.invite_member(
            &alice,
            InviteMember {
                tenant_id: tenant.id.clone(),
                email: email.to_owned(),
                role: "owner".to_owned(),
            },
            at,
        )
    };
    let first = invite("first@example.com", now).await.unwrap();
    let second = invite("second@example.com", now + 10).await.unwrap();
    let third = invite("third@example.com", now + 20).await.unwrap();
    cmd.revoke_invitation(&alice, &third.invitation.id, now + 30)
        .await
        .unwrap();

    catch_up(&executor, &pool).await;

    let open = open_invitations_of_tenant(&pool, &tenant.id, now + 30)
        .await
        .unwrap();
    assert_eq!(open.len(), 2);
    assert_eq!(
        count_open_invitations_of_tenant(&pool, &tenant.id, now + 30)
            .await
            .unwrap(),
        2
    );
    assert_eq!(open[0].email, "second@example.com", "newest first");
    assert_eq!(open[1].id, first.invitation.id);
    assert_eq!(open[1].invited_by, alice);
    assert_eq!(open[1].expires_at as u64, first.invitation.expires_at);
    assert!(
        open_invitations_of_tenant(&pool, &tenant.id, second.invitation.expires_at)
            .await
            .unwrap()
            .is_empty(),
        "expired ones drop out"
    );
    assert_eq!(
        count_open_invitations_of_tenant(&pool, &tenant.id, second.invitation.expires_at)
            .await
            .unwrap(),
        0
    );
    let revoked = invitation_by_id(&pool, &third.invitation.id)
        .await
        .unwrap()
        .unwrap();
    assert!(revoked.revoked && revoked.accepted_by.is_none());

    cmd.accept_invitation(
        AcceptInvitation {
            token: second.token.clone(),
            user_id: bob.clone(),
        },
        now + 40,
    )
    .await
    .unwrap();
    catch_up(&executor, &pool).await;
    catch_up(&executor, &pool).await;
    let open = open_invitations_of_tenant(&pool, &tenant.id, now + 50)
        .await
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(
        count_open_invitations_of_tenant(&pool, &tenant.id, now + 50)
            .await
            .unwrap(),
        1
    );
    let accepted = invitation_by_id(&pool, &second.invitation.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted.accepted_by.as_deref(), Some(bob.as_str()));
    assert_eq!(accepted.accepted_at, Some(now as i64 + 40));
    assert_eq!(tenants_of_user(&pool, &bob).await.unwrap().len(), 1);
    assert!(invitation_by_id(&pool, "nope").await.unwrap().is_none());
}
