//! `/t/{slug}/members`: who belongs to the tenant, the invitations still
//! waiting, and (with `members:manage`) inviting, moving members between
//! roles or removing them. Every member sees "Leave" on their own row.

pub mod change;
pub mod invite;
pub mod revoke;
pub mod revoke_invitation;

use timada_tenancy::{
    MAX_EMAIL_LEN, PERMISSION_MEMBERS_MANAGE, PERMISSION_MEMBERS_READ, ROLE_OWNER,
    open_invitations_of_tenant,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant, role_label};
use crate::app::auth::Flash;
use crate::auth::{now, user_labels};
use crate::db::ReadPool;
use crate::ui::{document, format_time, notice};

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
    let invitations =
        open_invitations_of_tenant(app_context::<ReadPool>(cx), &tenant.id, now()).await?;
    let mut ids: Vec<String> = tenant.members.keys().cloned().collect();
    ids.extend(invitations.iter().map(|i| i.invited_by.clone()));
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
                    if nav.products {
                        <a class="underline" href=(href!(super::products::index, Slug(slug.clone())))>"Products"</a>
                    }
                    if nav.roles {
                        <a class="underline" href=(href!(super::roles::index, Slug(slug.clone())))>"Roles"</a>
                    }
                    if nav.api_keys {
                        <a class="underline" href=(href!(super::api_keys::index, Slug(slug.clone())))>"API keys"</a>
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

            if can_manage {
                <section class="mt-8 rounded border border-neutral-200 bg-white p-4">
                    <h2 class="mb-2 text-lg font-medium">"Invite a member"</h2>
                    <form method="post" action=(href!(invite::invite, Slug(slug.clone()))) class="flex flex-wrap items-end gap-2">
                        <div class="grow">
                            <label class="block text-sm" for="invite-email">"E-mail address"</label>
                            <input id="invite-email" name="email" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                                type="email" maxlength=(MAX_EMAIL_LEN) required="">
                        </div>
                        <div>
                            <label class="block text-sm" for="invite-role">"Invite as"</label>
                            <select id="invite-role" name="role" class="mt-1 rounded border border-neutral-300 px-2 py-2">
                                for (name, label) in &choices {
                                    <option value=(name)>(label)</option>
                                }
                            </select>
                        </div>
                        <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Send invitation"</button>
                    </form>
                    <p class="mt-2 text-xs text-neutral-500">"The link in the e-mail is valid for 7 days. Whoever opens it signed in joins with that role."</p>
                </section>
            }

            <section class="mt-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Pending invitations"</h2>
                if invitations.is_empty() {
                    <p class="text-sm text-neutral-500">"None."</p>
                } else {
                    <ul class="divide-y divide-neutral-100 text-sm">
                        for invitation in &invitations {
                            <li class="flex items-center justify-between py-2" data-invitation-email=(&invitation.email)>
                                <span>
                                    (&invitation.email)
                                    <span class="text-neutral-500">
                                        " · " (role_label(&tenant, &invitation.role))
                                        " · invited by " (&labels[&invitation.invited_by])
                                        " · expires " <time datetime=(invitation.expires_at)>(format_time(invitation.expires_at as u64))</time>
                                    </span>
                                </span>
                                if can_manage {
                                    <form method="post" action=(href!(revoke_invitation::revoke_invitation, Slug(slug.clone())))>
                                        <input type="hidden" name="invitation_id" value=(&invitation.id)>
                                        <button class="rounded border border-neutral-300 px-2 py-1 text-xs hover:bg-neutral-100">"Withdraw"</button>
                                    </form>
                                }
                            </li>
                        }
                    </ul>
                }
            </section>
        )
    })
}
