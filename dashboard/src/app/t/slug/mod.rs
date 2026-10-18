//! `/t/{slug}`: a tenant's home, for its members. Child modules add the
//! members and roles pages; every one of them starts with [`require_tenant`]
//! and then asks the [`TenantAccess`] for the permission it needs.

pub mod api_keys;
pub mod carts;
pub mod categories;
pub mod customers;
pub mod discounts;
pub mod invoices;
pub mod invoicing;
pub mod mail;
pub mod members;
pub mod messages;
pub mod offers;
pub mod orders;
pub mod payments;
pub mod products;
pub mod rename;
pub mod reviews;
pub mod roles;
pub mod shipping;

use std::sync::Arc;

use timada_catalog::{ProductFilter, count_products};
use timada_invoicing::count_documents;
use timada_orders::{OrderFilter, count_open_carts, count_orders};
use timada_reviews::{ReviewFilter, ReviewStatus, count_reviews};
use timada_shoppers::count_shoppers;
use timada_support::{MessageFilter, MessageStatus, count_messages};
use timada_tenancy::count_open_invitations_of_tenant;
use timada_tenancy::{
    PERMISSION_API_KEYS_MANAGE, PERMISSION_CUSTOMERS_READ, PERMISSION_DISCOUNTS_READ,
    PERMISSION_MAIL_MANAGE, PERMISSION_MEMBERS_READ, PERMISSION_ORDERS_READ,
    PERMISSION_PAYMENTS_MANAGE, PERMISSION_PRODUCTS_READ, PERMISSION_REVIEWS_READ,
    PERMISSION_ROLES_MANAGE, PERMISSION_SHIPPING_MANAGE, PERMISSION_SUPPORT_READ,
    PERMISSION_TENANT_MANAGE, ROLE_OWNER, TenantState,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::{IconData, icon},
    router::{
        error::{forbidden, not_found},
        href, module_param, page, path_param, query_params,
    },
    view::{View, attributes, view},
};

use crate::app::auth::Flash;
use crate::app::tenants::tenancy;
use crate::auth::{now, require_user, unix_seconds};
use crate::authz::Authorizer;
use crate::cluster::Executor;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::button;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::db::ReadPool;
use crate::tenants::{RequestHold, TenantStatus, Tenants};
use crate::ui::{
    Shell, Tone, chrome::section_url, document, fact, facts, format_time, icons, notice,
    section_card, stat_tile, status_badge,
};

module_param!(pub slug);

/// The tenant of the request and the member looking at it.
pub struct TenantAccess {
    pub tenant: TenantState,
    /// The signed-in member.
    pub user_id: String,
    /// The role the signed-in user holds in it.
    pub role: String,
    /// The tenant's event store, open while it is not suspended, and held
    /// open until the request is done with it.
    hold: Option<RequestHold>,
}

impl TenantAccess {
    pub fn is_owner(&self) -> bool {
        self.role == ROLE_OWNER
    }

    /// Whether the member holds `permission` (`resource:action`).
    pub async fn can(&self, cx: &Cx, permission: &str) -> Result<bool> {
        app_context::<Arc<Authorizer>>(cx)
            .allows(&self.tenant, &self.user_id, permission)
            .await
            .map_err(topcoat::Error::from_anyhow)
    }

    /// 403 unless the member holds `permission`.
    pub async fn require_permission(&self, cx: &Cx, permission: &str) -> Result<()> {
        if self.can(cx, permission).await? {
            Ok(())
        } else {
            Err(forbidden().into())
        }
    }

    /// The tenant's executor, or 403 while the tenant is suspended: every
    /// write to the tenant's own data (its catalog) goes through it.
    pub fn executor(&self) -> Result<&Executor> {
        self.hold
            .as_ref()
            .map(RequestHold::executor)
            .ok_or_else(|| forbidden().into())
    }
}

/// The tenant named in the URL, for a signed-in member: 404 for an unknown
/// slug, 403 for everyone else. Opening the tenant's database here is what
/// counts as activity for it.
pub async fn require_tenant(cx: &Cx) -> Result<TenantAccess> {
    let user = require_user(cx).await?;
    let slug = path_param::<Slug>(cx);
    let Some(tenant) = tenancy(cx)
        .load_tenant_by_slug(slug)
        .await
        .map_err(topcoat::Error::from_anyhow)?
    else {
        return Err(not_found().into());
    };
    let Some(role) = tenant.role_of(&user.id) else {
        return Err(forbidden().into());
    };
    let role = role.to_owned();
    let hold = if tenant.is_suspended() {
        None
    } else {
        Some(
            app_context::<Arc<Tenants>>(cx)
                .hold(&tenant)
                .await
                .map_err(topcoat::Error::from_anyhow)?,
        )
    };
    Ok(TenantAccess {
        tenant,
        user_id: user.id.clone(),
        role,
        hold,
    })
}

/// How a role is shown: its label for a custom role, the name otherwise.
pub fn role_label(tenant: &TenantState, role: &str) -> String {
    tenant
        .roles
        .get(role)
        .map(|def| def.label.clone())
        .unwrap_or_else(|| role.to_owned())
}

#[topcoat::router::query_params(error = bad_request)]
struct TenantQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn tenant_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The sections of the tenant the member may see; the rail lists them.
pub struct TenantNav {
    pub products: bool,
    pub categories: bool,
    pub orders: bool,
    pub carts: bool,
    pub customers: bool,
    pub discounts: bool,
    pub offers: bool,
    pub reviews: bool,
    pub messages: bool,
    pub invoices: bool,
    pub payments: bool,
    pub shipping: bool,
    pub mail: bool,
    pub invoicing: bool,
    pub members: bool,
    pub roles: bool,
    pub api_keys: bool,
}

impl TenantNav {
    pub async fn of(cx: &Cx, access: &TenantAccess) -> Result<Self> {
        let products = access.can(cx, PERMISSION_PRODUCTS_READ).await?;
        let orders = access.can(cx, PERMISSION_ORDERS_READ).await?;
        let discounts = access.can(cx, PERMISSION_DISCOUNTS_READ).await?;
        Ok(Self {
            products,
            // The catalog is one section of the back office: whoever sees the
            // products sees the categories they are filed under.
            categories: products,
            // Sales are one section too: the open carts are the orders not
            // placed yet.
            orders,
            carts: orders,
            customers: access.can(cx, PERMISSION_CUSTOMERS_READ).await?,
            discounts,
            // Checkout offers are discounts of another shape.
            offers: discounts,
            reviews: access.can(cx, PERMISSION_REVIEWS_READ).await?,
            messages: access.can(cx, PERMISSION_SUPPORT_READ).await?,
            // Invoices are what the orders were billed as.
            invoices: orders,
            payments: access.can(cx, PERMISSION_PAYMENTS_MANAGE).await?,
            shipping: access.can(cx, PERMISSION_SHIPPING_MANAGE).await?,
            mail: access.can(cx, PERMISSION_MAIL_MANAGE).await?,
            invoicing: access.can(cx, PERMISSION_TENANT_MANAGE).await?,
            members: access.can(cx, PERMISSION_MEMBERS_READ).await?,
            roles: access.can(cx, PERMISSION_ROLES_MANAGE).await?,
            api_keys: access.can(cx, PERMISSION_API_KEYS_MANAGE).await?,
        })
    }
}

/// One figure of the tenant's home: what a section counts, read from the
/// shared read model so a suspended tenant's home still shows it.
struct Tile {
    label: &'static str,
    value: String,
    symbol: IconData,
    href: String,
    tone: Tone,
    hint: Option<String>,
}

/// The tiles the member may see, in the order of the rail. A figure that
/// asks for attention (reviews or messages waiting) is drawn in the warning
/// colour while it is above zero.
async fn tiles(cx: &Cx, tenant: &TenantState, nav: &TenantNav) -> Result<Vec<Tile>> {
    let pool = app_context::<ReadPool>(cx);
    let id = tenant.id.as_str();
    let link = |key: &str| section_url(cx, &tenant.slug, key);
    let attention = |n: u64| if n > 0 { Tone::Warning } else { Tone::Neutral };
    let mut tiles = Vec::new();
    if nav.products {
        let n = count_products(pool, id, &ProductFilter::default()).await?;
        tiles.push(Tile {
            label: "Products",
            value: n.to_string(),
            symbol: icons::PRODUCTS,
            href: link("products"),
            tone: Tone::Primary,
            hint: None,
        });
    }
    if nav.orders {
        let n = count_orders(pool, id, &OrderFilter::default()).await?;
        tiles.push(Tile {
            label: "Orders",
            value: n.to_string(),
            symbol: icons::ORDERS,
            href: link("orders"),
            tone: Tone::Info,
            hint: None,
        });
        let n = count_open_carts(pool, id).await?;
        tiles.push(Tile {
            label: "Open carts",
            value: n.to_string(),
            symbol: icons::CARTS,
            href: link("carts"),
            tone: Tone::Neutral,
            hint: None,
        });
    }
    if nav.customers {
        let n = count_shoppers(pool, id)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        tiles.push(Tile {
            label: "Customers",
            value: n.to_string(),
            symbol: icons::CUSTOMERS,
            href: link("customers"),
            tone: Tone::Primary,
            hint: None,
        });
    }
    if nav.reviews {
        let filter = ReviewFilter {
            status: Some(ReviewStatus::Pending),
            ..ReviewFilter::default()
        };
        let n = count_reviews(pool, id, &filter)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        tiles.push(Tile {
            label: "Pending reviews",
            value: n.to_string(),
            symbol: icons::REVIEWS,
            href: link("reviews"),
            tone: attention(n),
            hint: None,
        });
    }
    if nav.messages {
        let filter = MessageFilter {
            status: Some(MessageStatus::Open),
        };
        let n = count_messages(pool, id, &filter)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        tiles.push(Tile {
            label: "Open messages",
            value: n.to_string(),
            symbol: icons::MESSAGES,
            href: link("messages"),
            tone: attention(n),
            hint: None,
        });
    }
    if nav.invoices {
        let n = count_documents(pool, id).await?;
        tiles.push(Tile {
            label: "Invoices",
            value: n.to_string(),
            symbol: icons::INVOICES,
            href: link("invoices"),
            tone: Tone::Success,
            hint: None,
        });
    }
    if nav.members {
        let invitations = count_open_invitations_of_tenant(pool, id, now()).await?;
        tiles.push(Tile {
            label: "Members",
            value: tenant.members.len().to_string(),
            symbol: icons::MEMBERS,
            href: link("members"),
            tone: Tone::Neutral,
            hint: Some(match invitations {
                0 => "No open invitation".to_owned(),
                1 => "1 open invitation".to_owned(),
                n => format!("{n} open invitations"),
            }),
        });
    }
    Ok(tiles)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    let query = query_params::<TenantQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_TENANT_MANAGE).await?;
    let TenantAccess { tenant, role, .. } = access;
    let role = role_label(&tenant, &role);
    let status = app_context::<Arc<Tenants>>(cx).status(&tenant.id);
    let tiles = tiles(cx, &tenant, &nav).await?;
    let members = match tenant.members.len() {
        1 => "1 member".to_owned(),
        n => format!("{n} members"),
    };

    Ok(view! {
        document(
            title: &tenant.name,
            shell: Shell::tenant(&tenant, &nav, "home"),
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if let Some(reason) = &tenant.suspended {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" class="mb-6" },
                    icon(data: icons::WARNING)
                    alert_description(
                        "This tenant is suspended by the platform: " (reason) ". Its data is offline until an administrator resumes it."
                    )
                )
            }

            section_card(
                <div class="flex flex-wrap items-start justify-between gap-x-10 gap-y-6">
                    <div class="min-w-0">
                        <div class="flex flex-wrap items-center gap-3">
                            <h1 class="text-2xl font-bold tracking-tight md:text-3xl">(&tenant.name)</h1>
                            if tenant.is_suspended() {
                                status_badge(tone: Tone::Warning, "suspended")
                            }
                        </div>
                        <p class="mt-2 text-sm text-muted-foreground">
                            <code class="rounded-md bg-muted px-1.5 py-0.5 font-mono text-xs text-foreground">"/t/" (&tenant.slug)</code>
                            " · " (&members)
                        </p>
                    </div>
                    <div class="w-full lg:w-auto lg:min-w-[26rem]">
                        facts(
                            fact(term: "Your role", badge(variant: BadgeVariant::Tinted, (&role)))
                            fact(term: "Created", <time datetime=(tenant.created_at)>(format_time(tenant.created_at))</time>)
                            fact(
                                term: "Database",
                                attrs: attributes! {
                                    data-tenant-status=(if matches!(status, TenantStatus::Running { .. }) { "running" } else { "stopped" })
                                },
                                (match status {
                                    TenantStatus::Running { since, leases } => {
                                        let since = format_time(unix_seconds(since));
                                        match leases {
                                            0 => format!("Running since {since}"),
                                            1 => format!("Running since {since} · 1 store connection"),
                                            n => format!("Running since {since} · {n} store connections"),
                                        }
                                    }
                                    TenantStatus::Stopped => "Stopped".to_owned(),
                                })
                            )
                        )
                    </div>
                </div>
            )

            if !tiles.is_empty() {
                <div class="mb-6 grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
                    for tile in &tiles {
                        stat_tile(
                            label: tile.label,
                            value: tile.value.clone(),
                            symbol: tile.symbol.clone(),
                            href: tile.href.clone(),
                            tone: tile.tone,
                            hint: tile.hint.as_deref(),
                        )
                    }
                </div>
            }

            if can_manage {
                section_card(
                    title: Some("Settings"),
                    description: Some("The name members and the platform see this tenant under."),
                    <form method="post" action=(href!(rename::rename, Slug(tenant.slug.clone()))) class="flex max-w-xl flex-col gap-4">
                        field(
                            field_label(attrs: attributes! { for="tenant-name" }, "Name")
                            input(attrs: attributes! {
                                id="tenant-name" name="name" type="text"
                                maxlength=(timada_tenancy::MAX_NAME_LEN) required="" value=(&tenant.name)
                            })
                        )
                        <div>
                            button(attrs: attributes! { type="submit" }, "Rename")
                        </div>
                    </form>
                )
            }
        )
    })
}
