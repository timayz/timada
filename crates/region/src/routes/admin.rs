//! Admin pages for regions. Routes are relative to the mount point — the
//! umbrella admin crate nests this router under `/admin/regions`, which is the
//! prefix the templates' links and form actions hardcode.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};
use axum::routing::get;
use axum::{Form, Router};
use serde::Deserialize;
use timada_core::{AppError, AppResult, Currency};
use timada_web::HtmlTemplate;

use crate::aggregate::RegionCountry;
use crate::commands::{RegionError, create_region, update_region};
use crate::projections::{RegionRow, list_regions};
use crate::state::RegionState;
use crate::view::{RegionView, load_region};

/// Where the create/update actions send the browser back to.
const INDEX_PATH: &str = "/admin/regions";

pub fn admin_router(state: RegionState) -> Router {
    Router::new()
        .route("/", get(index).post(create))
        .route("/new", get(new_form))
        .route("/{region_id}", get(edit_form).post(update))
        .with_state(state)
}

#[derive(Template)]
#[template(path = "admin/regions/index.html")]
struct IndexTemplate {
    regions: Vec<RegionRow>,
}

#[derive(Template)]
#[template(path = "admin/regions/form.html")]
struct FormTemplate {
    /// `None` on the create form.
    region: Option<RegionView>,
    /// The country list rendered back into the form's `CODE:RATE_BPS` lines.
    countries_text: String,
}

async fn index(State(state): State<RegionState>) -> AppResult<impl IntoResponse> {
    let regions = list_regions(&state.ctx.read_pool).await?;

    Ok(HtmlTemplate(IndexTemplate { regions }))
}

async fn new_form() -> AppResult<impl IntoResponse> {
    Ok(HtmlTemplate(FormTemplate {
        region: None,
        countries_text: String::new(),
    }))
}

async fn edit_form(
    State(state): State<RegionState>,
    Path(region_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    let region = load_region(&state.ctx.executor, &region_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let countries_text = region
        .countries
        .iter()
        .map(|country| format!("{}:{}", country.code, country.tax_rate_bps))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(HtmlTemplate(FormTemplate {
        region: Some(region),
        countries_text,
    }))
}

#[derive(Deserialize)]
struct RegionForm {
    name: String,
    /// Present on create only; the edit form has no currency field.
    #[serde(default)]
    currency: String,
    /// `CODE:RATE_BPS` pairs, one per line or comma-separated,
    /// e.g. `FR:2000, DE:1900`.
    countries: String,
}

async fn create(
    State(state): State<RegionState>,
    Form(form): Form<RegionForm>,
) -> AppResult<impl IntoResponse> {
    let currency = Currency::from_code(form.currency.trim())
        .map_err(|source| AppError::BadRequest(source.to_string()))?;
    let countries = parse_countries(&form.countries)?;

    match create_region(&state.ctx.executor, &form.name, currency, countries).await {
        Ok(_) => Ok(Redirect::to(INDEX_PATH)),
        Err(RegionError::Storage(source)) => Err(AppError::Internal(source)),
        Err(refused) => Err(AppError::BadRequest(refused.to_string())),
    }
}

async fn update(
    State(state): State<RegionState>,
    Path(region_id): Path<String>,
    Form(form): Form<RegionForm>,
) -> AppResult<impl IntoResponse> {
    let countries = parse_countries(&form.countries)?;

    match update_region(&state.ctx.executor, &region_id, &form.name, countries).await {
        Ok(()) => Ok(Redirect::to(INDEX_PATH)),
        Err(RegionError::Storage(source)) => Err(AppError::Internal(source)),
        Err(refused) => Err(AppError::BadRequest(refused.to_string())),
    }
}

/// Parse `CODE:RATE_BPS` pairs separated by commas or newlines.
fn parse_countries(input: &str) -> Result<Vec<RegionCountry>, AppError> {
    let mut countries = Vec::new();
    for entry in input.split([',', '\n']) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((code, rate)) = entry.split_once(':') else {
            return Err(AppError::BadRequest(format!(
                "`{entry}` is not a CODE:RATE_BPS pair (e.g. FR:2000)"
            )));
        };
        let tax_rate_bps: u32 = rate.trim().parse().map_err(|_| {
            AppError::BadRequest(format!("`{}` is not a rate in basis points", rate.trim()))
        })?;
        countries.push(RegionCountry {
            code: code.trim().to_owned(),
            tax_rate_bps,
        });
    }

    Ok(countries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comma_and_newline_separated_pairs() {
        let countries = parse_countries("FR:2000, DE:1900\nLU:1700").unwrap();
        assert_eq!(countries.len(), 3);
        assert_eq!(countries[0].code, "FR");
        assert_eq!(countries[0].tax_rate_bps, 2000);
        assert_eq!(countries[2].code, "LU");
        assert_eq!(countries[2].tax_rate_bps, 1700);
    }

    #[test]
    fn refuses_malformed_pairs() {
        assert!(parse_countries("FR").is_err());
        assert!(parse_countries("FR:many").is_err());
        assert!(parse_countries("").unwrap().is_empty());
    }
}
