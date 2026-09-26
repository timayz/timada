//! Storefront pages. Explicit paths on purpose: the host must not use
//! `module_router!()` while the admin's module-derived pages are linked in.

pub mod account;
pub mod cart;
pub mod catalog;
pub mod category;
pub mod checkout;
pub mod company;
pub mod currency;
pub mod forgot;
mod format;
pub mod guest;
pub mod invoice;
pub mod listing;
pub mod media;
pub mod returns;
pub mod seo;
#[cfg(feature = "stripe")]
pub mod webhooks;

use topcoat::{
    Result,
    context::Cx,
    router::href,
    view::{Child, Unescaped, View, component, view},
};

use crate::{auth::current_account, cart_session::current_cart, db::mailer_config};

/// What a page tells search engines about itself, beyond its title.
#[derive(Debug, Clone, Default)]
pub struct Head {
    pub description: Option<String>,
    /// The one address to keep for this content, as a path of the shop.
    pub canonical: Option<String>,
    /// `noindex,follow` for the endless variations of a listing.
    pub robots: Option<&'static str>,
    /// The pages before and after, in a paged series.
    pub previous: Option<String>,
    pub next: Option<String>,
    /// A schema.org description, as JSON (see [`seo`]).
    pub json_ld: Option<String>,
}

/// A path of the shop as the full address others must use.
pub fn absolute(path: &str) -> String {
    format!("{}{path}", mailer_config().base_url.trim_end_matches('/'))
}

/// The palette, shared with the print-only invoice sheet. Both are written
/// as CSS files and inlined: `asset!()` would panic wherever the router runs
/// without an asset bundle, which is every test and a bare `cargo run`.
pub(super) const TOKENS: &str = include_str!("tokens.css");
const STOREFRONT: &str = include_str!("storefront.css");

/// The `<html>` shell: header with the cart and account links, then the page.
/// `refresh` reloads the page after that many seconds (pages waiting on a
/// process manager).
#[component]
pub async fn document(
    cx: &Cx,
    title: &str,
    #[default] refresh: Option<u32>,
    #[default] head: Option<&Head>,
    child: Child<'_>,
) -> Result<impl View> {
    let head = head.cloned().unwrap_or_default();
    let canonical = head.canonical.as_deref().map(absolute);
    let previous = head.previous.as_deref().map(absolute);
    let next = head.next.as_deref().map(absolute);
    // Trusted: built by `seo`, which escapes everything it is given.
    let json_ld = head.json_ld.map(Unescaped::new_unchecked);
    let account = match current_account(cx).await {
        Ok(account) => account.clone(),
        Err(err) => return Err(topcoat::Error::msg(format!("{err:#}"))),
    };
    let cart_count: u32 = match current_cart(cx).await {
        Ok(cart) => cart
            .as_ref()
            .map_or(0, |c| c.lines.iter().map(|l| l.quantity).sum()),
        Err(err) => return Err(topcoat::Error::msg(format!("{err:#}"))),
    };

    // The currencies to pick from, when the shop sells in more than one:
    // `(code, how it is written, current)`.
    let current_currency = crate::currency::shopper_currency(cx).await?;
    let currencies: Vec<(String, String, bool)> = {
        let shop = crate::db::shop_currencies();
        if shop.others().is_empty() {
            Vec::new()
        } else {
            shop.all()
                .map(|code| {
                    let symbol = timada_core::format::currency_symbol(code);
                    let written = if symbol == code {
                        code.to_owned()
                    } else {
                        format!("{code} ({symbol})")
                    };
                    (code.to_owned(), written, code == current_currency)
                })
                .collect()
        }
    };
    // Where the switch comes back to: this page — unless it answers a form,
    // whose address is not one to `GET`.
    let here = if topcoat::router::request::method(cx) == topcoat::router::Method::GET {
        topcoat::router::request::uri(cx)
            .path_and_query()
            .map_or_else(|| "/".to_owned(), |path| path.as_str().to_owned())
    } else {
        "/".to_owned()
    };

    Ok(view! {
        <!DOCTYPE html>
        <html lang="fr">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                if let Some(seconds) = refresh {
                    <meta http-equiv="refresh" content=(seconds.to_string())>
                }
                <title>(title) " · Timada demo"</title>
                if let Some(description) = &head.description {
                    <meta name="description" content=(description.clone())>
                }
                if let Some(robots) = head.robots { <meta name="robots" content=(robots)> }
                if let Some(canonical) = &canonical { <link rel="canonical" href=(canonical.clone())> }
                if let Some(previous) = &previous { <link rel="prev" href=(previous.clone())> }
                if let Some(next) = &next { <link rel="next" href=(next.clone())> }
                if let Some(json_ld) = json_ld {
                    <script type="application/ld+json">(json_ld)</script>
                }
                <style>(Unescaped::new_unchecked(TOKENS))(Unescaped::new_unchecked(STOREFRONT))</style>
            </head>
            <body>
                <header class="globalnav">
                    <a href=(href!(catalog::home))><strong>"Timada demo"</strong></a>
                    <nav aria-label="Principal">
                        <a href=(href!(cart::show))>"Panier (" (cart_count.to_string()) ")"</a>
                        match &account {
                            Some(account) => {
                                <a href=(href!(account::overview))>(account.email.clone())</a>
                                <form method="post" action=(href!(account::logout)) class="inline">
                                    <button type="submit" class="link">"Se déconnecter"</button>
                                </form>
                            }
                            None => {
                                <a href=(href!(account::login))>"Se connecter"</a>
                                <a href=(href!(account::register))>"Créer un compte"</a>
                            }
                        }
                        <a href="/admin">"Administration"</a>
                    </nav>
                </header>
                <nav class="shopnav" aria-label="Boutique">
                    <span class="shopname">"Boutique"</span>
                    <form method="get" action=(href!(catalog::search)) role="search" class="inline">
                        <input type="search" name="q" aria-label="Rechercher un produit" placeholder="Rechercher…" size="18">
                        <button type="submit">"Chercher"</button>
                    </form>
                    if !currencies.is_empty() {
                        <form method="post" action=(href!(currency::switch)) class="inline">
                            <input type="hidden" name="next" value=(here.clone())>
                            <label for="currency" class="muted">"Devise"</label>
                            <select id="currency" name="currency">
                                for (code, written, current) in &currencies {
                                    <option value=(code.clone()) selected=(*current)>(written.clone())</option>
                                }
                            </select>
                            <button type="submit">"Changer"</button>
                        </form>
                    }
                </nav>
                <main><div class="page">(child)</div></main>
                site_footer()
            </body>
        </html>
    })
}

/// The foot of every page: where else to go, and what this shop is not.
/// Deliberately free of any amount — a test reads everything after `<main>`
/// and refuses to find a currency there.
#[component]
pub async fn site_footer() -> Result<impl View> {
    Ok(view! {
        <footer class="sitefooter">
            <p>
                "Boutique de démonstration. Le catalogue, les prix et les stocks sont \
                 fictifs : aucune commande n'est expédiée et aucun paiement réel n'est \
                 encaissé."
            </p>
            <p>"Timada est une bibliothèque libre. Cette vitrine en est l'exemple d'intégration."</p>
            <nav aria-label="Pied de page">
                <div>
                    <h2>"La boutique"</h2>
                    <ul>
                        <li><a href=(href!(catalog::home))>"Catalogue"</a></li>
                        <li><a href=(href!(catalog::search))>"Rechercher un produit"</a></li>
                        <li><a href=(href!(cart::show))>"Panier"</a></li>
                    </ul>
                </div>
                <div>
                    <h2>"Votre compte"</h2>
                    <ul>
                        <li><a href=(href!(account::overview))>"Vue d\u{2019}ensemble"</a></li>
                        <li><a href=(href!(account::orders))>"Commandes"</a></li>
                        <li><a href=(href!(account::addresses))>"Adresses"</a></li>
                        <li><a href=(href!(account::saved_carts))>"Paniers enregistrés"</a></li>
                        <li><a href=(href!(account::alerts))>"Alertes de stock"</a></li>
                    </ul>
                </div>
                <div>
                    <h2>"Aide et services"</h2>
                    <ul>
                        <li><a href=(href!(account::orders))>"Suivre une commande"</a></li>
                        <li><a href=(href!(company::show))>"Informations entreprise"</a></li>
                        <li><a href=(href!(forgot::forgot))>"Mot de passe oublié"</a></li>
                    </ul>
                </div>
                <div>
                    <h2>"Accès"</h2>
                    <ul>
                        <li><a href=(href!(account::login))>"Se connecter"</a></li>
                        <li><a href=(href!(account::register))>"Créer un compte"</a></li>
                        <li><a href="/admin">"Administration"</a></li>
                    </ul>
                </div>
            </nav>
            <p class="legal">
                "Copyright © 2026 Timada. Tous droits réservés."
                " "
                <a href="/sitemap.xml">"Plan du site"</a>
                " "
                "France"
            </p>
        </footer>
    })
}

/// One step of a breadcrumb; the page itself has no link.
pub struct Crumb {
    pub label: String,
    pub link: Option<String>,
}

/// Where the page sits in the shop, as an ordered trail.
#[component]
pub async fn breadcrumb(trail: &[Crumb]) -> Result<impl View> {
    Ok(view! {
        <nav aria-label="Fil d'Ariane" class="crumbs">
            <ol>
                for crumb in trail {
                    match &crumb.link {
                        Some(link) => { <li><a href=(link.clone())>(crumb.label.clone())</a></li> }
                        None => { <li aria-current="page">(crumb.label.clone())</li> }
                    }
                }
            </ol>
        </nav>
    })
}

/// Only same-site absolute paths are followed after a login or a form.
pub fn safe_next(next: Option<String>) -> Option<String> {
    next.filter(|n| n.starts_with('/') && !n.starts_with("//") && !n.contains('\\'))
}
