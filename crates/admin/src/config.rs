use evento::Evento;
use sqlx::SqlitePool;

/// How the admin is mounted and styled. Registered as app context.
#[derive(Debug, Clone)]
pub struct AdminConfig {
    /// The single URL segment the admin lives under (`admin` → `/admin/...`).
    /// One segment only: it renames the `admin` module of the route tree.
    pub mount: String,
    /// Where the admin stylesheet comes from.
    pub stylesheet: Stylesheet,
    /// Who issues the shop's invoices: printed on an invoice's print view.
    pub invoice_issuer: timada_invoice::InvoiceIssuer,
    /// How long a paid order may wait for its parcel before the queue of
    /// orders to ship flags it as late. Two days by default.
    pub ship_within: std::time::Duration,
}

#[derive(Debug, Clone)]
pub enum Stylesheet {
    /// The Tailwind stylesheet built by this crate, served from the asset
    /// bundle (requires `topcoat asset bundle` on the host binary).
    Bundled,
    /// A fixed URL (a CDN, or a placeholder in tests).
    Url(String),
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            mount: "admin".into(),
            stylesheet: Stylesheet::Bundled,
            invoice_issuer: timada_invoice::InvoiceIssuer::default(),
            ship_within: std::time::Duration::from_secs(2 * 86_400),
        }
    }
}

impl AdminConfig {
    /// The mount prefix as an absolute path, `/admin`.
    pub fn prefix(&self) -> String {
        format!("/{}", self.mount)
    }
}

/// What the admin needs from the host: the event store and the SQL pool the
/// contexts' read models live in. Registered as app context.
#[derive(Clone)]
pub struct AdminServices {
    pub executor: Evento,
    pub db: SqlitePool,
    /// Where issued invoices are archived, when the shop has an archive: the
    /// invoice page then shows what was filed, checks it, and serves it.
    pub archive: Option<timada_invoice::InvoiceArchive>,
}

impl AdminServices {
    pub fn new<E: evento::Executor>(executor: E, db: SqlitePool) -> Self {
        Self {
            executor: Evento::new(executor),
            db,
            archive: None,
        }
    }

    pub fn with_archive(mut self, archive: timada_invoice::InvoiceArchive) -> Self {
        self.archive = Some(archive);
        self
    }
}
