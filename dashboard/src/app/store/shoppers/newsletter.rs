//! `/store/shoppers/newsletter` with `X-Shopper-Token`: whether the signed-in
//! shopper gets the shop's news, and the switch.
//!
//! - `GET`: 200 `{"subscribed": bool}`; asked from the form or at checkout
//!   but not confirmed yet is not subscribed.
//! - `POST` `subscribed=true|false`: 204. The shopper signed in with a code
//!   mailed to their address, which proves it: they are on the list at once,
//!   no link to follow.

use serde::Deserialize;
use serde_json::json;
use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, content::Form, route},
};

use super::{Answer, SIGNED_OUT, answer, opened, refused, signed_in, status};
use crate::app::store::newsletter::answer_error;
use timada_orders::STOREFRONT_ACTOR;

#[route(GET)]
pub async fn newsletter(cx: &Cx) -> Result<Answer> {
    let access = match super::super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let store = super::super::store(cx, &access);
    let Some(shopper) = signed_in(cx, &store).await? else {
        return refused(StatusCode::UNAUTHORIZED, SIGNED_OUT);
    };
    let Some(shopper) = opened(cx, &access.tenant.id, &shopper).await? else {
        return refused(StatusCode::UNAUTHORIZED, SIGNED_OUT);
    };
    let subscribed = timada_newsletter::Command::new(store.executor().clone(), store.tenant_id())
        .subscriber_of(&shopper.email)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .is_some_and(|subscriber| subscriber.is_confirmed());
    answer(StatusCode::OK, json!({ "subscribed": subscribed }))
}

#[derive(Deserialize)]
pub struct SwitchForm {
    subscribed: bool,
    #[serde(default)]
    language: Option<String>,
}

#[route(POST)]
pub async fn switch(cx: &Cx, Form(form): Form<SwitchForm>) -> Result<Answer> {
    let access = match super::super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let store = super::super::store(cx, &access);
    let Some(shopper) = signed_in(cx, &store).await? else {
        return refused(StatusCode::UNAUTHORIZED, SIGNED_OUT);
    };
    let Some(shopper) = opened(cx, &access.tenant.id, &shopper).await? else {
        return refused(StatusCode::UNAUTHORIZED, SIGNED_OUT);
    };
    let list = timada_newsletter::Command::new(store.executor().clone(), store.tenant_id());
    let done = if form.subscribed {
        list.subscribe_verified(
            &*super::keys(cx, &access.tenant.id),
            STOREFRONT_ACTOR,
            &shopper.email,
            form.language,
        )
        .await
    } else {
        match list
            .subscriber_of(&shopper.email)
            .await
            .map_err(topcoat::Error::from_anyhow)?
        {
            Some(subscriber) => list.unsubscribe(STOREFRONT_ACTOR, &subscriber.id).await,
            None => Ok(()),
        }
    };
    match done {
        Ok(()) => status(StatusCode::NO_CONTENT),
        Err(err) => answer_error(err),
    }
}
