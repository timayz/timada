//! `GET /checkout/places?country=IT&postal_code=95021&province=`: what the
//! checkout's script fills the province list and the city's suggestions with.

use timada_storefront::Places;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Json, query_params, route},
};

#[topcoat::router::query_params(error = bad_request)]
struct PlacesQuery {
    country: Option<String>,
    postal_code: Option<String>,
    province: Option<String>,
}

#[route(GET)]
pub async fn places(cx: &Cx) -> Result<Json<Places>> {
    let query = query_params::<PlacesQuery>(cx)?;
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    Ok(Json(
        timada_storefront::places(
            cx,
            &text(&query.country),
            &text(&query.postal_code),
            &text(&query.province),
        )
        .await?,
    ))
}
