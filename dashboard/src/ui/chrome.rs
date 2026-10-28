//! The document every page renders into: the dark rail on the left with the
//! sections the visitor may open, the topbar with the signed-in user, and
//! the page below. Signed-out pages (sign-in, invitations) get the centred
//! [`auth_document`] instead.
//!
//! The rail folds to icons on wide screens and slides over the page below
//! `md`; both states are runtime signals whose initial values come from the
//! server (the fold from a cookie, the sheet closed), so without the runtime
//! script the rail still renders, just without its toggles. The colour
//! scheme is a cookie too, set by a plain form in the rail's footer.

use timada_identity::UserState;
use timada_tenancy::TenantState;
use topcoat::{
    Result,
    asset::asset,
    context::Cx,
    icon::{IconData, icon},
    router::{href, request::uri},
    runtime::{Event, signal},
    tailwind,
    view::{Child, View, attributes, component, element_id, view},
};

use super::icons;
use super::theme::{self, Scheme};
use crate::app::t::slug::{Slug, TenantNav};
use crate::auth::current_user;
use crate::components::avatar::{AvatarSize, avatar, avatar_fallback};
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::dropdown_menu::{
    dropdown_menu, dropdown_menu_content, dropdown_menu_item, dropdown_menu_label,
    dropdown_menu_separator, dropdown_menu_trigger,
};
use crate::components::floating::FloatingAlign;
use crate::components::sidebar::{
    SidebarCollapsible, sidebar, sidebar_content, sidebar_footer, sidebar_group,
    sidebar_group_content, sidebar_group_label, sidebar_inset, sidebar_menu, sidebar_menu_button,
    sidebar_menu_item, sidebar_provider, sidebar_trigger,
};

/// What the rail highlights: the key of the page being shown (one of the
/// [`SECTIONS`] keys on a tenant page, `"dashboard"`, `"companies"`,
/// `"account"`, `"admin_tenants"`, `"admin_users"`, `"admin_companies"`,
/// `"admin_cluster"`, or
/// `""` for a page under none of them), and the tenant whose sections to
/// list.
#[derive(Clone, Copy)]
pub struct Shell<'a> {
    pub current: &'a str,
    pub tenant: Option<(&'a TenantState, &'a TenantNav)>,
}

impl<'a> Shell<'a> {
    pub fn page(current: &'a str) -> Self {
        Shell {
            current,
            tenant: None,
        }
    }

    pub fn tenant(tenant: &'a TenantState, nav: &'a TenantNav, current: &'a str) -> Self {
        Shell {
            current,
            tenant: Some((tenant, nav)),
        }
    }

    /// The line in the topbar: the tenant and the section, or the page.
    fn crumb(&self) -> String {
        if let Some((tenant, _)) = self.tenant {
            return match section(self.current) {
                Some(section) => format!("{} / {}", tenant.name, section.label),
                None if self.current == "home" => tenant.name.clone(),
                None => format!("{} /", tenant.name),
            };
        }
        match self.current {
            "dashboard" => "Dashboard",
            "companies" => "Companies",
            "account" => "Account",
            "admin_tenants" => "Platform / Tenants",
            "admin_users" => "Platform / Users",
            "admin_companies" => "Platform / Companies",
            "admin_cluster" => "Platform / This node",
            _ => "",
        }
        .to_owned()
    }
}

/// The groups of a tenant's rail.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Catalog,
    Sales,
    Marketing,
    Settings,
}

impl Group {
    const ALL: [Group; 4] = [
        Group::Catalog,
        Group::Sales,
        Group::Marketing,
        Group::Settings,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Catalog => "Catalog",
            Self::Sales => "Sales",
            Self::Marketing => "Marketing",
            Self::Settings => "Settings",
        }
    }
}

struct Section {
    key: &'static str,
    label: &'static str,
    icon: IconData,
    group: Group,
}

/// A tenant's sections, in rail order. The labels are what the pages and
/// the browser tests call them.
static SECTIONS: [Section; 20] = [
    Section {
        key: "products",
        label: "Products",
        icon: icons::PRODUCTS,
        group: Group::Catalog,
    },
    Section {
        key: "categories",
        label: "Categories",
        icon: icons::CATEGORIES,
        group: Group::Catalog,
    },
    Section {
        key: "orders",
        label: "Orders",
        icon: icons::ORDERS,
        group: Group::Sales,
    },
    Section {
        key: "carts",
        label: "Carts",
        icon: icons::CARTS,
        group: Group::Sales,
    },
    Section {
        key: "customers",
        label: "Customers",
        icon: icons::CUSTOMERS,
        group: Group::Sales,
    },
    Section {
        key: "invoices",
        label: "Invoices",
        icon: icons::INVOICES,
        group: Group::Sales,
    },
    Section {
        key: "discounts",
        label: "Discounts",
        icon: icons::DISCOUNTS,
        group: Group::Marketing,
    },
    Section {
        key: "offers",
        label: "Offers",
        icon: icons::OFFERS,
        group: Group::Marketing,
    },
    Section {
        key: "reviews",
        label: "Reviews",
        icon: icons::REVIEWS,
        group: Group::Marketing,
    },
    Section {
        key: "messages",
        label: "Messages",
        icon: icons::MESSAGES,
        group: Group::Marketing,
    },
    Section {
        key: "newsletter",
        label: "Newsletter",
        icon: icons::NEWSLETTER,
        group: Group::Marketing,
    },
    Section {
        key: "automations",
        label: "Automations",
        icon: icons::AUTOMATIONS,
        group: Group::Marketing,
    },
    Section {
        key: "payments",
        label: "Payments",
        icon: icons::PAYMENTS,
        group: Group::Settings,
    },
    Section {
        key: "aliexpress",
        label: "AliExpress",
        icon: icons::ALIEXPRESS,
        group: Group::Settings,
    },
    Section {
        key: "shipping",
        label: "Shipping",
        icon: icons::SHIPPING,
        group: Group::Settings,
    },
    Section {
        key: "mail",
        label: "Mail",
        icon: icons::MAIL,
        group: Group::Settings,
    },
    Section {
        key: "invoicing",
        label: "Invoicing",
        icon: icons::INVOICING,
        group: Group::Settings,
    },
    Section {
        key: "members",
        label: "Members",
        icon: icons::MEMBERS,
        group: Group::Settings,
    },
    Section {
        key: "roles",
        label: "Roles",
        icon: icons::ROLES,
        group: Group::Settings,
    },
    Section {
        key: "api_keys",
        label: "API keys",
        icon: icons::API_KEYS,
        group: Group::Settings,
    },
];

fn section(key: &str) -> Option<&'static Section> {
    SECTIONS.iter().find(|section| section.key == key)
}

/// Whether the member may see a section.
fn allowed(nav: &TenantNav, key: &str) -> bool {
    match key {
        "products" => nav.products,
        "categories" => nav.categories,
        "orders" => nav.orders,
        "carts" => nav.carts,
        "customers" => nav.customers,
        "invoices" => nav.invoices,
        "discounts" => nav.discounts,
        "offers" => nav.offers,
        "reviews" => nav.reviews,
        "messages" => nav.messages,
        "newsletter" => nav.newsletter,
        "automations" => nav.automations,
        "payments" => nav.payments,
        "aliexpress" => nav.aliexpress,
        "shipping" => nav.shipping,
        "mail" => nav.mail,
        "invoicing" => nav.invoicing,
        "members" => nav.members,
        "roles" => nav.roles,
        "api_keys" => nav.api_keys,
        _ => false,
    }
}

/// The address of a tenant's section.
pub fn section_url(cx: &Cx, slug: &str, key: &str) -> String {
    use crate::app::t::slug as t;
    let slug = Slug(slug.to_owned());
    match key {
        "products" => href!(t::products::index, slug).resolve(cx),
        "categories" => href!(t::categories::index, slug).resolve(cx),
        "orders" => href!(t::orders::index, slug).resolve(cx),
        "carts" => href!(t::carts::index, slug).resolve(cx),
        "customers" => href!(t::customers::index, slug).resolve(cx),
        "invoices" => href!(t::invoices::index, slug).resolve(cx),
        "discounts" => href!(t::discounts::index, slug).resolve(cx),
        "offers" => href!(t::offers::index, slug).resolve(cx),
        "reviews" => href!(t::reviews::index, slug).resolve(cx),
        "messages" => href!(t::messages::index, slug).resolve(cx),
        "newsletter" => href!(t::newsletter::index, slug).resolve(cx),
        "automations" => href!(t::automations::index, slug).resolve(cx),
        "payments" => href!(t::payments::index, slug).resolve(cx),
        "aliexpress" => href!(t::aliexpress::index, slug).resolve(cx),
        "shipping" => href!(t::shipping::index, slug).resolve(cx),
        "mail" => href!(t::mail::index, slug).resolve(cx),
        "invoicing" => href!(t::invoicing::index, slug).resolve(cx),
        "members" => href!(t::members::index, slug).resolve(cx),
        "roles" => href!(t::roles::index, slug).resolve(cx),
        "api_keys" => href!(t::api_keys::index, slug).resolve(cx),
        _ => href!(t::index, slug).resolve(cx),
    }
}

/// The `<head>` both documents share.
#[component]
async fn head(title: &str) -> Result<impl View> {
    Ok(view! {
        <head>
            <meta charset="utf-8">
            <meta name="viewport" content="width=device-width, initial-scale=1">
            <title>(title)</title>
            <link rel="stylesheet" href=(tailwind::stylesheet!())>
            topcoat::runtime::script()
            <script type="module" src=(asset!("../app/auth/auth.js"))></script>
            topcoat::dev::script()
        </head>
    })
}

/// One link of the rail.
#[component]
async fn nav_item(
    href: String,
    label: &str,
    symbol: IconData,
    #[default] active: bool,
) -> Result<impl View> {
    Ok(view! {
        sidebar_menu_item(
            sidebar_menu_button(
                href: Some(href.as_str()),
                active: active,
                tooltip: Some(label),
                <span class="flex size-7 shrink-0 items-center justify-center rounded-md bg-sidebar-icon/10 text-sidebar-icon [&>svg]:size-4">
                    icon(data: symbol)
                </span>
                <span>(label)</span>
            )
        )
    })
}

/// The rail's groups: the tenant's sections first, then the visitor's own
/// pages, then the platform for administrators.
#[component]
async fn rail_groups(cx: &Cx, user: Option<&UserState>, shell: Shell<'_>) -> Result<impl View> {
    let current = shell.current;
    Ok(view! {
        if let Some((tenant, nav)) = shell.tenant {
            <div class="px-3 pt-3 md:group-data-[collapsible=icon]/sidebar:hidden">
                <a
                    href=(href!(crate::app::t::slug::index, Slug(tenant.slug.clone())))
                    class="block truncate text-sm font-semibold text-sidebar-foreground hover:underline"
                    aria-current=((current == "home").then_some("page"))
                >
                    (&tenant.name)
                </a>
                if tenant.is_suspended() {
                    badge(variant: BadgeVariant::Warning, attrs: attributes! { class="mt-1" }, "suspended")
                }
            </div>
            for group in Group::ALL {
                if SECTIONS.iter().any(|s| s.group == group && allowed(nav, s.key)) {
                    sidebar_group(
                        sidebar_group_label((group.label()))
                        sidebar_group_content(
                            sidebar_menu(
                                for s in SECTIONS.iter().filter(|s| s.group == group && allowed(nav, s.key)) {
                                    nav_item(
                                        href: section_url(cx, &tenant.slug, s.key),
                                        label: s.label,
                                        symbol: s.icon.clone(),
                                        active: current == s.key,
                                    )
                                }
                            )
                        )
                    )
                }
            }
        }
        if user.is_some() {
            sidebar_group(
                sidebar_group_label("Workspace")
                sidebar_group_content(
                    sidebar_menu(
                        nav_item(href: href!(crate::app::home).resolve(cx), label: "Dashboard", symbol: icons::DASHBOARD, active: current == "dashboard")
                        nav_item(href: href!(crate::app::companies::index).resolve(cx), label: "Companies", symbol: icons::COMPANIES, active: current == "companies")
                        nav_item(href: href!(crate::app::auth::account::account).resolve(cx), label: "Account", symbol: icons::ACCOUNT, active: current == "account")
                    )
                )
            )
        }
        if user.is_some_and(|user| user.is_admin) {
            sidebar_group(
                sidebar_group_label("Platform")
                sidebar_group_content(
                    sidebar_menu(
                        nav_item(href: href!(crate::app::admin::tenants::tenants).resolve(cx), label: "Tenants", symbol: icons::TENANTS, active: current == "admin_tenants")
                        nav_item(href: href!(crate::app::admin::users::users).resolve(cx), label: "Users", symbol: icons::USERS, active: current == "admin_users")
                        nav_item(href: href!(crate::app::admin::companies::index).resolve(cx), label: "All companies", symbol: icons::COMPANIES, active: current == "admin_companies")
                        nav_item(href: href!(crate::app::admin::cluster::index).resolve(cx), label: "This node", symbol: icons::DASHBOARD, active: current == "admin_cluster")
                    )
                )
            )
        }
    })
}

/// Light / Dark / System, a form in the rail's footer.
#[component]
async fn theme_switch(scheme: Scheme, here: &str) -> Result<impl View> {
    Ok(view! {
        <form
            method="post"
            action=(href!(crate::app::ui::theme::theme))
            class="flex justify-center md:group-data-[collapsible=icon]/sidebar:hidden"
            aria-label="Theme"
        >
            <input type="hidden" name="next" value=(here)>
            <div class="flex rounded-2xl border border-sidebar-border p-0.5">
                for option in Scheme::ALL {
                    <button
                        name="scheme"
                        value=(option.as_str())
                        aria-pressed=(if option == scheme { "true" } else { "false" })
                        class="inline-flex items-center gap-1.5 rounded-xl px-2.5 py-1 text-xs font-medium text-sidebar-muted transition-colors hover:text-sidebar-foreground aria-pressed:bg-sidebar-accent aria-pressed:text-sidebar-foreground [&>svg]:size-3.5"
                    >
                        icon(data: option.icon())
                        <span>(option.label())</span>
                    </button>
                }
            </div>
        </form>
    })
}

/// The first letters of a user's label, for the avatar.
pub fn initials(label: &str) -> String {
    let mut letters: String = label
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect();
    if letters.is_empty() {
        letters.push('?');
    }
    letters.to_uppercase()
}

/// The right end of the topbar: who is signed in, the account menu, and the
/// sign-out button (outside the menu, so it works without scripts and the
/// browser tests find it).
#[component]
async fn user_menu(cx: &Cx, user: Option<&UserState>) -> Result<impl View> {
    Ok(view! {
        if let Some(user) = user {
            let menu = element_id(cx);
            <span class="hidden truncate text-sm sm:inline">"Signed in as " <strong>(&user.label)</strong></span>
            dropdown_menu(
                dropdown_menu_trigger(
                    target: &menu,
                    attrs: attributes! { aria-label="Account menu" class="rounded-full" },
                    avatar(size: AvatarSize::Sm, avatar_fallback(attrs: attributes! { class="bg-primary/10 text-primary" }, (initials(&user.label))))
                )
                dropdown_menu_content(
                    id: &menu,
                    align: FloatingAlign::End,
                    dropdown_menu_label((&user.label))
                    dropdown_menu_separator()
                    dropdown_menu_item(close: Some(&menu), <a href=(href!(crate::app::auth::account::account))>"Account"</a>)
                )
            )
            <form method="post" action=(href!(crate::app::auth::logout::logout))>
                button(
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Sm,
                    attrs: attributes! { type="submit" },
                    icon(data: icons::LOG_OUT)
                    <span>"Sign out"</span>
                )
            </form>
        } else {
            <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(href!(crate::app::auth::login::login))>
                icon(data: icons::LOG_IN)
                "Sign in"
            </a>
        }
    })
}

/// The HTML document of a signed-in page: rail, topbar, content.
#[component]
pub async fn document(
    cx: &Cx,
    title: &str,
    shell: Shell<'_>,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let user = current_user(cx).await;
    let scheme = theme::scheme(cx);
    let open = signal(cx, || theme::rail_open(cx));
    let mobile_open = signal(cx, || false);
    let here = uri(cx).path().to_owned();
    let crumb = shell.crumb();
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en" class=(scheme.html_class())>
            head(title: title)
            <body class="min-h-svh bg-background text-foreground">
                sidebar_provider(
                    sidebar(
                        open: $(open.get()),
                        mobile_open: $(mobile_open.get()),
                        collapsible: SidebarCollapsible::Icon,
                        sheet_attrs: attributes! { id="app-sidebar" },
                        <div class="flex h-14 shrink-0 items-center justify-between border-b border-sidebar-border px-3">
                            <a href=(href!(crate::app::home)) class="flex min-w-0 items-center gap-2 font-semibold" aria-label="Timada">
                                <span class="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary text-primary-foreground [&>svg]:size-4">
                                    icon(data: icons::TENANTS)
                                </span>
                                <span class="truncate md:group-data-[collapsible=icon]/sidebar:hidden">"Timada"</span>
                            </a>
                            button(
                                variant: ButtonVariant::Ghost,
                                size: ButtonSize::Icon,
                                attrs: attributes! {
                                    type="button"
                                    class="md:hidden"
                                    aria-label="Close menu"
                                    @click=$(|_e: Event| mobile_open.set(false))
                                },
                                icon(data: icons::CLOSE)
                            )
                        </div>
                        sidebar_content(
                            rail_groups(user: user, shell: shell)
                        )
                        sidebar_footer(
                            attrs: attributes! { class="border-t border-sidebar-border py-3" },
                            theme_switch(scheme: scheme, here: &here)
                        )
                    )
                    sidebar_inset(
                        <header class="sticky top-0 z-20 flex h-14 shrink-0 items-center gap-3 border-b border-topbar-border bg-topbar px-4 text-topbar-foreground">
                            <span class="hidden md:contents">
                            sidebar_trigger(
                                open: $(open.get()),
                                attrs: attributes! {
                                    @click=$(|_e: Event| {
                                        if open.get() {
                                            open.set(false);
                                            raw!("document.cookie = 'timada_rail=closed; Path=/; Max-Age=31536000; SameSite=Lax'")
                                        } else {
                                            open.set(true);
                                            raw!("document.cookie = 'timada_rail=open; Path=/; Max-Age=31536000; SameSite=Lax'")
                                        }
                                    })
                                },
                            )
                            </span>
                            sidebar_trigger(
                                open: $(mobile_open.get()),
                                attrs: attributes! {
                                    class="md:hidden"
                                    aria-controls="app-sidebar"
                                    @click=$(|_e: Event| mobile_open.set(true))
                                },
                            )
                            <p class="min-w-0 flex-1 truncate text-sm text-muted-foreground">(crumb)</p>
                            <div class="flex shrink-0 items-center gap-2">
                                user_menu(user: user)
                            </div>
                        </header>
                        <div class="mx-auto w-full max-w-6xl p-4 md:p-8">
                            (child)
                        </div>
                    )
                )
            </body>
        </html>
    })
}

/// The HTML document of a signed-out page: the content centred, no rail.
#[component]
pub async fn auth_document(cx: &Cx, title: &str, #[default] child: Child<'_>) -> Result<impl View> {
    let scheme = theme::scheme(cx);
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en" class=(scheme.html_class())>
            head(title: title)
            <body class="min-h-svh bg-background text-foreground">
                <main class="flex min-h-svh flex-col items-center justify-center p-6">
                    <a href=(href!(crate::app::home)) class="mb-6 flex items-center gap-2 text-lg font-semibold" aria-label="Timada">
                        <span class="flex size-9 items-center justify-center rounded-lg bg-primary text-primary-foreground [&>svg]:size-5">
                            icon(data: icons::TENANTS)
                        </span>
                        "Timada"
                    </a>
                    <div class="w-full max-w-md">(child)</div>
                </main>
            </body>
        </html>
    })
}
