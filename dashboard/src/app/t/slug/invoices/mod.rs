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
    view::{View, attributes, class, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::cluster::Executor;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::ReadPool;
use crate::ui::{
    LINK, Shell, Tone, document, empty_row, format_time, page_header, pager, section_card,
    status_badge,
};

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

/// The invoicing gateway in the tenant's streams (403 while suspended).
pub fn invoicing(access: &TenantAccess) -> Result<timada_invoicing::Command<Executor>> {
    Ok(timada_invoicing::Command::new(
        access.executor()?.clone(),
        access.tenant_id(),
    ))
}

/// How a document is taxed, as a page says it.
pub fn regime_label(regime: &str, country: &str) -> String {
    match regime {
        "domestic" => "France".to_owned(),
        "distance_origin" => format!("France (sold to {country})"),
        "oss" => format!("{country}, One-Stop Shop"),
        "reverse_charge" => format!("Reverse charge ({country})"),
        "export" => format!("Export ({country})"),
        "franchise" => "Exempt".to_owned(),
        other => other.to_owned(),
    }
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
            shell: Shell::tenant(&tenant, &nav, "invoices"),
            page_header(title: "Invoices", subtitle: Some("The invoices and credit notes the shop issued."))

            section_card(
                description: Some("Every paid order gets an invoice, every refund a credit note, numbered in one series per year and kind. They never change once issued. The company's One-Stop Shop return is on its page."),
                table(
                    table_header(
                        table_row(
                            table_head("Number")
                            table_head("Kind")
                            table_head("Issued")
                            table_head("Order")
                            table_head("Customer")
                            table_head("VAT")
                            table_head(attrs: attributes! { class="text-right" }, "Total")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 7, text: "No invoice yet.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-document=(&row.label) },
                                table_cell(
                                    <a class=(class!(LINK, "font-mono")) href=(pdf_href(cx, &slug, &row.id))>(&row.label)</a>
                                )
                                table_cell(
                                    status_badge(
                                        tone: if row.is_credit_note() { Tone::Warning } else { Tone::Success },
                                        (if row.is_credit_note() { "Credit note" } else { "Invoice" })
                                    )
                                )
                                table_cell(<time datetime=(row.issued_at)>(format_time(row.issued_at as u64))</time>)
                                table_cell(
                                    <a class=(class!(LINK, "font-mono")) href=(href!(super::orders::order_id::index, Slug(slug.clone()), super::orders::order_id::OrderId(row.order_id.clone())))>(&row.order_reference)</a>
                                )
                                table_cell((&row.buyer_name))
                                table_cell(attrs: attributes! { data-document-regime=(&row.regime) }, (regime_label(&row.regime, &row.tax_country)))
                                table_cell(
                                    attrs: attributes! { class="text-right tabular-nums" },
                                    if row.is_credit_note() { "−" }
                                    (row.total_label())
                                )
                            )
                        }
                    )
                )
                pager(
                    page: page_number,
                    pages: pages,
                    previous: (page_number > 1).then(|| page_link(page_number - 1)),
                    next: (page_number < pages).then(|| page_link(page_number + 1)),
                )
            )
        )
    })
}
