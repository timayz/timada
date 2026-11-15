use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{FeedLabelsForm, Tab, act};

/// Sets the five labels ad campaigns group the product by.
#[route(POST)]
pub async fn feed_labels(cx: &Cx, Form(form): Form<FeedLabelsForm>) -> Result<SeeOther> {
    let labels = [
        form.label_0,
        form.label_1,
        form.label_2,
        form.label_3,
        form.label_4,
    ];
    act(
        cx,
        Tab::Shopping,
        "Labels saved.",
        async |catalog, actor, id| catalog.set_feed_labels(actor, id, &labels).await,
    )
    .await
}
