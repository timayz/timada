//! The SQL outbox: e-mails wait here between the handler that decided to
//! send them and the transport that does. Operational data, not domain
//! facts — so SQL, not events.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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
    pub html_body: Option<String>,
    pub created_at: i64,
    pub sent_at: Option<i64>,
    pub attempts: i64,
    pub last_error: Option<String>,
    /// Unix seconds before which a failed e-mail is not tried again.
    pub next_attempt_at: i64,
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
            (message_id, kind, sender, recipient, subject, body, html_body, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(message_id)
    .bind(kind)
    .bind(&email.from)
    .bind(&email.to)
    .bind(&email.subject)
    .bind(&email.body)
    .bind(&email.html_body)
    .bind(timada_core::time::now_unix_secs()? as i64)
    .execute(db)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// How the outbox is delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryPolicy {
    /// How long to wait before each retry, in order; an e-mail is given up on
    /// after the last one. [`MAX_ATTEMPTS`] is this list plus the first try.
    pub retry_delays: Vec<Duration>,
    /// How long a worker may hold the rows it claimed. A worker that dies
    /// mid-pass keeps them for that long, then another one takes over.
    pub lease: Duration,
    /// Rows claimed per pass.
    pub batch: u32,
}

impl Default for DeliveryPolicy {
    /// 1 min, 5 min, 30 min, 2 h; a five-minute lease; 100 rows a pass.
    fn default() -> Self {
        Self {
            retry_delays: [60, 300, 1_800, 7_200]
                .into_iter()
                .map(Duration::from_secs)
                .collect(),
            lease: Duration::from_secs(300),
            batch: 100,
        }
    }
}

impl DeliveryPolicy {
    /// Retries at once: for tests, and for an operator's "send now".
    pub fn without_delays() -> Self {
        Self {
            retry_delays: vec![Duration::ZERO; (MAX_ATTEMPTS - 1) as usize],
            ..Self::default()
        }
    }

    fn max_attempts(&self) -> i64 {
        self.retry_delays.len() as i64 + 1
    }
}

/// A name no other pass shares: the rows a pass claims are its own.
fn worker_id() -> String {
    static PASSES: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
        PASSES.fetch_add(1, Ordering::Relaxed)
    )
}

/// [`deliver_pending_with`] the default [`DeliveryPolicy`].
pub async fn deliver_pending(
    db: &SqlitePool,
    transport: &dyn Transport,
) -> Result<Delivery, MailError> {
    deliver_pending_with(db, transport, &DeliveryPolicy::default()).await
}

/// Hands the e-mails that are due to the transport, oldest first.
///
/// The pass first **claims** its rows in a single `UPDATE … RETURNING`, so any
/// number of workers can run side by side without sending an e-mail twice; a
/// claim is a lease, and the rows of a worker that died are taken over once
/// it expires. A failure is recorded on the row and retried after the
/// policy's next delay, until the e-mail is given up on.
pub async fn deliver_pending_with(
    db: &SqlitePool,
    transport: &dyn Transport,
    policy: &DeliveryPolicy,
) -> Result<Delivery, MailError> {
    let now = timada_core::time::now_unix_secs()? as i64;
    let worker = worker_id();
    let rows: Vec<OutboxRow> = sqlx::query_as(
        "UPDATE mailer_outbox
         SET claimed_by = ?1, claimed_until = ?2
         WHERE message_id IN (
            SELECT message_id FROM mailer_outbox
            WHERE sent_at IS NULL AND attempts < ?3 AND next_attempt_at <= ?4
              AND (claimed_until IS NULL OR claimed_until < ?4)
            ORDER BY created_at, message_id
            LIMIT ?5)
         RETURNING message_id, kind, sender, recipient, subject, body, html_body, created_at,
                   sent_at, attempts, last_error, next_attempt_at",
    )
    .bind(&worker)
    .bind(now + policy.lease.as_secs() as i64)
    .bind(policy.max_attempts())
    .bind(now)
    .bind(policy.batch)
    .fetch_all(db)
    .await?;

    let mut delivery = Delivery::default();
    for row in rows {
        let email = Email {
            from: row.sender,
            to: row.recipient,
            subject: row.subject,
            body: row.body,
            html_body: row.html_body,
        };
        let now = timada_core::time::now_unix_secs()? as i64;
        match transport.send(&email).await {
            Ok(()) => {
                sqlx::query(
                    "UPDATE mailer_outbox
                     SET sent_at = ?, attempts = attempts + 1, last_error = NULL,
                         claimed_by = NULL, claimed_until = NULL
                     WHERE message_id = ? AND claimed_by = ?",
                )
                .bind(now)
                .bind(&row.message_id)
                .bind(&worker)
                .execute(db)
                .await?;
                delivery.sent += 1;
            }
            Err(err) => {
                tracing::warn!(message_id = %row.message_id, error = %err, "e-mail delivery failed");
                // `attempts` is the count before this one: it indexes the wait
                // before the next. Past the schedule the e-mail is given up on.
                let wait = policy
                    .retry_delays
                    .get(row.attempts.max(0) as usize)
                    .map_or(0, |d| d.as_secs() as i64);
                sqlx::query(
                    "UPDATE mailer_outbox
                     SET attempts = attempts + 1, last_error = ?, next_attempt_at = ?,
                         claimed_by = NULL, claimed_until = NULL
                     WHERE message_id = ? AND claimed_by = ?",
                )
                .bind(err.to_string())
                .bind(now + wait)
                .bind(&row.message_id)
                .bind(&worker)
                .execute(db)
                .await?;
                delivery.failed += 1;
            }
        }
    }
    Ok(delivery)
}

/// Delivers the outbox every `every`, forever. Any number of these can run:
/// each pass claims its own rows.
pub async fn run_delivery(db: SqlitePool, transport: Arc<dyn Transport>, every: Duration) {
    let policy = DeliveryPolicy::default();
    let mut ticker = tokio::time::interval(every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if let Err(err) = deliver_pending_with(&db, transport.as_ref(), &policy).await {
            tracing::error!(error = %err, "e-mail delivery pass failed");
        }
    }
}

/// Gives a failed e-mail a fresh set of attempts.
pub async fn retry(db: &SqlitePool, message_id: &str) -> Result<(), MailError> {
    sqlx::query(
        "UPDATE mailer_outbox SET attempts = 0, last_error = NULL, next_attempt_at = 0
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
        "SELECT message_id, kind, sender, recipient, subject, body, html_body, created_at,
                sent_at, attempts, last_error, next_attempt_at
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
        "SELECT message_id, kind, sender, recipient, subject, body, html_body, created_at,
                sent_at, attempts, last_error, next_attempt_at
         FROM mailer_outbox
         WHERE message_id = ?",
    )
    .bind(message_id)
    .fetch_optional(db)
    .await?)
}
