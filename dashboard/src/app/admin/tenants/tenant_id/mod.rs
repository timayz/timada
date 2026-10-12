//! `/admin/tenants/{tenant_id}`: one tenant and the actions on it.

pub mod resume;
pub mod suspend;

use std::sync::Arc;

use timada_identity::users_by_ids;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{error::not_found, href, module_param, page, path_param, query_params},
    view::{View, view},
};

use super::status_label;
use crate::app::admin::require_admin;
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, index as tenant_page};
use crate::app::tenants::tenancy;
use crate::auth::unix_seconds;
use crate::db::ReadPool;
use crate::tenants::{TenantStatus, Tenants};
use crate::ui::{document, format_time, notice};

module_param!(pub tenant_id);

#[topcoat::router::query_params(error = bad_request)]
struct TenantQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn admin_tenant_url(
    cx: &Cx,
    tenant_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(index, TenantId(tenant_id.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    require_admin(cx).await?;
    let id = path_param::<TenantId>(cx);
    let query = query_params::<TenantQuery>(cx)?;
    let Some(tenant) = tenancy(cx)
        .load_tenant(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
    else {
        return Err(not_found().into());
    };
    let runtime = app_context::<Arc<Tenants>>(cx);
    let status = runtime.status(&tenant.id);
    let member_ids: Vec<String> = tenant.members.keys().cloned().collect();
    let labels = users_by_ids(app_context::<ReadPool>(cx), &member_ids).await?;
    let label_of = move |user_id: &str| -> String {
        labels
            .iter()
            .find(|u| u.id == user_id)
            .map(|u| u.label.clone())
            .unwrap_or_else(|| user_id.to_owned())
    };
    let title = format!("{} · Tenants", tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">(&tenant.name)</h1>
                <nav class="flex items-center gap-4 text-sm">
                    <a class="underline" href=(href!(super::tenants))>"Tenants"</a>
                    <a class="underline" href=(href!(crate::app::home))>"Dashboard"</a>
                </nav>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Overview"</h2>
                <dl class="grid grid-cols-[10rem_1fr] gap-y-1 text-sm">
                    <dt class="text-neutral-500">"Slug"</dt>
                    <dd><a class="underline" href=(href!(tenant_page, Slug(tenant.slug.clone())))><code>"/t/" (&tenant.slug)</code></a></dd>
                    <dt class="text-neutral-500">"Id"</dt><dd><code>(&tenant.id)</code></dd>
                    <dt class="text-neutral-500">"Created"</dt><dd><time datetime=(tenant.created_at)>(format_time(tenant.created_at))</time> " by " (label_of(&tenant.created_by))</dd>
                    <dt class="text-neutral-500">"Status"</dt>
                    <dd data-tenant-status="">
                        (status_label(tenant.is_suspended(), status))
                        if let TenantStatus::Running { since } = status {
                            " since " <time datetime=(unix_seconds(since))>(format_time(unix_seconds(since)))</time>
                        }
                        if let Some(reason) = &tenant.suspended { ": " (reason) }
                    </dd>
                </dl>
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Members"</h2>
                <ul class="divide-y divide-neutral-100 text-sm">
                    for (user_id, role) in &tenant.members {
                        <li class="flex justify-between py-2">
                            <a class="underline" href=(href!(crate::app::admin::users::user_id::user, crate::app::admin::users::user_id::UserId(user_id.clone())))>(label_of(user_id))</a>
                            <span class="text-neutral-500">(role)</span>
                        </li>
                    }
                </ul>
            </section>

            <section class="rounded border border-red-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Administration"</h2>
                if tenant.is_suspended() {
                    <form method="post" action=(href!(resume::resume, TenantId(tenant.id.clone())))>
                        <button class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100">"Resume tenant"</button>
                    </form>
                } else {
                    <form method="post" action=(href!(suspend::suspend, TenantId(tenant.id.clone()))) class="flex flex-wrap items-end gap-2">
                        <div class="grow">
                            <label class="block text-sm" for="suspend-reason">"Reason"</label>
                            <input id="suspend-reason" name="reason" class="mt-1 w-full rounded border border-neutral-300 px-3 py-1" type="text" maxlength="200" required="">
                        </div>
                        <button class="rounded border border-red-700 px-3 py-1 text-sm text-red-700 hover:bg-red-50">"Suspend tenant"</button>
                    </form>
                    <p class="mt-2 text-sm text-neutral-500">"Suspending closes the tenant's database and takes its pages offline until it is resumed."</p>
                }
            </section>
        )
    })
}
