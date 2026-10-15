use timada_tenancy::PERMISSION_REVIEWS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{reviews, reviews_url};
use crate::app::t::slug::require_tenant;

#[derive(serde::Deserialize)]
pub struct RequestsForm {
    /// `on` or `off`.
    ask: String,
    /// Days after fulfilment, while on.
    #[serde(default)]
    days: String,
}

/// Says whether, and how many days after an order is fulfilled, its buyer
/// is mailed a link to review what they bought.
#[route(POST)]
pub async fn requests(cx: &Cx, Form(form): Form<RequestsForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_REVIEWS_MANAGE)
        .await?;
    let slug = access.tenant.slug.clone();
    let days = match form.ask.as_str() {
        "off" => None,
        _ => match form.days.trim().parse::<u8>() {
            Ok(days) => Some(days),
            Err(_) => {
                return Ok(see_other(reviews_url(
                    cx,
                    &slug,
                    Some("review_request_delay_too_long"),
                    None,
                )));
            }
        },
    };
    match reviews(&access)?
        .set_review_requests(&access.user_id, days)
        .await
    {
        Ok(()) => {
            let message = if days.is_some() {
                "Review requests saved."
            } else {
                "Review requests turned off."
            };
            Ok(see_other(reviews_url(cx, &slug, None, Some(message))))
        }
        Err(err) => match err.code() {
            Some(code) => Ok(see_other(reviews_url(cx, &slug, Some(code), None))),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}
