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
use crate::app::words::t;
use crate::shopper::{forget_checkout_draft, forget_shopper, session, shopper_token};

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let to_sign_in = sign_in_url(cx, Some(&href!(index).resolve(cx)), None);
    let shopper = session(cx).await?.clone().ok_or_redirect(to_sign_in)?;
    let nav = menu(cx).await?;
    let title = t!(account_delete_title).resolve(cx);
    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <div class="wrap">
                <h1>(t!(account_delete_title))</h1>
                <p>(t!(account_delete_of_before)) " " <strong>(&shopper.email)</strong> (t!(account_delete_of_after))</p>
                <ul>
                    <li>(t!(account_delete_forgets_addresses))</li>
                    <li>(t!(account_delete_signs_out))</li>
                </ul>
                <p>(t!(account_delete_orders_stay))</p>
                <form method="post" action=(href!(delete))>
                    <button class="primary">(t!(account_delete_confirm))</button>
                    " "
                    <a href=(account_url(cx, None))>(t!(account_delete_keep))</a>
                </form>
            </div>
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
