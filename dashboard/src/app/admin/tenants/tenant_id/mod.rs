//! `/admin/tenants/{tenant_id}`: one tenant and the actions on it.

pub mod resume;
pub mod suspend;

use std::sync::Arc;

use timada_tenancy::count_open_invitations_of_tenant;
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{error::not_found, href, module_param, page, path_param, query_params},
    view::{View, attributes, view},
};

use super::{status_label, status_tone};
use crate::app::admin::require_admin;
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, index as tenant_page};
use crate::app::tenants::tenancy;
use crate::auth::{now, unix_seconds, user_labels};
use crate::cluster::PeerState;
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonVariant, button};
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::ReadPool;
use crate::tenants::{TenantStatus, Tenants};
use crate::ui::{
    LINK, Shell, document, empty_row, fact, facts, format_time, icons, notice, page_header,
    section_card, status_badge,
};

module_param!(pub tenant_id);

#[topcoat::router::query_params(error = bad_request)]
struct TenantQuery {
    error: Option<String>,
    message: Option<String>,
}

/// How another node stands on this tenant, for the overview.
fn peer_label(peer: &PeerState) -> String {
    let silent = peer.silent_for.as_secs();
    if peer.given_up {
        format!(
            "no longer waited for, silent for {}",
            format_silence(silent)
        )
    } else if silent >= 10 {
        format!("not heard from for {}", format_silence(silent))
    } else {
        "in touch".to_owned()
    }
}

fn format_silence(secs: u64) -> String {
    if secs >= 2 * 24 * 3600 {
        format!("{} days", secs / (24 * 3600))
    } else if secs >= 2 * 3600 {
        format!("{} h", secs / 3600)
    } else if secs >= 120 {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
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
    let backlog = runtime.backlog(&tenant.id);
    let peers = runtime.peers(&tenant.id);
    let pending =
        count_open_invitations_of_tenant(app_context::<ReadPool>(cx), &tenant.id, now()).await?;
    let mut ids: Vec<String> = tenant.members.keys().cloned().collect();
    ids.push(tenant.created_by.clone());
    let labels = user_labels(cx, &ids).await?;
    let label_of = move |user_id: &str| -> String {
        labels
            .get(user_id)
            .cloned()
            .unwrap_or_else(|| user_id.to_owned())
    };
    let suspended = tenant.is_suspended();
    let title = format!("{} · Tenants", tenant.name);
    let pending_label = format!(
        "{pending} pending invitation{}",
        if pending == 1 { "" } else { "s" }
    );

    Ok(view! {
        document(
            title: &title,
            shell: Shell::page("admin_tenants"),
            page_header(
                title: &tenant.name,
                subtitle: Some("A tenant of the platform, as its administrators see it."),
                status_badge(tone: status_tone(suspended, status), (status_label(suspended, status)))
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some("Overview"),
                facts(
                    fact(term: "Address", <a class=(LINK) href=(href!(tenant_page, Slug(tenant.slug.clone())))><code>"/t/" (&tenant.slug)</code></a>)
                    fact(term: "Id", <code class="text-xs">(&tenant.id)</code>)
                    fact(term: "Created", <time datetime=(tenant.created_at)>(format_time(tenant.created_at))</time> " by " (label_of(&tenant.created_by)))
                    fact(
                        term: "Status",
                        attrs: attributes! { data-tenant-status="" },
                        (status_label(suspended, status))
                        if let TenantStatus::Running { since, leases } = status {
                            " since " <time datetime=(unix_seconds(since))>(format_time(unix_seconds(since)))</time>
                            if leases > 0 { " · " (leases) " store connection(s)" }
                        }
                        if let Some(reason) = &tenant.suspended { ": " (reason) }
                    )
                    if let Some((kept, limit)) = backlog {
                        fact(
                            term: "Writes kept",
                            attrs: attributes! { data-tenant-backlog=(kept) },
                            (kept) " of " (limit) " for nodes that are away"
                        )
                    }
                    if !peers.is_empty() {
                        fact(
                            term: "Other nodes",
                            <ul data-tenant-peers="" class="flex flex-col gap-1">
                                for peer in &peers {
                                    <li data-node=(peer.id) data-given-up=(if peer.given_up { "true" } else { "false" })>
                                        "Node " (peer.id) " · " (peer_label(peer))
                                    </li>
                                }
                            </ul>
                        )
                    }
                )
            )

            section_card(
                title: Some("Members"),
                <p class="mb-3 text-sm text-muted-foreground" data-pending-invitations="">(&pending_label)</p>
                table(
                    table_header(
                        table_row(
                            table_head("Member")
                            table_head("Role")
                        )
                    )
                    table_body(
                        if tenant.members.is_empty() {
                            empty_row(colspan: 2, text: "No member.")
                        }
                        for (user_id, role) in &tenant.members {
                            table_row(
                                table_cell(
                                    <a class=(LINK) href=(href!(crate::app::admin::users::user_id::user, crate::app::admin::users::user_id::UserId(user_id.clone())))>(label_of(user_id))</a>
                                )
                                table_cell(badge(variant: BadgeVariant::Secondary, (role)))
                            )
                        }
                    )
                )
            )

            section_card(
                title: Some("Administration"),
                description: Some(if suspended {
                    "Resuming reopens the tenant's database and brings its pages back."
                } else {
                    "Suspending closes the tenant's database and takes its pages offline until it is resumed."
                }),
                attrs: attributes! { class="border-destructive/30" },
                if suspended {
                    <form method="post" action=(href!(resume::resume, TenantId(tenant.id.clone())))>
                        button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, icon(data: icons::CHECK) "Resume tenant")
                    </form>
                } else {
                    <form method="post" action=(href!(suspend::suspend, TenantId(tenant.id.clone()))) class="flex flex-wrap items-end gap-3">
                        field(
                            attrs: attributes! { class="grow sm:max-w-md" },
                            field_label(attrs: attributes! { for="suspend-reason" }, "Reason")
                            input(attrs: attributes! { id="suspend-reason" name="reason" type="text" maxlength="200" required="" })
                        )
                        button(variant: ButtonVariant::Destructive, attrs: attributes! { type="submit" }, icon(data: icons::WARNING) "Suspend tenant")
                    </form>
                }
            )
        )
    })
}
