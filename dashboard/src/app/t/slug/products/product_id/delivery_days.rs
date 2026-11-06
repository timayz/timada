use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{DeliveryDaysForm, Tab, act};

/// Sets how long delivery of the product takes; empty uses the shop's default.
#[route(POST)]
pub async fn delivery_days(cx: &Cx, Form(form): Form<DeliveryDaysForm>) -> Result<SeeOther> {
    act(
        cx,
        Tab::Overview,
        "Delivery time set.",
        async |catalog, actor, id| {
            let days = timada_catalog::DeliveryDays::parse(&form.min, &form.max)?;
            catalog.set_product_delivery_days(actor, id, days).await
        },
    )
    .await
}
