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

const STYLES: &str = "\
body{font-family:system-ui,sans-serif;max-width:60rem;margin:2rem auto;padding:0 1rem;line-height:1.5;color:#1a1a1a}\
header{display:flex;flex-wrap:wrap;gap:.5rem 1.25rem;align-items:baseline;border-bottom:1px solid #ddd;padding-bottom:.5rem}\
header nav{display:flex;flex-wrap:wrap;gap:1rem;margin-left:auto;align-items:baseline}\
a{color:#0b5fa5}ul{padding-left:1.2rem}.price{font-size:1.5rem;font-weight:600}.muted{color:#595959}\
table{border-collapse:collapse;width:100%}th,td{text-align:left;padding:.5rem;border-bottom:1px solid #e5e5e5;vertical-align:top}\
td.num,th.num{text-align:right;white-space:nowrap}\
form.inline{display:inline}form.stack{display:grid;gap:.75rem;max-width:28rem}\
label{display:grid;gap:.25rem;font-weight:500}label.choice{display:flex;gap:.5rem;align-items:baseline;font-weight:400}\
input,select,button{font:inherit;padding:.45rem .6rem;border:1px solid #767676;border-radius:.3rem}\
input[type=number]{width:5rem}input[type=radio]{padding:0}\
button{background:#0b5fa5;border-color:#0b5fa5;color:#fff;cursor:pointer}\
button.link{background:none;border:none;color:#0b5fa5;padding:0;text-decoration:underline}\
:focus-visible{outline:3px solid #ffbf47;outline-offset:2px}\
fieldset{border:1px solid #ddd;border-radius:.3rem;margin:0 0 1rem;padding:.75rem 1rem}legend{font-weight:600;padding:0 .25rem}\
.error{color:#b00020;font-weight:500}.notice{background:#eef6ee;border:1px solid #b7d8b7;padding:.5rem .75rem;border-radius:.3rem}\
nav.crumbs ol{display:flex;flex-wrap:wrap;gap:.25rem .5rem;list-style:none;padding:0;margin:0 0 1rem;color:#595959}\
nav.crumbs li+li::before{content:'\\203A';margin-right:.5rem}\
ul.tags{display:flex;flex-wrap:wrap;gap:.5rem;list-style:none;padding:0}ul.tags a{display:inline-block;border:1px solid #767676;border-radius:1rem;padding:.2rem .75rem;text-decoration:none}\
.listing{display:grid;gap:1.5rem;grid-template-columns:minmax(0,1fr)}\
@media(min-width:48rem){.listing{grid-template-columns:15rem minmax(0,1fr)}}\
form.filters{display:grid;gap:.75rem;align-content:start}form.filters fieldset{display:grid;gap:.35rem;border:1px solid #ddd;border-radius:.3rem}\
form.filters input[type=number],form.filters input[type=search],form.filters select{width:100%;box-sizing:border-box}\
ul.products{display:grid;gap:1rem;grid-template-columns:repeat(auto-fill,minmax(12rem,1fr));list-style:none;padding:0;margin:0}\
li.product{border:1px solid #ddd;border-radius:.3rem;padding:.75rem;display:grid;gap:.25rem;align-content:start}\
li.product h2{font-size:1rem;margin:0}li.product p{margin:0}li.product .price{font-size:1.15rem}\
li.product .thumb{display:block;aspect-ratio:1;background:#f4f4f4;border-radius:.2rem;overflow:hidden}\
li.product img{width:100%;height:100%;object-fit:contain;display:block}\
li.product .no-image{display:grid;place-items:center;height:100%;color:#595959;font-size:.85rem}\
table.sheet th[scope=colgroup]{background:#f4f4f4}table.sheet th[scope=row]{font-weight:400;color:#595959;width:40%}\
.in-stock{color:#176b2c;font-weight:500}\
nav.pager{display:flex;gap:1rem;align-items:baseline;margin-top:1rem}\
.cards{display:grid;gap:1rem;grid-template-columns:repeat(auto-fit,minmax(16rem,1fr))}\
.card{border:1px solid #ddd;border-radius:.3rem;padding:.75rem 1rem}.card address{font-style:normal}\
.totals{margin-left:auto;max-width:22rem}.totals td{border:none;padding:.2rem .5rem}.totals tr.total td{font-weight:700;border-top:1px solid #ccc}";

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
        Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
    };
    let cart_count: u32 = match current_cart(cx).await {
        Ok(cart) => cart
            .as_ref()
            .map_or(0, |c| c.lines.iter().map(|l| l.quantity).sum()),
        Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
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
                <style>(STYLES)</style>
            </head>
            <body>
                <header>
                    <a href=(href!(catalog::home))><strong>"Timada demo"</strong></a>
                    <form method="get" action=(href!(catalog::search)) role="search" class="inline">
                        <input type="search" name="q" aria-label="Rechercher un produit" placeholder="Rechercher…" size="18">
                        <button type="submit">"Chercher"</button>
                    </form>
                    if !currencies.is_empty() {
                        <form method="post" action=(href!(currency::switch)) class="inline">
                            <input type="hidden" name="next" value=(here.clone())>
                            <label for="currency" class="muted">"Devise"</label>
                            " "
                            <select id="currency" name="currency">
                                for (code, written, current) in &currencies {
                                    <option value=(code.clone()) selected=(*current)>(written.clone())</option>
                                }
                            </select>
                            " "
                            <button type="submit">"Changer"</button>
                        </form>
                    }
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
                        <a href="/admin" class="muted">"Administration"</a>
                    </nav>
                </header>
                <main>(child)</main>
            </body>
        </html>
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
