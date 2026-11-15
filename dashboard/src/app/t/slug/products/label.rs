use std::collections::BTreeMap;

use timada_tenancy::PERMISSION_PRODUCTS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, route,
    },
};

use super::{PAGE_SIZE, ProductsLink, catalog, error_code};
use crate::app::auth::Flash;
use crate::app::t::slug::{require_tenant, slug};

/// The prefix of the fields naming a ticked product. The form cannot know
/// the rows in advance, so it posts `product:<id>` for each and the handler
/// reads them back out.
pub const PRODUCT_FIELD: &str = "product:";

/// Everything the form posts: `index` and `label`, where the list stood
/// (`q`, `category`, `status`, `after`, `before`), plus one `product:<id>`
/// a ticked row. `BTreeMap` because the set of fields is the page's, not
/// the form's.
type LabelForm = BTreeMap<String, String>;

/// Sets one campaign label on every ticked product and comes back to the
/// list where it stood.
///
/// The products are written here, one after the other, rather than by a
/// saga behind an intent of its own: they are a page of the list at most,
/// each write stands alone (a product already at the label is left as it
/// is), and nothing is owed once the answer is given. A write that fails
/// stops there; the products before it keep the label and the same form
/// sent again finishes the rest.
#[route(POST)]
pub async fn label(cx: &Cx, Form(form): Form<LabelForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.clone();
    let field = |name: &str| form.get(name).map(String::as_str).filter(|v| !v.is_empty());
    let back = |error: Option<&str>, message: Option<&str>| {
        see_other(
            href!(super::index, slug: slug.clone())
                .query(ProductsLink {
                    q: field("q"),
                    category: field("category"),
                    status: field("status"),
                    after: field("after"),
                    before: field("before"),
                    flash: Flash { error, message },
                })
                .resolve(cx),
        )
    };
    let products: Vec<&str> = form
        .keys()
        .filter_map(|key| key.strip_prefix(PRODUCT_FIELD))
        .collect();
    if products.is_empty() {
        return Ok(back(Some("no_product_ticked"), None));
    }
    let index = field("index").and_then(|index| index.parse::<usize>().ok());
    let (Some(index), true) = (index, products.len() <= PAGE_SIZE as usize) else {
        return Ok(back(Some("invalid_feed_label"), None));
    };
    let label = form.get("label").map(String::as_str).unwrap_or_default();
    let gateway = catalog(&access)?;
    for product_id in &products {
        match gateway
            .set_feed_label(&access.user_id, product_id, index, label)
            .await
        {
            Ok(()) => {}
            Err(err) if err.is_user_facing() => return Ok(back(Some(error_code(&err)), None)),
            Err(err) => return Err(topcoat::Error::from_anyhow(err.into())),
        }
    }
    let message = match products.len() {
        1 => format!("Label {index} set on 1 product."),
        n => format!("Label {index} set on {n} products."),
    };
    Ok(back(None, Some(&message)))
}
