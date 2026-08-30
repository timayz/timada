//! Admin pages for suppliers. Routes are relative to the mount point — the
//! umbrella admin crate nests this router under `/admin/suppliers`.

use askama::Template;
use axum::Router;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use timada_core::AppResult;
use timada_web::HtmlTemplate;

use crate::projections::{AdminSupplierOrderRow, recent_supplier_orders};
use crate::state::DropshipState;

/// Enough rows to see what is happening without paginating.
const RECENT_LIMIT: i64 = 50;

pub fn admin_router(state: DropshipState) -> Router {
    Router::new().route("/", get(index)).with_state(state)
}

#[derive(Template)]
#[template(path = "admin/suppliers/index.html")]
struct IndexTemplate {
    suppliers: Vec<&'static str>,
    orders: Vec<AdminSupplierOrderRow>,
}

async fn index(State(state): State<DropshipState>) -> AppResult<impl IntoResponse> {
    let orders = recent_supplier_orders(&state.ctx.read_pool, RECENT_LIMIT).await?;

    Ok(HtmlTemplate(IndexTemplate {
        suppliers: state.registry.ids(),
        orders,
    }))
}
