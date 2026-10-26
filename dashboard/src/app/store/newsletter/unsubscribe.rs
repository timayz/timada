//! `POST /store/newsletter/unsubscribe` `token=…`: a storefront hands back
//! the token of the unsubscribe link of a mail its shopper got, and the
//! shopper leaves the list. 204, also when they had left already; 422
//! `{"error": "newsletter_invalid_link"}` for a link that does not check
//! out (of another shop, or of a subscriber erased since).

use timada_orders::STOREFRONT_ACTOR;
use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, content::Form, route},
};

use super::{answer_error, confirm::TokenForm};
use crate::app::store::shoppers::{Answer, status};

#[route(POST)]
pub async fn unsubscribe(cx: &Cx, Form(form): Form<TokenForm>) -> Result<Answer> {
    let access = match crate::app::store::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let store = crate::app::store::store(cx, &access);
    let keys = crate::app::store::shoppers::keys(cx, &access.tenant.id);
    match timada_newsletter::Command::new(store.executor().clone(), store.tenant_id())
        .unsubscribe_with_link(&*keys, STOREFRONT_ACTOR, &form.token)
        .await
    {
        Ok(_) => status(StatusCode::NO_CONTENT),
        Err(err) => answer_error(err),
    }
}
