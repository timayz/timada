use topcoat::{
    Result,
    context::{Cx, app_context},
    router::href,
    view::{Child, View, attributes, component, view},
};

use crate::{
    app::admin::_secure::{
        categories, customers, disputes, emails, families, inventory, invoices, journal, orders,
        password, products, promotions, questions, refunds, returns, reviews, team, vat,
    },
    auth::{Role, Section, signed_in_admin},
    config::{AdminConfig, Stylesheet},
    ui::{icon, icons},
};

/// The `<html>` shell: stylesheet, header with navigation, and the page body.
#[component]
pub async fn shell(cx: &Cx, child: Child<'_>) -> Result<impl View> {
    let config = app_context::<AdminConfig>(cx);
    let admin = signed_in_admin(cx);
    // Every section, in the order shown: `(section, link, current, label)` —
    // an operator's navigation holds those their role opens.
    macro_rules! entry {
        ($section:ident, $page:path) => {{
            let link = href!($page);
            (
                Section::$section,
                link.resolve(cx),
                link.is_current(cx),
                Section::$section.label(),
            )
        }};
    }
    let sections = [
        entry!(Orders, orders::index),
        entry!(Products, products::index),
        entry!(Categories, categories::index),
        entry!(Families, families::index),
        entry!(Inventory, inventory::index),
        entry!(Customers, customers::index),
        entry!(Promotions, promotions::index),
        entry!(Invoices, invoices::index),
        entry!(Returns, returns::index),
        entry!(Refunds, refunds::index),
        entry!(Disputes, disputes::index),
        entry!(Vat, vat::index),
        entry!(Reviews, reviews::index),
        entry!(Questions, questions::index),
        entry!(Emails, emails::index),
    ];
    let mut navigation: Vec<(String, bool, &'static str)> = sections
        .into_iter()
        .filter(|(section, ..)| admin.is_some_and(|admin| admin.role.opens(*section)))
        .map(|(_, link, current, label)| (link, current, label))
        .collect();
    // No role's section: the owners' own.
    if admin.is_some_and(|admin| admin.role == Role::Owner) {
        let team_link = href!(team::index);
        navigation.push((team_link.resolve(cx), team_link.is_current(cx), "Équipe"));
        let journal_link = href!(journal::index);
        navigation.push((
            journal_link.resolve(cx),
            journal_link.is_current(cx),
            "Journal",
        ));
    }
    let own_password = href!(password::index).resolve(cx);
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
                                <a href=(own_password) class="underline-offset-4 hover:underline">"Mon mot de passe"</a>
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

/// What a list says when it has nothing to list.
///
/// The icon is decorative — the sentence already says what is missing — so it
/// carries no label and stays out of the reading order.
#[component]
pub async fn empty_state(message: &str) -> Result<impl View> {
    Ok(view! {
        <div class="flex flex-col items-center gap-3 rounded-xl border border-dashed border-border p-10 text-center">
            icon(data: icons::INBOX, attrs: attributes! { class="size-6 text-muted-foreground" })
            <p class="text-sm text-muted-foreground">(message)</p>
        </div>
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
                if page > 1 {
                    <a href=(with_page(page - 1)) class="inline-flex items-center gap-1 underline-offset-4 hover:underline">
                        icon(data: icons::CHEVRON_LEFT, attrs: attributes! { class="size-4" })
                        "Précédente"
                    </a>
                }
                if page < pages {
                    <a href=(with_page(page + 1)) class="inline-flex items-center gap-1 underline-offset-4 hover:underline">
                        "Suivante"
                        icon(data: icons::CHEVRON_RIGHT, attrs: attributes! { class="size-4" })
                    </a>
                }
            </span>
        </nav>
    })
}
