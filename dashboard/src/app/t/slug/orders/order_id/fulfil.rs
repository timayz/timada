use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{FulfilForm, act};

#[route(POST)]
pub async fn fulfil(cx: &Cx, Form(form): Form<FulfilForm>) -> Result<SeeOther> {
    act(
        cx,
        "Order marked as fulfilled.",
        async |orders, actor, id| orders.fulfil_order(actor, id, Some(form.tracking)).await,
    )
    .await
}
