use timada_proposals::KIND_PRODUCT;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{content::Form, error::SeeOther, route},
};

use super::PAGE_SIZE;
use super::label::{TickedForm, back, ticked};
use crate::app::t::slug::writer::{Asked, ask, require_drafter};
use crate::writer::Writers;

/// Asks for the texts of every ticked product to be drafted, each for its
/// own name, and comes back to the list where it stood. The drafts are
/// written off the page and wait under Proposals; a product whose draft is
/// on its way already is passed over.
#[route(POST)]
pub async fn draft(cx: &Cx, Form(form): Form<TickedForm>) -> Result<SeeOther> {
    let access = require_drafter(cx).await?;
    let back = |error: Option<&str>, message: Option<&str>| back(cx, &form, error, message);
    if !context::<Writers>(cx).enabled() {
        return Ok(back(Some("writer_off"), None));
    }
    let products = ticked(&form);
    if products.is_empty() || products.len() > PAGE_SIZE as usize {
        return Ok(back(Some("no_product_ticked"), None));
    }
    let (mut asked, mut passed) = (0, 0);
    for product_id in &products {
        match ask(cx, &access, KIND_PRODUCT, product_id, "").await {
            Ok(Asked::Requested) => asked += 1,
            Ok(_) => passed += 1,
            Err(err) if err.is_user_facing() => passed += 1,
            Err(err) => return Err(topcoat::Error::from_anyhow(err.into())),
        }
    }
    let message = match (asked, passed) {
        (0, _) => "Nothing asked for: their drafts are on their way already.".to_owned(),
        (1, 0) => "1 draft asked for: it will wait under Proposals in a moment.".to_owned(),
        (n, 0) => {
            format!("{n} drafts asked for: they will wait under Proposals as they are written.")
        }
        (n, passed) => format!(
            "{n} drafts asked for: they will wait under Proposals as they are written. {passed} passed over, on their way already."
        ),
    };
    Ok(back(None, Some(&message)))
}
