//! `GET /store/places?country=FR&postal_code=30100&province=Gard`: what a
//! storefront's address form offers in a shop that forwards to AliExpress,
//! which refuses an address not in its own words: the country's provinces,
//! the one the postal code names, and the cities of the province asked for
//! (else of that one) with their towns.
//!
//! 200 `{"provinces": […], "province": "Gard", "cities": [{"name": "Ales",
//! "towns": []}, …]}`, all empty for
//! a shop not connected to AliExpress, a country without a list, or one
//! AliExpress lists apart (Martinique, under France here): the form then asks
//! for no province. 401, 403 and 503 as [`super::storefront`] says.

use std::sync::Arc;

use serde_json::{Value, json};
use timada_aliexpress::{delivery_country, places_of, province_of};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{StatusCode, content::Json, query_params, route},
};

use crate::aliexpress::Aliexpress;

#[topcoat::router::query_params(error = bad_request)]
struct PlacesQuery {
    country: Option<String>,
    postal_code: Option<String>,
    province: Option<String>,
}

#[route(GET)]
pub async fn places(cx: &Cx) -> Result<(StatusCode, Json<Value>)> {
    let access = match super::storefront(cx).await? {
        Ok(access) => access,
        Err(status) => return Ok((status, Json(json!({})))),
    };
    let query = query_params::<PlacesQuery>(cx)?;
    let none = json!({ "provinces": [], "province": null, "cities": [] });
    let country = query
        .country
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    let postal_code = query.postal_code.as_deref().unwrap_or_default();
    if country.len() != 2
        || !country.bytes().all(|b| b.is_ascii_uppercase())
        || delivery_country(&country, postal_code) != country
    {
        return Ok((StatusCode::OK, Json(none)));
    }
    let aliexpress = app_context::<Arc<Aliexpress>>(cx);
    let connected = aliexpress
        .connected(&access.tenant)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    if !connected {
        return Ok((StatusCode::OK, Json(none)));
    }
    let Some(places) = places_of(&country) else {
        return Ok((StatusCode::OK, Json(none)));
    };
    let named = province_of(&country, postal_code, "");
    let named = places.names_province(&named).then_some(named);
    let asked = query.province.as_deref().filter(|p| !p.is_empty());
    let province = asked.or(named.as_deref()).unwrap_or_default();
    let cities: Vec<Value> = places
        .cities_in(province)
        .into_iter()
        .map(|(name, towns)| json!({ "name": name, "towns": towns }))
        .collect();
    Ok((
        StatusCode::OK,
        Json(json!({
            "provinces": places.provinces(),
            "province": named,
            "cities": cities,
        })),
    ))
}
