//! The subscription that keeps the read models current.
//!
//! Each handler reloads the strict write model of the aggregate the event
//! belongs to and rewrites its row, so delivery may repeat (the subscription
//! is at-least-once) without the row drifting: streams are a handful of
//! events and the write models are already the truth. The rows live in the
//! store's own database and are written through it ([`Db::write`]).

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Db, execute, fetch_optional, search_key};

use crate::aggregator::{Session, User};
use crate::command::{SessionState, UserState, session_projection, user_projection};

/// The cursor identity of the read-model subscription. One per node.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-identity/read-models";

/// Builds the subscription over every routing key (administrators see every
/// region). Start it with `.start(&executor)` and keep the handle for
/// shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_user_event())
        .handler(on_session_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_user_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<User>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(user) = user_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_user(&db, &user).await
}

#[evento::subscription_all]
async fn on_session_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Session>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(session) = session_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_session(&db, &session).await
}

/// The user's row, what a search finds it by, and the sessions a revocation
/// ended, in one transaction.
async fn upsert_user(db: &Db, user: &UserState) -> anyhow::Result<()> {
    db.write(user, |tx, user| {
        Box::pin(async move {
            let revoked = user.sessions_revoked.as_ref();
            let was: Option<(String,)> = fetch_optional(
                tx,
                "SELECT id FROM identity_users WHERE id = ?1",
                (user.id.as_str(),),
            )
            .await?;
            execute(
                tx,
                "INSERT INTO identity_users (id, account, label, is_admin, deleted, registered_at,
                    last_sign_in_at, sign_in_count, oauth2_count, passkey_count,
                    sessions_revoked_at_ms, email, has_password, account_search, label_search)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 ON CONFLICT (id) DO UPDATE SET
                    account = excluded.account, label = excluded.label,
                    is_admin = excluded.is_admin, deleted = excluded.deleted,
                    registered_at = excluded.registered_at,
                    last_sign_in_at = excluded.last_sign_in_at,
                    sign_in_count = excluded.sign_in_count, oauth2_count = excluded.oauth2_count,
                    passkey_count = excluded.passkey_count,
                    sessions_revoked_at_ms = excluded.sessions_revoked_at_ms,
                    email = excluded.email, has_password = excluded.has_password,
                    account_search = excluded.account_search,
                    label_search = excluded.label_search",
                (
                    user.id.as_str(),
                    user.account.as_str(),
                    user.label.as_str(),
                    user.is_admin,
                    user.deleted,
                    user.registered_at as i64,
                    user.last_sign_in().map(|s| s.at as i64),
                    user.sign_ins.len() as i64,
                    user.oauth2_accounts.len() as i64,
                    user.passkeys.len() as i64,
                    revoked.map(|r| r.at_ms as i64),
                    user.password.as_ref().map(|p| p.email.as_str()),
                    user.password.is_some(),
                    search_key(&user.account),
                    search_key(&user.label),
                ),
            )
            .await?;
            // A new row is one more user to the administrator's list; a
            // repeated delivery finds the row there and counts nothing.
            if was.is_none() {
                execute(
                    tx,
                    "UPDATE identity_totals SET users = users + 1 WHERE id = 1",
                    (),
                )
                .await?;
            }
            // Sessions the revocation voided show as ended; the spared one
            // stays.
            if let Some(revoked) = revoked {
                execute(
                    tx,
                    "UPDATE identity_sessions SET ended = 1
                     WHERE user_id = ?1 AND started_at_ms <= ?2 AND (?3 IS NULL OR id <> ?3)",
                    (
                        user.id.as_str(),
                        revoked.at_ms as i64,
                        revoked.except_session.as_deref(),
                    ),
                )
                .await?;
            }
            Ok(())
        })
    })
    .await
}

async fn upsert_session(db: &Db, session: &SessionState) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO identity_sessions (id, user_id, started_at, started_at_ms, expires_at, ended,
            user_agent, ip)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (id) DO UPDATE SET
            expires_at = excluded.expires_at,
            ended = identity_sessions.ended OR excluded.ended",
        (
            session.id.as_str(),
            session.user_id.as_str(),
            session.started_at as i64,
            session.started_at_ms as i64,
            session.expires_at as i64,
            session.ended,
            session.user_agent.as_deref(),
            session.ip.as_deref(),
        ),
    )
    .await?;
    Ok(())
}
