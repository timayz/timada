mod common;

use timada_core::db::Db;

use common::*;
use timada_tenancy::{
    AcceptInvitation, Command, InviteMember, SentCounts, count_open_invitations_of_tenant,
    count_tenants, invitation_by_id, invitations_sent_since, list_tenants,
    open_invitations_of_tenant, read_model_subscription, tenants_of_user,
};

async fn catch_up(executor: &evento::Turso, pool: &Db) {
    read_model_subscription::<evento::Turso>(pool.clone())
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

    // Delivering everything again changes nothing.
    read_model_subscription::<evento::Turso>(pool.clone())
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

#[tokio::test]
async fn invitations_sent_are_counted_by_tenant_and_by_account() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let alice = new_id();
    let bob = new_id();
    let now = 1_800_000_000;
    let acme = create_tenant(&cmd, "Acme", &alice).await;
    let beta = create_tenant(&cmd, "Beta", &alice).await;
    let zeta = create_tenant(&cmd, "Zeta", &bob).await;
    async fn invite(
        cmd: &Command<evento::Turso>,
        by: &str,
        tenant_id: &str,
        email: &str,
        at: u64,
    ) -> timada_tenancy::SentInvitation {
        cmd.invite_member(
            by,
            InviteMember {
                tenant_id: tenant_id.to_owned(),
                email: email.to_owned(),
                role: "owner".to_owned(),
            },
            at,
        )
        .await
        .unwrap()
    }
    invite(&cmd, &alice, &acme.id, "old@example.com", now - 1).await;
    invite(&cmd, &alice, &acme.id, "a@example.com", now).await;
    let revoked = invite(&cmd, &alice, &acme.id, "b@example.com", now + 10).await;
    cmd.revoke_invitation(&alice, &revoked.invitation.id, now + 20)
        .await
        .unwrap();
    invite(&cmd, &alice, &beta.id, "c@example.com", now + 30).await;
    invite(&cmd, &bob, &zeta.id, "d@example.com", now + 40).await;

    catch_up(&executor, &pool).await;

    assert_eq!(
        invitations_sent_since(&pool, &acme.id, &alice, now)
            .await
            .unwrap(),
        SentCounts {
            tenant: 2,
            account: 3
        },
        "the window starts at `since`; revoked ones were still mailed; the account spans tenants"
    );
    assert_eq!(
        invitations_sent_since(&pool, &zeta.id, &bob, 0)
            .await
            .unwrap(),
        SentCounts {
            tenant: 1,
            account: 1
        }
    );
    assert_eq!(
        invitations_sent_since(&pool, &acme.id, &alice, now + 31)
            .await
            .unwrap(),
        SentCounts {
            tenant: 0,
            account: 0
        }
    );
}
