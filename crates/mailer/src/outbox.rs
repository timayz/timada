//! The SQL outbox: e-mails wait here between the handler that decided to
//! send them and the transport that does. Operational data, not domain
//! facts — so SQL, not events.

use std::{sync::Arc, time::Duration};

use sqlx::SqlitePool;

use crate::{email::Email, error::MailError, transport::Transport};

/// An e-mail that keeps failing is given up on after this many attempts.
pub const MAX_ATTEMPTS: i64 = 5;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct OutboxRow {
    pub message_id: String,
    /// What the e-mail is about, e.g. `order-confirmation`.
    pub kind: String,
    pub sender: String,
    pub recipient: String,
    pub subject: String,
    pub body: String,
    pub created_at: i64,
    pub sent_at: Option<i64>,
    pub attempts: i64,
    pub last_error: Option<String>,
}

impl OutboxRow {
    pub fn status(&self) -> OutboxStatus {
        if self.sent_at.is_some() {
            OutboxStatus::Sent
        } else if self.attempts >= MAX_ATTEMPTS {
            OutboxStatus::Failed
        } else {
            OutboxStatus::Pending
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxStatus {
    Pending,
    Sent,
    /// Gave up after [`MAX_ATTEMPTS`].
    Failed,
}

/// What a [`deliver_pending`] pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Delivery {
    pub sent: u32,
    pub failed: u32,
}

/// Queues an e-mail under `message_id`. Queuing the same id again is a
/// no-op, which is what makes handlers safe to redeliver. Returns whether it
/// was new.
pub async fn enqueue(
    db: &SqlitePool,
    message_id: &str,
    kind: &str,
    email: &Email,
) -> Result<bool, MailError> {
    let done = sqlx::query(
        "INSERT OR IGNORE INTO mailer_outbox
            (message_id, kind, sender, recipient, subject, body, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(message_id)
    .bind(kind)
    .bind(&email.from)
    .bind(&email.to)
    .bind(&email.subject)
    .bind(&email.body)
    .bind(timada_core::time::now_unix_secs()? as i64)
    .execute(db)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Hands every waiting e-mail to the transport, oldest first. A failure is
/// recorded on the row and retried by a later pass, up to [`MAX_ATTEMPTS`].
/// Run it from a single worker: two concurrent passes could both send the
/// same row.
pub async fn deliver_pending(
    db: &SqlitePool,
    transport: &dyn Transport,
) -> Result<Delivery, MailError> {
    let rows: Vec<OutboxRow> = sqlx::query_as(
        "SELECT message_id, kind, sender, recipient, subject, body, created_at, sent_at,
                attempts, last_error
         FROM mailer_outbox
         WHERE sent_at IS NULL AND attempts < ?
         ORDER BY created_at, message_id
         LIMIT 100",
    )
    .bind(MAX_ATTEMPTS)
    .fetch_all(db)
    .await?;

    let mut delivery = Delivery::default();
    for row in rows {
        let email = Email {
            from: row.sender,
            to: row.recipient,
            subject: row.subject,
            body: row.body,
        };
        match transport.send(&email).await {
            Ok(()) => {
                sqlx::query(
                    "UPDATE mailer_outbox
                     SET sent_at = ?, attempts = attempts + 1, last_error = NULL
                     WHERE message_id = ?",
                )
                .bind(timada_core::time::now_unix_secs()? as i64)
                .bind(&row.message_id)
                .execute(db)
                .await?;
                delivery.sent += 1;
            }
            Err(err) => {
                tracing::warn!(message_id = %row.message_id, error = %err, "e-mail delivery failed");
                sqlx::query(
                    "UPDATE mailer_outbox SET attempts = attempts + 1, last_error = ?
                     WHERE message_id = ?",
                )
                .bind(err.to_string())
                .bind(&row.message_id)
                .execute(db)
                .await?;
                delivery.failed += 1;
            }
        }
    }
    Ok(delivery)
}

/// Delivers the outbox every `every`, forever: spawn it once per deployment.
pub async fn run_delivery(db: SqlitePool, transport: Arc<dyn Transport>, every: Duration) {
    let mut ticker = tokio::time::interval(every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if let Err(err) = deliver_pending(&db, transport.as_ref()).await {
            tracing::error!(error = %err, "e-mail delivery pass failed");
        }
    }
}

/// Gives a failed e-mail a fresh set of attempts.
pub async fn retry(db: &SqlitePool, message_id: &str) -> Result<(), MailError> {
    sqlx::query(
        "UPDATE mailer_outbox SET attempts = 0, last_error = NULL
         WHERE message_id = ? AND sent_at IS NULL",
    )
    .bind(message_id)
    .execute(db)
    .await?;
    Ok(())
}

fn status_clause(status: Option<OutboxStatus>) -> (Option<i64>, i64) {
    // (?1 selector, ?2 max attempts): 0 pending, 1 sent, 2 failed.
    let selector = status.map(|s| match s {
        OutboxStatus::Pending => 0,
        OutboxStatus::Sent => 1,
        OutboxStatus::Failed => 2,
    });
    (selector, MAX_ATTEMPTS)
}

/// The outbox, newest first, optionally one status.
pub async fn list_outbox(
    db: &SqlitePool,
    status: Option<OutboxStatus>,
    limit: u32,
    offset: u32,
) -> Result<Vec<OutboxRow>, MailError> {
    let (selector, max) = status_clause(status);
    Ok(sqlx::query_as(
        "SELECT message_id, kind, sender, recipient, subject, body, created_at, sent_at,
                attempts, last_error
         FROM mailer_outbox
         WHERE ?1 IS NULL
            OR (?1 = 1 AND sent_at IS NOT NULL)
            OR (?1 = 0 AND sent_at IS NULL AND attempts < ?2)
            OR (?1 = 2 AND sent_at IS NULL AND attempts >= ?2)
         ORDER BY created_at DESC, message_id DESC
         LIMIT ?3 OFFSET ?4",
    )
    .bind(selector)
    .bind(max)
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await?)
}

pub async fn count_outbox(db: &SqlitePool, status: Option<OutboxStatus>) -> Result<i64, MailError> {
    let (selector, max) = status_clause(status);
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM mailer_outbox
         WHERE ?1 IS NULL
            OR (?1 = 1 AND sent_at IS NOT NULL)
            OR (?1 = 0 AND sent_at IS NULL AND attempts < ?2)
            OR (?1 = 2 AND sent_at IS NULL AND attempts >= ?2)",
    )
    .bind(selector)
    .bind(max)
    .fetch_one(db)
    .await?)
}

pub async fn load_outbox_message(
    db: &SqlitePool,
    message_id: &str,
) -> Result<Option<OutboxRow>, MailError> {
    Ok(sqlx::query_as(
        "SELECT message_id, kind, sender, recipient, subject, body, created_at, sent_at,
                attempts, last_error
         FROM mailer_outbox
         WHERE message_id = ?",
    )
    .bind(message_id)
    .fetch_optional(db)
    .await?)
}
