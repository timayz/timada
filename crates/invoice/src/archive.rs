//! The archive of issued documents: the PDF of an invoice, rendered once when
//! it is issued, kept unaltered and served from then on — the file a customer
//! downloads in ten years is the one they were sent. Without it a document is
//! re-rendered from events each time, and changes whenever the issuer's
//! address or the layout does.
//!
//! Two parts. The **files** go to an [`ArchiveStore`], a port the host picks:
//! [`SqliteArchiveStore`] (a BLOB table, replicated with the database),
//! [`DirectoryArchiveStore`], or its own. The **index** — `invoice_archive`:
//! number, key, SHA-256, size, date — always lives in SQL, so
//! [`verify_archived`] can tell whether a file is still what was archived.
//! Operational data, like the mailer's outbox: no event.

use std::{
    future::Future,
    path::{Component, Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("`{0}` is not a key an archive accepts")]
    InvalidKey(String),
    #[error("`{0}` is already archived with another content")]
    Conflict(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// The result of a store call, boxed so stores can be `dyn`.
pub type ArchiveFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ArchiveError>> + Send + 'a>>;

/// Where archived files are kept. Keys look like
/// `invoice/2026/F2026-000042.pdf`: relative, `/`-separated, made of letters,
/// digits, `-`, `_` and `.`.
pub trait ArchiveStore: Send + Sync {
    /// Stores `bytes` under `key`. Write-once: the same bytes again are fine,
    /// other bytes are an [`ArchiveError::Conflict`] — an archive never
    /// replaces a file.
    fn put<'a>(&'a self, key: &'a str, bytes: &'a [u8]) -> ArchiveFuture<'a, ()>;

    fn get<'a>(&'a self, key: &'a str) -> ArchiveFuture<'a, Option<Vec<u8>>>;
}

/// The archive, in a shape subscriptions and pages can carry.
#[derive(Clone)]
pub struct InvoiceArchive(pub Arc<dyn ArchiveStore>);

impl InvoiceArchive {
    pub fn new(store: impl ArchiveStore + 'static) -> Self {
        Self(Arc::new(store))
    }
}

fn checked(key: &str) -> Result<&str, ArchiveError> {
    let well_formed = !key.is_empty()
        && key.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        });
    if well_formed {
        Ok(key)
    } else {
        Err(ArchiveError::InvalidKey(key.to_owned()))
    }
}

/// Files in the `invoice_archive_blob` table: nothing to set up, and what
/// replicates the database replicates the archive. Some tens of kilobytes a
/// document; a shop issuing many thousands a year moves to files.
#[derive(Debug, Clone)]
pub struct SqliteArchiveStore {
    db: SqlitePool,
}

impl SqliteArchiveStore {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

impl ArchiveStore for SqliteArchiveStore {
    fn put<'a>(&'a self, key: &'a str, bytes: &'a [u8]) -> ArchiveFuture<'a, ()> {
        Box::pin(async move {
            let key = checked(key)?;
            sqlx::query("INSERT OR IGNORE INTO invoice_archive_blob (key, content) VALUES (?, ?)")
                .bind(key)
                .bind(bytes)
                .execute(&self.db)
                .await?;
            let stored: Vec<u8> =
                sqlx::query_scalar("SELECT content FROM invoice_archive_blob WHERE key = ?")
                    .bind(key)
                    .fetch_one(&self.db)
                    .await?;
            if stored == bytes {
                Ok(())
            } else {
                Err(ArchiveError::Conflict(key.to_owned()))
            }
        })
    }

    fn get<'a>(&'a self, key: &'a str) -> ArchiveFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            let key = checked(key)?;
            Ok(
                sqlx::query_scalar("SELECT content FROM invoice_archive_blob WHERE key = ?")
                    .bind(key)
                    .fetch_optional(&self.db)
                    .await?,
            )
        })
    }
}

/// Files under a directory, one per document. The directory is the host's to
/// back up: it is not part of the database.
#[derive(Debug, Clone)]
pub struct DirectoryArchiveStore {
    root: PathBuf,
}

impl DirectoryArchiveStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_of(&self, key: &str) -> Result<PathBuf, ArchiveError> {
        let relative = Path::new(checked(key)?);
        // `checked` already refuses them; a path is not trusted twice.
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(ArchiveError::InvalidKey(key.to_owned()));
        }
        Ok(self.root.join(relative))
    }
}

impl ArchiveStore for DirectoryArchiveStore {
    fn put<'a>(&'a self, key: &'a str, bytes: &'a [u8]) -> ArchiveFuture<'a, ()> {
        Box::pin(async move {
            let path = self.path_of(key)?;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            // Written aside, then linked into place: the name never shows a
            // half-written file, and an existing file is never replaced.
            let aside = path.with_extension(format!("{}.part", std::process::id()));
            tokio::fs::write(&aside, bytes).await?;
            let linked = tokio::fs::hard_link(&aside, &path).await;
            tokio::fs::remove_file(&aside).await?;
            match linked {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if tokio::fs::read(&path).await? == bytes {
                        Ok(())
                    } else {
                        Err(ArchiveError::Conflict(key.to_owned()))
                    }
                }
                Err(err) => Err(err.into()),
            }
        })
    }

    fn get<'a>(&'a self, key: &'a str) -> ArchiveFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            match tokio::fs::read(self.path_of(key)?).await {
                Ok(bytes) => Ok(Some(bytes)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(err.into()),
            }
        })
    }
}

/// What the index says of an archived document.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ArchivedDocument {
    /// The invoice's id.
    pub document_id: String,
    /// `invoice`.
    pub kind: String,
    /// The legal number: `F2026-000042`.
    pub number: String,
    pub storage_key: String,
    /// Hex, lower case.
    pub sha256: String,
    pub size: i64,
    /// Unix seconds.
    pub archived_at: i64,
    /// Rendered long after the document was issued — the archive did not
    /// exist then — so with the issuer and the layout of the day it was
    /// archived, not of the day it was issued. Frozen since, all the same.
    pub reconstituted: bool,
}

/// Whether an archived file still is what was archived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveCheck {
    Intact,
    /// The file's hash is not the one recorded when it was archived.
    Altered,
    /// The index knows the document, the store has no file for it.
    Missing,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub async fn archived_document(
    db: &SqlitePool,
    document_id: &str,
) -> sqlx::Result<Option<ArchivedDocument>> {
    sqlx::query_as(
        "SELECT document_id, kind, number, storage_key, sha256, size, archived_at, reconstituted
         FROM invoice_archive WHERE document_id = ?",
    )
    .bind(document_id)
    .fetch_optional(db)
    .await
}

/// The archived file of a document with what the index says of it; `None`
/// when it was never archived (or its file is gone — [`verify_archived`] tells
/// which).
pub async fn read_archived(
    db: &SqlitePool,
    store: &dyn ArchiveStore,
    document_id: &str,
) -> Result<Option<(ArchivedDocument, Vec<u8>)>, ArchiveError> {
    let Some(entry) = archived_document(db, document_id).await? else {
        return Ok(None);
    };
    Ok(store
        .get(&entry.storage_key)
        .await?
        .map(|bytes| (entry, bytes)))
}

/// Re-reads an archived file and compares it with the hash taken when it was
/// archived. `None`: the document was never archived.
pub async fn verify_archived(
    db: &SqlitePool,
    store: &dyn ArchiveStore,
    document_id: &str,
) -> Result<Option<ArchiveCheck>, ArchiveError> {
    let Some(entry) = archived_document(db, document_id).await? else {
        return Ok(None);
    };
    Ok(Some(match store.get(&entry.storage_key).await? {
        None => ArchiveCheck::Missing,
        Some(bytes) if sha256_hex(&bytes) == entry.sha256 => ArchiveCheck::Intact,
        Some(_) => ArchiveCheck::Altered,
    }))
}

/// Files a document. Write-once like the store: archiving the same bytes
/// again returns what was recorded the first time.
pub async fn archive_bytes(
    db: &SqlitePool,
    store: &dyn ArchiveStore,
    entry: NewArchive<'_>,
    bytes: &[u8],
) -> Result<ArchivedDocument, ArchiveError> {
    store.put(entry.storage_key, bytes).await?;
    sqlx::query(
        "INSERT OR IGNORE INTO invoice_archive
            (document_id, kind, number, storage_key, sha256, size, archived_at, reconstituted)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(entry.document_id)
    .bind(entry.kind)
    .bind(entry.number)
    .bind(entry.storage_key)
    .bind(sha256_hex(bytes))
    .bind(bytes.len() as i64)
    .bind(timada_core::time::now_unix_secs()? as i64)
    .bind(entry.reconstituted)
    .execute(db)
    .await?;
    archived_document(db, entry.document_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("{} archived but not indexed", entry.document_id).into())
}

/// What [`archive_bytes`] files.
#[derive(Debug, Clone, Copy)]
pub struct NewArchive<'a> {
    pub document_id: &'a str,
    pub kind: &'a str,
    pub number: &'a str,
    pub storage_key: &'a str,
    pub reconstituted: bool,
}

#[cfg(feature = "pdf")]
pub use issue::*;

#[cfg(feature = "pdf")]
mod issue {
    use std::time::Duration;

    use evento::{
        Executor,
        metadata::Event,
        subscription::{Context, SubscriptionBuilder},
    };
    use sqlx::SqlitePool;

    use super::{ArchiveError, ArchiveStore, ArchivedDocument, InvoiceArchive, NewArchive};
    use crate::{
        aggregator::InvoiceIssued,
        document::{InvoiceIssuer, invoice_document},
        pdf::render_invoice_pdf,
        query::load_invoice,
    };

    /// Subscription key. Data: the pool, an [`InvoiceArchive`] and the
    /// [`InvoiceIssuer`]; optionally an [`ArchivePolicy`].
    pub const INVOICE_ARCHIVE_SUBSCRIPTION: &str = "invoice-archive";

    /// Not strict: it only looks at invoices being issued. Started on a shop
    /// with history, it archives every invoice ever issued — flagged
    /// `reconstituted`.
    pub fn invoice_archive_subscription<E: Executor>() -> SubscriptionBuilder<E> {
        SubscriptionBuilder::new(INVOICE_ARCHIVE_SUBSCRIPTION).handler(archive_on_invoice_issued())
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ArchivePolicy {
        /// An invoice archived longer than this after it was issued is flagged
        /// `reconstituted`: it was rendered with the issuer and the layout of
        /// another day. A day by default — a subscription that was down for a
        /// night is no reconstitution.
        pub reconstituted_after: Duration,
    }

    impl Default for ArchivePolicy {
        fn default() -> Self {
            Self {
                reconstituted_after: Duration::from_secs(86_400),
            }
        }
    }

    /// Renders an issued invoice **as issued** — no credit note on it: those
    /// come later and are documents of their own — and files it, unless it is
    /// archived already, in which case what was filed then is returned.
    /// `Ok(None)`: there is no such issued invoice (a draft, or voided since).
    /// Whoever needs the file — the archive's subscription, the mailer, a
    /// download — calls this and gets the same bytes.
    pub async fn archive_invoice<E: Executor>(
        executor: &E,
        db: &SqlitePool,
        store: &dyn ArchiveStore,
        issuer: &InvoiceIssuer,
        invoice_id: &str,
        policy: &ArchivePolicy,
    ) -> Result<Option<(ArchivedDocument, Vec<u8>)>, ArchiveError> {
        if let Some(archived) = super::read_archived(db, store, invoice_id).await? {
            return Ok(Some(archived));
        }
        let Some(invoice) = load_invoice(executor, invoice_id).await? else {
            return Ok(None);
        };
        // The order itself gives its number: no read model to wait for.
        let order_number = timada_order::load_order_details(executor, &invoice.order_id)
            .await?
            .and_then(|order| order.order_number);
        let Some(document) = invoice_document(issuer, invoice, order_number, Vec::new())? else {
            return Ok(None);
        };
        let now = timada_core::time::now_unix_secs()?;
        let reconstituted =
            now.saturating_sub(document.issued_at) > policy.reconstituted_after.as_secs();
        let year = timada_core::time::year_of(document.issued_at);
        let number = document.number.clone();
        let storage_key = format!("invoice/{year}/{number}.pdf");

        let bytes = tokio::task::spawn_blocking(move || render_invoice_pdf(&document))
            .await
            .map_err(anyhow::Error::from)?
            .map_err(anyhow::Error::from)?;
        let entry = super::archive_bytes(
            db,
            store,
            NewArchive {
                document_id: invoice_id,
                kind: "invoice",
                number: &number,
                storage_key: &storage_key,
                reconstituted,
            },
            &bytes,
        )
        .await?;
        Ok(Some((entry, bytes)))
    }

    #[evento::subscription]
    async fn archive_on_invoice_issued<E: Executor>(
        ctx: &Context<'_, E>,
        event: Event<InvoiceIssued>,
    ) -> anyhow::Result<()> {
        let db = ctx
            .get::<SqlitePool>()
            .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
        let archive = ctx
            .get::<InvoiceArchive>()
            .ok_or_else(|| anyhow::anyhow!("InvoiceArchive missing from subscription context"))?;
        let issuer = ctx
            .get::<InvoiceIssuer>()
            .ok_or_else(|| anyhow::anyhow!("InvoiceIssuer missing from subscription context"))?;
        let policy = ctx.get::<ArchivePolicy>().unwrap_or_default();
        archive_invoice(
            ctx.executor,
            &db,
            archive.0.as_ref(),
            &issuer,
            &event.aggregate_id,
            &policy,
        )
        .await?;
        Ok(())
    }
}
