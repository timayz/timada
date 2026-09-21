//! What operators did, and tried to: one row per request that *writes*,
//! kept by the layer every signed-in page sits behind, plus the sign-ins.
//! The owners read it on the « Journal » page.
//!
//! A row says who (the e-mail and the role as they were), when, what — the
//! method and the path under the mount, which holds the ids — and how it
//! went. It never holds a form: a body may carry a password or a customer's
//! words. Writing it never fails a request; a journal that cannot be written
//! is logged.

use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use super::{role::Role, store::AdminUser};

/// How a request went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It reached its page: what the page answered is in the status.
    Passed,
    /// Stopped at the door: not this role's, a temporary password, or — for
    /// a sign-in — wrong credentials.
    Refused,
    /// The page failed.
    Error,
}

impl Outcome {
    pub const ALL: [Outcome; 3] = [Outcome::Passed, Outcome::Refused, Outcome::Error];

    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Passed => "passed",
            Outcome::Refused => "refused",
            Outcome::Error => "error",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|outcome| outcome.as_str() == value)
    }

    pub fn label(self) -> &'static str {
        match self {
            Outcome::Passed => "Transmis",
            Outcome::Refused => "Refusé",
            Outcome::Error => "Erreur",
        }
    }
}

/// The path a sign-in is journaled under.
pub const SIGN_IN: &str = "login";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub id: i64,
    pub at: i64,
    /// `None`: a sign-in nobody's credentials matched.
    pub admin_id: Option<String>,
    pub email: String,
    pub role: Option<Role>,
    pub method: String,
    /// Under the mount: `orders/o-1/refund`.
    pub path: String,
    pub outcome: Outcome,
    /// The HTTP status answered.
    pub status: u16,
}

impl JournalEntry {
    /// The section's segment: `orders`.
    pub fn section(&self) -> &str {
        self.path.split('/').next().unwrap_or_default()
    }

    /// What was acted on, when the path names it: `o-1`.
    pub fn target(&self) -> Option<&str> {
        let mut segments = self.path.split('/');
        segments.next();
        let target = segments.next()?;
        // `products/new`, `team/new`: an action on the section, no target.
        segments.next().map(|_| target)
    }

    /// What was done: `refund`, `new-discount`, `refunds/settle`.
    pub fn action(&self) -> String {
        let segments: Vec<&str> = self.path.split('/').collect();
        match segments.as_slice() {
            [_section] => String::new(),
            [_section, action] => (*action).to_owned(),
            [_section, _target, action @ ..] => action.join("/"),
            [] => String::new(),
        }
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |now| now.as_secs() as i64)
}

/// Writes a row. Never an error: the request it is about goes on.
pub async fn record(
    db: &SqlitePool,
    who: Option<&AdminUser>,
    email: &str,
    method: &str,
    path: &str,
    outcome: Outcome,
    status: u16,
) {
    let written = sqlx::query(
        "INSERT INTO admin_journal (at, admin_id, email, role, method, path, outcome, status)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(now_secs())
    .bind(who.map(|admin| admin.id.as_str()))
    .bind(email)
    .bind(who.map(|admin| admin.role.as_str()))
    .bind(method)
    .bind(path.trim_matches('/'))
    .bind(outcome.as_str())
    .bind(i64::from(status))
    .execute(db)
    .await;
    if let Err(err) = written {
        tracing::error!(%email, %method, %path, "the journal could not be written: {err}");
    }
}

/// What the « Journal » page lists. Every filter narrows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalFilter {
    pub admin_id: Option<String>,
    pub outcome: Option<Outcome>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for JournalFilter {
    fn default() -> Self {
        Self {
            admin_id: None,
            outcome: None,
            limit: 50,
            offset: 0,
        }
    }
}

fn push_filter(sql: &mut QueryBuilder<Sqlite>, filter: &JournalFilter) {
    sql.push(" FROM admin_journal WHERE 1 = 1");
    if let Some(admin_id) = &filter.admin_id {
        sql.push(" AND admin_id = ").push_bind(admin_id.clone());
    }
    if let Some(outcome) = filter.outcome {
        sql.push(" AND outcome = ").push_bind(outcome.as_str());
    }
}

/// The newest first.
pub async fn list_journal(
    db: &SqlitePool,
    filter: &JournalFilter,
) -> sqlx::Result<Vec<JournalEntry>> {
    let mut sql = QueryBuilder::<Sqlite>::new(
        "SELECT id, at, admin_id, email, role, method, path, outcome, status",
    );
    push_filter(&mut sql, filter);
    sql.push(" ORDER BY id DESC LIMIT ")
        .push_bind(filter.limit)
        .push(" OFFSET ")
        .push_bind(filter.offset);
    type Row = (
        i64,
        i64,
        Option<String>,
        String,
        Option<String>,
        String,
        String,
        String,
        i64,
    );
    let rows: Vec<Row> = sql.build_query_as().fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, at, admin_id, email, role, method, path, outcome, status)| JournalEntry {
                id,
                at,
                admin_id,
                email,
                role: role.as_deref().and_then(Role::parse),
                method,
                path,
                outcome: Outcome::parse(&outcome).unwrap_or(Outcome::Error),
                status: u16::try_from(status).unwrap_or(0),
            },
        )
        .collect())
}

pub async fn count_journal(db: &SqlitePool, filter: &JournalFilter) -> sqlx::Result<i64> {
    let mut sql = QueryBuilder::<Sqlite>::new("SELECT COUNT(*)");
    push_filter(&mut sql, filter);
    sql.build_query_scalar().fetch_one(db).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> JournalEntry {
        JournalEntry {
            id: 1,
            at: 0,
            admin_id: None,
            email: String::new(),
            role: None,
            method: "POST".into(),
            path: path.into(),
            outcome: Outcome::Passed,
            status: 303,
        }
    }

    #[test]
    fn a_path_says_where_on_what_and_what() {
        let refund = entry("orders/o-1/refunds/settle");
        assert_eq!(
            (refund.section(), refund.target(), refund.action().as_str()),
            ("orders", Some("o-1"), "refunds/settle")
        );
        let new = entry("promotions/new-discount");
        assert_eq!(
            (new.section(), new.target(), new.action().as_str()),
            ("promotions", None, "new-discount")
        );
        let own = entry("password");
        assert_eq!(
            (own.section(), own.target(), own.action().as_str()),
            ("password", None, "")
        );
    }
}
