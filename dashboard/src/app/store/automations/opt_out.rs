//! `POST /store/automations/opt-out` `token=…`: a storefront hands back the
//! token of the link at the foot of an automated mail its shopper got
//! (`{storefront}/emails/opt-out/{token}`), and the address stops getting
//! the shop's automated mails. 204, also when it had stopped already; 422
//! `{"error": "automation_invalid_link"}` for a link that does not check
//! out (of another shop, or tampered with); 503 for a dashboard without the
//! secret its links are signed under. The key must allow `store:checkout`,
//! as for the contact form.

use timada_orders::STOREFRONT_ACTOR;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{StatusCode, content::Form, route},
};

use crate::app::OptOutLinks;
use crate::app::store::newsletter::confirm::TokenForm;
use crate::app::store::shoppers::{Answer, refused, status};

#[route(POST)]
pub async fn opt_out(cx: &Cx, Form(form): Form<TokenForm>) -> Result<Answer> {
    let access = match crate::app::store::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let Some(key) = app_context::<OptOutLinks>(cx).0 else {
        return status(StatusCode::SERVICE_UNAVAILABLE);
    };
    let store = crate::app::store::store(cx, &access);
    match timada_automations::Command::new(store.executor().clone(), store.tenant_id())
        .opt_out_with_link(&key.0, STOREFRONT_ACTOR, &form.token)
        .await
    {
        Ok(()) => status(StatusCode::NO_CONTENT),
        Err(err) => match err.code() {
            Some(code) if code == timada_core::WRITES_PAUSED => {
                refused(StatusCode::SERVICE_UNAVAILABLE, code)
            }
            Some(code) if err.is_user_facing() => refused(StatusCode::UNPROCESSABLE_ENTITY, code),
            _ => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}
