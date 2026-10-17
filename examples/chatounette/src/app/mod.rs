//! The pages: the home at `/`, the whole catalogue at `/boutique`, a category
//! at `/c/{slug}`, a product at `/p/{slug}`, the cart at `/cart`, the checkout
//! at `/checkout`, a placed order at `/orders/{order_id}`, the shopper's
//! account at `/account`, the shop's own pages at `/pages/{slug}`, and the
//! library's `/store/*` (its status and every action the forms post to). The
//! look is chatounette.com's: one stylesheet (`theme.css`) inlined in every
//! page, no asset bundle. Every page is markup around what the library's
//! `pages` worked out for it.

pub mod account;
pub mod boutique;
pub mod c;
pub mod cart;
pub mod checkout;
pub mod menu;
pub mod orders;
pub mod p;
pub mod pages;
pub mod reviews;

use timada_storefront::{
    Product, RouterBuilderStoreExt, Storefront, add_to_cart_form, cart_script, menu_script,
    pages::{HomeOptions, home_page, shell},
    price, product_image, stars,
};
use topcoat::{
    Result,
    context::Cx,
    router::{Router, RouterBuilderDiscoverExt, href, module_router, page},
    view::{Child, Unescaped, View, attributes, view},
};

use cart::drawer::{DRAWER_CSS, cart_drawer};
use menu::{MENU_CSS, site_menu};

/// The shop's name, as the pages and the mails say it.
pub const SHOP_NAME: &str = "Chatounette";
/// The code the top bar advertises; the seed creates it.
pub const PROMO_CODE: &str = "MONSTER10";
/// What the shop says about itself, under the logo.
const TAGLINE: &str = "La boutique chat n°1 sur internet";

/// The look, inlined in every page.
const THEME_CSS: &str = include_str!("theme.css");

pub fn router(storefront: Storefront) -> Router {
    module_router!().discover().store(storefront).build()
}

/// A paw, the shop's mark, drawn inline.
pub const PAW_SVG: &str = r##"<svg viewBox="0 0 100 100" aria-hidden="true" fill="currentColor"><ellipse cx="30" cy="32" rx="10" ry="13"/><ellipse cx="70" cy="32" rx="10" ry="13"/><ellipse cx="14" cy="55" rx="9" ry="11"/><ellipse cx="86" cy="55" rx="9" ry="11"/><path d="M50 48c-16 0-30 14-30 28 0 9 7 14 15 14 6 0 10-3 15-3s9 3 15 3c8 0 15-5 15-14 0-14-14-28-30-28z"/></svg>"##;

/// The HTML shell every page shares: the top bar, the masthead, the
/// category bar, the page, the footer, and (unless `no_drawer`, for the
/// cart and the checkout, which show the cart already) the cart drawer.
#[topcoat::view::component]
pub async fn document(
    cx: &Cx,
    title: &str,
    #[default] no_drawer: bool,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let shell = shell(cx).await?;
    let page_title = if title == SHOP_NAME {
        format!("{SHOP_NAME} : {TAGLINE}")
    } else {
        format!("{title} – {SHOP_NAME}")
    };
    let cart_total = shell
        .cart_subtotal
        .clone()
        .unwrap_or_else(|| "0,00€".to_owned());
    Ok(view! {
        <!DOCTYPE html>
        <html lang="fr">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>(page_title)</title>
                <style>(Unescaped::new_unchecked(THEME_CSS))</style>
                <style>(Unescaped::new_unchecked(DRAWER_CSS))</style>
                <style>(Unescaped::new_unchecked(MENU_CSS))</style>
            </head>
            <body>
                <div class="topbar">
                    <div class="wrap">
                        <span class="topbar-phone">"☎ 01 89 76 02 12"</span>
                        <p>"-10% maintenant avec le code " <strong>(PROMO_CODE)</strong></p>
                        <span class="topbar-rating"><span class="stars" aria-hidden="true">"★★★★★"</span> " 4,8/5 Avis clients"</span>
                    </div>
                </div>
                <header class="masthead">
                    <div class="wrap">
                        if !shell.nav.is_empty() {
                            <a class="menu-toggle" href="#menu" aria-controls="menu">"Menu"</a>
                        }
                        <a class="brand" href=(&shell.home_url)>"Chat" <span>"ounette"</span></a>
                        <form class="search" method="get" action=(href!(boutique::index)) role="search">
                            <input name="q" type="search" placeholder="Rechercher un produit…" aria-label="Rechercher un produit">
                            <button>"Rechercher"</button>
                        </form>
                        <nav class="quick" aria-label="Compte et panier">
                            <a class="quick-support" href=(href!(pages::slug::index, pages::slug::Slug("support-client")))>"Support client"</a>
                            if shell.accounts_enabled {
                                <a class="account-link" href=(&shell.account_url) data-account-link="">
                                    if shell.signed_in { "Mon compte" } else { "Se connecter" }
                                </a>
                            }
                            <a class="cart-link" href=(&shell.cart_url) data-cart-link="" aria-controls="cart">
                                <span class="quick-label">"Panier"</span>
                                <span class="count" data-cart-count="" hidden=(shell.cart_count == 0)>(shell.cart_count)</span>
                                <span class="cart-total" data-cart-total="">(cart_total)</span>
                            </a>
                        </nav>
                    </div>
                </header>
                if !shell.nav.is_empty() {
                    site_menu(nav: &shell.nav)
                    menu_script()
                }
                <main>(child)</main>
                site_footer()
                if !no_drawer {
                    cart_drawer()
                    cart_script()
                }
            </body>
        </html>
    })
}

/// The footer: the shop, its legal pages, the useful links and how to
/// reach it, as the live shop lays them out.
#[topcoat::view::component]
pub async fn site_footer() -> Result<impl View> {
    let page = |slug: &str| href!(pages::slug::index, pages::slug::Slug(slug.to_owned()));
    Ok(view! {
        <footer class="sitefooter">
            <div class="wrap">
                <div>
                    <a class="brand" href=(href!(index))>"Chat" <span>"ounette"</span></a>
                    <p>"Chatounette, l'endroit parfait pour les accessoires et jouets pour votre chat. Une boutique de démonstration : rien n'est expédié."</p>
                </div>
                <div>
                    <h2>"A propos"</h2>
                    <ul>
                        <li><a href=(page("mentions-legales"))>"Mentions légales"</a></li>
                        <li><a href=(page("conditions-generales-de-vente"))>"Conditions générales de vente"</a></li>
                        <li><a href=(page("politique-de-confidentialite"))>"Politique de confidentialité"</a></li>
                        <li><a href=(page("politique-de-remboursement"))>"Politique de remboursement"</a></li>
                        <li><a href=(page("politique-de-livraison"))>"Politique de livraison"</a></li>
                    </ul>
                </div>
                <div>
                    <h2>"Liens utiles"</h2>
                    <ul>
                        <li><a href=(href!(account::index))>"Suivre ma commande"</a></li>
                        <li><a href=(href!(boutique::index))>"Nos produits"</a></li>
                        <li><a href=(page("support-client"))>"Support client"</a></li>
                        <li><a href=(page("qui-sommes-nous"))>"Qui sommes-nous ?"</a></li>
                    </ul>
                </div>
                <div>
                    <h2>"Contact"</h2>
                    <p>"Service client du lundi au samedi, de 9h à 18h."</p>
                    <p><a href="mailto:contact@chatounette.localhost">"contact@chatounette.localhost"</a></p>
                </div>
            </div>
            <p class="legal">"© 2019 – 2026 Chatounette™ · Boutique de démonstration sur timada"</p>
        </footer>
    })
}

/// A product's card, as the listings show it: the picture, the name, the
/// price (a range across the variants) and the button: a product sold as
/// is goes straight into the cart, one with options sends to its page.
#[topcoat::view::component]
pub async fn product_card(cx: &Cx, product: &Product) -> Result<impl View> {
    let link = href!(p::slug::index, p::slug::Slug(product.slug.clone())).resolve(cx);
    let varies = product.price_range().is_some_and(|range| range.varies());
    Ok(view! {
        <li class="card" data-product=(&product.slug)>
            <a class="card-pic" href=(&link)>
                if let Some(image) = product.image() {
                    product_image(image: image, attrs: attributes! { class="thumb" })
                }
            </a>
            <div class="card-body">
                <h3 class="card-title"><a href=(&link)>(&product.name)</a></h3>
                <p class="card-price">
                    if varies { <small>"Plage de prix"</small> }
                    price(product: product)
                </p>
                if let (Some(rating), Some(whole)) = (product.rating(), product.rating_stars()) {
                    <p class="card-rating" data-rating=(format!("{rating:.1}"))>
                        <span class="stars" aria-hidden="true">(stars(whole))</span>
                        " " (product.review_count) " avis"
                    </p>
                }
                <div class="card-act">
                    if product.options.is_empty() {
                        add_to_cart_form(product: product, quantity: Some(1),
                            <button class="btn btn-ghost">"Ajouter au panier"</button>
                        )
                    } else {
                        <a class="btn btn-ghost" href=(&link)>"Choix des options"</a>
                    }
                </div>
            </div>
        </li>
    })
}

/// A grid of product cards: the catalogue, a category and a search share it.
#[topcoat::view::component]
pub async fn catalog_grid(products: &[Product]) -> Result<impl View> {
    Ok(view! {
        <ul class="catalog">
            for product in products {
                product_card(product: product)
            }
        </ul>
    })
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let home = home_page(cx, &HomeOptions::default()).await?;

    Ok(view! {
        document(
            title: SHOP_NAME,
            <section class="hero">
                <div class="wrap">
                    <div>
                        <span class="eyebrow">(TAGLINE)</span>
                        <h1>"Chatounette, l'endroit parfait pour les accessoires et jouets pour votre chat."</h1>
                        <p>"Votre chat fait partie de la famille : offrez-lui le couchage, les jouets, les accessoires et les petites attentions qu'il mérite, choisis avec amour par des passionnés."</p>
                        <a class="btn" href=(&home.discover_url) data-discover="">"Découvrir"</a>
                        <p class="rating"><span class="stars" aria-hidden="true">"★★★★★"</span> " 4,8/5 d'après nos clients"</p>
                    </div>
                    <div class="hero-art" aria-hidden="true" style="color: var(--coral)">(Unescaped::new_unchecked(PAW_SVG))</div>
                </div>
            </section>
            if !home.newest.is_empty() {
                <section class="block" data-section="nouveautes">
                    <div class="wrap">
                        <h2>"Nouveautés"</h2>
                        <ul class="rail">
                            for product in &home.newest {
                                product_card(product: product)
                            }
                        </ul>
                    </div>
                </section>
            }
            <section class="block band">
                <div class="wrap prose">
                    <h2>"Home sweet home"</h2>
                    <p>"Un panier douillet, une gamelle à sa taille, un griffoir pour sauver le canapé et de quoi jouer quand vous n'êtes pas là : tout ce qu'il faut pour qu'il se sente chez lui."</p>
                    <a class="btn btn-ink" href=(&home.catalog_url)>"Voir la sélection"</a>
                </div>
            </section>
            if !home.tiles.is_empty() {
                <section class="block" data-section="categories">
                    <div class="wrap">
                        <h2>"Catégories"</h2>
                        <ul class="tiles">
                            for tile in &home.tiles {
                                <li>
                                    <a class="tile" data-tile=(&tile.node.category.slug) href=(&tile.url)>
                                        if let Some(image) = &tile.image {
                                            product_image(image: image, attrs: attributes! {})
                                        } else {
                                            <span class="tile-art" style="color: var(--coral)">(Unescaped::new_unchecked(PAW_SVG))</span>
                                        }
                                        <span>(&tile.node.category.name) <small>(tile.node.product_count) " articles"</small></span>
                                    </a>
                                </li>
                            }
                        </ul>
                    </div>
                </section>
            }
            <section class="block band">
                <div class="wrap prose">
                    <h2>"À propos"</h2>
                    <p>"Chatounette est née de deux amoureux des chats qui ne trouvaient pas, en une seule boutique, tout ce dont leurs compagnons avaient besoin. Nous sélectionnons chaque article pour sa qualité, son confort et, avouons-le, sa bouille."</p>
                    <a class="btn btn-ghost" href=(href!(pages::slug::index, pages::slug::Slug("qui-sommes-nous")))>"En savoir plus"</a>
                </div>
            </section>
            if !home.quotes.is_empty() {
                <section class="block" data-section="avis">
                    <div class="wrap">
                        <h2>"Ils nous font confiance"</h2>
                        <ul class="quotes">
                            for review in &home.quotes {
                                <li data-review=(&review.id)>
                                    <span class="stars" aria-label=(format!("{} sur 5", review.rating))>(review.stars())</span>
                                    if let Some(title) = &review.title { <strong>" " (title)</strong> }
                                    <blockquote>(&review.body)</blockquote>
                                    <cite>(&review.author_name) " · Achat vérifié"</cite>
                                </li>
                            }
                        </ul>
                    </div>
                </section>
            }
            if !home.picks.is_empty() {
                <section class="block" data-section="selection">
                    <div class="wrap">
                        <h2>"Rien que pour vous"</h2>
                        <ul class="rail">
                            for product in &home.picks {
                                product_card(product: product)
                            }
                        </ul>
                    </div>
                </section>
            }
            <section class="block">
                <div class="wrap">
                    <div class="promo">
                        <h2>"Profitez de -10%"</h2>
                        <p>"Sur toute la boutique, dès votre première commande : saisissez le code au moment de payer."</p>
                        <code>(PROMO_CODE)</code>
                    </div>
                </div>
            </section>
            <section class="block">
                <div class="wrap">
                    reassurance()
                </div>
            </section>
        )
    })
}

/// The four promises every shop page repeats.
#[topcoat::view::component]
pub async fn reassurance() -> Result<impl View> {
    Ok(view! {
        <ul class="reassure" data-reassure="">
            <li>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M3 7h11v9H3zM14 10h4l3 3v3h-7z"/><circle cx="7" cy="18" r="1.6"/><circle cx="17" cy="18" r="1.6"/></svg>
                <strong>"Expédition rapide"</strong>
                <p>"Commande assurée, livraison avec suivi de colis."</p>
            </li>
            <li>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><rect x="4" y="10" width="16" height="10" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/></svg>
                <strong>"Paiement sécurisé"</strong>
                <p>"Vos paiements en ligne sont entièrement sécurisés."</p>
            </li>
            <li>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M4 12a8 8 0 0 1 16 0v5a2 2 0 0 1-2 2h-2v-6h4M4 12v5a2 2 0 0 0 2 2h2v-6H4"/></svg>
                <strong>"Service client"</strong>
                <p>"À votre écoute du lundi au samedi, de 9h à 18h."</p>
            </li>
            <li>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M4 12a8 8 0 1 0 3-6.2"/><path d="M4 4v5h5"/></svg>
                <strong>"Retours sous 14 jours"</strong>
                <p>"Changez d'avis jusqu'à 14 jours après la livraison."</p>
            </li>
        </ul>
    })
}
