//! Queries over `articles`, for the blog's list of a tenant.

use timada_core::db::{Db, FromRow};
use timada_core::page::{Cursor, Page, PageFrom, Side, read_page};

/// One row of `articles`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ArticleRow {
    pub tenant_id: String,
    pub id: String,
    pub slug: String,
    pub title: String,
    pub published: bool,
    /// Unix seconds; 0 while the article never came out.
    pub published_at: i64,
    /// Unix seconds: `published_at`, or when a draft was written.
    pub listed_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
}

const ARTICLE_COLUMNS: &str =
    "SELECT tenant_id, id, slug, title, published, published_at, listed_at, updated_at";

/// The statement of a page of the list: the rows on one side of a cursor
/// (`?2`, `?3`), down for the older ones, as the list reads, or up, nearest
/// first, off the index in its order.
pub(super) fn page_statement(older: bool) -> String {
    let tail = if older {
        "listed_at <= ?2 AND (listed_at < ?2 OR id < ?3)
         ORDER BY listed_at DESC, id DESC LIMIT ?4"
    } else {
        "listed_at >= ?2 AND (listed_at > ?2 OR id > ?3)
         ORDER BY listed_at ASC, id ASC LIMIT ?4"
    };
    format!(
        "{ARTICLE_COLUMNS} FROM articles INDEXED BY articles_listed
         WHERE tenant_id = ?1 AND {tail}"
    )
}

/// Where an article stands in the list.
fn cursor_of(row: &ArticleRow) -> Cursor {
    Cursor {
        at: row.listed_at,
        id: row.id.clone(),
    }
}

/// A page of the tenant's articles, the most recent first, `limit` rows at
/// most (see [`timada_core::page`]).
pub async fn articles_of_tenant(
    db: &Db,
    tenant_id: &str,
    from: PageFrom<'_>,
    limit: i64,
) -> anyhow::Result<Page<ArticleRow>> {
    let older = page_statement(true);
    let newer = page_statement(false);
    let read = |side: Side, cursor: Option<Cursor>, n: i64| {
        let statement = match side {
            Side::Older => older.as_str(),
            Side::Newer => newer.as_str(),
        };
        let (at, id) = cursor
            .map(|cursor| (cursor.at, cursor.id))
            .unwrap_or((i64::MAX, String::new()));
        db.fetch_all::<ArticleRow>(statement, (tenant_id.to_owned(), at, id, n))
    };
    read_page(from, limit, read, cursor_of).await
}
