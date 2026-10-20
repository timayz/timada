//! `POST /store/newsletter/confirm` `token=…`: a storefront hands back the
//! token of a confirmation link its shopper followed. 200
//! `{"status": "confirmed" | "already_confirmed"}`; 422 `{"error": code}`
//! for a link that does not check out (`newsletter_invalid_link`) or has
//! expired (`newsletter_link_expired`). The key names the shop: a token of
//! another shop's link does not check out.

use serde::Deserialize;
use serde_json::json;
use timada_newsletter::ConfirmOutcome;
use timada_orders::STOREFRONT_ACTOR;
use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, content::Form, route},
};

use super::answer_error;
use crate::app::store::shoppers::{Answer, answer, status};
use crate::auth::now;

#[derive(Deserialize)]
pub struct TokenForm {
    #[serde(default)]
    pub token: String,
}

#[route(POST)]
pub async fn confirm(cx: &Cx, Form(form): Form<TokenForm>) -> Result<Answer> {
    let access = match crate::app::store::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let hold = crate::app::store::hold(cx, &access).await?;
    let keys = crate::app::store::shoppers::keys(cx, &access.tenant.id);
    match timada_newsletter::Command(hold.executor().clone())
        .confirm_subscription(&*keys, STOREFRONT_ACTOR, &form.token, now())
        .await
    {
        Ok(outcome) => answer(
            StatusCode::OK,
            json!({ "status": match outcome {
                ConfirmOutcome::Confirmed => "confirmed",
                ConfirmOutcome::AlreadyConfirmed => "already_confirmed",
            } }),
        ),
        Err(err) => answer_error(err),
    }
}
