use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{SeoForm, act};

#[route(POST)]
pub async fn seo(cx: &Cx, Form(form): Form<SeoForm>) -> Result<SeeOther> {
    act(
        cx,
        "Saved for search engines.",
        async |catalog, actor, id| {
            catalog
                .set_category_seo(actor, id, &form.meta_title, &form.meta_description)
                .await
        },
    )
    .await
}
