use timada_catalog::CatalogError;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{PinForm, act};

/// The products named by their addresses, one a line, become the ones the
/// category's page shows first.
#[route(POST)]
pub async fn pin(cx: &Cx, Form(form): Form<PinForm>) -> Result<SeeOther> {
    act(cx, "First products saved.", async |catalog, actor, id| {
        let mut ids = Vec::new();
        for slug in form.products.lines().map(str::trim) {
            if slug.is_empty() {
                continue;
            }
            // A whole address pasted is taken for its end.
            let slug = slug
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(slug);
            let product = catalog
                .load_product_by_slug(&slug.to_ascii_lowercase())
                .await?
                .ok_or(CatalogError::NotFound)?;
            ids.push(product.id);
        }
        catalog.pin_category_products(actor, id, &ids).await
    })
    .await
}
