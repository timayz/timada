use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{FeedListingForm, Tab, act};

/// Puts the product in the shop's shopping feed or takes it out.
#[route(POST)]
pub async fn feed_listing(cx: &Cx, Form(form): Form<FeedListingForm>) -> Result<SeeOther> {
    let listed = form.listed.is_some();
    let message = if listed {
        "In the shopping feed."
    } else {
        "Out of the shopping feed."
    };
    act(cx, Tab::Shopping, message, async |catalog, actor, id| {
        catalog.list_in_feed(actor, id, listed).await
    })
    .await
}
