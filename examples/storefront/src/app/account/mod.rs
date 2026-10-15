//! `/account`: the signed-in shopper's orders (every order placed with
//! their address, on any device) and saved addresses. Signed out, it sends
//! to the sign-in page.

pub mod addresses;
pub mod sign_in;
pub mod sign_out;

use timada_storefront::{menu, shopper_orders};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{RouterErrorExt, redirect},
        href, page, query_params,
    },
    view::{View, view},
};

use crate::app::orders::order_id::{OrderId, index as order_page};
use crate::app::{document, say};
use crate::shopper::{forget_shopper, shopper_token, signed_in};

#[topcoat::router::query_params(error = bad_request)]
struct AccountQuery {
    error: Option<String>,
}

pub fn account_url(cx: &Cx, error: Option<&str>) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        error: Option<&'a str>,
    }
    href!(index).query(Link { error }).resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let to_sign_in = sign_in::sign_in_url(cx, Some(&href!(index).resolve(cx)), None);
    let shopper = signed_in(cx)
        .await
        .cloned()
        .ok_or_redirect(to_sign_in.clone())?;
    let token = shopper_token(cx).ok_or_redirect(to_sign_in.clone())?;
    let orders = match shopper_orders(cx, &token).await? {
        Ok(orders) => orders,
        Err(refusal) if refusal.is_signed_out() => {
            forget_shopper(cx);
            return Err(redirect(to_sign_in).into());
        }
        Err(_) => Vec::new(),
    };
    let query = query_params::<AccountQuery>(cx)?;
    let nav = menu(cx).await?;
    Ok(view! {
        document(
            title: "Your account",
            nav: &nav,
            <h1>"Your account"</h1>
            <p>"Signed in as " <strong data-account-email="">(&shopper.email)</strong></p>
            <form method="post" action=(href!(sign_out::sign_out))>
                <button>"Sign out"</button>
            </form>
            if let Some(code) = query.error.as_deref() {
                <p class="notice" role="alert">(say(code))</p>
            }
            <section data-account-orders="">
                <h2>"Your orders"</h2>
                if orders.is_empty() {
                    <p>"No order yet."</p>
                } else {
                    <table class="history">
                        <thead>
                            <tr><th>"Order"</th><th>"Items"</th><th>"Total"</th><th>"Status"</th></tr>
                        </thead>
                        <tbody>
                            for order in &orders {
                                <tr data-account-order=(&order.reference)>
                                    <td><a href=(href!(order_page, OrderId(order.id.clone())))>(&order.reference)</a></td>
                                    <td>(order.item_count)</td>
                                    <td>(order.total_label())</td>
                                    <td>(order.status().label())</td>
                                </tr>
                            }
                        </tbody>
                    </table>
                }
            </section>
            <section data-account-addresses="">
                <h2>"Your addresses"</h2>
                if shopper.addresses.is_empty() {
                    <p>"Addresses you ship to are kept here when you check out."</p>
                } else {
                    <ul class="saved">
                        for saved in &shopper.addresses {
                            <li data-saved-address=(&saved.address.city)>
                                <strong>(&saved.address.name)</strong> ", "
                                (&saved.address.line1) ", "
                                (&saved.address.postal_code) " " (&saved.address.city) ", "
                                (&saved.address.country) " "
                                <form method="post" action=(href!(addresses::remove::remove))>
                                    <input type="hidden" name="address_id" value=(&saved.id)>
                                    <button>"Remove"</button>
                                </form>
                            </li>
                        }
                    </ul>
                }
            </section>
        )
    })
}
