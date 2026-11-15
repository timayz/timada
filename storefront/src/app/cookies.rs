//! The cookie banner and `/cookies`: what the visitor allows the shop to
//! keep on their device beyond what selling needs (the cart, the sign-in).
//!
//! The banner sits at the top of every page until it is answered: accept,
//! refuse, or choose by purpose on `/cookies`, which the footer links to so
//! an answer can be changed at any time. Each is a plain form posted here:
//! nothing needs a script. The answer is kept for
//! [`CONSENT_MAX_AGE_DAYS`](timada_storefront::CONSENT_MAX_AGE_DAYS), and
//! the next page tells Google's Consent Mode of it
//! ([`consent_script`](timada_storefront::consent_script)).

use std::collections::HashMap;

use timada_storefront::{Consent, menu};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, page, query_params,
        request::original_uri,
        route,
    },
    view::{View, view},
};

use crate::app::cart::site_path;
use crate::app::document;
use crate::shopper::{consent, set_consent};

#[topcoat::router::query_params(error = bad_request)]
struct CookiesQuery {
    /// Back from saving a choice.
    done: Option<String>,
}

/// The banner: shown by the layout while the visitor has not answered.
/// Refusing takes one click, as accepting does.
#[topcoat::view::component]
pub async fn consent_banner(cx: &Cx) -> Result<impl View> {
    // Back to the page the banner sat on, with its query: what brought the
    // visitor (a campaign's parameters) is still in it.
    let here = original_uri(cx)
        .path_and_query()
        .map(|here| here.as_str().to_owned())
        .unwrap_or_else(|| "/".to_owned());
    Ok(view! {
        <section class="consent" aria-label="Cookies" data-consent="">
            <div class="wrap">
                <p>
                    "Avec votre accord, nous utilisons des cookies pour mesurer l'audience du site et l'efficacité de nos publicités. "
                    "Vous pouvez changer d'avis à tout moment, depuis le bas de chaque page."
                </p>
                <form method="post" action=(href!(answer))>
                    <input type="hidden" name="from" value=(here)>
                    <button class="btn" name="answer" value="none">"Tout refuser"</button>
                    <button class="btn" name="answer" value="all">"Tout accepter"</button>
                    <a href=(href!(index))>"Choisir"</a>
                </form>
            </div>
        </section>
    })
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<CookiesQuery>(cx)?;
    let nav = menu(cx).await?;
    let given = consent(cx);
    let allowed = given.unwrap_or_default();
    let title = "Gestion des cookies";
    Ok(view! {
        document(
            title: title,
            nav: &nav,
            no_banner: true,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane">
                    <a href=(href!(crate::app::index))>"Accueil"</a> " / " (title)
                </nav>
                <h1>(title)</h1>
                <div class="card panel" data-cookies=(given.map(|consent| consent.cookie_value()).unwrap_or("unanswered"))>
                    if query.done.is_some() {
                        <p class="notice" role="status">"Votre choix est enregistré."</p>
                    }
                    <p>
                        "Les cookies nécessaires au fonctionnement de la boutique (votre panier, votre commande, votre connexion) sont toujours déposés. "
                        "Les autres ne le sont qu'avec votre accord, que vous pouvez retirer ici à tout moment."
                    </p>
                    <form class="cookies" method="post" action=(href!(answer))>
                        <label class="check">
                            <input type="checkbox" name="analytics" value="1" checked=(allowed.analytics)>
                            <span><strong>"Mesure d'audience"</strong>" : savoir quelles pages sont vues et d'où viennent nos visiteurs."</span>
                        </label>
                        <label class="check">
                            <input type="checkbox" name="ads" value="1" checked=(allowed.ads)>
                            <span><strong>"Publicité"</strong>" : mesurer l'efficacité de nos annonces et les adapter à vos centres d'intérêt."</span>
                        </label>
                        <p class="cookies-act">
                            <button class="btn" name="answer" value="choice">"Enregistrer mes choix"</button>
                            <button class="btn" name="answer" value="none">"Tout refuser"</button>
                            <button class="btn" name="answer" value="all">"Tout accepter"</button>
                        </p>
                    </form>
                </div>
            </div>
        )
    })
}

/// Keeps the answer, then back to the page the banner sat on (`from`, a path
/// on this site), or to this page saying so.
#[route(POST)]
pub async fn answer(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let field = |name: &str| form.get(name).map(String::as_str).unwrap_or_default();
    let consent = match field("answer") {
        "all" => Consent::ALL,
        "choice" => Consent {
            analytics: !field("analytics").is_empty(),
            ads: !field("ads").is_empty(),
        },
        // Anything else refuses: nothing is allowed by accident.
        _ => Consent::NONE,
    };
    set_consent(cx, consent);
    let to = match form.get("from") {
        Some(from) => site_path(from).to_owned(),
        None => {
            #[derive(serde::Serialize)]
            struct Link {
                done: &'static str,
            }
            href!(index).query(Link { done: "1" }).resolve(cx)
        }
    };
    Ok(see_other(to))
}
