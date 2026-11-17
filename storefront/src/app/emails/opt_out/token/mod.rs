//! `/emails/opt-out/{token}`: the link at the foot of every automated mail
//! of the shop. Opening it asks for one click (a mail scanner opening it
//! stops nothing); the button, and a mail client's one-click unsubscribe
//! (`List-Unsubscribe-Post`, a POST here), hand the token to the dashboard.

use timada_storefront::{menu, stop_automated_mails};
use topcoat::{
    Result,
    context::Cx,
    router::{href, module_param, page},
    view::{View, view},
};

use crate::app::{document, words::t};

module_param!(pub token);

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let token = token(cx).to_owned();
    let nav = menu(cx).await?;
    let title = t!(newsletter_automated_stop_title).resolve(cx);
    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <div class="wrap">
                <h1>(&*title)</h1>
                <div class="card panel">
                    <p>(t!(newsletter_automated_stop_text))</p>
                    <form method="post" action=(href!(submit, token: token)) data-automation-opt-out="">
                        <button class="btn">(t!(newsletter_automated_stop_button))</button>
                    </form>
                </div>
            </div>
        )
    })
}

#[page(POST)]
pub async fn submit(cx: &Cx) -> Result<impl View> {
    let token = token(cx).to_owned();
    let stopped = stop_automated_mails(cx, &token).await?;
    let nav = menu(cx).await?;
    let title = t!(newsletter_automated_title).resolve(cx);
    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <div class="wrap">
                <h1>(&*title)</h1>
                <div class="card panel" role="status">
                    if stopped {
                        <p>(t!(newsletter_automated_stopped))</p>
                    } else {
                        <p>(t!(newsletter_automated_broken_link))</p>
                    }
                </div>
            </div>
        )
    })
}
