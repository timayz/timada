//! `/t/{slug}/articles`: the shop's blog, for members with `pages:read`;
//! those with `pages:manage` write its articles in Markdown and publish
//! them, and the storefront lists the published ones, the most recent
//! first. The list comes from the shared read model (a moment behind); an
//! article's own screen reads its stream.

pub mod article_id;
pub mod new;

use timada_core::page::{Cursor, PageFrom};
use timada_pages::{Command, PageError, articles_of_tenant};
use timada_tenancy::{PERMISSION_PAGES_MANAGE, PERMISSION_PAGES_READ};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::pages::status_of;
use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::components::button::{ButtonSize, ButtonVariant, button_variants};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{
    LINK, Shell, cursor_pager, document, empty_row, format_date, notice, page_header, section_card,
    status_badge,
};

/// The list shows this many articles at once.
const PAGE_SIZE: i64 = 50;

#[topcoat::router::query_params(error = bad_request)]
struct ArticlesQuery {
    /// The cursor of the row the page starts after (older articles).
    after: Option<String>,
    /// The cursor of the row the page ends before (newer articles).
    before: Option<String>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct ArticlesLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<&'a str>,
}

/// The blog's list, with a flash error code or message.
pub fn articles_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

/// The blog's gateway in the tenant's streams (403 while suspended).
pub fn blog(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command::new(access.executor()?.clone(), access.tenant_id()))
}

/// The code of a refusal, for the flash.
pub fn error_code(err: &PageError) -> &'static str {
    err.code().unwrap_or("internal")
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_PAGES_READ).await?;
    let query = query_params::<ArticlesQuery>(cx)?;
    let after = query.after.as_deref().and_then(Cursor::parse);
    let before = query.before.as_deref().and_then(Cursor::parse);
    let from = PageFrom::of(after.as_ref(), before.as_ref());
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_PAGES_MANAGE).await?;
    let TenantAccess { tenant, .. } = access;
    let listed = articles_of_tenant(context::<Db>(cx), &tenant.id, from, PAGE_SIZE)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let slug = tenant.slug.clone();
    let page_link = |after: Option<&str>, before: Option<&str>| {
        href!(index, slug: slug.clone())
            .query(ArticlesLink { after, before })
            .resolve(cx)
    };
    let newer = listed
        .newer
        .as_ref()
        .map(|cursor| page_link(None, Some(&cursor.param())));
    let older = listed
        .older
        .as_ref()
        .map(|cursor| page_link(Some(&cursor.param()), None));
    let rows = listed.rows;
    let title = format!("Blog · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "articles"),
            page_header(
                title: "Blog",
                subtitle: Some("Articles the storefront lists at /blog, the most recent first: guides and news that bring visitors from search engines and lead them to your products."),
                if can_manage {
                    <a class=(button_variants(ButtonVariant::Primary, ButtonSize::Sm)) href=(href!(new::new, slug: slug.clone()))>"New article"</a>
                }
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                table(
                    table_header(
                        table_row(
                            table_head("Title")
                            table_head("Address")
                            table_head("Date")
                            table_head("Status")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 4, text: "No article yet.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-article=(&row.slug) data-article-status=(status_of(row.published).0) },
                                table_cell(<a class=(LINK) href=(href!(article_id::index, slug: slug.clone(), article_id::article_id: row.id.clone()))>(&row.title)</a>)
                                table_cell(<code class="text-xs">"/blog/" (&row.slug)</code>)
                                table_cell(
                                    if row.published_at > 0 {
                                        <time datetime=(format_date(row.published_at as u64))>(format_date(row.published_at as u64))</time>
                                    } else {
                                        <span class="text-muted-foreground">"Not published yet"</span>
                                    }
                                )
                                table_cell(status_badge(tone: status_of(row.published).1, (status_of(row.published).0)))
                            )
                        }
                    )
                )
                cursor_pager(summary: "Most recent first", newer: newer, older: older)
            )
        )
    })
}
