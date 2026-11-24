use timada_catalog::parse_amount;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::tree_url;
use crate::app::research::study_id::require_owned;
use crate::app::research::{error_code, research};

#[derive(serde::Deserialize)]
pub struct PriceForm {
    keyword: String,
    /// Empty goes back to the study's price.
    #[serde(default)]
    price: String,
}

/// Gives a category of the tree an average price of its own, which its
/// cost per click is read against in place of the study's.
#[route(POST)]
pub async fn price(cx: &Cx, Form(form): Form<PriceForm>) -> Result<SeeOther> {
    let (study, user_id) = require_owned(cx).await?;
    let back = |error: Option<&str>, message: Option<&str>| {
        Ok(see_other(tree_url(cx, &study.id, None, error, message)))
    };
    let price_cents = match form.price.trim() {
        "" => None,
        text => match parse_amount(text) {
            Ok(cents) => Some(cents),
            Err(_) => return back(Some("invalid_study_amount"), None),
        },
    };
    match research(cx)
        .price_category(&user_id, &study.id, &form.keyword, price_cents)
        .await
    {
        Ok(()) => back(None, Some("Price saved.")),
        Err(err) if err.is_user_facing() => back(Some(error_code(&err)), None),
        Err(err) => Err(err.into()),
    }
}
