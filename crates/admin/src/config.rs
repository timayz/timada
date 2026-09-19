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
}

impl AdminServices {
    pub fn new<E: evento::Executor>(executor: E, db: SqlitePool) -> Self {
        Self {
            executor: Evento::new(executor),
            db,
        }
    }
}
