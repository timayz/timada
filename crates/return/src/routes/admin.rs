//! Admin pages for returns. Routes are relative to the mount point — the
//! umbrella admin crate nests this router under `/admin/returns`, which is
//! the prefix the template's form actions hardcode.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;
use timada_core::AppResult;
use timada_web::HtmlTemplate;

use crate::commands::{approve_return, reject_return};
use crate::projections::{AdminReturnRow, recent_returns};
use crate::state::ReturnState;

/// Enough rows to see what is happening without paginating.
const RECENT_LIMIT: i64 = 100;

/// Where the approve/reject actions send the browser back to.
const INDEX_PATH: &str = "/admin/returns";

pub fn admin_router(state: ReturnState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/{return_id}/approve", post(approve))
        .route("/{return_id}/reject", post(reject))
        .with_state(state)
}

#[derive(Template)]
#[template(path = "admin/returns/index.html")]
struct IndexTemplate {
    returns: Vec<AdminReturnRow>,
}

async fn index(State(state): State<ReturnState>) -> AppResult<impl IntoResponse> {
    let returns = recent_returns(&state.ctx.read_pool, RECENT_LIMIT).await?;

    Ok(HtmlTemplate(IndexTemplate { returns }))
}

/// Approving only appends `ReturnApproved`; the refund is the return-flow
/// subscription's job, so the redirect may briefly show "approved" before
/// "refunded" — refresh shows the truth.
async fn approve(
    State(state): State<ReturnState>,
    Path(return_id): Path<String>,
) -> AppResult<Redirect> {
    approve_return(&state.ctx.executor, &return_id).await?;

    Ok(Redirect::to(INDEX_PATH))
}

#[derive(Deserialize)]
struct RejectForm {
    #[serde(default)]
    reason: String,
}

async fn reject(
    State(state): State<ReturnState>,
    Path(return_id): Path<String>,
    Form(form): Form<RejectForm>,
) -> AppResult<Redirect> {
    reject_return(&state.ctx.executor, &return_id, &form.reason).await?;

    Ok(Redirect::to(INDEX_PATH))
}
