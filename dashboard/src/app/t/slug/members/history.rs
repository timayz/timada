//! `/t/{slug}/members/history`: who invited, added, moved or removed whom,
//! and when, newest first. Anyone with `members:read` reads it.

use timada_tenancy::{
    AUDIT_GRANTED, AUDIT_INVITATION_ACCEPTED, AUDIT_INVITATION_REVOKED, AUDIT_INVITED,
    AUDIT_REVOKED, AUDIT_ROLE_CHANGED, MemberAuditRow, PERMISSION_MEMBERS_READ, TenantState,
    member_audit_of_tenant,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::super::{Slug, TenantNav, require_tenant, role_label};
use crate::auth::user_labels;
use crate::components::button::{ButtonSize, ButtonVariant, button_variants};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::ReadPool;
use crate::ui::{Shell, document, empty_state, format_time, page_header, section_card};

const PAGE_SIZE: i64 = 50;

#[topcoat::router::query_params(error = bad_request)]
struct HistoryQuery {
    /// The last row of the previous page; the rows older than it are shown.
    before: Option<i64>,
}

#[derive(serde::Serialize)]
struct HistoryLink {
    before: i64,
}

#[page]
pub async fn history(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MEMBERS_READ)
        .await?;
    let query = query_params::<HistoryQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let tenant = access.tenant;
    // One more than shown tells whether an older page exists.
    let mut rows = member_audit_of_tenant(
        app_context::<ReadPool>(cx),
        &tenant.id,
        query.before,
        PAGE_SIZE + 1,
    )
    .await?;
    let older = (rows.len() as i64 > PAGE_SIZE).then(|| {
        rows.truncate(PAGE_SIZE as usize);
        href!(history, Slug(tenant.slug.clone()))
            .query(HistoryLink {
                before: rows[rows.len() - 1].seq,
            })
            .resolve(cx)
    });
    let ids: Vec<String> = rows
        .iter()
        .flat_map(|row| [Some(row.actor.clone()), row.user_id.clone()])
        .flatten()
        .collect();
    let labels = user_labels(cx, &ids).await?;
    let entries: Vec<(i64, String, String)> = rows
        .iter()
        .map(|row| {
            (
                row.at,
                labels[&row.actor].clone(),
                describe(&tenant, row, &labels),
            )
        })
        .collect();
    let members_link = href!(super::index, Slug(tenant.slug.clone())).resolve(cx);
    let paged = query.before.is_some();
    let newest_link = href!(history, Slug(tenant.slug.clone())).resolve(cx);
    let title = format!("Members history · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "members"),
            page_header(
                title: "Members history",
                subtitle: Some("Who invited, added, moved or removed whom, newest first."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(members_link)>"All members"</a>
            )

            section_card(
                if entries.is_empty() {
                    empty_state(title: "Nothing yet.")
                } else {
                    table(
                        table_header(
                            table_row(
                                table_head("When")
                                table_head("Who")
                                table_head("What")
                            )
                        )
                        table_body(
                            for (at, actor, what) in &entries {
                                table_row(
                                    attrs: attributes! { data-audit-row="" },
                                    table_cell(
                                        attrs: attributes! { class="text-muted-foreground" },
                                        <time datetime=(at)>(format_time(*at as u64))</time>
                                    )
                                    table_cell(attrs: attributes! { class="font-medium" }, (actor))
                                    table_cell(attrs: attributes! { class="whitespace-normal" }, (what))
                                )
                            }
                        )
                    )
                }
                if paged || older.is_some() {
                    <p class="mt-4 flex items-center gap-2 text-sm">
                        if paged {
                            <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(&newest_link)>"Newest"</a>
                        }
                        if let Some(older) = &older {
                            <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(older)>"Older"</a>
                        }
                    </p>
                }
            )
        )
    })
}

/// What a row says happened, from the point of view of its actor.
fn describe(
    tenant: &TenantState,
    row: &MemberAuditRow,
    labels: &std::collections::HashMap<String, String>,
) -> String {
    let member = row
        .user_id
        .as_ref()
        .map(|id| labels[id].clone())
        .unwrap_or_default();
    let role = row
        .role
        .as_deref()
        .map(|role| role_label(tenant, role))
        .unwrap_or_default();
    let email = row.email.as_deref().unwrap_or_default();
    let by_self = row.user_id.as_deref() == Some(row.actor.as_str());
    match row.action.as_str() {
        AUDIT_GRANTED if by_self => format!("joined as {role}"),
        AUDIT_GRANTED => format!("added {member} as {role}"),
        AUDIT_ROLE_CHANGED => format!("moved {member} to {role}"),
        AUDIT_REVOKED if by_self => "left".to_owned(),
        AUDIT_REVOKED => format!("removed {member}"),
        AUDIT_INVITED => format!("invited {email} as {role}"),
        AUDIT_INVITATION_ACCEPTED => format!("accepted the invitation sent to {email}"),
        AUDIT_INVITATION_REVOKED => format!("withdrew the invitation to {email}"),
        other => other.to_owned(),
    }
}
