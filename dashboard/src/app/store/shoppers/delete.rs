//! `POST /store/shoppers/delete` with `X-Shopper-Token`: deletes the
//! shopper's account. Their key is destroyed, which leaves their personal
//! data unreadable, and every session ends,
//! this one included; the orders placed with the address stay with the shop.
//! The address is erased from the shop's newsletter too, by
//! [`crate::shopper_erasure`], from what the account's deletion names. 204.

use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, route},
};

use super::{
    Answer, SIGNED_OUT, answer_error, newsletter_subscriber_of, refused, signed_in, status,
};

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
    let subscriber =
        newsletter_subscriber_of(cx, store.executor(), store.tenant_id(), &shopper).await?;
    if let Err(err) = timada_shoppers::Command::new(store.executor().clone(), store.tenant_id())
        .forget(&*keys, &shopper.id, subscriber.as_deref())
        .await
    {
        return answer_error(err);
    }
    status(StatusCode::NO_CONTENT)
}
