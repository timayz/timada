//! `/emails/opt-out/{token}`: the link at the foot of every automated mail
//! of the shop. Opening it asks for one click (a mail scanner opening it
//! stops nothing); the button, and a mail client's one-click unsubscribe
//! (`List-Unsubscribe-Post`, a POST here), hand the token to the dashboard.

use timada_storefront::{menu, stop_automated_mails};
use topcoat::{
    Result,
    context::Cx,
    router::{href, module_param, page, path_param},
    view::{View, view},
};

use crate::app::document;

module_param!(pub token);

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let token = path_param::<Token>(cx).to_owned();
    let nav = menu(cx).await?;
    Ok(view! {
        document(
            title: "Ne plus recevoir nos e-mails automatiques",
            nav: &nav,
            <div class="wrap">
                <h1>"Ne plus recevoir nos e-mails automatiques"</h1>
                <div class="card panel">
                    <p>"Vous ne recevrez plus les e-mails que la boutique envoie d'elle-même (après une commande, un avis…). Vos commandes, elles, continuent de vous être confirmées."</p>
                    <form method="post" action=(href!(submit, Token(token))) data-automation-opt-out="">
                        <button class="btn">"Ne plus les recevoir"</button>
                    </form>
                </div>
            </div>
        )
    })
}

#[page(POST)]
pub async fn submit(cx: &Cx) -> Result<impl View> {
    let token = path_param::<Token>(cx).to_owned();
    let stopped = stop_automated_mails(cx, &token).await?;
    let nav = menu(cx).await?;
    Ok(view! {
        document(
            title: "E-mails automatiques",
            nav: &nav,
            <div class="wrap">
                <h1>"E-mails automatiques"</h1>
                <div class="card panel" role="status">
                    if stopped {
                        <p>"C'est noté : vous ne recevrez plus nos e-mails automatiques."</p>
                    } else {
                        <p>"Ce lien ne fonctionne pas : il a peut-être été coupé. Écrivez-nous et nous nous en chargeons."</p>
                    }
                </div>
            </div>
        )
    })
}
