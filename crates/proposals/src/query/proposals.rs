//! Queries over `proposals`, for the proposals page of a tenant.

use timada_core::db::{Db, FromRow, Json};
use timada_core::page::{Cursor, Page, PageFrom, Side, read_page};

use crate::aggregator::Change;
use crate::value_object::ProposalStatus;

/// One row of `proposals`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ProposalRow {
    pub tenant_id: String,
    pub id: String,
    pub kind: String,
    pub subject_id: String,
    pub subject_name: String,
    pub source: String,
    pub prepared_by: String,
    pub reason: String,
    /// As prepared while it waits, as approved after.
    pub changes: Json<Vec<Change>>,
    /// Whether the member who approved it changed a value first.
    pub edited: bool,
    pub status: String,
    /// Unix seconds after which it is no longer worth approving.
    pub valid_until: i64,
    /// Unix seconds.
    pub prepared_at: i64,
    /// Who approved or refused it; empty otherwise.
    pub answered_by: String,
    /// What ended it, in words.
    pub note: String,
    /// Unix seconds: when it was prepared while it waits, when it ended
    /// after.
    pub listed_at: i64,
}

impl ProposalRow {
    pub fn status(&self) -> ProposalStatus {
        ProposalStatus::parse(&self.status).unwrap_or_default()
    }

    /// Whether it waited past its date at `now` (unix seconds).
    pub fn is_expired(&self, now: u64) -> bool {
        self.status() == ProposalStatus::Waiting && now as i64 > self.valid_until
    }
}

/// The columns of [`ProposalRow`].
const COLUMNS: &str = "SELECT tenant_id, id, kind, subject_id, subject_name, source,
    prepared_by, reason, changes, edited, status, valid_until, prepared_at,
    answered_by, note, listed_at";

/// A page of a tenant's proposals on either side of `(?2, ?3)`
/// (`listed_at`, `id`), `?4` rows at most: the ones that wait, or the ones
/// that ended; walking down from the cursor (`older`), as the list reads,
/// or up, nearest first. Off the index of the list, in its order.
pub(super) fn page_statement(waiting: bool, older: bool) -> String {
    let waiting = i64::from(waiting);
    let tail = if older {
        "listed_at <= ?2 AND (listed_at < ?2 OR id < ?3)
         ORDER BY listed_at DESC, id DESC LIMIT ?4"
    } else {
        "listed_at >= ?2 AND (listed_at > ?2 OR id > ?3)
         ORDER BY listed_at ASC, id ASC LIMIT ?4"
    };
    format!(
        "{COLUMNS} FROM proposals INDEXED BY proposals_listed
         WHERE tenant_id = ?1 AND waiting = {waiting} AND {tail}"
    )
}

fn cursor_of(row: &ProposalRow) -> Cursor {
    Cursor {
        at: row.listed_at,
        id: row.id.clone(),
    }
}

async fn page(
    db: &Db,
    tenant_id: &str,
    waiting: bool,
    from: PageFrom<'_>,
    limit: i64,
) -> anyhow::Result<Page<ProposalRow>> {
    let older = page_statement(waiting, true);
    let newer = page_statement(waiting, false);
    let read = |side: Side, cursor: Option<Cursor>, n: i64| {
        let (statement, top) = match side {
            Side::Older => (older.as_str(), i64::MAX),
            Side::Newer => (newer.as_str(), i64::MIN),
        };
        let (at, id) = cursor
            .map(|cursor| (cursor.at, cursor.id))
            .unwrap_or((top, String::new()));
        db.fetch_all::<ProposalRow>(statement, (tenant_id.to_owned(), at, id, n))
    };
    read_page(from, limit, read, cursor_of).await
}

/// A page of the tenant's proposals nobody answered yet, the latest
/// prepared first, those past their date included (see
/// [`timada_core::page`]).
pub async fn waiting_proposals(
    db: &Db,
    tenant_id: &str,
    from: PageFrom<'_>,
    limit: i64,
) -> anyhow::Result<Page<ProposalRow>> {
    page(db, tenant_id, true, from, limit).await
}

/// A page of the tenant's proposals that ended (approved, refused, out of
/// date), the latest ended first.
pub async fn answered_proposals(
    db: &Db,
    tenant_id: &str,
    from: PageFrom<'_>,
    limit: i64,
) -> anyhow::Result<Page<ProposalRow>> {
    page(db, tenant_id, false, from, limit).await
}

pub(super) const WAITING_ABOUT: &str = "SELECT id FROM proposals
    INDEXED BY proposals_listed WHERE tenant_id = ?1 AND waiting = 1
    AND kind = ?2 AND subject_id = ?3
    ORDER BY listed_at DESC, id DESC LIMIT 1";

/// The latest of the tenant's waiting proposals of `kind` about
/// `subject_id`, by id: what a page that prepares one shows in place of
/// its form.
pub async fn waiting_about(
    db: &Db,
    tenant_id: &str,
    kind: &str,
    subject_id: &str,
) -> anyhow::Result<Option<String>> {
    let row: Option<(String,)> = db
        .fetch_optional(WAITING_ABOUT, (tenant_id, kind, subject_id))
        .await?;
    Ok(row.map(|(id,)| id))
}

pub(super) const COUNT_WAITING: &str = "SELECT COUNT(*) FROM proposals
    INDEXED BY proposals_listed WHERE tenant_id = ?1 AND waiting = 1";

/// How many of the tenant's proposals wait for an answer.
pub async fn count_waiting(db: &Db, tenant_id: &str) -> anyhow::Result<u64> {
    let count: i64 = db.fetch_scalar(COUNT_WAITING, (tenant_id,)).await?;
    Ok(count.max(0) as u64)
}

pub(super) const WAITING_SOURCES: &str = "SELECT source, COUNT(*) FROM proposals
    INDEXED BY proposals_listed WHERE tenant_id = ?1 AND waiting = 1
    GROUP BY source ORDER BY source";

/// What prepared the tenant's waiting proposals, with how many each: what
/// the waiting list is grouped by.
pub async fn waiting_sources(db: &Db, tenant_id: &str) -> anyhow::Result<Vec<(String, u64)>> {
    let rows: Vec<(String, i64)> = db.fetch_all(WAITING_SOURCES, (tenant_id,)).await?;
    Ok(rows
        .into_iter()
        .map(|(source, count)| (source, count.max(0) as u64))
        .collect())
}
