//! Queries over `writer_drafts`, for the writer's page of a tenant.

use timada_core::db::{Db, FromRow};
use timada_core::page::{Cursor, Page, PageFrom, Side, read_page};

use crate::value_object::{DRAFT_STALE_SECS, DraftStatus};

/// One row of `writer_drafts`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct DraftRow {
    pub tenant_id: String,
    pub id: String,
    pub kind: String,
    pub subject_id: String,
    pub subject_name: String,
    pub keyword: String,
    pub requested_by: String,
    pub status: String,
    /// The proposal its texts wait as; empty until drafted.
    pub proposal_id: String,
    pub cost_usd_micros: i64,
    /// What the draft states that the shop's data does not, a line each.
    pub to_check: String,
    /// Why nothing was drafted.
    pub reason: String,
    /// Unix seconds.
    pub requested_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
}

impl DraftRow {
    pub fn status(&self) -> DraftStatus {
        DraftStatus::parse(&self.status).unwrap_or_default()
    }

    /// Whether it is still on its way at `now` (unix seconds): asked for,
    /// not ended, and not so long ago that whoever took it on stopped.
    pub fn is_open(&self, now: u64) -> bool {
        !self.status().is_settled()
            && (now as i64).saturating_sub(self.updated_at) < DRAFT_STALE_SECS as i64
    }
}

/// The columns of [`DraftRow`].
const COLUMNS: &str = "SELECT tenant_id, id, kind, subject_id, subject_name, keyword,
    requested_by, status, proposal_id, cost_usd_micros, to_check, reason, requested_at,
    updated_at";

/// A page of a tenant's drafts on either side of `(?2, ?3)`
/// (`requested_at`, `id`), `?4` rows at most: walking down from the cursor
/// (`older`), as the list reads, or up, nearest first. Off the index of
/// the list, in its order.
pub(super) fn page_statement(older: bool) -> String {
    let tail = if older {
        "requested_at <= ?2 AND (requested_at < ?2 OR id < ?3)
         ORDER BY requested_at DESC, id DESC LIMIT ?4"
    } else {
        "requested_at >= ?2 AND (requested_at > ?2 OR id > ?3)
         ORDER BY requested_at ASC, id ASC LIMIT ?4"
    };
    format!(
        "{COLUMNS} FROM writer_drafts INDEXED BY writer_drafts_listed
         WHERE tenant_id = ?1 AND {tail}"
    )
}

fn cursor_of(row: &DraftRow) -> Cursor {
    Cursor {
        at: row.requested_at,
        id: row.id.clone(),
    }
}

/// A page of the drafts the tenant asked for, the latest first (see
/// [`timada_core::page`]).
pub async fn drafts_of_tenant(
    db: &Db,
    tenant_id: &str,
    from: PageFrom<'_>,
    limit: i64,
) -> anyhow::Result<Page<DraftRow>> {
    let older = page_statement(true);
    let newer = page_statement(false);
    let read = |side: Side, cursor: Option<Cursor>, n: i64| {
        let (statement, top) = match side {
            Side::Older => (older.as_str(), i64::MAX),
            Side::Newer => (newer.as_str(), i64::MIN),
        };
        let (at, id) = cursor
            .map(|cursor| (cursor.at, cursor.id))
            .unwrap_or((top, String::new()));
        db.fetch_all::<DraftRow>(statement, (tenant_id.to_owned(), at, id, n))
    };
    read_page(from, limit, read, cursor_of).await
}

pub(super) const LATEST_DRAFT: &str = "SELECT tenant_id, id, kind, subject_id, subject_name,
    keyword, requested_by, status, proposal_id, cost_usd_micros, to_check, reason, requested_at,
    updated_at
    FROM writer_drafts INDEXED BY writer_drafts_subject
    WHERE tenant_id = ?1 AND subject_id = ?2 AND requested_at >= ?3
    ORDER BY requested_at DESC LIMIT 1";

/// The latest draft asked for the tenant's `subject_id` since `since`
/// (unix seconds), however it went.
pub async fn latest_draft_of(
    db: &Db,
    tenant_id: &str,
    subject_id: &str,
    since: u64,
) -> anyhow::Result<Option<DraftRow>> {
    db.fetch_optional(LATEST_DRAFT, (tenant_id, subject_id, since as i64))
        .await
}

/// The latest draft asked for the tenant's `subject_id`, when it is still
/// on its way at `now` (unix seconds): what keeps a second one from being
/// asked for meanwhile.
pub async fn open_draft_of(
    db: &Db,
    tenant_id: &str,
    subject_id: &str,
    now: u64,
) -> anyhow::Result<Option<DraftRow>> {
    let since = now.saturating_sub(DRAFT_STALE_SECS);
    Ok(latest_draft_of(db, tenant_id, subject_id, since)
        .await?
        .filter(|row| row.is_open(now)))
}
