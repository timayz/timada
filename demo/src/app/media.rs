//! `/media/demo/{file}`: a drawn stand-in for the product photos the demo
//! does not have — a tinted square with the product's reference, so listings
//! and product pages look like a shop's.

use topcoat::{
    Result,
    context::Cx,
    router::{
        error::RouterErrorExt,
        path_param, path_param as param,
        response::{IntoResponse, Response},
        route,
    },
};

path_param!(pub file: String, error = not_found);

pub struct Svg(String);

impl IntoResponse for Svg {
    fn into_response(self, _cx: &Cx) -> Result<Response> {
        Ok(Response::builder()
            .header("Content-Type", "image/svg+xml; charset=utf-8")
            .header("Cache-Control", "public, max-age=86400")
            .body(self.0.into_bytes().into())?)
    }
}

#[route(GET "/media/demo/{file}")]
pub async fn placeholder(cx: &Cx) -> Result<Svg> {
    let file = param::<File>(cx)?;
    // Only what a slug is made of ever reaches the drawing.
    let reference = file
        .strip_suffix(".svg")
        .filter(|name| timada_core::slug::is_slug(name) && name.len() <= 40)
        .ok_or_not_found()?;
    let hue = reference
        .bytes()
        .fold(0u32, |sum, byte| (sum * 31 + u32::from(byte)) % 360);
    let label = reference.to_uppercase();
    Ok(Svg(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 480 480\" role=\"img\" aria-label=\"{label}\">\
         <rect width=\"480\" height=\"480\" fill=\"hsl({hue} 45% 90%)\"/>\
         <rect x=\"90\" y=\"130\" width=\"300\" height=\"190\" rx=\"14\" fill=\"hsl({hue} 40% 35%)\"/>\
         <rect x=\"210\" y=\"320\" width=\"60\" height=\"40\" fill=\"hsl({hue} 40% 35%)\"/>\
         <rect x=\"160\" y=\"356\" width=\"160\" height=\"14\" rx=\"7\" fill=\"hsl({hue} 40% 35%)\"/>\
         <text x=\"240\" y=\"236\" text-anchor=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"30\" font-weight=\"600\" fill=\"#fff\">{label}</text>\
         </svg>"
    )))
}
