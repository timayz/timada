//! `/t/{slug}`: a tenant's home, for its members.

pub mod rename;

use std::sync::Arc;

use timada_identity::users_by_ids;
use timada_tenancy::{ROLE_OWNER, TenantState};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{forbidden, not_found},
        href, module_param, page, path_param, query_params,
    },
    view::{View, view},
};

use crate::app::auth::Flash;
use crate::app::tenants::tenancy;
use crate::auth::{require_user, unix_seconds};
use crate::cluster::Executor;
use crate::db::ReadPool;
use crate::tenants::{TenantStatus, Tenants};
use crate::ui::{document, format_time, notice};

module_param!(pub slug);

/// The tenant of the request and the member looking at it.
pub struct TenantAccess {
    pub tenant: TenantState,
    /// The role the signed-in user holds in it.
    pub role: String,
    /// The tenant's event store, open while it is not suspended.
    #[allow(dead_code)]
    executor: Option<Executor>,
}

impl TenantAccess {
    pub fn is_owner(&self) -> bool {
        self.role == ROLE_OWNER
    }

    /// The tenant's executor, or 403 while the tenant is suspended. Nothing
    /// tenant-scoped exists to write yet; the first commerce context uses it.
    #[allow(dead_code)]
    pub fn executor(&self) -> Result<&Executor> {
        self.executor.as_ref().ok_or_else(|| forbidden().into())
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
    let executor = if tenant.is_suspended() {
        None
    } else {
        Some(
            app_context::<Arc<Tenants>>(cx)
                .open(&tenant)
                .await
                .map_err(topcoat::Error::from_anyhow)?,
        )
    };
    Ok(TenantAccess {
        tenant,
        role,
        executor,
    })
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

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    let query = query_params::<TenantQuery>(cx)?;
    let is_owner = access.is_owner();
    let TenantAccess { tenant, role, .. } = access;
    let status = app_context::<Arc<Tenants>>(cx).status(&tenant.id);
    let member_ids: Vec<String> = tenant.members.keys().cloned().collect();
    let labels = users_by_ids(app_context::<ReadPool>(cx), &member_ids).await?;
    let label_of = move |user_id: &str| -> String {
        labels
            .iter()
            .find(|u| u.id == user_id)
            .map(|u| u.label.clone())
            .unwrap_or_else(|| user_id.to_owned())
    };

    Ok(view! {
        document(
            title: &tenant.name,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">
                    (&tenant.name)
                    if tenant.is_suspended() { " " <span class="rounded bg-amber-100 px-2 text-sm font-normal text-amber-800">"suspended"</span> }
                </h1>
                <nav class="flex items-center gap-4 text-sm">
                    <a class="underline" href=(href!(crate::app::home))>"Dashboard"</a>
                    <a class="underline" href=(href!(crate::app::auth::account::account))>"Account"</a>
                </nav>
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
                    <dt class="text-neutral-500">"Created"</dt><dd><time datetime=(tenant.created_at)>(format_time(tenant.created_at))</time></dd>
                    <dt class="text-neutral-500">"Database"</dt>
                    <dd data-tenant-status=(if matches!(status, TenantStatus::Running { .. }) { "running" } else { "stopped" })>
                        (match status {
                            TenantStatus::Running { since } => format!("Running since {}", format_time(unix_seconds(since))),
                            TenantStatus::Stopped => "Stopped".to_owned(),
                        })
                    </dd>
                </dl>
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Members"</h2>
                <ul class="divide-y divide-neutral-100 text-sm">
                    for (user_id, role) in &tenant.members {
                        <li class="flex justify-between py-2">
                            <span>(label_of(user_id))</span>
                            <span class="text-neutral-500">(role)</span>
                        </li>
                    }
                </ul>
            </section>

            if is_owner {
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
