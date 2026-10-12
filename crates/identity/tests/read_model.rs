mod common;

use common::*;
use timada_identity::{
    Command, OAuth2Mode, RegistrationMode, active_sessions, count_users, list_users,
    read_model_subscription, user_sessions,
};

const NOW: u64 = 1_700_000_000;

async fn catch_up(executor: &evento::Sqlite, pool: &sqlx::SqlitePool) {
    read_model_subscription::<evento::Sqlite>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn users_and_sessions_are_listed_and_stay_idempotent() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let root = register_via_oauth2(&cmd, "google", "root").await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    cmd.register_passkey(register_passkey(
        RegistrationMode::AddToUser,
        Some(&alice),
        &alice,
        "cred-alice",
    ))
    .await
    .unwrap();
    let laptop = [1u8; 32];
    let phone = [2u8; 32];
    cmd.start_session(&laptop, &alice, NOW + 600, context())
        .await
        .unwrap();
    cmd.start_session(&phone, &alice, NOW + 600, context())
        .await
        .unwrap();

    catch_up(&executor, &pool).await;

    assert_eq!(count_users(&pool, None).await.unwrap(), 2);
    let rows = list_users(&pool, None, 0, 50).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].label, "User alice", "sorted by name");
    assert_eq!(rows[0].oauth2_count, 1);
    assert_eq!(rows[0].passkey_count, 1);
    assert_eq!(rows[0].sign_in_count, 1);
    assert!(!rows[0].is_admin);
    assert!(rows[1].is_admin, "root registered first");
    assert_eq!(rows[1].id, root);
    assert_eq!(
        list_users(&pool, Some("ALICE@"), 0, 50)
            .await
            .unwrap()
            .len(),
        1,
        "search is case-insensitive on account and label"
    );
    assert_eq!(
        count_users(&pool, Some("%")).await.unwrap(),
        0,
        "wildcards are literal"
    );

    let sessions = user_sessions(&pool, &alice).await.unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].user_agent.as_deref(), Some("tests"));
    assert_eq!(
        active_sessions(&pool, &alice, NOW as i64)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        active_sessions(&pool, &alice, NOW as i64 + 600)
            .await
            .unwrap()
            .is_empty(),
        "expired"
    );

    // Replaying the whole stream changes nothing.
    read_model_subscription::<evento::Sqlite>(pool.clone())
        .run_once(&executor)
        .await
        .unwrap();
    assert_eq!(count_users(&pool, None).await.unwrap(), 2);
    assert_eq!(user_sessions(&pool, &alice).await.unwrap().len(), 2);
}

#[tokio::test]
async fn revocations_deletions_and_sign_outs_show_up() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let root = register_via_oauth2(&cmd, "google", "root").await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let laptop = [1u8; 32];
    let phone = [2u8; 32];
    cmd.start_session(&laptop, &alice, NOW + 600, context())
        .await
        .unwrap();
    cmd.start_session(&phone, &alice, NOW + 600, context())
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    cmd.revoke_sessions(&alice, &alice, Some(&laptop))
        .await
        .unwrap();
    catch_up(&executor, &pool).await;

    let active = active_sessions(&pool, &alice, NOW as i64).await.unwrap();
    assert_eq!(active.len(), 1, "the asking browser survives");
    assert_eq!(active[0].id, timada_identity::session_id(&laptop));

    cmd.end_session(&laptop).await.unwrap();
    cmd.complete_oauth2(oauth2(OAuth2Mode::Login, None, "google", "alice"))
        .await
        .unwrap();
    cmd.delete_user(&root, &alice).await.unwrap();
    catch_up(&executor, &pool).await;

    assert!(
        active_sessions(&pool, &alice, NOW as i64)
            .await
            .unwrap()
            .is_empty()
    );
    let rows = list_users(&pool, Some("alice"), 0, 50).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].deleted);
    assert_eq!(rows[0].sign_in_count, 2);
    assert!(rows[0].last_sign_in_at.is_some());
    assert_eq!(
        list_users(&pool, None, 0, 50).await.unwrap()[1].id,
        alice,
        "deleted users sort last"
    );
}
