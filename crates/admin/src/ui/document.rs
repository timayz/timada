use topcoat::{
    Result,
    context::{Cx, app_context},
    router::href,
    view::{Child, View, component, view},
};

use crate::{
    app::admin::_secure::{
        categories, customers, disputes, emails, families, inventory, invoices, orders, products,
        promotions, questions, refunds, returns, reviews, vat,
    },
    auth::{Section, signed_in_admin},
    config::{AdminConfig, Stylesheet},
};

/// The `<html>` shell: stylesheet, header with navigation, and the page body.
#[component]
pub async fn shell(cx: &Cx, child: Child<'_>) -> Result<impl View> {
    let config = app_context::<AdminConfig>(cx);
    let admin = signed_in_admin(cx);
    // Every section, in the order shown: `(section, link, current, label)` —
    // an operator's navigation holds those their role opens.
    macro_rules! entry {
        ($section:ident, $page:path, $label:literal) => {{
            let link = href!($page);
            (
                Section::$section,
                link.resolve(cx),
                link.is_current(cx),
                $label,
            )
        }};
    }
    let sections = [
        entry!(Orders, orders::index, "Commandes"),
        entry!(Products, products::index, "Produits"),
        entry!(Categories, categories::index, "Catégories"),
        entry!(Families, families::index, "Familles"),
        entry!(Inventory, inventory::index, "Stock"),
        entry!(Customers, customers::index, "Clients"),
        entry!(Promotions, promotions::index, "Promotions"),
        entry!(Invoices, invoices::index, "Factures"),
        entry!(Returns, returns::index, "Retours"),
        entry!(Refunds, refunds::index, "Remboursements"),
        entry!(Disputes, disputes::index, "Litiges"),
        entry!(Vat, vat::index, "TVA"),
        entry!(Reviews, reviews::index, "Avis"),
        entry!(Questions, questions::index, "Questions"),
        entry!(Emails, emails::index, "E-mails"),
    ];
    let navigation: Vec<(String, bool, &'static str)> = sections
        .into_iter()
        .filter(|(section, ..)| admin.is_some_and(|admin| admin.role.opens(*section)))
        .map(|(_, link, current, label)| (link, current, label))
        .collect();
    // The name of the shop leads to where the operator works.
    let home = navigation
        .first()
        .map(|(link, ..)| link.clone())
        .unwrap_or_else(|| href!(orders::index).resolve(cx));
    let standing = admin.map(|admin| format!("{} · {}", admin.email, admin.role.label()));

    Ok(view! {
        <!DOCTYPE html>
        <html lang="fr" class="h-full bg-background text-foreground">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"Timada admin"</title>
                match &config.stylesheet {
                    Stylesheet::Bundled => <link rel="stylesheet" href=(topcoat::tailwind::stylesheet!())>,
                    Stylesheet::Url(url) => <link rel="stylesheet" href=(url)>,
                }
            </head>
            <body class="min-h-full flex flex-col">
                <header class="border-b border-border bg-background print:hidden">
                    <div class="mx-auto flex max-w-6xl items-center gap-6 px-4 py-3">
                        <a href=(home) class="font-semibold tracking-tight">"Timada admin"</a>
                        if admin.is_some() {
                            <nav aria-label="Sections" class="flex gap-1 text-sm">
                                for (link, current, label) in &navigation {
                                    nav_link(link: link.clone(), current: *current, (*label))
                                }
                            </nav>
                            <form method="post" action=(href!(crate::app::admin::logout)) class="ml-auto flex items-center gap-3 text-sm text-muted-foreground">
                                if let Some(standing) = &standing { <span>(standing.clone())</span> }
                                <button type="submit" class="underline-offset-4 hover:underline">"Se déconnecter"</button>
                            </form>
                        }
                    </div>
                </header>
                <main class="mx-auto w-full max-w-6xl flex-1 px-4 py-6">
                    (child)
                </main>
            </body>
        </html>
    })
}

#[component]
async fn nav_link(link: String, current: bool, child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <a
            href=(link)
            aria-current=(current.then_some("page"))
            class=(if current {
                "rounded-md bg-foreground/5 px-3 py-1.5 font-medium"
            } else {
                "rounded-md px-3 py-1.5 text-muted-foreground hover:bg-foreground/5 hover:text-foreground"
            })
        >
            (child)
        </a>
    })
}

#[component]
pub async fn page_header(title: &str, #[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <div class="mb-6 flex flex-wrap items-center justify-between gap-4">
            <h1 class="text-2xl font-semibold tracking-tight">(title)</h1>
            <div class="flex items-center gap-2">(child)</div>
        </div>
    })
}

#[component]
pub async fn empty_state(message: &str) -> Result<impl View> {
    Ok(view! {
        <p class="rounded-lg border border-dashed border-border p-8 text-center text-sm text-muted-foreground">(message)</p>
    })
}

/// Previous/next links driven by `?page=`; keeps the other query parameters.
#[component]
pub async fn pagination(cx: &Cx, page: u32, page_size: u32, total: u64) -> Result<impl View> {
    let pages = (total.div_ceil(u64::from(page_size)).max(1)) as u32;
    let query = topcoat::router::request::uri(cx)
        .query()
        .unwrap_or("")
        .to_owned();
    let with_page = move |n: u32| {
        let mut parts: Vec<String> = query
            .split('&')
            .filter(|p| !p.is_empty() && !p.starts_with("page="))
            .map(str::to_owned)
            .collect();
        parts.push(format!("page={n}"));
        format!("?{}", parts.join("&"))
    };
    Ok(view! {
        <nav aria-label="Pagination" class="mt-4 flex items-center justify-between text-sm text-muted-foreground">
            <span>"Page " (page.to_string()) " / " (pages.to_string()) " · " (total.to_string()) " au total"</span>
            <span class="flex gap-3">
                if page > 1 { <a href=(with_page(page - 1)) class="underline-offset-4 hover:underline">"Précédente"</a> }
                if page < pages { <a href=(with_page(page + 1)) class="underline-offset-4 hover:underline">"Suivante"</a> }
            </span>
        </nav>
    })
}
