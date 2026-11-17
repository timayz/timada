//! `/account`: the signed-in shopper's orders (every order placed with
//! their address, on any device) and saved addresses, the newsletter switch,
//! a way to sign out every other browser, and the way to delete the account. Signed out, it
//! sends to the sign-in page.

pub mod addresses;
pub mod delete;
pub mod newsletter;
pub mod sign_in;
pub mod sign_out;
pub mod sign_out_everywhere;

use timada_storefront::{OrderStatus, menu, shopper_newsletter, shopper_orders};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{RouterErrorExt, redirect},
        href, page, query_params,
    },
    view::{View, view},
};

use crate::app::orders::order_id::{index as order_page, order_id};
use crate::app::words::{Worded, t};
use crate::app::{document, say};
use crate::shopper::{forget_shopper, session, shopper_token};

#[topcoat::router::query_params(error = bad_request)]
struct AccountQuery {
    error: Option<String>,
    done: Option<String>,
}

pub fn account_url(cx: &Cx, error: Option<&str>) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        error: Option<&'a str>,
    }
    href!(index).query(Link { error }).resolve(cx)
}

/// The account page, saying what was just `done` (see [`say_done`]).
pub fn account_done_url(cx: &Cx, done: &str) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        done: &'a str,
    }
    href!(index).query(Link { done }).resolve(cx)
}

/// What the account page says once something is done.
fn say_done(code: &str) -> Option<Worded<'static>> {
    match code {
        sign_out_everywhere::DONE => Some(t!(account_done_signed_out_elsewhere)),
        newsletter::SUBSCRIBED => Some(t!(account_done_newsletter_on)),
        newsletter::UNSUBSCRIBED => Some(t!(account_done_newsletter_off)),
        _ => None,
    }
}

/// An order's status, as the orders table shows it.
fn status_label(status: OrderStatus) -> Worded<'static> {
    match status {
        OrderStatus::Authorized => t!(account_status_authorized),
        OrderStatus::Paid => t!(account_status_paid),
        OrderStatus::Fulfilled => t!(account_status_fulfilled),
        OrderStatus::Cancelled => t!(account_status_cancelled),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let to_sign_in = sign_in::sign_in_url(cx, Some(&href!(index).resolve(cx)), None);
    // The dashboard away is told as such, not as a sign-in to do again.
    let shopper = session(cx)
        .await?
        .clone()
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
    // Unknown (the dashboard refused to say) reads as not subscribed.
    let subscribed = shopper_newsletter(cx, &token).await?.unwrap_or(false);
    let query = query_params::<AccountQuery>(cx)?;
    let nav = menu(cx).await?;
    let title = t!(shell_account).resolve(cx);
    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <div class="wrap">
                <h1>(t!(shell_account))</h1>
                <p>(t!(account_signed_in_as)) " " <strong data-account-email="">(&shopper.email)</strong></p>
                <form method="post" action=(href!(sign_out::sign_out))>
                    <button>(t!(account_sign_out))</button>
                </form>
                if let Some(code) = query.error.as_deref() {
                    <p class="notice" role="alert">(say(code))</p>
                }
                if let Some(done) = query.done.as_deref().and_then(say_done) {
                    <p class="notice" role="status">(done)</p>
                }
                <section data-account-orders="">
                    <h2>(t!(account_orders_title))</h2>
                    if orders.is_empty() {
                        <p>(t!(account_orders_empty))</p>
                    } else {
                        <table class="history">
                            <thead>
                                <tr><th>(t!(account_orders_reference))</th><th>(t!(account_orders_items))</th><th>(t!(account_orders_total))</th><th>(t!(account_orders_status))</th></tr>
                            </thead>
                            <tbody>
                                for order in &orders {
                                    <tr data-account-order=(&order.reference)>
                                        <td><a href=(href!(order_page, order_id: order.id.clone()))>(&order.reference)</a></td>
                                        <td>(order.item_count)</td>
                                        <td>(order.total_label())</td>
                                        <td>(status_label(order.status()))</td>
                                    </tr>
                                }
                            </tbody>
                        </table>
                    }
                </section>
                <section data-account-addresses="">
                    <h2>(t!(account_addresses_title))</h2>
                    if shopper.addresses.is_empty() {
                        <p>(t!(account_addresses_empty))</p>
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
                                        <button>(t!(account_address_remove))</button>
                                    </form>
                                </li>
                            }
                        </ul>
                    }
                </section>
                <section data-account-newsletter=(if subscribed { "on" } else { "off" })>
                    <h2>(t!(footer_newsletter))</h2>
                    if subscribed {
                        <p>(t!(account_newsletter_on_text))</p>
                        <form method="post" action=(href!(newsletter::newsletter))>
                            <input type="hidden" name="subscribed" value="false">
                            <button>(t!(account_newsletter_stop))</button>
                        </form>
                    } else {
                        <p>(t!(account_newsletter_off_text))</p>
                        <form method="post" action=(href!(newsletter::newsletter))>
                            <input type="hidden" name="subscribed" value="true">
                            <button>(t!(account_newsletter_start))</button>
                        </form>
                    }
                </section>
                <section data-account-sessions="">
                    <h2>(t!(account_sessions_title))</h2>
                    <p>(t!(account_sessions_text))</p>
                    <form method="post" action=(href!(sign_out_everywhere::sign_out_everywhere))>
                        <button>(t!(account_sign_out_elsewhere))</button>
                    </form>
                </section>
                <section data-account-delete="">
                    <h2>(t!(account_delete_title))</h2>
                    <p>(t!(account_delete_text))</p>
                    <a href=(href!(delete::index))>(t!(account_delete_link))</a>
                </section>
            </div>
        )
    })
}
