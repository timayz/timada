use topcoat::{
    Result,
    context::{Cx, app_context, try_app_context},
    icon::IconData,
    router::href,
    view::{Child, View, attributes, component, view},
};

use crate::{
    app::admin::_secure::{
        categories, customers, disputes, emails, families, inventory, invoices, journal, orders,
        password, products, promotions, questions, refunds, returns, reviews, team, vat,
    },
    auth::{Group, Role, Section, signed_in_admin},
    config::{AdminConfig, Stylesheet},
    ui::{
        icon, icons,
        theme::{self, Scheme},
    },
};

/// One entry of the navigation: where it goes, whether it is the page being
/// shown, what it is called, and what it is drawn as.
struct Entry {
    link: String,
    current: bool,
    label: &'static str,
    glyph: IconData,
}

/// A heading of the navigation, and the entries under it. A heading with no
/// entries is not rendered, which is why the grouping mirrors `Role::opens`.
struct Heading {
    label: &'static str,
    entries: Vec<Entry>,
}

/// The page the whole back office is drawn in: the rail, the bar above the
/// work, and the work.
#[component]
pub async fn shell(cx: &Cx, child: Child<'_>) -> Result<impl View> {
    let admin = signed_in_admin(cx);
    macro_rules! entry {
        ($section:ident, $page:path) => {{
            let link = href!($page);
            (
                Section::$section,
                Entry {
                    link: link.resolve(cx),
                    current: link.is_current(cx),
                    label: Section::$section.label(),
                    glyph: section_glyph(Section::$section),
                },
            )
        }};
    }
    // Every section's entry, by section, so the headings can pick theirs out.
    let mut entries: Vec<(Section, Entry)> = vec![
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
    entries.retain(|(section, _)| admin.is_some_and(|admin| admin.role.opens(*section)));

    let owns_the_shop = admin.is_some_and(|admin| admin.role == Role::Owner);
    let mut headings: Vec<Heading> = Vec::new();
    for group in Group::ALL {
        let entries: Vec<Entry> = match group {
            // No role's sections: the owner's own, so they are not in the
            // table `Role::opens` reads and are gathered here instead.
            Group::Administration if owns_the_shop => {
                let team = href!(team::index);
                let records = href!(journal::index);
                vec![
                    Entry {
                        link: team.resolve(cx),
                        current: team.is_current(cx),
                        label: "Équipe",
                        glyph: icons::USER_COG,
                    },
                    Entry {
                        link: records.resolve(cx),
                        current: records.is_current(cx),
                        label: "Journal",
                        glyph: icons::HISTORY,
                    },
                ]
            }
            Group::Administration => Vec::new(),
            // Each section belongs to exactly one heading, so the entries are
            // partitioned out of the pool rather than copied from it.
            group => {
                let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut entries)
                    .into_iter()
                    .partition(|(section, _)| section.group() == group);
                entries = rest;
                mine.into_iter().map(|(_, entry)| entry).collect()
            }
        };
        if !entries.is_empty() {
            headings.push(Heading {
                label: group.label(),
                entries,
            });
        }
    }

    let own_password = href!(password::index).resolve(cx);
    // The name of the shop leads to where the operator works. Derived from the
    // first entry their role opens, which is what keeps a catalogue operator's
    // pages free of any `href` to a section they cannot reach.
    let home = headings
        .first()
        .and_then(|heading| heading.entries.first())
        .map(|entry| entry.link.clone())
        .unwrap_or_else(|| href!(orders::index).resolve(cx));
    let standing = admin.map(|admin| format!("{} · {}", admin.email, admin.role.label()));
    // The bar above the work names the page, so the rail is not the only thing
    // saying where the operator is.
    let here = headings
        .iter()
        .flat_map(|heading| heading.entries.iter())
        .find(|entry| entry.current)
        .map(|entry| entry.label);

    Ok(view! {
        document(
            title: "Timada admin",
            // First stop for a keyboard, and on a phone the way out of the
            // open drawer.
            <a
                href="#contenu"
                class="sr-only focus:not-sr-only focus:absolute focus:top-4 focus:left-4 focus:z-50 focus:rounded-md focus:bg-card focus:px-3 focus:py-2 focus:text-sm focus:shadow-sm"
            >
                "Aller au contenu"
            </a>
            <div class="flex min-h-dvh">
                if admin.is_some() {
                    sidebar(
                        headings: headings,
                        home: home.clone(),
                        own_password: own_password,
                        standing: standing.clone().unwrap_or_default(),
                    )
                }
                <div class="flex min-w-0 flex-1 flex-col">
                    <header class="sticky top-0 z-20 flex h-14 shrink-0 items-center gap-3 border-b border-border bg-background/95 px-4 backdrop-blur supports-backdrop-filter:bg-background/60 md:px-6 print:hidden">
                        if admin.is_some() {
                            <a
                                href="#menu"
                                aria-label="Ouvrir le menu"
                                class="inline-flex size-9 items-center justify-center rounded-md border border-border text-muted-foreground hover:bg-accent hover:text-accent-foreground md:hidden"
                            >
                                icon(data: icons::MENU, attrs: attributes! { class="size-4" })
                            </a>
                        } else {
                            <a href=(home) class="font-semibold tracking-tight">"Timada admin"</a>
                        }
                        if let Some(here) = here { <p class="truncate text-sm font-medium">(here)</p> }
                        <div class="ml-auto flex items-center gap-2">
                            scheme_switch()
                        </div>
                    </header>
                    <main
                        id="contenu"
                        class="mx-auto w-full max-w-7xl flex-1 px-4 py-6 md:px-6 print:max-w-none print:px-0"
                    >
                        (child)
                    </main>
                </div>
            </div>
        )
    })
}

/// The rail: the shop's name, the navigation, and who is signed in.
///
/// One copy of every link, in one element that is a sticky column from `md` up
/// and a full-screen panel below it. The panel is opened by a fragment and
/// closed by navigating away from one, so it needs no script and no state: a
/// nav link is an ordinary navigation, and the page it lands on has no
/// fragment, so the drawer is shut when it arrives.
///
/// Closed, the rail is `display:none`, which takes its links out of the tab
/// order entirely. A drawer slid off-screen with a transform keeps them
/// focusable, and a keyboard then walks into a panel nobody can see.
///
/// The rail keeps its own dark skin under either colour scheme — `--sidebar-*`
/// rather than `--card` and `--border` — so the one fixed landmark of the back
/// office looks the same wherever the operator has the lights.
///
/// Folding it down to its icons would need a second cookie and a round trip per
/// toggle, because the labels have to stop being rendered and that is a branch,
/// not a class. The work is centred anyway, so the reclaimed width would change
/// no layout; it is not worth the state.
#[component]
async fn sidebar(
    headings: Vec<Heading>,
    home: String,
    own_password: String,
    standing: String,
) -> Result<impl View> {
    Ok(view! {
        <aside
            id="menu"
            class="z-40 flex w-full flex-col gap-4 overflow-y-auto border-sidebar-border bg-sidebar px-3 py-4 text-sidebar-foreground max-md:not-target:hidden max-md:target:fixed max-md:target:inset-0 md:sticky md:top-0 md:h-dvh md:w-64 md:shrink-0 md:border-r print:hidden"
        >
            <div class="flex items-center gap-2 px-2">
                <a href=(home) class="font-semibold tracking-tight">"Timada admin"</a>
                <a
                    href="#contenu"
                    aria-label="Fermer le menu"
                    class="ml-auto inline-flex size-9 items-center justify-center rounded-lg text-sidebar-icon hover:bg-sidebar-hover md:hidden"
                >
                    icon(data: icons::X, attrs: attributes! { class="size-4" })
                </a>
            </div>
            <nav aria-label="Sections" class="flex flex-col gap-4">
                for heading in &headings {
                    <div class="flex flex-col gap-1">
                        <p class="px-2 text-xs font-medium tracking-wider text-sidebar-foreground-muted uppercase">
                            (heading.label)
                        </p>
                        for entry in &heading.entries {
                            nav_link(
                                link: entry.link.clone(),
                                current: entry.current,
                                glyph: entry.glyph.clone(),
                                (entry.label)
                            )
                        }
                    </div>
                }
            </nav>
            <div class="mt-auto flex flex-col gap-2 border-t border-sidebar-border px-2 pt-4 text-sm">
                if !standing.is_empty() {
                    <p class="text-sidebar-foreground-muted">(standing)</p>
                }
                <a href=(own_password) class="underline-offset-4 hover:underline">"Mon mot de passe"</a>
                <form method="post" action=(href!(crate::app::admin::logout))>
                    <button
                        type="submit"
                        class="inline-flex items-center gap-2 underline-offset-4 hover:underline"
                    >
                        icon(data: icons::LOG_OUT, attrs: attributes! { class="size-4" })
                        "Se déconnecter"
                    </button>
                </form>
            </div>
        </aside>
    })
}

/// What each section is drawn as in the rail.
///
/// Presentation, so it lives here rather than beside `Section`: the permission
/// table has no opinion about glyphs.
const fn section_glyph(section: Section) -> IconData {
    match section {
        Section::Orders => icons::SHOPPING_CART,
        Section::Products => icons::PACKAGE,
        Section::Categories => icons::FOLDER_TREE,
        Section::Families => icons::LAYERS,
        Section::Inventory => icons::WAREHOUSE,
        Section::Customers => icons::USERS,
        Section::Promotions => icons::TICKET_PERCENT,
        Section::Invoices => icons::FILE_TEXT,
        Section::Returns => icons::PACKAGE_OPEN,
        Section::Refunds => icons::BANKNOTE,
        Section::Disputes => icons::SHIELD_ALERT,
        Section::Vat => icons::PERCENT,
        Section::Reviews => icons::STAR,
        Section::Questions => icons::MESSAGE_CIRCLE,
        Section::Emails => icons::MAIL,
    }
}

/// The `<html>` frame: the colour scheme, the stylesheet, and nothing else.
///
/// Separate from [`shell`] because the refusal page is rendered from a layer,
/// where no page has run and there is no navigation to draw, and it still has
/// to be the same document — same scheme, same declared `color-scheme`, same
/// stylesheet. Two hand-written heads is how one of them ends up light while
/// the operator asked for dark.
#[component]
pub async fn document(cx: &Cx, title: &str, #[default] child: Child<'_>) -> Result<impl View> {
    let scheme = theme::chosen(cx);
    Ok(view! {
        <!DOCTYPE html>
        <html lang="fr" class=(class_list("h-full bg-background text-foreground", scheme.html_class()))>
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                // Declared, not only styled: the browser paints the canvas and
                // draws native widgets from this before any CSS arrives.
                <meta name="color-scheme" content=(scheme.color_scheme())>
                <title>(title)</title>
                <link rel="stylesheet" href=(stylesheet_url(cx))>
            </head>
            <body class="min-h-full flex flex-col">
                (child)
            </body>
        </html>
    })
}

/// Where the admin stylesheet is served from.
///
/// Outside a view an asset is resolved by hand; without a bundle the page goes
/// unstyled rather than not at all.
#[must_use]
pub fn stylesheet_url(cx: &Cx) -> String {
    match &app_context::<AdminConfig>(cx).stylesheet {
        Stylesheet::Bundled => try_app_context::<topcoat::asset::AssetConfig>(cx)
            .map(|assets| assets.resolve(topcoat::tailwind::stylesheet!()))
            .unwrap_or_default(),
        Stylesheet::Url(url) => url.clone(),
    }
}

fn class_list(base: &str, extra: Option<&str>) -> String {
    match extra {
        Some(extra) => format!("{base} {extra}"),
        None => base.to_owned(),
    }
}

/// The colour-scheme switch: three buttons, one form, a POST and a redirect
/// back to the page it was pressed on.
///
/// Three buttons rather than one that cycles, so the operator can see which
/// state is active and so "Système" is reachable at all. The target is an
/// `action=`, and each choice a `value=`, so nothing here can be mistaken for
/// the `href=` the role tests read.
#[component]
async fn scheme_switch(cx: &Cx) -> Result<impl View> {
    let current = theme::chosen(cx);
    let back = theme::here(cx);
    Ok(view! {
        <form
            method="post"
            action=(href!(crate::app::admin::choose_scheme))
            aria-label="Thème"
            class="flex items-center gap-0.5 rounded-full border border-border bg-card p-0.5 print:hidden"
        >
            <input type="hidden" name="next" value=(back)>
            for scheme in Scheme::ALL {
                <button
                    type="submit"
                    name="scheme"
                    value=(scheme.as_str())
                    aria-pressed=(if scheme == current { "true" } else { "false" })
                    aria-label=(format!("Thème {}", scheme.label().to_lowercase()))
                    class=(if scheme == current {
                        "inline-flex size-7 items-center justify-center rounded-full bg-accent text-accent-foreground"
                    } else {
                        "inline-flex size-7 items-center justify-center rounded-full text-muted-foreground hover:bg-accent hover:text-accent-foreground"
                    })
                >
                    icon(data: scheme_icon(scheme), attrs: attributes! { class="size-4" })
                </button>
            }
        </form>
    })
}

const fn scheme_icon(scheme: Scheme) -> IconData {
    match scheme {
        Scheme::Light => icons::SUN,
        Scheme::Dark => icons::MOON,
        Scheme::System => icons::MONITOR,
    }
}

#[component]
async fn nav_link(
    link: String,
    current: bool,
    glyph: IconData,
    child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <a
            href=(link)
            aria-current=(current.then_some("page"))
            class=(if current {
                "flex items-center gap-3 rounded-lg bg-sidebar-active p-2 text-sm font-medium text-sidebar-icon-active"
            } else {
                "flex items-center gap-3 rounded-lg p-2 text-sm text-sidebar-foreground transition-colors hover:bg-sidebar-hover"
            })
        >
            icon(
                data: glyph,
                attrs: attributes! {
                    class=(if current { "size-5 text-sidebar-icon-active" } else { "size-5 text-sidebar-icon" })
                },
            )
            <span class="truncate">(child)</span>
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
