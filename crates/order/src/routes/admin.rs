//! Admin pages for orders. Routes are relative to the mount point — the
//! umbrella admin crate nests this router under `/admin/orders`.
//!
//! The list reads the `admin_order_list` SQL table; the detail page replays
//! [`load_order`]. That is a deliberate deviation from "one SQL table per query
//! shape": the detail shape *is* the [`OrderView`] replay, down to the lines and
//! the address, so a second wide denormalized table would only duplicate it —
//! and it would show a staler order than the customer's own page does.

use askama::Template;
use axum::Router;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::routing::get;
use timada_core::{AppError, AppResult};
use timada_web::HtmlTemplate;

use crate::projections::{AdminOrderRow, recent_orders};
use crate::state::OrderState;
use crate::view::{OrderView, load_order};

/// Enough rows to see what is happening without paginating.
const RECENT_LIMIT: i64 = 100;

pub fn admin_router(state: OrderState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/{order_id}", get(detail))
        .with_state(state)
}

#[derive(Template)]
#[template(path = "admin/orders/index.html")]
struct IndexTemplate {
    orders: Vec<AdminOrderRow>,
}

#[derive(Template)]
#[template(path = "admin/orders/detail.html")]
struct DetailTemplate {
    order: OrderView,
}

async fn index(State(state): State<OrderState>) -> AppResult<impl IntoResponse> {
    let orders = recent_orders(&state.ctx.read_pool, RECENT_LIMIT).await?;

    Ok(HtmlTemplate(IndexTemplate { orders }))
}

async fn detail(
    State(state): State<OrderState>,
    Path(order_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    let order = load_order(&state.ctx.executor, &order_id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(HtmlTemplate(DetailTemplate { order }))
}
