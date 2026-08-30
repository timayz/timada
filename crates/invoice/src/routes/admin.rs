//! Admin pages for invoices. Routes are relative to the mount point — the
//! umbrella admin crate nests this router under `/admin/invoices`.

use askama::Template;
use axum::Router;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use timada_core::AppResult;
use timada_web::HtmlTemplate;

use crate::projections::{AdminInvoiceRow, recent_invoices};
use crate::state::InvoiceState;

/// Enough rows to see what is happening without paginating.
const RECENT_LIMIT: i64 = 100;

pub fn admin_router(state: InvoiceState) -> Router {
    Router::new().route("/", get(index)).with_state(state)
}

#[derive(Template)]
#[template(path = "admin/invoices/index.html")]
struct IndexTemplate {
    invoices: Vec<AdminInvoiceRow>,
}

async fn index(State(state): State<InvoiceState>) -> AppResult<impl IntoResponse> {
    let invoices = recent_invoices(&state.ctx.read_pool, RECENT_LIMIT).await?;

    Ok(HtmlTemplate(IndexTemplate { invoices }))
}
