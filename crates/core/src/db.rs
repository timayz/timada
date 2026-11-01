//! The database the read models share with the store.
//!
//! A read model is a table in the store's own file, written by the
//! subscription that follows its streams and read by the pages. The store
//! ([`evento::Turso`]) is opened for several writers at once, so a handler's
//! transaction ([`Db::write`]) runs alongside the node's event writes and is
//! run again, whole, should another writer change the same rows; a page
//! reads on a connection of its own ([`Db::connect`], or the one-shot
//! [`Db::fetch_all`] and friends). The SQL is written as text, with `?N`
//! placeholders and a tuple of values, and rows are read back into a struct
//! deriving [`FromRow`] (one field per column) or a tuple (by position).
//!
//! Full-text search is Turso's index on the table itself: a [`search_key`]
//! column next to each searched one, lowercased and stripped of accents, and
//! an index `USING fts(…) WITH (tokenizer = 'ngram', min_gram = 3, max_gram = 3)`
//! over them, queried with `fts_match(cols…, ?)` and the phrase [`fts_query`]
//! makes of what was typed.

use std::time::Duration;

use anyhow::{Context as _, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use evento::Turso;
pub use evento::turso::turso::{self, Connection, IntoParams, Value, params_from_iter};
pub use evento::turso::write::Body;
pub use evento::turso::{FromRow, FromValue, Row, Text};

use turso::params::Params;

/// The read models' database: the store's file, written through it.
#[derive(Clone)]
pub struct Db(Turso);

impl From<Turso> for Db {
    fn from(store: Turso) -> Self {
        Self(store)
    }
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db").finish_non_exhaustive()
    }
}

impl Db {
    /// The store the tables live in.
    pub fn store(&self) -> &Turso {
        &self.0
    }

    /// A connection for several reads in a row (a page that runs more than
    /// one query). Cheap: connections share the database's pager.
    pub fn connect(&self) -> Result<Connection> {
        self.0.connect()
    }

    /// Runs `body` over `env` as one transaction, committed: what a
    /// subscription handler writes its rows through. The transaction runs
    /// alongside the store's and the other handlers' and is run again, whole,
    /// should another writer change the same rows, so `body` has to be
    /// repeatable: every effect it has goes through the connection it is
    /// lent. Schema changes (`CREATE …`) do not belong in it: they go in the
    /// crate's [`Migrations`](evento::turso::migrate::Migrations).
    pub async fn write<T, E, F>(&self, env: &E, body: F) -> Result<T>
    where
        E: ?Sized + Sync,
        F: for<'c> FnMut(&'c Connection, &'c E) -> Body<'c, T>,
    {
        self.0.transaction(env, body).await
    }

    /// Runs one statement that returns no rows in a transaction of its own;
    /// the rows affected.
    pub async fn execute(&self, sql: &str, params: impl IntoParams) -> Result<u64> {
        let params = params.into_params()?;
        self.write(&(sql, params), |tx, (sql, params)| {
            Box::pin(async move { execute(tx, sql, params.clone()).await })
        })
        .await
    }

    /// Every row `sql` returns, decoded as `T`.
    pub async fn fetch_all<T: FromRow>(
        &self,
        sql: &str,
        params: impl IntoParams,
    ) -> Result<Vec<T>> {
        fetch_all(&self.connect()?, sql, params).await
    }

    /// The first row `sql` returns, decoded as `T`, if any.
    pub async fn fetch_optional<T: FromRow>(
        &self,
        sql: &str,
        params: impl IntoParams,
    ) -> Result<Option<T>> {
        fetch_optional(&self.connect()?, sql, params).await
    }

    /// The one row `sql` returns, decoded as `T`; an error when there is none.
    pub async fn fetch_one<T: FromRow>(&self, sql: &str, params: impl IntoParams) -> Result<T> {
        fetch_one(&self.connect()?, sql, params).await
    }

    /// The first column of the one row `sql` returns (a count, an id).
    pub async fn fetch_scalar<T: FromValue>(
        &self,
        sql: &str,
        params: impl IntoParams,
    ) -> Result<T> {
        fetch_scalar(&self.connect()?, sql, params).await
    }
}

/// How many times a statement finding the database locked is run again: an
/// MVCC checkpoint stops the world for a moment.
const BUSY_ATTEMPTS: u32 = 200;

/// Runs `sql` on `conn` for its rows: the column names and the rows.
async fn rows(
    conn: &Connection,
    sql: &str,
    params: Params,
) -> Result<(Vec<String>, Vec<turso::Row>)> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let outcome: turso::Result<_> = async {
            let mut rows = conn.query(sql, params.clone()).await?;
            let columns = rows.column_names();
            let mut out = Vec::new();
            while let Some(row) = rows.next().await? {
                out.push(row);
            }
            Ok((columns, out))
        }
        .await;
        match outcome {
            Err(turso::Error::Busy(_)) if attempt < BUSY_ATTEMPTS => {
                tokio::time::sleep(Duration::from_millis(u64::from(Ord::min(attempt, 20)))).await;
            }
            outcome => return outcome.with_context(|| format!("query failed: {}", summary(sql))),
        }
    }
}

/// The first line of a statement, for an error.
fn summary(sql: &str) -> String {
    sql.split_whitespace().take(8).collect::<Vec<_>>().join(" ")
}

/// Runs a statement that returns no rows on `conn` (a transaction's
/// connection, in a [`Db::write`] body); the rows affected.
pub async fn execute(conn: &Connection, sql: &str, params: impl IntoParams) -> Result<u64> {
    conn.execute(sql, params)
        .await
        .with_context(|| format!("statement failed: {}", summary(sql)))
}

/// Every row `sql` returns on `conn`, decoded as `T`.
pub async fn fetch_all<T: FromRow>(
    conn: &Connection,
    sql: &str,
    params: impl IntoParams,
) -> Result<Vec<T>> {
    let (columns, rows) = rows(conn, sql, params.into_params()?).await?;
    rows.iter()
        .map(|row| T::from_row(&Row::new(&columns, row)))
        .collect::<Result<Vec<T>>>()
        .with_context(|| format!("cannot read the rows of: {}", summary(sql)))
}

/// The first row `sql` returns on `conn`, decoded as `T`, if any.
pub async fn fetch_optional<T: FromRow>(
    conn: &Connection,
    sql: &str,
    params: impl IntoParams,
) -> Result<Option<T>> {
    Ok(fetch_all(conn, sql, params).await?.into_iter().next())
}

/// The one row `sql` returns on `conn`, decoded as `T`; an error when there
/// is none.
pub async fn fetch_one<T: FromRow>(
    conn: &Connection,
    sql: &str,
    params: impl IntoParams,
) -> Result<T> {
    fetch_optional(conn, sql, params)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no row for: {}", summary(sql)))
}

/// The first column of the one row `sql` returns on `conn`.
pub async fn fetch_scalar<T: FromValue>(
    conn: &Connection,
    sql: &str,
    params: impl IntoParams,
) -> Result<T> {
    let (_, rows) = rows(conn, sql, params.into_params()?).await?;
    let row = rows
        .first()
        .ok_or_else(|| anyhow::anyhow!("no row for: {}", summary(sql)))?;
    T::from_value(row.get_value(0)?).with_context(|| format!("cannot read: {}", summary(sql)))
}

/// `?1, ?2, …` for `count` values: the list of an `IN (…)`.
pub fn placeholders(count: usize) -> String {
    (1..=count)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A column holding JSON: read back into `T`, written from it with
/// [`Json::to_text`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Json<T>(pub T);

impl<T: DeserializeOwned> FromValue for Json<T> {
    fn from_value(value: Value) -> Result<Self> {
        match value {
            Value::Text(text) => Ok(Json(serde_json::from_str(&text)?)),
            Value::Blob(bytes) => Ok(Json(serde_json::from_slice(&bytes)?)),
            other => anyhow::bail!("expected JSON text, got {other:?}"),
        }
    }
}

impl<T: Serialize> Json<T> {
    /// The JSON text to store.
    pub fn to_text(&self) -> Result<String> {
        Ok(serde_json::to_string(&self.0)?)
    }
}

/// What a searched column is indexed by: the text lowercased and stripped
/// of its accents, so a search ignores both. Kept in a column of its own
/// next to the text it is made of, under the full-text index.
pub fn search_key(text: &str) -> String {
    deunicode::deunicode(text).to_lowercase()
}

/// The full-text query matching `q` anywhere in a word, ignoring case and
/// accents, when `q` is long enough for the index (it is cut in runs of
/// three characters): the folded text as one phrase, so the match is the
/// text itself and not any of its runs. `None` for a shorter `q`, which the
/// caller looks for with `LIKE` instead.
pub fn fts_query(q: &str) -> Option<String> {
    let folded = search_key(q);
    let cleaned: String = folded
        .chars()
        .filter(|c| !matches!(c, '"' | '\\') && !c.is_control())
        .collect();
    let cleaned = cleaned.trim();
    (cleaned.chars().count() >= 3).then(|| format!("\"{cleaned}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_keys_fold_case_and_accents() {
        assert_eq!(search_key("Châtounette Mug"), "chatounette mug");
        assert_eq!(search_key("ÉLODIE"), "elodie");
    }

    #[test]
    fn fts_queries_are_one_folded_phrase_or_nothing() {
        assert_eq!(fts_query("Thé"), Some("\"the\"".into()));
        assert_eq!(fts_query(" tea mug "), Some("\"tea mug\"".into()));
        assert_eq!(fts_query("a\"b\\c"), Some("\"abc\"".into()));
        assert_eq!(fts_query("ab"), None);
        assert_eq!(fts_query("  \"  "), None);
    }

    #[tokio::test]
    async fn rows_read_back_as_structs_tuples_and_scalars() {
        #[derive(FromRow, Debug, PartialEq)]
        struct Thing {
            id: String,
            n: i64,
            #[turso(rename = "meta_json")]
            meta: Json<Vec<u32>>,
            #[turso(default)]
            seen: bool,
        }
        let db = Db::from(Turso::in_memory_concurrent().await.unwrap());
        db.connect()
            .unwrap()
            .execute(
                "CREATE TABLE thing (id TEXT PRIMARY KEY, n INTEGER NOT NULL, meta_json TEXT NOT NULL)",
                (),
            )
            .await
            .unwrap();
        db.execute(
            "INSERT INTO thing (id, n, meta_json) VALUES (?1, ?2, ?3)",
            ("a", 3i64, Json(vec![1u32, 2]).to_text().unwrap()),
        )
        .await
        .unwrap();
        db.write(&("b", 5i64), |tx, (id, n)| {
            Box::pin(async move {
                execute(
                    tx,
                    "INSERT INTO thing (id, n, meta_json) VALUES (?1, ?2, '[]')",
                    (*id, *n),
                )
                .await?;
                Ok(())
            })
        })
        .await
        .unwrap();

        let things: Vec<Thing> = db
            .fetch_all("SELECT id, n, meta_json FROM thing ORDER BY id", ())
            .await
            .unwrap();
        assert_eq!(things.len(), 2);
        assert_eq!(things[0].meta, Json(vec![1, 2]));
        assert!(!things[0].seen);
        let pair: (String, i64) = db
            .fetch_one("SELECT id, n FROM thing WHERE id = ?1", ("b",))
            .await
            .unwrap();
        assert_eq!(pair, ("b".into(), 5));
        let count: i64 = db
            .fetch_scalar("SELECT COUNT(*) FROM thing WHERE n > ?1", (1i64,))
            .await
            .unwrap();
        assert_eq!(count, 2);
        let none: Option<Thing> = db
            .fetch_optional("SELECT id, n, meta_json FROM thing WHERE id = 'z'", ())
            .await
            .unwrap();
        assert!(none.is_none());
        let ids = ["a", "b", "c"];
        let found: Vec<(String,)> = db
            .fetch_all(
                &format!(
                    "SELECT id FROM thing WHERE id IN ({}) ORDER BY id",
                    placeholders(ids.len())
                ),
                params_from_iter(ids),
            )
            .await
            .unwrap();
        assert_eq!(found.len(), 2);
    }
}
