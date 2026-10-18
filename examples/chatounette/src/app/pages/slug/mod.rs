//! One of the shop's own pages: a title and a few paragraphs, written for
//! this demonstration shop (nothing here is legal advice, and no real
//! company stands behind it).

use timada_storefront::menu;
use topcoat::{
    Result,
    context::Cx,
    router::{error::not_found, href, module_param, page, path_param},
    view::{View, view},
};

use crate::app::document;

module_param!(pub slug);

/// A page: its slug, its title, its paragraphs. The support page has a
/// module of its own (`/support-client`) and only lends its entry to the
/// search.
pub struct StaticPage {
    pub slug: &'static str,
    pub title: &'static str,
    pub paragraphs: &'static [&'static str],
}

pub const PAGES: &[StaticPage] = &[
    StaticPage {
        slug: "mentions-legales",
        title: "Mentions légales",
        paragraphs: &[
            "Ce site est une boutique de démonstration construite sur la plateforme timada. Il reprend la présentation d'une boutique en ligne pour montrer ce qu'un marchand peut y publier ; aucune société n'exploite ce site et aucune commande n'y est honorée.",
            "Éditeur : l'équipe timada, pour les besoins de la démonstration. Hébergement : la machine sur laquelle vous l'avez lancé.",
            "Les textes et photographies des produits proviennent du catalogue public dont cette démonstration s'inspire et restent la propriété de leurs auteurs.",
        ],
    },
    StaticPage {
        slug: "conditions-generales-de-vente",
        title: "Conditions générales de vente",
        paragraphs: &[
            "Les présentes conditions encadrent toute commande passée sur la boutique. Passer commande vaut acceptation de ces conditions.",
            "Les prix sont indiqués en euros, toutes taxes comprises, hors frais de livraison précisés avant le paiement. La commande est ferme une fois le paiement accepté ; une confirmation est envoyée par e-mail.",
            "Conformément au droit de la consommation, vous disposez de 14 jours à compter de la réception pour changer d'avis, sans avoir à vous justifier. Les articles doivent être retournés complets et en bon état.",
            "Cette boutique étant une démonstration, aucune de ces dispositions ne crée d'engagement réel.",
        ],
    },
    StaticPage {
        slug: "politique-de-confidentialite",
        title: "Politique de confidentialité",
        paragraphs: &[
            "Les informations saisies lors d'une commande (nom, adresse de livraison, e-mail, téléphone) servent uniquement à préparer, expédier et suivre la commande, puis à éditer la facture.",
            "Un compte client conserve vos commandes et vos adresses pour vos prochains achats ; vous pouvez le supprimer à tout moment depuis la page Mon compte, ce qui rend ses données illisibles.",
            "Aucune donnée n'est cédée à des tiers. Le paiement est traité par un prestataire de paiement : la boutique ne voit jamais votre numéro de carte.",
        ],
    },
    StaticPage {
        slug: "politique-de-remboursement",
        title: "Politique de remboursement",
        paragraphs: &[
            "Un article qui ne vous convient pas peut nous être retourné dans les 14 jours suivant sa réception. Prévenez le service client, qui vous indique la marche à suivre.",
            "Le remboursement intervient dans les 14 jours suivant la réception du retour, par le moyen de paiement utilisé à la commande. Un avoir est joint à votre commande dans votre espace client.",
            "Un article arrivé abîmé ou non conforme est remplacé ou remboursé, frais de retour compris.",
        ],
    },
    StaticPage {
        slug: "politique-de-livraison",
        title: "Politique de livraison",
        paragraphs: &[
            "Nous livrons en France, en Belgique et en Suisse, avec suivi de colis. La livraison est offerte dès 65 € d'achats ; en dessous, elle est facturée 4,90 €.",
            "Les commandes sont préparées sous 24 à 48 heures ouvrées, puis acheminées sous 3 à 7 jours selon la destination. Un e-mail vous prévient à l'expédition.",
            "Si votre colis tarde, le service client le localise avec vous et, au besoin, renvoie la commande.",
        ],
    },
    StaticPage {
        slug: "qui-sommes-nous",
        title: "Qui sommes-nous ?",
        paragraphs: &[
            "Chatounette est née de deux amoureux des chats qui ne trouvaient pas, en une seule boutique, tout ce dont leurs compagnons avaient besoin : un couchage vraiment douillet, des jouets qui résistent, des accessoires jolis sans être fragiles.",
            "Nous testons ce que nous vendons avec nos propres chats, Moustache et Praline, qui ont un avis très arrêté sur les griffoirs.",
            "Cette version de la boutique est une démonstration de la plateforme timada : la vitrine, le panier, le paiement et les avis fonctionnent comme dans une vraie boutique, sans qu'aucun colis ne parte.",
        ],
    },
];

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let slug = path_param::<Slug>(cx);
    let page = PAGES
        .iter()
        .find(|page| page.slug == slug)
        .ok_or_else(not_found)?;
    let nav = menu(cx).await?;
    Ok(view! {
        document(
            title: page.title,
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(href!(crate::app::index))>"Accueil"</a> " / " (page.title)
                </nav>
                <article class="page" data-page=(page.slug)>
                    <h1>(page.title)</h1>
                    for paragraph in page.paragraphs {
                        <p>(paragraph)</p>
                    }
                </article>
            </div>
        )
    })
}
