//! The pages: the home at `/`, the whole catalogue at `/boutique`, a category
//! at `/c/{slug}`, a product at `/p/{slug}`, the cart at `/cart`, the checkout
//! at `/checkout`, a placed order at `/orders/{order_id}`, the shopper's
//! account at `/account`, the support page at `/support-client`, the shop's
//! own pages at their own addresses (`/mentions-legales`, …), and the library's `/store/status`. The look is chatounette.com's: one stylesheet
//! (`theme.css`) inlined in every page, no asset bundle.

pub mod account;
pub mod boutique;
pub mod c;
pub mod cart;
pub mod checkout;
pub mod conditions_generales_de_vente;
pub mod emails;
pub mod filters;
pub mod mentions_legales;
pub mod menu;
pub mod newsletter;
pub mod orders;
pub mod own_page;
pub mod p;
pub mod politique_de_confidentialite;
pub mod politique_de_livraison;
pub mod politique_de_remboursement;
pub mod qui_sommes_nous;
pub mod reviews;
pub mod search;
pub mod support_client;

use timada_storefront::{
    CategoryNode, Product, ProductQuery, ProductSort, RouterBuilderStoreExt, Store,
    TIME_ZONE_SCRIPT, menu, product_image, products, recent_reviews,
};
use topcoat::{
    Result,
    context::Cx,
    cookie::{Key, RouterBuilderCookieExt},
    router::{
        BodyLimit, IpNet, Router, RouterBuilderDiscoverExt, TrustedProxies, href, module_router,
        page, request::original_uri,
    },
    view::{Child, Unescaped, View, attributes, view},
};

use crate::shopper::accounts_enabled;
use cart::drawer::{CART_SCRIPT, DRAWER_CSS, cart_drawer};
use menu::{MENU_CSS, MENU_SCRIPT, site_menu};
use search::{SEARCH_CSS, SEARCH_SCRIPT};

/// A review carries up to three photos of up to 8 MiB each.
const REVIEW_LIMIT: usize = 26 * 1024 * 1024;

/// The shop's name, as the pages and the mails say it.
pub const SHOP_NAME: &str = "Chatounette";
/// The code the top bar advertises; the seed creates it.
pub const PROMO_CODE: &str = "MONSTER10";
/// What the shop says about itself, under the logo.
const TAGLINE: &str = "La boutique chat n°1 sur internet";

/// The look, inlined in every page.
const THEME_CSS: &str = include_str!("theme.css");

/// `trusted_proxies`: the proxies in front of the app, whose
/// `X-Forwarded-For` names the shopper; the dashboard limits the sign-in
/// codes each shopper's address may ask for.
pub fn router(store: Store, cookie_key: Key, trusted_proxies: Vec<IpNet>) -> Router {
    module_router!()
        .discover()
        .layer(BodyLimit::max(REVIEW_LIMIT).at("/orders"))
        .trusted_proxies(TrustedProxies::new().networks(trusted_proxies))
        .cookies()
        .app_context(cookie_key)
        .store(store)
        .build()
}

/// `19,90€`, as the shop writes its prices (`12,50 USD` for another
/// currency).
pub fn money(cents: i64, currency: &str) -> String {
    let cents = cents.max(0);
    let whole = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, digit) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push('\u{a0}');
        }
        grouped.push(digit);
    }
    match currency {
        "EUR" => format!("{grouped},{:02}€", cents % 100),
        other => format!("{grouped},{:02}\u{a0}{other}", cents % 100),
    }
}

/// A product's price line: `19,90€`, or `19,90€ – 29,90€` across its variants.
pub fn price_line(product: &Product) -> String {
    match product.price_range() {
        Some(range) if range.varies() => format!(
            "{} – {}",
            money(range.min_cents, &range.currency),
            money(range.max_cents, &range.currency)
        ),
        Some(range) => money(range.max_cents, &range.currency),
        None => String::new(),
    }
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
    #[default] nav: &[CategoryNode],
    #[default] no_drawer: bool,
    /// What search engines show under the title.
    #[default]
    description: Option<&str>,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let cart = cart::current_cart(cx).await?;
    let count = cart
        .as_ref()
        .map(|cart| cart.item_count)
        .unwrap_or_default();
    let total = cart
        .as_ref()
        .map(|cart| {
            money(
                cart.subtotal_cents as i64,
                cart.currency.as_deref().unwrap_or("EUR"),
            )
        })
        .unwrap_or_else(|| money(0, "EUR"));
    let accounts = accounts_enabled(cx);
    let signed_in = accounts && crate::shopper::signed_in(cx).await.is_some();
    let page_title = if title == SHOP_NAME {
        format!("{SHOP_NAME} : {TAGLINE}")
    } else {
        format!("{title} – {SHOP_NAME}")
    };
    Ok(view! {
        <!DOCTYPE html>
        <html lang="fr">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>(page_title)</title>
                if let Some(description) = description {
                    <meta name="description" content=(description)>
                }
                <style>(Unescaped::new_unchecked(THEME_CSS))</style>
                <style>(Unescaped::new_unchecked(DRAWER_CSS))</style>
                <style>(Unescaped::new_unchecked(MENU_CSS))</style>
                <style>(Unescaped::new_unchecked(SEARCH_CSS))</style>
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
                        if !nav.is_empty() {
                            <a class="menu-toggle" href="#menu" aria-controls="menu">"Menu"</a>
                        }
                        <a class="brand" href=(href!(index))>"Chat" <span>"ounette"</span></a>
                        <form class="search" method="get" action=(href!(boutique::index)) role="search"
                            data-search=(href!(search::index)) data-search-min=(search::MIN_CHARS)>
                            <input name="q" type="search" placeholder="Rechercher un produit…" aria-label="Rechercher un produit"
                                autocomplete="off" aria-autocomplete="list" aria-controls="search-drop" aria-expanded="false">
                            <button>"Rechercher"</button>
                            <div id="search-drop" class="search-drop" data-search-drop="" hidden=""></div>
                        </form>
                        <nav class="quick" aria-label="Compte et panier">
                            <a class="quick-support" href=(href!(support_client::index))>"Support client"</a>
                            if accounts {
                                <a class="account-link" href=(href!(account::index)) data-account-link="">
                                    if signed_in { "Mon compte" } else { "Se connecter" }
                                </a>
                            }
                            <a class="cart-link" href=(href!(cart::index)) data-cart-link="" aria-controls="cart">
                                <span class="quick-label">"Panier"</span>
                                <span class="count" data-cart-count="" hidden=(count == 0)>(count)</span>
                                <span class="cart-total" data-cart-total="">(total)</span>
                            </a>
                        </nav>
                    </div>
                </header>
                if !nav.is_empty() {
                    site_menu(nav: nav)
                    <script>(Unescaped::new_unchecked(MENU_SCRIPT))</script>
                }
                <script>(Unescaped::new_unchecked(SEARCH_SCRIPT))</script>
                <script>(Unescaped::new_unchecked(TIME_ZONE_SCRIPT))</script>
                <main>(child)</main>
                site_footer()
                if !no_drawer {
                    cart_drawer(cart: cart.as_ref())
                    <script>(Unescaped::new_unchecked(CART_SCRIPT))</script>
                }
            </body>
        </html>
    })
}

/// The footer: the shop, its legal pages, the useful links and how to
/// reach it, as the live shop lays them out. The pages are the shop's own
/// routes; their text comes from the back office.
#[topcoat::view::component]
pub async fn site_footer() -> Result<impl View> {
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
                        <li><a href=(href!(mentions_legales::index))>"Mentions légales"</a></li>
                        <li><a href=(href!(conditions_generales_de_vente::index))>"Conditions générales de vente"</a></li>
                        <li><a href=(href!(politique_de_confidentialite::index))>"Politique de confidentialité"</a></li>
                        <li><a href=(href!(politique_de_remboursement::index))>"Politique de remboursement"</a></li>
                        <li><a href=(href!(politique_de_livraison::index))>"Politique de livraison"</a></li>
                    </ul>
                </div>
                <div>
                    <h2>"Liens utiles"</h2>
                    <ul>
                        <li><a href=(href!(account::index))>"Suivre ma commande"</a></li>
                        <li><a href=(href!(boutique::index))>"Nos produits"</a></li>
                        <li><a href=(href!(support_client::index))>"Support client"</a></li>
                        <li><a href=(href!(qui_sommes_nous::index))>"Qui sommes-nous ?"</a></li>
                    </ul>
                </div>
                <div>
                    <h2>"Contact"</h2>
                    <p>"Service client du lundi au samedi, de 9h à 18h."</p>
                    <p><a href=(format!("mailto:{}", support_client::CONTACT_EMAIL))>(support_client::CONTACT_EMAIL)</a></p>
                </div>
                <div>
                    <h2>"Newsletter"</h2>
                    <p>"Nouveautés et offres réservées aux abonnés."</p>
                    newsletter::newsletter_form()
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
    let simple = product.options.is_empty();
    // Where the cart drawer opens after the add: this very page.
    let here = original_uri(cx)
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "/".to_owned());
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
                    <span data-price="">(price_line(product))</span>
                </p>
                if let (Some(rating), Some(whole)) = (product.rating(), product.rating_stars()) {
                    <p class="card-rating" data-rating=(format!("{rating:.1}"))>
                        <span class="stars" aria-hidden="true">(timada_storefront::stars(whole))</span>
                        " " (product.review_count) " avis"
                    </p>
                }
                <div class="card-act">
                    if simple {
                        <form method="post" action=(href!(cart::add::add))>
                            <input type="hidden" name="slug" value=(&product.slug)>
                            <input type="hidden" name="quantity" value="1">
                            <input type="hidden" name="from" value=(here)>
                            <button class="btn btn-ghost">"Ajouter au panier"</button>
                        </form>
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

/// How many products the home's rails show.
const NEW_ARRIVALS: u32 = 8;
const PICKS: u32 = 6;
const TILES: usize = 8;
/// How many of a category's newest products are looked at for its tile.
const TILE_CANDIDATES: u32 = 8;
const QUOTES: u32 = 3;

/// A category's tile: its name, its count, and the picture of a product in it.
struct Tile {
    node: CategoryNode,
    image: Option<timada_storefront::Image>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let nav = menu(cx).await?;
    let newest = products(
        cx,
        &ProductQuery::default()
            .sort(ProductSort::Newest)
            .per_page(NEW_ARRIVALS),
    )
    .await?;
    let picks = products(
        cx,
        &ProductQuery::default()
            .sort(ProductSort::TopRated)
            .exclude_all(newest.items.iter().map(|p| p.id.clone()))
            .per_page(PICKS),
    )
    .await?;
    let mut tiles = Vec::with_capacity(TILES);
    for node in nav.iter().take(TILES) {
        // The newest product in it that has a picture lends it.
        let recent = products(
            cx,
            &ProductQuery::default()
                .in_category(node.category.id.clone())
                .sort(ProductSort::Newest)
                .per_page(TILE_CANDIDATES),
        )
        .await?;
        tiles.push(Tile {
            node: node.clone(),
            image: recent
                .items
                .into_iter()
                .find_map(|p| p.images.into_iter().next()),
        });
    }
    let quotes = recent_reviews(cx, QUOTES).await?;
    let discover = nav
        .first()
        .map(|node| href!(c::slug::index, c::slug::Slug(node.category.slug.clone())).resolve(cx))
        .unwrap_or_else(|| href!(boutique::index).resolve(cx));

    Ok(view! {
        document(
            title: SHOP_NAME,
            nav: &nav,
            <section class="hero">
                <div class="wrap">
                    <div>
                        <span class="eyebrow">(TAGLINE)</span>
                        <h1>"Chatounette, l'endroit parfait pour les accessoires et jouets pour votre chat."</h1>
                        <p>"Votre chat fait partie de la famille : offrez-lui le couchage, les jouets, les accessoires et les petites attentions qu'il mérite, choisis avec amour par des passionnés."</p>
                        <a class="btn" href=(discover) data-discover="">"Découvrir"</a>
                        <p class="rating"><span class="stars" aria-hidden="true">"★★★★★"</span> " 4,8/5 d'après nos clients"</p>
                    </div>
                    <div class="hero-art" aria-hidden="true" style="color: var(--coral)">(Unescaped::new_unchecked(PAW_SVG))</div>
                </div>
            </section>
            if !newest.items.is_empty() {
                <section class="block" data-section="nouveautes">
                    <div class="wrap">
                        <h2>"Nouveautés"</h2>
                        <ul class="rail">
                            for product in &newest.items {
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
                    <a class="btn btn-ink" href=(href!(boutique::index))>"Voir la sélection"</a>
                </div>
            </section>
            if !tiles.is_empty() {
                <section class="block" data-section="categories">
                    <div class="wrap">
                        <h2>"Catégories"</h2>
                        <ul class="tiles">
                            for tile in &tiles {
                                <li>
                                    <a class="tile" data-tile=(&tile.node.category.slug)
                                        href=(href!(c::slug::index, c::slug::Slug(tile.node.category.slug.clone())))>
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
                    <a class="btn btn-ghost" href=(href!(qui_sommes_nous::index))>"En savoir plus"</a>
                </div>
            </section>
            if !quotes.is_empty() {
                <section class="block" data-section="avis">
                    <div class="wrap">
                        <h2>"Ils nous font confiance"</h2>
                        <ul class="quotes">
                            for review in &quotes {
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
            if !picks.items.is_empty() {
                <section class="block" data-section="selection">
                    <div class="wrap">
                        <h2>"Rien que pour vous"</h2>
                        <ul class="rail">
                            for product in &picks.items {
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

/// How a refusal (an [`OrdersError`](timada_storefront::OrdersError) code
/// carried in the URL) reads to a shopper.
pub fn say(code: &str) -> &'static str {
    match code {
        "invalid_quantity" => "Choisissez une quantité de 1 à 99.",
        "too_many_lines" => "Votre panier est plein : commandez d'abord ce qu'il contient.",
        "product_unavailable" => "Désolé, ce produit n'est plus en vente.",
        "out_of_stock" => "Désolé, il n'en reste pas assez.",
        "variant_required" => "Choisissez d'abord une option.",
        "variant_not_found" => "Cette combinaison n'est pas en vente.",
        "mixed_currencies" => "Ce produit est vendu dans une autre devise que votre panier.",
        "empty_cart" => "Votre panier est vide.",
        "price_changed" => {
            "Un prix a changé depuis votre visite : vérifiez le nouveau total et confirmez à nouveau."
        }
        "payments_unavailable" => {
            "Le paiement est indisponible pour le moment. Réessayez plus tard."
        }
        "invoicing_not_configured" => {
            "Cette boutique ne peut pas encore prendre de commandes : sa configuration n'est pas terminée. Réessayez plus tard."
        }
        "payment_failed" => "Le paiement n'a pas abouti. Réessayez ou utilisez une autre carte.",
        "payment_expired" => "Ce paiement n'est plus ouvert. Veuillez recommencer votre commande.",
        "invalid_email" => "Saisissez une adresse e-mail valide.",
        "invalid_vat_number" => {
            "Saisissez un numéro de TVA intracommunautaire : le préfixe du pays, puis lettres et chiffres."
        }
        "shipping_unavailable" => "Choisissez un mode de livraison qui dessert votre pays.",
        "business_buyers_refused" => {
            "Cette boutique ne vend pas aux professionnels : laissez le numéro de TVA vide."
        }
        "invalid_ship_name" => "Saisissez le nom du destinataire.",
        "invalid_address" => "Saisissez la rue, le code postal et la ville.",
        "invalid_country" => "Saisissez le pays sous forme de code à deux lettres, par exemple FR.",
        "invalid_phone" => "Le numéro de téléphone est trop long.",
        "invalid_note" => "La remarque est trop longue.",
        "cart_not_found" | "cart_checked_out" => "Ce panier a déjà été commandé.",
        "discount_unknown" => "Ce code promo n'est pas valable.",
        "discount_inactive" => "Ce code promo ne peut pas être utilisé pour le moment.",
        "discount_currency" => "Ce code promo concerne une autre devise.",
        "discount_exhausted" => "Ce code promo a été utilisé autant de fois que possible.",
        "discount_already_used" => "Vous avez déjà utilisé ce code promo.",
        "offer_ended" => {
            "Cette offre n'est plus disponible : l'article a été ajouté à son prix habituel."
        }
        "review_invalid_rating" => "Choisissez de une à cinq étoiles.",
        "review_invalid_title" => "Le titre fait au plus 100 caractères.",
        "review_invalid_body" => "Dites-nous en un peu plus : de 10 à 2000 caractères.",
        "review_too_many_photos" => "Joignez au plus trois photos.",
        "review_invalid_photo" => "Joignez des photos JPEG, PNG, WebP ou GIF de 8 Mio au plus.",
        "review_order_not_found" => "Cette commande ne peut pas être évaluée.",
        "review_not_purchased" => "Ce produit ne fait pas partie de cette commande.",
        "review_order_cancelled" => "Une commande annulée ne peut pas être évaluée.",
        "review_already_reviewed" => "Vous avez déjà donné votre avis sur ce produit.",
        "review_not_pending" => {
            "La boutique a déjà examiné votre avis : il ne peut plus être modifié."
        }
        "review_not_found" => "Cet avis est introuvable.",
        "review_photo_not_found" => "L'une de vos photos a été retirée entre-temps : réessayez.",
        "wrong_code" => {
            "Ce n'est pas le code que nous avons envoyé. Vérifiez l'e-mail et réessayez."
        }
        "code_expired" => "Ce code a expiré : demandez-en un nouveau.",
        "too_many_attempts" => "Trop de codes erronés : demandez-en un nouveau.",
        "code_recently_sent" => {
            "Un code a été envoyé il y a moins d'une minute : vérifiez votre boîte de réception."
        }
        "too_many_requests" => "Trop de codes demandés : réessayez dans quelques minutes.",
        "too_many_addresses" => "Votre compte conserve dix adresses : retirez-en une d'abord.",
        "signed_out" => "Vous avez été déconnecté. Veuillez vous reconnecter.",
        "writes_paused" => {
            "Cette boutique n'accepte temporairement aucune modification. Réessayez dans quelques minutes."
        }
        _ => "Une erreur s'est produite. Veuillez réessayer.",
    }
}
