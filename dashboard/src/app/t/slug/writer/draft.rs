//! Asks for the texts of one product or one category to be drafted, from
//! its own page, and lands back on it.

use serde::Deserialize;
use timada_proposals::KIND_PRODUCT;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, route,
    },
};

use super::{Asked, ask, require_drafter};
use crate::app::auth::Flash;
use crate::app::t::slug::{categories::category_id, products::product_id, slug};
use crate::writer::Writers;

#[derive(Deserialize)]
pub struct DraftForm {
    kind: String,
    subject: String,
    #[serde(default)]
    keyword: String,
}

/// What a draft on its way is told with, on the page it was asked from.
pub const ASKED: &str = "Asked: the draft will wait under Proposals in a moment.";

#[route(POST)]
pub async fn draft(cx: &Cx, Form(form): Form<DraftForm>) -> Result<SeeOther> {
    let access = require_drafter(cx).await?;
    let slug = access.tenant.slug.clone();
    let back = |error: Option<&str>, message: Option<&str>| {
        let flash = Flash { error, message };
        if form.kind == KIND_PRODUCT {
            href!(product_id::index, slug: slug.clone(), product_id::product_id: form.subject.clone())
                .query(flash)
                .resolve(cx)
        } else {
            href!(category_id::index, slug: slug.clone(), category_id::category_id: form.subject.clone())
                .query(flash)
                .resolve(cx)
        }
    };
    if !context::<Writers>(cx).enabled() {
        return Ok(see_other(back(Some("writer_off"), None)));
    }
    match ask(cx, &access, &form.kind, &form.subject, &form.keyword).await {
        Ok(Asked::Requested) => Ok(see_other(back(None, Some(ASKED)))),
        Ok(Asked::OnItsWay) => Ok(see_other(back(Some("writer_on_its_way"), None))),
        Ok(Asked::Missing) => Ok(see_other(back(Some("proposal_subject_missing"), None))),
        Err(err) if err.is_user_facing() => Ok(see_other(back(err.code(), None))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
