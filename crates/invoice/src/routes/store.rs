//! The customer's copy of the invoice. Merged at the site root, so this path
//! is a public URL.

use askama::Template;
use axum::Router;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::routing::get;
use timada_core::{AppError, AppResult};
use timada_web::HtmlTemplate;

use crate::commands::invoice_id;
use crate::state::InvoiceState;
use crate::view::{InvoiceView, load_invoice};

pub fn store_router(state: InvoiceState) -> Router {
    Router::new()
        .route("/orders/{order_id}/invoice", get(invoice_document))
        .with_state(state)
}

/// A standalone printable page, not a storefront one — it deliberately does not
/// extend `layout.html`, because a document that is meant to be printed and
/// filed should not carry a shop's navigation into the filing cabinet.
#[derive(Template)]
#[template(path = "store/invoice.html")]
struct InvoiceTemplate {
    invoice: InvoiceView,
}

/// Replayed rather than read from `admin_invoice_list`: this is a
/// read-your-own-write over one SQLite file, so a customer following the link
/// the moment their order is paid gets the invoice that was just issued. The
/// SQL read model is fed by a subscription and would still be empty.
///
/// An order with no invoice is a 404 rather than an empty page — until the
/// charge lands there is genuinely no document to show.
async fn invoice_document(
    State(state): State<InvoiceState>,
    Path(order_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    let invoice = load_invoice(&state.ctx.executor, &invoice_id(&order_id))
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(HtmlTemplate(InvoiceTemplate { invoice }))
}
