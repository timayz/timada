use timada_catalog::parse_amount;
use timada_tenancy::PERMISSION_PRODUCTS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::label::{TickedForm, back, field, ticked};
use super::{PAGE_SIZE, catalog, error_code};
use crate::app::t::slug::require_tenant;

/// Says what one of every ticked product costs the shop, each in its own
/// currency and for all its combinations alike, and comes back to the list
/// where it stood. Left empty, the cost is forgotten.
///
/// Written here one product after the other, as the labels are and for the
/// same reasons (see [`label`](super::label::label)): a page of the list at
/// most, each write standing alone, and the same form sent again finishing
/// what a failure left.
#[route(POST)]
pub async fn cost(cx: &Cx, Form(form): Form<TickedForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let back = |error: Option<&str>, message: Option<&str>| back(cx, &form, error, message);
    let products = ticked(&form);
    if products.is_empty() || products.len() > PAGE_SIZE as usize {
        return Ok(back(Some("no_product_ticked"), None));
    }
    let amount = match field(&form, "cost").map(parse_amount).transpose() {
        Ok(amount) => amount,
        Err(err) => return Ok(back(Some(error_code(&err)), None)),
    };
    let gateway = catalog(&access)?;
    for product_id in &products {
        match gateway
            .set_every_cost(&access.user_id, product_id, amount)
            .await
        {
            Ok(()) => {}
            Err(err) if err.is_user_facing() => return Ok(back(Some(error_code(&err)), None)),
            Err(err) => return Err(topcoat::Error::from_anyhow(err.into())),
        }
    }
    let message = match (amount, products.len()) {
        (Some(_), 1) => "Cost set on 1 product.".to_owned(),
        (Some(_), n) => format!("Cost set on {n} products."),
        (None, 1) => "Cost forgotten on 1 product.".to_owned(),
        (None, n) => format!("Cost forgotten on {n} products."),
    };
    Ok(back(None, Some(&message)))
}
