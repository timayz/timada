//! `/admin/tenants`: every tenant on the platform.

pub mod tenant_id;

use timada_tenancy::{count_tenants, list_tenants};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use crate::app::admin::require_admin;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::input::input;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::ReadPool;
use crate::ui::{
    LINK, Shell, Tone, document, empty_row, filter_bar, format_time, notice, page_header, pager,
    section_card, status_badge,
};

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
pub fn status_label(suspended: bool) -> &'static str {
    if suspended { "Suspended" } else { "Active" }
}

/// The colour of that word.
pub fn status_tone(suspended: bool) -> Tone {
    if suspended {
        Tone::Warning
    } else {
        Tone::Success
    }
}

#[page]
pub async fn tenants(cx: &Cx) -> Result<impl View> {
    require_admin(cx).await?;
    let query = query_params::<TenantsQuery>(cx)?;
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let page_number = query.page.unwrap_or(1).max(1);
    let pool = app_context::<ReadPool>(cx);
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
    let summary = format!("{total} tenant{}", if total == 1 { "" } else { "s" });

    Ok(view! {
        document(
            title: "Tenants",
            shell: Shell::page("admin_tenants"),
            page_header(title: "Tenants", subtitle: Some("Every tenant on the platform."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                filter_bar(
                    <label class="sr-only" for="tenants-q">"Search tenants"</label>
                    input(attrs: attributes! {
                        id="tenants-q" name="q" type="search" value=(q.unwrap_or(""))
                        class="sm:max-w-xs" placeholder="Slug or name"
                    })
                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Search")
                    <span class="ml-auto text-sm text-muted-foreground">(&summary)</span>
                )
                table(
                    table_header(
                        table_row(
                            table_head("Name")
                            table_head("Slug")
                            table_head(attrs: attributes! { class="text-right" }, "Members")
                            table_head("Created")
                            table_head("Status")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 5, text: "No tenant matches.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { class=(if row.suspended { "text-muted-foreground" } else { "" }) },
                                table_cell(
                                    <a class=(LINK) href=(href!(tenant_id::index, tenant_id::TenantId(row.id.clone())))>(&row.name)</a>
                                )
                                table_cell(<code class="text-xs">(&row.slug)</code>)
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (row.member_count))
                                table_cell(<time datetime=(row.created_at)>(format_time(row.created_at as u64))</time>)
                                table_cell(
                                    status_badge(
                                        tone: status_tone(row.suspended),
                                        attrs: attributes! { data-tenant-status="" },
                                        (status_label(row.suspended))
                                    )
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
