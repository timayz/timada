//! The customer-facing return page. Merged at the site root; the order id in
//! the URL is the capability, exactly like the public order-status page.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};
use axum::routing::get;
use axum::{Form, Router};
use serde::Deserialize;
use timada_core::{AppError, AppResult};
use timada_web::HtmlTemplate;

use crate::commands::{RequestReturnError, request_return, return_id};
use crate::state::ReturnState;
use crate::view::{ReturnView, load_return};

pub fn store_router(state: ReturnState) -> Router {
    Router::new()
        .route(
            "/orders/{order_id}/return",
            get(return_page).post(submit_return),
        )
        .with_state(state)
}

#[derive(Template)]
#[template(path = "store/return.html")]
struct ReturnTemplate {
    order_id: String,
    /// The order's return, if one exists — drives which of form/status shows.
    current: Option<ReturnView>,
    /// Whether the request form shows: no return yet, or the last one was
    /// rejected. Computed here because templates cannot run closures.
    can_request: bool,
    window_days: u32,
}

async fn return_page(
    State(state): State<ReturnState>,
    Path(order_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    // 404 for unknown orders — same policy as the order page.
    if timada_order::load_order(&state.ctx.executor, &order_id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound);
    }

    let current = load_return(&state.ctx.executor, &return_id(&order_id)).await?;
    let can_request = match &current {
        None => true,
        Some(current) => current.status == crate::view::ReturnStatus::Rejected,
    };

    Ok(HtmlTemplate(ReturnTemplate {
        order_id,
        current,
        can_request,
        window_days: state.policy.window_days,
    }))
}

#[derive(Deserialize)]
struct ReturnForm {
    reason: String,
}

/// A plain form post, answered with a redirect so a reload cannot request
/// twice (the command would refuse anyway).
async fn submit_return(
    State(state): State<ReturnState>,
    Path(order_id): Path<String>,
    Form(form): Form<ReturnForm>,
) -> AppResult<Redirect> {
    match request_return(&state.ctx.executor, &state.policy, &order_id, &form.reason).await {
        Ok(_) => Ok(Redirect::to(&format!("/orders/{order_id}/return"))),
        Err(RequestReturnError::UnknownOrder) => Err(AppError::NotFound),
        Err(RequestReturnError::Storage(source)) => Err(AppError::Internal(source)),
        Err(refused) => Err(AppError::BadRequest(refused.to_string())),
    }
}
