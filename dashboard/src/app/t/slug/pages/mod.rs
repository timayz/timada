//! `/t/{slug}/pages`: the shop's own pages (legal notice, terms of sale,
//! privacy policy, about us), for members with `pages:read`; those with
//! `pages:manage` write them in Markdown and publish them; the storefront
//! shows each one where it links to it, by its address. The list comes from the shared read model (a
//! moment behind); a page's own screen reads its stream.

pub mod new;
pub mod page_id;

use timada_pages::{Command, PageError, pages_of_tenant};
use timada_tenancy::{PERMISSION_PAGES_MANAGE, PERMISSION_PAGES_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::components::button::{ButtonSize, ButtonVariant, button_variants};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{
    LINK, Shell, Tone, document, empty_row, format_time, notice, page_header, section_card,
    status_badge,
};

#[topcoat::router::query_params(error = bad_request)]
struct PagesQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The pages list, with a flash error code or message.
pub fn pages_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The pages gateway in the tenant's streams (403 while suspended).
pub fn pages(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command::new(access.executor()?.clone(), access.tenant_id()))
}

/// The code of a refusal, for the flash.
pub fn error_code(err: &PageError) -> &'static str {
    err.code().unwrap_or("internal")
}

/// Where a page stands, in words, and its tone.
pub fn status_of(published: bool) -> (&'static str, Tone) {
    if published {
        ("Published", Tone::Success)
    } else {
        ("Draft", Tone::Neutral)
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_PAGES_READ).await?;
    let query = query_params::<PagesQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_PAGES_MANAGE).await?;
    let TenantAccess { tenant, .. } = access;
    let rows = pages_of_tenant(app_context::<Db>(cx), &tenant.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let slug = tenant.slug.clone();
    let title = format!("Pages · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "pages"),
            page_header(
                title: "Pages",
                subtitle: Some("The shop's own pages: legal notice, terms of sale, privacy, about us. The storefront shows the published ones where it links to them, by their address."),
                if can_manage {
                    <a class=(button_variants(ButtonVariant::Primary, ButtonSize::Sm)) href=(href!(new::new, Slug(slug.clone())))>"New page"</a>
                }
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                <p class="mb-4 text-sm text-muted-foreground" data-page-count=(rows.len())>(rows.len()) " page(s)"</p>
                table(
                    table_header(
                        table_row(
                            table_head("Title")
                            table_head("Address")
                            table_head("Updated")
                            table_head("Status")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 4, text: "No page yet.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-page=(&row.slug) data-page-status=(status_of(row.published).0) },
                                table_cell(<a class=(LINK) href=(href!(page_id::index, Slug(slug.clone()), page_id::PageId(row.id.clone())))>(&row.title)</a>)
                                table_cell(<code class="text-xs">"/pages/" (&row.slug)</code>)
                                table_cell(<time datetime=(row.updated_at)>(format_time(row.updated_at as u64))</time>)
                                table_cell(status_badge(tone: status_of(row.published).1, (status_of(row.published).0)))
                            )
                        }
                    )
                )
            )
        )
    })
}
