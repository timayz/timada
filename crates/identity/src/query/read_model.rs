//! The subscription that keeps the SQL read models current.
//!
//! Each handler reloads the strict write model of the aggregate the event
//! belongs to and rewrites its row, so delivery may repeat (the subscription
//! is at-least-once) without the row drifting: streams are a handful of
//! events and the write models are already the truth.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::{Session, User};
use crate::command::{SessionState, UserState, session_projection, user_projection};

/// The cursor identity of the read-model subscription. One per node.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-identity/read-models";

/// Builds the subscription over every routing key (administrators see every
/// region). `pool` must be allowed to write; start it with
/// `.start(&executor)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(pool: SqlitePool) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(pool)
        .handler(on_user_event())
        .handler(on_session_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_user_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<User>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(user) = user_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_user(&pool, &user).await
}

#[evento::subscription_all]
async fn on_session_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Session>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(session) = session_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_session(&pool, &session).await
}

/// The user's row and what a search finds it by, in one transaction.
async fn upsert_user(pool: &SqlitePool, user: &UserState) -> anyhow::Result<()> {
    let revoked = user.sessions_revoked.as_ref();
    let mut tx = pool.begin().await?;
    let was: Option<(String, String)> =
        sqlx::query_as("SELECT account, label FROM identity_users WHERE id = ?1")
            .bind(&user.id)
            .fetch_optional(&mut *tx)
            .await?;
    sqlx::query(
        "INSERT INTO identity_users (id, account, label, is_admin, deleted, registered_at,
            last_sign_in_at, sign_in_count, oauth2_count, passkey_count, sessions_revoked_at_ms,
            email, has_password)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT (id) DO UPDATE SET
            account = excluded.account, label = excluded.label,
            is_admin = excluded.is_admin, deleted = excluded.deleted,
            registered_at = excluded.registered_at, last_sign_in_at = excluded.last_sign_in_at,
            sign_in_count = excluded.sign_in_count, oauth2_count = excluded.oauth2_count,
            passkey_count = excluded.passkey_count,
            sessions_revoked_at_ms = excluded.sessions_revoked_at_ms,
            email = excluded.email, has_password = excluded.has_password",
    )
    .bind(&user.id)
    .bind(&user.account)
    .bind(&user.label)
    .bind(user.is_admin)
    .bind(user.deleted)
    .bind(user.registered_at as i64)
    .bind(user.last_sign_in().map(|s| s.at as i64))
    .bind(user.sign_ins.len() as i64)
    .bind(user.oauth2_accounts.len() as i64)
    .bind(user.passkeys.len() as i64)
    .bind(revoked.map(|r| r.at_ms as i64))
    .bind(user.password.as_ref().map(|p| p.email.as_str()))
    .bind(user.password.is_some())
    .execute(&mut *tx)
    .await?;
    // A new row is one more user to the administrator's list; a repeated
    // delivery finds the row there and counts nothing.
    if was.is_none() {
        sqlx::query("UPDATE identity_totals SET users = users + 1 WHERE id = 1")
            .execute(&mut *tx)
            .await?;
    }
    // A sign-in changes the row but not what a search finds it by.
    if was
        .as_ref()
        .is_none_or(|(account, label)| *account != user.account || *label != user.label)
    {
        sqlx::query(
            "DELETE FROM identity_users_fts
             WHERE rowid = (SELECT rowid FROM identity_users WHERE id = ?1)",
        )
        .bind(&user.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO identity_users_fts (rowid, account, label)
             SELECT rowid, account, label FROM identity_users WHERE id = ?1",
        )
        .bind(&user.id)
        .execute(&mut *tx)
        .await?;
    }

    // Sessions the revocation voided show as ended; the spared one stays.
    if let Some(revoked) = revoked {
        sqlx::query(
            "UPDATE identity_sessions SET ended = 1
             WHERE user_id = ?1 AND started_at_ms <= ?2 AND (?3 IS NULL OR id <> ?3)",
        )
        .bind(&user.id)
        .bind(revoked.at_ms as i64)
        .bind(revoked.except_session.as_deref())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn upsert_session(pool: &SqlitePool, session: &SessionState) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO identity_sessions (id, user_id, started_at, started_at_ms, expires_at, ended,
            user_agent, ip)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (id) DO UPDATE SET
            expires_at = excluded.expires_at,
            ended = identity_sessions.ended OR excluded.ended",
    )
    .bind(&session.id)
    .bind(&session.user_id)
    .bind(session.started_at as i64)
    .bind(session.started_at_ms as i64)
    .bind(session.expires_at as i64)
    .bind(session.ended)
    .bind(session.user_agent.as_deref())
    .bind(session.ip.as_deref())
    .execute(pool)
    .await?;
    Ok(())
}
