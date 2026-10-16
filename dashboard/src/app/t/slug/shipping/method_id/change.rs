use topcoat::{Result, context::Cx, router::content::Form, router::error::SeeOther, router::route};

use super::act;
use crate::app::t::slug::shipping::MethodForm;

#[route(POST)]
pub async fn change(cx: &Cx, Form(form): Form<MethodForm>) -> Result<SeeOther> {
    act(cx, "Shipping method saved.", async |orders, actor, id| {
        orders
            .change_shipping_method(actor, id, form.method()?)
            .await
    })
    .await
}
