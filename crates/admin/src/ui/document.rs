use topcoat::{
    Result,
    context::{Cx, app_context},
    router::href,
    view::{Child, View, component, view},
};

use crate::{
    app::admin::_secure::{
        customers, emails, inventory, invoices, orders, products, promotions, questions, refunds,
        returns, reviews,
    },
    auth::signed_in_admin,
    config::{AdminConfig, Stylesheet},
};

/// The `<html>` shell: stylesheet, header with navigation, and the page body.
#[component]
pub async fn shell(cx: &Cx, child: Child<'_>) -> Result<impl View> {
    let config = app_context::<AdminConfig>(cx);
    let admin = signed_in_admin(cx);
    let orders_link = href!(orders::index);
    let products_link = href!(products::index);
    let inventory_link = href!(inventory::index);
    let customers_link = href!(customers::index);
    let promotions_link = href!(promotions::index);
    let invoices_link = href!(invoices::index);
    let refunds_link = href!(refunds::index);
    let returns_link = href!(returns::index);
    let reviews_link = href!(reviews::index);
    let questions_link = href!(questions::index);
    let emails_link = href!(emails::index);

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
                        <a href=(href!(orders::index)) class="font-semibold tracking-tight">"Timada admin"</a>
                        if admin.is_some() {
                            <nav aria-label="Sections" class="flex gap-1 text-sm">
                                nav_link(link: orders_link.resolve(cx), current: orders_link.is_current(cx), "Commandes")
                                nav_link(link: products_link.resolve(cx), current: products_link.is_current(cx), "Produits")
                                nav_link(link: inventory_link.resolve(cx), current: inventory_link.is_current(cx), "Stock")
                                nav_link(link: customers_link.resolve(cx), current: customers_link.is_current(cx), "Clients")
                                nav_link(link: promotions_link.resolve(cx), current: promotions_link.is_current(cx), "Promotions")
                                nav_link(link: invoices_link.resolve(cx), current: invoices_link.is_current(cx), "Factures")
                                nav_link(link: returns_link.resolve(cx), current: returns_link.is_current(cx), "Retours")
                                nav_link(link: refunds_link.resolve(cx), current: refunds_link.is_current(cx), "Remboursements")
                                nav_link(link: reviews_link.resolve(cx), current: reviews_link.is_current(cx), "Avis")
                                nav_link(link: questions_link.resolve(cx), current: questions_link.is_current(cx), "Questions")
                                nav_link(link: emails_link.resolve(cx), current: emails_link.is_current(cx), "E-mails")
                            </nav>
                            <form method="post" action=(href!(crate::app::admin::logout)) class="ml-auto flex items-center gap-3 text-sm text-muted-foreground">
                                if let Some(admin) = admin { <span>(admin.email.clone())</span> }
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
