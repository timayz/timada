//! `/t/{slug}/redirects`: the old addresses the storefront still answers,
//! each with a permanent redirect to where its content is now, for members
//! with `pages:read`; those with `pages:manage` add and delete them. For a
//! shop moved from another platform: a product or a page renamed here is
//! redirected without being listed. Read from the tenant's own stream, so a
//! change shows at once here; the storefront follows a moment later.

pub mod add;
pub mod redirect_id;

use serde::Deserialize;
use timada_pages::{MAX_REDIRECT_PATH_LEN, MAX_REDIRECTS};
use timada_tenancy::{PERMISSION_PAGES_MANAGE, PERMISSION_PAGES_READ};
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::announcements::pages;
use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::app::auth::Flash;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::ui::{Shell, document, empty_row, notice, page_header, section_card};

#[topcoat::router::query_params(error = bad_request)]
struct RedirectsQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The list, with a flash error code or message.
pub fn redirects_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

/// A redirect as the merchant fills it in.
#[derive(Deserialize)]
pub struct RedirectForm {
    from: String,
    to: String,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_PAGES_READ).await?;
    let query = query_params::<RedirectsQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_PAGES_MANAGE).await?;
    let redirects = pages(&access)?
        .redirects()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Redirects · {}", tenant.name);
    let full = redirects.len() >= MAX_REDIRECTS;
    let columns = if can_manage { 3 } else { 2 };

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "redirects"),
            page_header(
                title: "Redirects",
                subtitle: Some("Old addresses the storefront sends on for good, so links and search engines follow. A product or a page you rename is redirected without being listed here."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                table(
                    table_header(
                        table_row(
                            table_head("Old path")
                            table_head("Sends to")
                            if can_manage {
                                table_head(<span class="sr-only">"Actions"</span>)
                            }
                        )
                    )
                    table_body(
                        if redirects.is_empty() {
                            empty_row(colspan: columns, text: "No redirect.")
                        }
                        for redirect in &redirects {
                            table_row(
                                attrs: attributes! { data-redirect=(&redirect.from) },
                                table_cell(attrs: attributes! { class="whitespace-normal break-all" }, <code>(&redirect.from)</code>)
                                table_cell(attrs: attributes! { class="whitespace-normal break-all" data-redirect-to="" }, <code>(&redirect.to)</code>)
                                if can_manage {
                                    table_cell(
                                        <div class="flex justify-end">
                                            <form method="post" action=(href!(redirect_id::remove::remove, slug: slug.clone(), redirect_id::redirect_id: redirect.id.clone()))>
                                                button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" aria-label=(format!("Delete the redirect from {}", redirect.from)) }, "Delete")
                                            </form>
                                        </div>
                                    )
                                }
                            )
                        }
                    )
                )
            )

            if can_manage {
                section_card(
                    title: Some("Add a redirect"),
                    if full {
                        <p class="text-sm text-muted-foreground">"A shop keeps " (MAX_REDIRECTS) " redirects. Delete one to add another."</p>
                    } else {
                        <form method="post" action=(href!(add::add, slug: slug.clone())) class="flex max-w-2xl flex-col gap-4" data-redirect-form="">
                            field(
                                field_label(attrs: attributes! { for="redirect-from" }, "Old path")
                                field_description(
                                    attrs: attributes! { id="redirect-from-hint" },
                                    "As the old address had it after the domain, without what follows a " <code>"?"</code> ": "
                                    <code>"/products/blue-mug"</code> ". Write an accent as the address bar copies it (" <code>"%C3%A9"</code> ")."
                                )
                                input(attrs: attributes! {
                                    id="redirect-from" name="from" type="text" required="" maxlength=(MAX_REDIRECT_PATH_LEN)
                                    aria-describedby="redirect-from-hint" placeholder="/products/blue-mug"
                                })
                            )
                            field(
                                field_label(attrs: attributes! { for="redirect-to" }, "Sends to")
                                field_description(attrs: attributes! { id="redirect-to-hint" }, "A path of the storefront: " <code>"/mugs/blue-mug"</code> ". Send each old address to its own page, not all of them to the home.")
                                input(attrs: attributes! {
                                    id="redirect-to" name="to" type="text" required="" maxlength=(MAX_REDIRECT_PATH_LEN)
                                    aria-describedby="redirect-to-hint" placeholder="/mugs/blue-mug"
                                })
                            )
                            <div>button(attrs: attributes! { type="submit" }, "Add redirect")</div>
                        </form>
                    }
                )
            }
        )
    })
}
