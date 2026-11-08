use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{SettleOutsideForm, act};

/// The merchant collected the money outside timada for an order kept open
/// once its hold lapsed while AliExpress may have it: the order is paid,
/// nothing through Stripe, its invoice is issued, and AliExpress's tracking
/// fulfils it as usual. Any other order is refused (`not_kept_open`).
#[route(POST)]
pub async fn settle_outside(cx: &Cx, Form(form): Form<SettleOutsideForm>) -> Result<SeeOther> {
    act(
        cx,
        "Noted: the order was settled outside timada. Its invoice is being issued.",
        async |orders, actor, id| orders.settle_outside(actor, id, Some(form.note)).await,
    )
    .await
}
