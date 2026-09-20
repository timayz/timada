//! Storefront pages. Explicit paths on purpose: the host must not use
//! `module_router!()` while the admin's module-derived pages are linked in.

pub mod account;
pub mod cart;
pub mod catalog;
pub mod category;
pub mod checkout;
mod format;
pub mod invoice;
pub mod returns;
#[cfg(feature = "stripe")]
pub mod webhooks;

use topcoat::{
    Result,
    context::Cx,
    router::href,
    view::{Child, View, component, view},
};

use crate::{auth::current_account, cart_session::current_cart};

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
    child: Child<'_>,
) -> Result<impl View> {
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
                <style>(STYLES)</style>
            </head>
            <body>
                <header>
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
