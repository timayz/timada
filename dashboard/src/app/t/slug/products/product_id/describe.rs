use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{DescribeForm, act};

#[route(POST)]
pub async fn describe(cx: &Cx, Form(form): Form<DescribeForm>) -> Result<SeeOther> {
    act(cx, "Description saved.", async |catalog, actor, id| {
        catalog.describe_product(actor, id, &form.description).await
    })
    .await
}
