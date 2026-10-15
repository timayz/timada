//! `/t/{slug}/invoices`: the invoices and credit notes the tenant issued,
//! for members with `orders:read` (an accountant's view of the sales). The
//! list comes from the shared read model (a moment behind); each document
//! downloads as the PDF it was issued as.

pub mod document_id;

use timada_invoicing::{count_documents, documents_of_tenant};
use timada_tenancy::PERMISSION_ORDERS_READ;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::cluster::Executor;
use crate::db::ReadPool;
use crate::ui::{document, format_time};

const PAGE_SIZE: i64 = 50;

#[topcoat::router::query_params(error = bad_request)]
struct InvoicesQuery {
    /// 1-based.
    page: Option<i64>,
}

#[derive(serde::Serialize)]
struct InvoicesLink {
    page: i64,
}

/// The invoicing gateway over the tenant's own store (403 while suspended).
pub fn invoicing(access: &TenantAccess) -> Result<timada_invoicing::Command<Executor>> {
    Ok(timada_invoicing::Command(access.executor()?.clone()))
}

/// Where a document's PDF downloads from.
pub fn pdf_href(cx: &Cx, slug: &str, document_id: &str) -> String {
    href!(
        document_id::pdf::pdf,
        Slug(slug.to_owned()),
        document_id::DocumentId(document_id.to_owned())
    )
    .resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let query = query_params::<InvoicesQuery>(cx)?;
    let page_number = query.page.unwrap_or(1).max(1);
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<ReadPool>(cx);
    let total = count_documents(pool, &tenant.id).await?;
    let rows =
        documents_of_tenant(pool, &tenant.id, (page_number - 1) * PAGE_SIZE, PAGE_SIZE).await?;
    let pages = (total + PAGE_SIZE - 1) / PAGE_SIZE;
    let slug = tenant.slug.clone();
    let page_link = {
        let slug = slug.clone();
        move |page: i64| {
            href!(index, Slug(slug.clone()))
                .query(InvoicesLink { page })
                .resolve(cx)
        }
    };
    let title = format!("Invoices · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Invoices"</h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "invoices")
            </header>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <p class="mb-4 text-sm text-neutral-600">
                    "Every paid order gets an invoice, every refund a credit note, numbered in one series per year and kind. They never change once issued."
                </p>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Number"</th>
                            <th class="py-2 font-medium">"Kind"</th>
                            <th class="py-2 font-medium">"Issued"</th>
                            <th class="py-2 font-medium">"Order"</th>
                            <th class="py-2 font-medium">"Customer"</th>
                            <th class="py-2 text-right font-medium">"Total"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        if rows.is_empty() {
                            <tr><td class="py-2 text-neutral-500" colspan="6">"No invoice yet."</td></tr>
                        }
                        for row in &rows {
                            <tr data-document=(&row.label)>
                                <td class="py-2">
                                    <a class="font-mono underline" href=(pdf_href(cx, &slug, &row.id))>(&row.label)</a>
                                </td>
                                <td class="py-2">(if row.is_credit_note() { "Credit note" } else { "Invoice" })</td>
                                <td class="py-2"><time datetime=(row.issued_at)>(format_time(row.issued_at as u64))</time></td>
                                <td class="py-2">
                                    <a class="font-mono underline" href=(href!(super::orders::order_id::index, Slug(slug.clone()), super::orders::order_id::OrderId(row.order_id.clone())))>(&row.order_reference)</a>
                                </td>
                                <td class="py-2">(&row.buyer_name)</td>
                                <td class="py-2 text-right">
                                    if row.is_credit_note() { "−" }
                                    (row.total_label())
                                </td>
                            </tr>
                        }
                    </tbody>
                </table>
                if pages > 1 {
                    <p class="mt-4 flex items-center gap-4 text-sm">
                        if page_number > 1 { <a class="underline" href=(page_link(page_number - 1))>"Previous"</a> }
                        <span>"Page " (page_number) " of " (pages)</span>
                        if page_number < pages { <a class="underline" href=(page_link(page_number + 1))>"Next"</a> }
                    </p>
                }
            </section>
        )
    })
}
