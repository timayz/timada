//! The storefront region picker. Merged at the site root.

use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect};
use axum::routing::get;
use axum::{Form, Router};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use timada_core::{AppError, AppResult};
use timada_web::HtmlTemplate;

use crate::cookie::{current_region, region_cookie};
use crate::projections::{RegionRow, list_regions};
use crate::state::RegionState;

pub fn store_router(state: RegionState) -> Router {
    Router::new()
        .route("/region", get(region_page).post(choose_region))
        .with_state(state)
}

#[derive(Template)]
#[template(path = "store/region.html")]
struct RegionTemplate {
    regions: Vec<RegionRow>,
    /// Id of the region this browser currently shops in, if any exists.
    current_id: String,
}

async fn region_page(
    State(state): State<RegionState>,
    jar: CookieJar,
) -> AppResult<impl IntoResponse> {
    let regions = list_regions(&state.ctx.read_pool).await?;
    let current_id = current_region(&state.ctx.read_pool, &jar)
        .await?
        .map(|region| region.id)
        .unwrap_or_default();

    Ok(HtmlTemplate(RegionTemplate {
        regions,
        current_id,
    }))
}

#[derive(Deserialize)]
struct ChooseRegionForm {
    region_id: String,
}

/// Set the cookie and go back to the storefront. Switching region does not
/// touch the cart: its lines keep their snapshotted currency, and checkout
/// refuses a cart/region currency mismatch with a readable reason.
async fn choose_region(
    State(state): State<RegionState>,
    jar: CookieJar,
    Form(form): Form<ChooseRegionForm>,
) -> AppResult<(CookieJar, Redirect)> {
    let regions = list_regions(&state.ctx.read_pool).await?;
    if !regions.iter().any(|region| region.id == form.region_id) {
        return Err(AppError::BadRequest("no such region".to_owned()));
    }

    Ok((jar.add(region_cookie(form.region_id)), Redirect::to("/")))
}
