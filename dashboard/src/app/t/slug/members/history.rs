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
    view::{View, view},
};

use super::super::{Slug, TenantNav, require_tenant, role_label, tenant_nav};
use crate::auth::user_labels;
use crate::db::ReadPool;
use crate::ui::{document, format_time};

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
        .flat_map(|row| [row.actor.clone(), row.user_id.clone()])
        .flatten()
        .collect();
    let labels = user_labels(cx, &ids).await?;
    let entries: Vec<(i64, String, String)> = rows
        .iter()
        .map(|row| {
            let actor = row
                .actor
                .as_ref()
                .map_or_else(|| "—".to_owned(), |id| labels[id].clone());
            (row.at, actor, describe(&tenant, row, &labels))
        })
        .collect();
    let members_link = href!(super::index, Slug(tenant.slug.clone())).resolve(cx);
    let paged = query.before.is_some();
    let newest_link = href!(history, Slug(tenant.slug.clone())).resolve(cx);
    let title = format!("Members history · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Members history"</h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "members")
            </header>
            <p class="mb-4 text-sm"><a class="underline" href=(members_link)>"Back to members"</a></p>

            <section class="rounded border border-neutral-200 bg-white p-4">
                if entries.is_empty() {
                    <p class="text-sm text-neutral-500">"Nothing yet."</p>
                } else {
                    <table class="w-full text-left text-sm">
                        <thead>
                            <tr class="border-b border-neutral-200 text-neutral-500">
                                <th class="py-2 font-medium">"When"</th>
                                <th class="py-2 font-medium">"Who"</th>
                                <th class="py-2 font-medium">"What"</th>
                            </tr>
                        </thead>
                        <tbody class="divide-y divide-neutral-100">
                            for (at, actor, what) in &entries {
                                <tr data-audit-row="">
                                    <td class="py-2 whitespace-nowrap text-neutral-500">
                                        <time datetime=(at)>(format_time(*at as u64))</time>
                                    </td>
                                    <td class="py-2">(actor)</td>
                                    <td class="py-2">(what)</td>
                                </tr>
                            }
                        </tbody>
                    </table>
                }
                if paged || older.is_some() {
                    <p class="mt-4 flex items-center gap-4 text-sm">
                        if paged { <a class="underline" href=(&newest_link)>"Newest"</a> }
                        if let Some(older) = &older { <a class="underline" href=(older)>"Older"</a> }
                    </p>
                }
            </section>
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
    let by_self = row.actor.is_some() && row.actor == row.user_id;
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
