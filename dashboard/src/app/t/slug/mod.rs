//! `/t/{slug}`: a tenant's home, for its members. Child modules add the
//! members and roles pages; every one of them starts with [`require_tenant`]
//! and then asks the [`TenantAccess`] for the permission it needs.

pub mod api_keys;
pub mod categories;
pub mod members;
pub mod products;
pub mod rename;
pub mod roles;

use std::sync::Arc;

use timada_tenancy::{
    PERMISSION_API_KEYS_MANAGE, PERMISSION_MEMBERS_READ, PERMISSION_PRODUCTS_READ,
    PERMISSION_ROLES_MANAGE, PERMISSION_TENANT_MANAGE, ROLE_OWNER, TenantState,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{forbidden, not_found},
        href, module_param, page, path_param, query_params,
    },
    view::{View, component, view},
};

use crate::app::auth::Flash;
use crate::app::tenants::tenancy;
use crate::auth::{require_user, unix_seconds};
use crate::authz::Authorizer;
use crate::cluster::Executor;
use crate::tenants::{RequestHold, TenantStatus, Tenants};
use crate::ui::{document, format_time, notice};

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

/// The navigation every tenant page shares: the sections the member may see.
pub struct TenantNav {
    pub products: bool,
    pub categories: bool,
    pub members: bool,
    pub roles: bool,
    pub api_keys: bool,
}

impl TenantNav {
    pub async fn of(cx: &Cx, access: &TenantAccess) -> Result<Self> {
        let products = access.can(cx, PERMISSION_PRODUCTS_READ).await?;
        Ok(Self {
            products,
            // The catalog is one section of the back office: whoever sees the
            // products sees the categories they are filed under.
            categories: products,
            members: access.can(cx, PERMISSION_MEMBERS_READ).await?,
            roles: access.can(cx, PERMISSION_ROLES_MANAGE).await?,
            api_keys: access.can(cx, PERMISSION_API_KEYS_MANAGE).await?,
        })
    }
}

/// The header links of a tenant page: the tenant's home, each section the
/// member may see, the dashboard and the account. `current` names the
/// section being shown (`"home"`, `"products"`, `"categories"`, `"members"`,
/// `"roles"`, `"api_keys"`, or `""` for a page below a section), which gets
/// no link.
#[component]
pub async fn tenant_nav(tenant: &TenantState, nav: &TenantNav, current: &str) -> Result<impl View> {
    let slug = tenant.slug.clone();
    Ok(view! {
        <nav class="flex items-center gap-4 text-sm">
            if current != "home" {
                <a class="underline" href=(href!(index, Slug(slug.clone())))>(&tenant.name)</a>
            }
            if nav.products && current != "products" {
                <a class="underline" href=(href!(products::index, Slug(slug.clone())))>"Products"</a>
            }
            if nav.categories && current != "categories" {
                <a class="underline" href=(href!(categories::index, Slug(slug.clone())))>"Categories"</a>
            }
            if nav.members && current != "members" {
                <a class="underline" href=(href!(members::index, Slug(slug.clone())))>"Members"</a>
            }
            if nav.roles && current != "roles" {
                <a class="underline" href=(href!(roles::index, Slug(slug.clone())))>"Roles"</a>
            }
            if nav.api_keys && current != "api_keys" {
                <a class="underline" href=(href!(api_keys::index, Slug(slug.clone())))>"API keys"</a>
            }
            <a class="underline" href=(href!(crate::app::home))>"Dashboard"</a>
            <a class="underline" href=(href!(crate::app::auth::account::account))>"Account"</a>
        </nav>
    })
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

    Ok(view! {
        document(
            title: &tenant.name,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">
                    (&tenant.name)
                    if tenant.is_suspended() { " " <span class="rounded bg-amber-100 px-2 text-sm font-normal text-amber-800">"suspended"</span> }
                </h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "home")
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if let Some(reason) = &tenant.suspended {
                <p role="alert" class="mb-4 rounded border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900">
                    "This tenant is suspended by the platform: " (reason) ". Its data is offline until an administrator resumes it."
                </p>
            }

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Overview"</h2>
                <dl class="grid grid-cols-[10rem_1fr] gap-y-1 text-sm">
                    <dt class="text-neutral-500">"Address"</dt><dd><code>"/t/" (&tenant.slug)</code></dd>
                    <dt class="text-neutral-500">"Your role"</dt><dd>(&role)</dd>
                    <dt class="text-neutral-500">"Members"</dt><dd>(tenant.members.len())</dd>
                    <dt class="text-neutral-500">"Created"</dt><dd><time datetime=(tenant.created_at)>(format_time(tenant.created_at))</time></dd>
                    <dt class="text-neutral-500">"Database"</dt>
                    <dd data-tenant-status=(if matches!(status, TenantStatus::Running { .. }) { "running" } else { "stopped" })>
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
                    </dd>
                </dl>
            </section>

            if can_manage {
                <section class="rounded border border-neutral-200 bg-white p-4">
                    <h2 class="mb-2 text-lg font-medium">"Settings"</h2>
                    <form method="post" action=(href!(rename::rename, Slug(tenant.slug.clone()))) class="space-y-3">
                        <label class="block text-sm" for="tenant-name">"Name"</label>
                        <input id="tenant-name" name="name" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="text" maxlength=(timada_tenancy::MAX_NAME_LEN) required="" value=(&tenant.name)>
                        <button class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100">"Rename"</button>
                    </form>
                </section>
            }
        )
    })
}
