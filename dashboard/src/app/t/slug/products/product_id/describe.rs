use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{DescribeForm, Tab, act};

#[route(POST)]
pub async fn describe(cx: &Cx, Form(form): Form<DescribeForm>) -> Result<SeeOther> {
    act(
        cx,
        Tab::Overview,
        "Description saved.",
        async |catalog, actor, id| catalog.describe_product(actor, id, &form.description).await,
    )
    .await
}
