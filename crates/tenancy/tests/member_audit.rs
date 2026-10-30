mod common;

use common::*;
use timada_tenancy::{
    AUDIT_GRANTED, AUDIT_INVITATION_ACCEPTED, AUDIT_INVITATION_REVOKED, AUDIT_INVITED,
    AUDIT_REVOKED, AUDIT_ROLE_CHANGED, AcceptInvitation, Command, DefineRole, InviteMember,
    MemberAuditRow, PERMISSION_MEMBERS_MANAGE, invitation_id, member_audit_of_tenant,
    member_audit_subscription,
};

const NOW: u64 = 1_800_000_000;

/// Events of different streams within one millisecond come in either order;
/// a step apart, they come in the order they were written.
async fn tick() {
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
}

async fn catch_up(executor: &evento::Sqlite, pool: &sqlx::SqlitePool) {
    member_audit_subscription::<evento::Sqlite>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

fn invite(tenant_id: &str, email: &str, role: &str) -> InviteMember {
    InviteMember {
        tenant_id: tenant_id.to_owned(),
        email: email.to_owned(),
        role: role.to_owned(),
    }
}

/// `(action, actor, user, email, role)`, for comparing whole histories.
type Line = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn lines(rows: &[MemberAuditRow]) -> Vec<Line> {
    rows.iter()
        .map(|row| {
            (
                row.action.clone(),
                row.actor.clone(),
                row.user_id.clone(),
                row.email.clone(),
                row.role.clone(),
            )
        })
        .collect()
}

fn line(
    action: &str,
    actor: &str,
    user: Option<&str>,
    email: Option<&str>,
    role: Option<&str>,
) -> Line {
    (
        action.to_owned(),
        actor.to_owned(),
        user.map(str::to_owned),
        email.map(str::to_owned),
        role.map(str::to_owned),
    )
}

#[tokio::test]
async fn the_history_names_who_did_what_newest_first() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let alice = new_id();
    let bob = new_id();
    let carol = new_id();
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

    tick().await;
    let sent = cmd
        .invite_member(
            &alice,
            invite(&tenant.id, "bob@example.com", "support"),
            NOW,
        )
        .await
        .unwrap();
    tick().await;
    cmd.accept_invitation(
        AcceptInvitation {
            token: sent.token,
            user_id: bob.clone(),
        },
        NOW + 1,
    )
    .await
    .unwrap();
    tick().await;
    cmd.change_member_role(&alice, &tenant.id, &bob, "manager")
        .await
        .unwrap();
    tick().await;
    let withdrawn = cmd
        .invite_member(
            &bob,
            invite(&tenant.id, "eve@example.com", "support"),
            NOW + 2,
        )
        .await
        .unwrap();
    tick().await;
    cmd.revoke_invitation(&alice, &invitation_id(&withdrawn.token), NOW + 3)
        .await
        .unwrap();
    tick().await;
    cmd.grant_member(&bob, &tenant.id, &carol, "support")
        .await
        .unwrap();
    tick().await;
    cmd.revoke_member(&carol, &tenant.id, &carol).await.unwrap();
    tick().await;
    cmd.revoke_member(&alice, &tenant.id, &bob).await.unwrap();

    catch_up(&executor, &pool).await;

    let rows = member_audit_of_tenant(&pool, &tenant.id, None, 50)
        .await
        .unwrap();
    let bob_mail = Some("bob@example.com");
    let eve_mail = Some("eve@example.com");
    let mut history = lines(&rows);
    // Accepting writes the grant and the acceptance in one go.
    history[6..8].sort();
    let mut joined = vec![
        line(
            AUDIT_INVITATION_ACCEPTED,
            &bob,
            Some(&bob),
            bob_mail,
            Some("support"),
        ),
        line(AUDIT_GRANTED, &bob, Some(&bob), None, Some("support")),
    ];
    joined.sort();
    let expected: Vec<Line> = [
        vec![
            line(AUDIT_REVOKED, &alice, Some(&bob), None, None),
            line(AUDIT_REVOKED, &carol, Some(&carol), None, None),
            line(AUDIT_GRANTED, &bob, Some(&carol), None, Some("support")),
            line(
                AUDIT_INVITATION_REVOKED,
                &alice,
                None,
                eve_mail,
                Some("support"),
            ),
            line(AUDIT_INVITED, &bob, None, eve_mail, Some("support")),
            line(
                AUDIT_ROLE_CHANGED,
                &alice,
                Some(&bob),
                None,
                Some("manager"),
            ),
        ],
        joined,
        vec![
            line(AUDIT_INVITED, &alice, None, bob_mail, Some("support")),
            line(AUDIT_GRANTED, &alice, Some(&alice), None, Some("owner")),
        ],
    ]
    .concat();
    assert_eq!(history, expected);
    assert!(
        rows.iter()
            .all(|row| row.tenant_id == tenant.id && row.at > 0)
    );

    // Another tenant's history is its own.
    let other = create_tenant(&cmd, "Zeta", &carol).await;
    catch_up(&executor, &pool).await;
    let theirs = member_audit_of_tenant(&pool, &other.id, None, 50)
        .await
        .unwrap();
    assert_eq!(
        lines(&theirs),
        vec![line(
            AUDIT_GRANTED,
            &carol,
            Some(&carol),
            None,
            Some("owner")
        )]
    );

    // Pages continue from the last row shown.
    let first = member_audit_of_tenant(&pool, &tenant.id, None, 4)
        .await
        .unwrap();
    let second = member_audit_of_tenant(&pool, &tenant.id, Some(first[3].seq), 4)
        .await
        .unwrap();
    let last = member_audit_of_tenant(&pool, &tenant.id, Some(second[3].seq), 4)
        .await
        .unwrap();
    let paged: Vec<_> = [first, second, last].concat();
    assert_eq!(paged, rows);
}

#[tokio::test]
async fn a_repeated_delivery_records_nothing_twice() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let alice = new_id();
    let tenant = create_tenant(&cmd, "Acme", &alice).await;
    catch_up(&executor, &pool).await;

    // A fresh cursor replays the whole log over the rows already there.
    sqlx::query("DELETE FROM subscriber")
        .execute(&pool)
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    let rows = member_audit_of_tenant(&pool, &tenant.id, None, 50)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
}
