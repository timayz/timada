use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{CancelForm, act};

#[route(POST)]
pub async fn cancel(cx: &Cx, Form(form): Form<CancelForm>) -> Result<SeeOther> {
    act(cx, "Order cancelled.", async |orders, actor, id| {
        orders.cancel_order(actor, id, form.reason).await
    })
    .await
}
