//! `/t/{slug}/invoices/oss`: what to declare on the quarterly One-Stop Shop
//! return: the VAT of other EU countries charged to their consumers, by
//! country and rate, credit notes deducted. From the shared read model.

use timada_invoicing::{format_amount_fr, format_rate_fr, oss_return, quarter_bounds, quarter_of};
use timada_tenancy::PERMISSION_ORDERS_READ;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::db::ReadPool;
use crate::ui::document;

#[topcoat::router::query_params(error = bad_request)]
struct OssQuery {
    /// `2026-Q4`; the current quarter when absent or unreadable.
    quarter: Option<String>,
}

#[derive(serde::Serialize)]
struct OssLink {
    quarter: String,
}

/// The quarter before or after `quarter`.
fn step(quarter: &str, forward: bool) -> String {
    let (from, to) = quarter_bounds(quarter).unwrap_or_default();
    let at = if forward { to } else { from - 1 };
    quarter_of(at.max(0) as u64)
}

#[page]
pub async fn oss(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let query = query_params::<OssQuery>(cx)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let quarter = query
        .quarter
        .clone()
        .filter(|quarter| quarter_bounds(quarter).is_some())
        .unwrap_or_else(|| quarter_of(now));
    let (from, to) = quarter_bounds(&quarter).unwrap_or_default();
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let rows = oss_return(app_context::<ReadPool>(cx), &tenant.id, from, to).await?;
    let slug = tenant.slug.clone();
    let link = |quarter: String| {
        href!(oss, Slug(slug.clone()))
            .query(OssLink { quarter })
            .resolve(cx)
    };
    let previous = link(step(&quarter, false));
    let next = link(step(&quarter, true));
    let title = format!("One-Stop Shop {quarter} · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"One-Stop Shop return"</h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "")
            </header>
            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <p class="mb-4 flex items-center gap-4 text-sm">
                    <a class="underline" href=(previous)>"Previous quarter"</a>
                    <strong data-oss-quarter="">(&quarter)</strong>
                    <a class="underline" href=(next)>"Next quarter"</a>
                </p>
                <p class="mb-4 text-sm text-neutral-600">
                    "Sales to consumers in other EU countries taxed at their rates, as invoiced in the quarter (UTC), credit notes deducted. Declare them on the OSS return of impots.gouv.fr."
                </p>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Country"</th>
                            <th class="py-2 font-medium">"Rate"</th>
                            <th class="py-2 text-right font-medium">"Taxable amount"</th>
                            <th class="py-2 text-right font-medium">"VAT"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        if rows.is_empty() {
                            <tr><td class="py-2 text-neutral-500" colspan="4" data-oss-empty="">"Nothing to declare this quarter."</td></tr>
                        }
                        for row in &rows {
                            <tr data-oss-row=(format!("{}-{}", row.country, row.rate_bps))>
                                <td class="py-2">(&row.country)</td>
                                <td class="py-2">(format_rate_fr(row.rate_bps.max(0) as u32))</td>
                                <td class="py-2 text-right">(format_amount_fr(row.base_cents.max(0) as u64, &row.currency))</td>
                                <td class="py-2 text-right" data-oss-vat="">(format_amount_fr(row.vat_cents.max(0) as u64, &row.currency))</td>
                            </tr>
                        }
                    </tbody>
                </table>
            </section>
        )
    })
}
