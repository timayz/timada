//! Admin pages for discounts. Routes are relative to the mount point — the
//! umbrella admin crate nests this router under `/admin/discounts`, which is
//! the prefix the template's form actions hardcode.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;
use timada_core::{AppError, AppResult, Currency, Money, now_millis};
use timada_web::HtmlTemplate;

use crate::aggregate::DiscountKind;
use crate::commands::{CreateDiscountError, create_discount, disable_discount};
use crate::projections::{AdminDiscountRow, recent_discounts};
use crate::state::PromotionState;

/// Enough rows to see what is happening without paginating.
const RECENT_LIMIT: i64 = 100;

/// Where the create/disable actions send the browser back to.
const INDEX_PATH: &str = "/admin/discounts";

pub fn admin_router(state: PromotionState) -> Router {
    Router::new()
        .route("/", get(index).post(create))
        .route("/{discount_id}/disable", post(disable))
        .with_state(state)
}

#[derive(Template)]
#[template(path = "admin/discounts/index.html")]
struct IndexTemplate {
    discounts: Vec<AdminDiscountRow>,
}

async fn index(State(state): State<PromotionState>) -> AppResult<impl IntoResponse> {
    let discounts = recent_discounts(&state.ctx.read_pool, RECENT_LIMIT).await?;

    Ok(HtmlTemplate(IndexTemplate { discounts }))
}

#[derive(Deserialize)]
struct CreateForm {
    code: String,
    /// `"percentage"` or `"fixed"`.
    kind: String,
    /// Basis points for a percentage, cents for a fixed amount.
    value: i64,
    /// Only read for fixed amounts.
    #[serde(default)]
    currency: String,
    /// Days from now; empty means open-ended.
    #[serde(default)]
    valid_days: Option<u32>,
    #[serde(default)]
    usage_limit: Option<u32>,
}

async fn create(
    State(state): State<PromotionState>,
    Form(form): Form<CreateForm>,
) -> AppResult<impl IntoResponse> {
    let kind = match form.kind.as_str() {
        "percentage" => DiscountKind::Percentage {
            bps: u32::try_from(form.value)
                .map_err(|_| AppError::BadRequest("a percentage cannot be negative".into()))?,
        },
        "fixed" => {
            let currency = Currency::from_code(form.currency.trim())
                .map_err(|source| AppError::BadRequest(source.to_string()))?;
            DiscountKind::Fixed {
                amount: Money::new(form.value, currency),
            }
        }
        other => {
            return Err(AppError::BadRequest(format!(
                "unknown discount kind `{other}`"
            )));
        }
    };

    let starts_at = now_millis();
    let ends_at = form
        .valid_days
        .map(|days| starts_at + i64::from(days) * 24 * 60 * 60 * 1000);

    match create_discount(
        &state.ctx.executor,
        &form.code,
        kind,
        starts_at,
        ends_at,
        form.usage_limit,
    )
    .await
    {
        Ok(_) => Ok(Redirect::to(INDEX_PATH)),
        Err(CreateDiscountError::Storage(source)) => Err(AppError::Internal(source)),
        Err(refused) => Err(AppError::BadRequest(refused.to_string())),
    }
}

async fn disable(
    State(state): State<PromotionState>,
    Path(discount_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    disable_discount(&state.ctx.executor, &discount_id).await?;

    Ok(Redirect::to(INDEX_PATH))
}
