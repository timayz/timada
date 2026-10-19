use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{DescribeForm, act};

#[route(POST)]
pub async fn describe(cx: &Cx, Form(form): Form<DescribeForm>) -> Result<SeeOther> {
    act(cx, "Description saved.", async |catalog, actor, id| {
        catalog
            .describe_category(actor, id, &form.description)
            .await?;
        if let Some(body) = &form.body {
            catalog.write_category_body(actor, id, body).await?;
        }
        Ok(())
    })
    .await
}
