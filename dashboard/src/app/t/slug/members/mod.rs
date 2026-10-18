//! `/t/{slug}/members`: who belongs to the tenant, the invitations still
//! waiting, and (with `members:manage`) inviting, moving members between
//! roles or removing them. Every member sees "Leave" on their own row;
//! [`history`] lists who did what.

pub mod change;
pub mod history;
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
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant, role_label};
use crate::app::auth::Flash;
use crate::auth::{now, user_labels};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::ReadPool;
use crate::ui::{
    Shell, document, empty_state, format_time, icons, notice, page_header, section_card,
};

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
            shell: Shell::tenant(&tenant, &nav, "members"),
            page_header(
                title: "Members",
                subtitle: Some("Who belongs to the tenant, and the invitations still waiting."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(href!(history::history, Slug(slug.clone())))>
                    icon(data: icons::CLOCK)
                    "History"
                </a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                table(
                    table_header(
                        table_row(
                            table_head("Member")
                            table_head("Role")
                            table_head(<span class="sr-only">"Actions"</span>)
                        )
                    )
                    table_body(
                        for (user_id, role) in &tenant.members {
                            table_row(
                                attrs: attributes! { data-member=(user_id) },
                                table_cell(
                                    <span class="font-medium">(&labels[user_id])</span>
                                    if user_id == &me { " " <span class="text-muted-foreground">"(you)"</span> }
                                )
                                table_cell(
                                    // Only owners touch owner seats; a manager sees them read-only.
                                    if can_manage && (is_owner || role != ROLE_OWNER) {
                                        <form method="post" action=(href!(change::change, Slug(slug.clone()))) class="flex items-center gap-2">
                                            <input type="hidden" name="user_id" value=(user_id)>
                                            select(
                                                attrs: attributes! { name="role" class="w-44" aria-label=(format!("Role of {}", labels[user_id])) },
                                                for (name, label) in &choices {
                                                    <option value=(name) selected=(name == role)>(label)</option>
                                                }
                                            )
                                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Change role")
                                        </form>
                                    } else {
                                        (role_label(&tenant, role))
                                    }
                                )
                                table_cell(
                                    attrs: attributes! { class="text-right" },
                                    if user_id == &me {
                                        <form method="post" action=(href!(revoke::revoke, Slug(slug.clone())))>
                                            <input type="hidden" name="user_id" value=(user_id)>
                                            button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Leave tenant")
                                        </form>
                                    } else if can_manage && (is_owner || role != ROLE_OWNER) {
                                        <form method="post" action=(href!(revoke::revoke, Slug(slug.clone())))>
                                            <input type="hidden" name="user_id" value=(user_id)>
                                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Remove")
                                        </form>
                                    }
                                )
                            )
                        }
                    )
                )
            )

            if can_manage {
                section_card(
                    title: Some("Invite a member"),
                    description: Some("The link in the e-mail is valid for 7 days. Whoever opens it signed in joins with that role."),
                    <form method="post" action=(href!(invite::invite, Slug(slug.clone()))) class="flex flex-wrap items-end gap-3">
                        field(
                            attrs: attributes! { class="grow" },
                            field_label(attrs: attributes! { for="invite-email" }, "E-mail address")
                            input(attrs: attributes! {
                                id="invite-email" name="email" type="email" maxlength=(MAX_EMAIL_LEN) required=""
                            })
                        )
                        field(
                            attrs: attributes! { class="w-48" },
                            field_label(attrs: attributes! { for="invite-role" }, "Invite as")
                            select(
                                attrs: attributes! { id="invite-role" name="role" },
                                for (name, label) in &choices {
                                    <option value=(name)>(label)</option>
                                }
                            )
                        )
                        button(attrs: attributes! { type="submit" }, "Send invitation")
                    </form>
                )
            }

            section_card(
                title: Some("Pending invitations"),
                if invitations.is_empty() {
                    empty_state(title: "None.", symbol: Some(icons::INVITATIONS))
                } else {
                    <ul class="divide-y divide-border text-sm">
                        for invitation in &invitations {
                            <li class="flex flex-wrap items-center justify-between gap-2 py-3" data-invitation-email=(&invitation.email)>
                                <span class="min-w-0">
                                    <span class="font-medium">(&invitation.email)</span>
                                    <span class="block text-xs text-muted-foreground">
                                        (role_label(&tenant, &invitation.role))
                                        " · invited by " (&labels[&invitation.invited_by])
                                        " · expires " <time datetime=(invitation.expires_at)>(format_time(invitation.expires_at as u64))</time>
                                    </span>
                                </span>
                                if can_manage {
                                    <form method="post" action=(href!(revoke_invitation::revoke_invitation, Slug(slug.clone())))>
                                        <input type="hidden" name="invitation_id" value=(&invitation.id)>
                                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Withdraw")
                                    </form>
                                }
                            </li>
                        }
                    </ul>
                }
            )
        )
    })
}
