use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{Tab, act};

#[derive(serde::Deserialize)]
pub struct AdsForm {
    /// An unticked checkbox is not sent at all.
    #[serde(default)]
    out_of_ads: Option<String>,
}

/// Keeps the product out of ads, or lets campaigns advertise it again.
#[route(POST)]
pub async fn ads(cx: &Cx, Form(form): Form<AdsForm>) -> Result<SeeOther> {
    let in_ads = form.out_of_ads.is_none();
    let message = if in_ads {
        "It may be advertised."
    } else {
        "Kept out of ads."
    };
    act(cx, Tab::Shopping, message, async |catalog, actor, id| {
        catalog.set_product_in_ads(actor, id, in_ads).await
    })
    .await
}
