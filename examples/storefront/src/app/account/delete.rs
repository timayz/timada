//! `/account/delete`: says what deleting the account forgets and what stays,
//! and deletes it on confirmation. The browser then forgets the shopper too.

use timada_storefront::{delete_account, menu};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{RouterErrorExt, SeeOther, see_other},
        href, page, route,
    },
    view::{View, view},
};

use crate::app::account::{account_url, sign_in::sign_in_url};
use crate::app::document;
use crate::shopper::{forget_checkout_draft, forget_shopper, shopper_token, signed_in};

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let to_sign_in = sign_in_url(cx, Some(&href!(index).resolve(cx)), None);
    let shopper = signed_in(cx).await.cloned().ok_or_redirect(to_sign_in)?;
    let nav = menu(cx).await?;
    Ok(view! {
        document(
            title: "Delete your account",
            nav: &nav,
            <h1>"Delete your account"</h1>
            <p>"Deleting the account of " <strong>(&shopper.email)</strong> ":"</p>
            <ul>
                <li>"forgets the addresses you saved;"</li>
                <li>"signs you out everywhere, this browser included."</li>
            </ul>
            <p>"Your orders and their invoices stay with the shop, which must keep them. If you sign in again with this address, you start a new account and find your orders there."</p>
            <form method="post" action=(href!(delete))>
                <button class="primary">"Delete my account"</button>
                " "
                <a href=(account_url(cx, None))>"Keep my account"</a>
            </form>
        )
    })
}

#[route(POST)]
pub async fn delete(cx: &Cx) -> Result<SeeOther> {
    let Some(token) = shopper_token(cx) else {
        return Ok(see_other(account_url(cx, None)));
    };
    match delete_account(cx, &token).await? {
        Ok(()) => {}
        // The session was already over and nothing was deleted: sign in first.
        Err(refusal) if refusal.is_signed_out() => {
            forget_shopper(cx);
            let back = href!(index).resolve(cx);
            return Ok(see_other(sign_in_url(
                cx,
                Some(&back),
                Some(refusal.code()),
            )));
        }
        Err(refusal) => return Ok(see_other(account_url(cx, Some(refusal.code())))),
    }
    forget_shopper(cx);
    // It keeps the address and the e-mail typed at the last checkout.
    forget_checkout_draft(cx);
    Ok(see_other(href!(crate::app::index).resolve(cx)))
}
