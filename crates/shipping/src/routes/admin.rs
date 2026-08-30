//! Admin pages for shipments. Routes are relative to the mount point — the
//! umbrella admin crate nests this router under `/admin/shipping`, which is the
//! prefix the template's form actions hardcode.

use askama::Template;
use axum::Router;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use timada_core::{AppError, AppResult};
use timada_dropship::SupplierError;
use timada_web::HtmlTemplate;

use crate::commands::refresh_tracking;
use crate::projections::{AdminShipmentRow, recent_shipments};
use crate::state::ShippingState;

/// Enough rows to see what is happening without paginating.
const RECENT_LIMIT: i64 = 100;

/// Where the refresh action sends the browser back to.
const INDEX_PATH: &str = "/admin/shipping";

pub fn admin_router(state: ShippingState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/{id}/refresh", post(refresh))
        .with_state(state)
}

#[derive(Template)]
#[template(path = "admin/shipping/index.html")]
struct IndexTemplate {
    shipments: Vec<AdminShipmentRow>,
}

async fn index(State(state): State<ShippingState>) -> AppResult<impl IntoResponse> {
    let shipments = recent_shipments(&state.ctx.read_pool, RECENT_LIMIT).await?;

    Ok(HtmlTemplate(IndexTemplate { shipments }))
}

/// Poll the supplier for one shipment, then reload the list.
///
/// The redirect races the admin read model, which a separate subscription
/// writes: a refresh that produced an event may still render the previous
/// status, and a second reload shows it. Reading the shipment's own projection
/// here instead would show fresher data than the rest of the page, which is
/// more confusing than a stale row.
async fn refresh(
    State(state): State<ShippingState>,
    Path(shipment_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    if let Err(source) = refresh_tracking(&state.ctx.executor, &state.registry, &shipment_id).await
    {
        let unknown_supplier = matches!(
            source.downcast_ref::<SupplierError>(),
            Some(SupplierError::UnknownSupplier(_))
        );

        return Err(if unknown_supplier {
            AppError::BadRequest(source.to_string())
        } else {
            AppError::Internal(source)
        });
    }

    Ok(Redirect::to(INDEX_PATH))
}
