//! The support page at `/support-client`, as chatounette.com lays it out: the
//! contact form on one side, the questions shoppers ask most on the other,
//! the shop's four promises below. The form posts here too and hands the
//! message to the dashboard, where the shop reads it; the shopper is told
//! it was sent, or why it was refused.

use std::collections::HashMap;

use timada_storefront::{MessageDraft, menu, send_message, takes_messages};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, query_params, route,
    },
    view::{View, view},
};

use crate::app::pages::slug::StaticPage;
use crate::app::{document, reassurance, say};

/// The address the page names, and the footer with it.
pub const CONTACT_EMAIL: &str = "contact@chatounette.localhost";

/// What the form says when the shop has nowhere to send messages (no
/// dashboard address): the visitor is told to write instead.
const MESSAGES_UNAVAILABLE: &str = "messages_unavailable";

/// The questions and their answers, in the demo's own terms (its policy
/// pages say the same).
pub const FAQ: &[(&str, &str)] = &[
    (
        "Votre site Web est-il sécurisé ?",
        "Oui : tout ce que vous saisissez transite chiffré, et le paiement est confié à Stripe. La boutique ne voit jamais votre numéro de carte.",
    ),
    (
        "Sur quel serveur le site Web est-il hébergé ?",
        "Sur la machine qui fait tourner cette démonstration. Chatounette est une boutique de démonstration de la plateforme timada : rien n'est expédié.",
    ),
    (
        "Quel est le délai de livraison prévu ?",
        "Votre commande est préparée sous 24 à 48 heures ouvrées, puis livrée sous 3 à 7 jours en France, en Belgique et en Suisse, avec suivi de colis. La livraison est offerte dès 65 € d'achats.",
    ),
    (
        "Quelles sont les étapes pour suivre votre colis ?",
        "Dès l'expédition, un e-mail vous est envoyé avec le lien de suivi. Votre commande affiche aussi son état dans la page Mon compte.",
    ),
    (
        "Puis-je demander une annulation de ma commande ?",
        "Tant que la commande n'est pas expédiée, écrivez-nous et nous l'annulons. Une fois expédiée, vous disposez de 14 jours après la livraison pour nous la retourner.",
    ),
    (
        "Vous souhaitez effectuer un retour produit ?",
        "Écrivez-nous à contact@chatounette.localhost : nous vous indiquons la marche à suivre. L'article revient complet et en bon état ; le remboursement intervient dans les 14 jours suivant sa réception.",
    ),
    (
        "Puis-je me renseigner sur votre politique concernant les retours ?",
        "Vous avez 14 jours après la livraison pour changer d'avis, sans avoir à vous justifier. Un article arrivé abîmé est remplacé ou remboursé, frais de retour compris. Le détail est dans notre politique de remboursement.",
    ),
];

/// How the live search finds this page: its title and its questions.
pub const SEARCH_ENTRY: StaticPage = StaticPage {
    slug: "support-client",
    title: "Support client",
    paragraphs: &[
        "Nous contacter : une question sur un article, une commande, un retour ? Le service client répond du lundi au samedi.",
        "Questions fréquemment posées : sécurité, hébergement, délai de livraison, suivi de colis, annulation, retour produit, politique de retours.",
    ],
};

#[topcoat::router::query_params(error = bad_request)]
struct SupportQuery {
    error: Option<String>,
    sent: Option<String>,
}

#[derive(serde::Serialize)]
struct Link<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sent: Option<u8>,
}

/// The page, carrying a refusal or the word that the message went.
fn support_url(cx: &Cx, error: Option<&str>, sent: bool) -> String {
    href!(index)
        .query(Link {
            error,
            sent: sent.then_some(1),
        })
        .resolve(cx)
}

/// How a refusal of the form reads; the codes the rest of the shop shares
/// read as they do elsewhere.
fn say_support(code: &str) -> &'static str {
    match code {
        "message_invalid_name" => {
            "Indiquez votre prénom et votre nom (80 caractères au plus chacun)."
        }
        "message_invalid_email" => "Saisissez une adresse e-mail valide.",
        "message_invalid_body" => "Dites-nous en un peu plus : de 10 à 4000 caractères.",
        "too_many_requests" => "Trop de messages envoyés : réessayez dans quelques minutes.",
        MESSAGES_UNAVAILABLE => {
            "L'envoi de messages est indisponible pour le moment : écrivez-nous par e-mail."
        }
        other => say(other),
    }
}

#[topcoat::router::page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<SupportQuery>(cx)?;
    let nav = menu(cx).await?;
    let takes = takes_messages(cx);
    let sent = query.sent.is_some();
    let title = "Support client";
    Ok(view! {
        document(
            title: title,
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(href!(crate::app::index))>"Accueil"</a> " / " (title)
                </nav>
                <h1>"Support client : " (CONTACT_EMAIL)</h1>
                <section class="support" data-support="">
                    <div class="card panel">
                        <span class="eyebrow">"Besoin d'aide supplémentaire ?"</span>
                        <h2>"Nous contacter"</h2>
                        if let Some(code) = &query.error {
                            <p class="notice" role="alert">(say_support(code))</p>
                        }
                        if sent {
                            <p class="notice" role="status" data-sent="">"Message envoyé : nous vous répondons sous 24 h ouvrées, à l'adresse indiquée."</p>
                        }
                        if takes {
                            <form class="checkout" method="post" action=(href!(send)) data-support-form="">
                                <div class="row">
                                    <label>"Prénom"
                                        <input name="first_name" type="text" required="" maxlength="80" autocomplete="given-name">
                                    </label>
                                    <label>"Nom"
                                        <input name="last_name" type="text" required="" maxlength="80" autocomplete="family-name">
                                    </label>
                                </div>
                                <label>"Email"
                                    <input name="email" type="email" required="" autocomplete="email">
                                </label>
                                <label>"Message"
                                    <textarea name="body" rows="6" required="" minlength="10" maxlength="4000"></textarea>
                                </label>
                                <p class="hint">"Nous répondons du lundi au samedi, de 9h à 18h."</p>
                                <button class="btn">"Envoyer"</button>
                            </form>
                        } else {
                            <p data-support-unavailable="">"Écrivez-nous à " <a href=(format!("mailto:{CONTACT_EMAIL}"))>(CONTACT_EMAIL)</a> " : nous répondons du lundi au samedi, de 9h à 18h."</p>
                        }
                    </div>
                    <div class="card panel">
                        <span class="eyebrow">"Trouvez votre réponse ici"</span>
                        <h2>"Questions fréquemment posées"</h2>
                        <div class="faq" data-faq="">
                            for (i, (question, answer)) in FAQ.iter().enumerate() {
                                if i == 0 {
                                    <details open="">
                                        <summary>(question)</summary>
                                        <p>(answer)</p>
                                    </details>
                                } else {
                                    <details>
                                        <summary>(question)</summary>
                                        <p>(answer)</p>
                                    </details>
                                }
                            }
                        </div>
                        <p class="hint">
                            "Le détail est dans notre "
                            <a href=(href!(crate::app::pages::slug::index, crate::app::pages::slug::Slug("politique-de-remboursement".to_owned())))>"politique de remboursement"</a>
                            " et notre "
                            <a href=(href!(crate::app::pages::slug::index, crate::app::pages::slug::Slug("politique-de-livraison".to_owned())))>"politique de livraison"</a>
                            "."
                        </p>
                    </div>
                </section>
            </div>
            <section class="block">
                <div class="wrap">
                    reassurance()
                </div>
            </section>
        )
    })
}

/// Hands the form's message to the dashboard, then back to the page with
/// the word that it went, or why it was refused.
#[route(POST)]
pub async fn send(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    if !takes_messages(cx) {
        return Ok(see_other(support_url(
            cx,
            Some(MESSAGES_UNAVAILABLE),
            false,
        )));
    }
    let field = |name: &str| form.get(name).cloned().unwrap_or_default();
    let draft = MessageDraft {
        first_name: field("first_name"),
        last_name: field("last_name"),
        email: field("email"),
        body: field("body"),
    };
    match send_message(cx, draft).await? {
        Ok(_) => Ok(see_other(support_url(cx, None, true))),
        Err(refusal) => Ok(see_other(support_url(cx, Some(refusal.code()), false))),
    }
}
