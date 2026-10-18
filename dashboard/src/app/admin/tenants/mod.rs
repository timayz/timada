//! `/admin/tenants`: every tenant on the platform.

pub mod tenant_id;

use std::sync::Arc;

use timada_tenancy::{count_tenants, list_tenants};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use crate::app::admin::require_admin;
use crate::db::ReadPool;
use crate::tenants::{TenantStatus, Tenants};
use crate::ui::{Shell, document, format_time, notice};

const PAGE_SIZE: i64 = 50;

#[topcoat::router::query_params(error = bad_request)]
struct TenantsQuery {
    /// Matches the slug or the name.
    q: Option<String>,
    /// 1-based.
    page: Option<i64>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct TenantsLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    q: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
}

/// The word for a tenant's state on the admin pages.
pub fn status_label(suspended: bool, status: TenantStatus) -> &'static str {
    match (suspended, status) {
        (true, _) => "Suspended",
        (false, TenantStatus::Running { .. }) => "Running",
        (false, TenantStatus::Stopped) => "Stopped",
    }
}

#[page]
pub async fn tenants(cx: &Cx) -> Result<impl View> {
    require_admin(cx).await?;
    let query = query_params::<TenantsQuery>(cx)?;
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let page_number = query.page.unwrap_or(1).max(1);
    let pool = app_context::<ReadPool>(cx);
    let runtime = app_context::<Arc<Tenants>>(cx);
    let total = count_tenants(pool, q).await?;
    let rows = list_tenants(pool, q, (page_number - 1) * PAGE_SIZE, PAGE_SIZE).await?;
    let pages = (total + PAGE_SIZE - 1) / PAGE_SIZE;
    let page_link = move |page: i64| {
        href!(tenants)
            .query(TenantsLink {
                q,
                page: Some(page),
                message: None,
            })
            .resolve(cx)
    };

    Ok(view! {
        document(
            title: "Tenants",
            shell: Shell::page("admin_tenants"),
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Tenants"</h1>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <form method="get" class="mb-4 flex gap-2" role="search">
                <label class="sr-only" for="tenants-q">"Search tenants"</label>
                <input id="tenants-q" class="w-full rounded border border-neutral-300 px-3 py-1" type="search" name="q" value=(q.unwrap_or("")) placeholder="Slug or name">
                <button class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100">"Search"</button>
            </form>
            <p class="mb-2 text-sm text-neutral-500">
                (total) " tenant" (if total == 1 { "" } else { "s" })
                ", " (runtime.open_count()) " running on this node"
            </p>

            <table class="w-full text-left text-sm">
                <thead class="border-b border-neutral-300 text-neutral-500">
                    <tr>
                        <th class="py-2 pr-3 font-medium">"Name"</th>
                        <th class="py-2 pr-3 font-medium">"Slug"</th>
                        <th class="py-2 pr-3 font-medium">"Members"</th>
                        <th class="py-2 pr-3 font-medium">"Created"</th>
                        <th class="py-2 font-medium">"Status"</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-neutral-100">
                    for row in &rows {
                        <tr class=(if row.suspended { "text-neutral-400" } else { "" })>
                            <td class="py-2 pr-3">
                                <a class="underline" href=(href!(tenant_id::index, tenant_id::TenantId(row.id.clone())))>(&row.name)</a>
                            </td>
                            <td class="py-2 pr-3"><code>(&row.slug)</code></td>
                            <td class="py-2 pr-3">(row.member_count)</td>
                            <td class="py-2 pr-3"><time datetime=(row.created_at)>(format_time(row.created_at as u64))</time></td>
                            <td class="py-2" data-tenant-status="">(status_label(row.suspended, runtime.status(&row.id)))</td>
                        </tr>
                    }
                </tbody>
            </table>

            if pages > 1 {
                <nav class="mt-4 flex gap-3 text-sm" aria-label="Pages">
                    if page_number > 1 { <a class="underline" href=(page_link(page_number - 1))>"Previous"</a> }
                    <span>"Page " (page_number) " of " (pages)</span>
                    if page_number < pages { <a class="underline" href=(page_link(page_number + 1))>"Next"</a> }
                </nav>
            }
        )
    })
}
