use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;

#[derive(serde::Deserialize)]
pub struct StockForm {
    /// Empty for a product without variants.
    #[serde(default)]
    pub variant_id: String,
    #[serde(default)]
    pub on_hand: String,
    /// Present when the merchant stops counting it.
    #[serde(default)]
    pub untrack: Option<String>,
}

/// Counts what is on the shelf of the product (or one of its variants), or
/// stops counting it.
#[route(POST)]
pub async fn stock(cx: &Cx, Form(form): Form<StockForm>) -> Result<SeeOther> {
    let variant_id = Some(form.variant_id.trim()).filter(|id| !id.is_empty());
    if form.untrack.is_some() {
        return act(
            cx,
            "Stock no longer counted.",
            async |catalog, actor, id| catalog.untrack_stock(actor, id, variant_id).await,
        )
        .await;
    }
    act(cx, "Stock set.", async |catalog, actor, id| {
        let on_hand = form
            .on_hand
            .trim()
            .parse::<u32>()
            .map_err(|_| timada_catalog::CatalogError::InvalidStock)?;
        catalog.set_stock(actor, id, variant_id, on_hand).await
    })
    .await
}
