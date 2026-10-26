//! `POST /store/shoppers/delete` with `X-Shopper-Token`: deletes the
//! shopper's account. Their key is destroyed, which leaves their personal
//! data unreadable, and every session ends,
//! this one included; the orders placed with the address stay with the shop.
//! The address is erased from the shop's newsletter too. 204.

use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, route},
};

use super::{Answer, SIGNED_OUT, answer_error, opened, refused, signed_in, status};
use timada_orders::STOREFRONT_ACTOR;

#[route(POST)]
pub async fn delete(cx: &Cx) -> Result<Answer> {
    let access = match super::super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let store = super::super::store(cx, &access);
    let Some(shopper) = signed_in(cx, &store).await? else {
        return refused(StatusCode::UNAUTHORIZED, SIGNED_OUT);
    };
    let keys = super::keys(cx, &access.tenant.id);
    // Read before the key goes: the newsletter knows them by their address.
    let email = opened(cx, &access.tenant.id, &shopper)
        .await?
        .map(|shopper| shopper.email);
    if let Err(err) = timada_shoppers::Command::new(store.executor().clone(), store.tenant_id())
        .forget(&*keys, &shopper.id)
        .await
    {
        return answer_error(err);
    }
    if let Some(email) = email {
        let newsletter =
            timada_newsletter::Command::new(store.executor().clone(), store.tenant_id());
        if let Some(subscriber) = newsletter
            .subscriber_of(&email)
            .await
            .map_err(topcoat::Error::from_anyhow)?
        {
            newsletter
                .forget_subscriber(&*keys, STOREFRONT_ACTOR, &subscriber.id)
                .await
                .map_err(|err| topcoat::Error::from_anyhow(err.into()))?;
        }
    }
    status(StatusCode::NO_CONTENT)
}
