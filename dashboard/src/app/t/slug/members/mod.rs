//! `/t/{slug}/members`: who belongs to the tenant, and (with
//! `members:manage`) moving them between roles or removing them. Every
//! member sees the page's "Leave" on their own row.

pub mod change;
pub mod revoke;

use timada_tenancy::{PERMISSION_MEMBERS_MANAGE, PERMISSION_MEMBERS_READ, ROLE_OWNER};
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant, role_label};
use crate::app::auth::Flash;
use crate::auth::user_labels;
use crate::ui::{document, notice};

#[topcoat::router::query_params(error = bad_request)]
struct MembersQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn members_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MEMBERS_READ)
        .await?;
    let query = query_params::<MembersQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_MEMBERS_MANAGE).await?;
    let is_owner = access.is_owner();
    let TenantAccess {
        tenant,
        user_id: me,
        ..
    } = access;
    let ids: Vec<String> = tenant.members.keys().cloned().collect();
    let labels = user_labels(cx, &ids).await?;
    let slug = tenant.slug.clone();
    // The roles a manager can move someone to: owner only for owners.
    let choices: Vec<(String, String)> = std::iter::once(ROLE_OWNER)
        .filter(|_| is_owner)
        .map(|r| (r.to_owned(), r.to_owned()))
        .chain(
            tenant
                .roles
                .iter()
                .map(|(name, def)| (name.clone(), def.label.clone())),
        )
        .collect();
    let title = format!("Members · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Members"</h1>
                <nav class="flex items-center gap-4 text-sm">
                    <a class="underline" href=(href!(super::index, Slug(slug.clone())))>(&tenant.name)</a>
                    if nav.roles {
                        <a class="underline" href=(href!(super::roles::index, Slug(slug.clone())))>"Roles"</a>
                    }
                    <a class="underline" href=(href!(crate::app::home))>"Dashboard"</a>
                </nav>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="rounded border border-neutral-200 bg-white p-4">
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Member"</th>
                            <th class="py-2 font-medium">"Role"</th>
                            <th class="py-2 font-medium"><span class="sr-only">"Actions"</span></th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        for (user_id, role) in &tenant.members {
                            <tr data-member=(user_id)>
                                <td class="py-2">
                                    (&labels[user_id])
                                    if user_id == &me { " " <span class="text-neutral-500">"(you)"</span> }
                                </td>
                                <td class="py-2">
                                    // Only owners touch owner seats; a manager sees them read-only.
                                    if can_manage && (is_owner || role != ROLE_OWNER) {
                                        <form method="post" action=(href!(change::change, Slug(slug.clone()))) class="flex items-center gap-2">
                                            <input type="hidden" name="user_id" value=(user_id)>
                                            <select name="role" class="rounded border border-neutral-300 px-2 py-1" aria-label=(format!("Role of {}", labels[user_id]))>
                                                for (name, label) in &choices {
                                                    <option value=(name) selected=(name == role)>(label)</option>
                                                }
                                            </select>
                                            <button class="rounded border border-neutral-300 px-2 py-1 text-xs hover:bg-neutral-100">"Change role"</button>
                                        </form>
                                    } else {
                                        (role_label(&tenant, role))
                                    }
                                </td>
                                <td class="py-2 text-right">
                                    if user_id == &me {
                                        <form method="post" action=(href!(revoke::revoke, Slug(slug.clone())))>
                                            <input type="hidden" name="user_id" value=(user_id)>
                                            <button class="rounded border border-red-700 px-2 py-1 text-xs text-red-700 hover:bg-red-50">"Leave tenant"</button>
                                        </form>
                                    } else if can_manage && (is_owner || role != ROLE_OWNER) {
                                        <form method="post" action=(href!(revoke::revoke, Slug(slug.clone())))>
                                            <input type="hidden" name="user_id" value=(user_id)>
                                            <button class="rounded border border-neutral-300 px-2 py-1 text-xs hover:bg-neutral-100">"Remove"</button>
                                        </form>
                                    }
                                </td>
                            </tr>
                        }
                    </tbody>
                </table>
            </section>
        )
    })
}
