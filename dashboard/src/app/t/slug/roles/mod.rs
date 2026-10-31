//! `/t/{slug}/roles`: the tenant's custom roles and what each one may do,
//! for members with `roles:manage`. Owners alone define, edit and remove
//! them; the others see them read-only. `owner` is listed read-only for all.

pub mod define;
pub mod remove;
pub mod update;

use std::collections::HashMap;

use timada_tenancy::{MAX_ROLE_LABEL_LEN, PERMISSION_ROLES_MANAGE, PERMISSIONS, ROLE_OWNER};
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::ui::{Shell, describe_permission, document, notice, page_header, section_card};

/// The form field of a permission checkbox: `perm:<permission>`.
const PERMISSION_FIELD_PREFIX: &str = "perm:";

#[topcoat::router::query_params(error = bad_request)]
struct RolesQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn roles_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The permissions ticked in a posted role form.
pub fn checked_permissions(form: &HashMap<String, String>) -> Vec<String> {
    form.keys()
        .filter_map(|key| key.strip_prefix(PERMISSION_FIELD_PREFIX))
        .map(str::to_owned)
        .collect()
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ROLES_MANAGE)
        .await?;
    let query = query_params::<RolesQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let is_owner = access.is_owner();
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Roles · {}", tenant.name);
    let subtitle = if is_owner {
        "What each kind of member may do."
    } else {
        "What each kind of member may do. Only the owners change the roles."
    };

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "roles"),
            page_header(title: "Roles", subtitle: Some(subtitle))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                attrs: attributes! { data-role=(ROLE_OWNER) },
                <h2 class="text-base font-semibold">"Owner" <code class="ml-2 text-sm font-normal text-muted-foreground">(ROLE_OWNER)</code></h2>
                <p class="mt-1 text-sm text-muted-foreground">
                    "Built in: holds every permission, and alone hands out or takes back owner seats. "
                    (tenant.owners()) " member(s)."
                </p>
            )

            for (name, def) in &tenant.roles {
                section_card(
                    attrs: attributes! { data-role=(name) },
                    if !is_owner {
                        <div class="flex flex-wrap items-center justify-between gap-2">
                            <h2 class="text-base font-semibold">(&def.label) <code class="ml-2 text-sm font-normal text-muted-foreground">(name)</code></h2>
                            <span class="text-sm text-muted-foreground">(tenant.members_with_role(name)) " member(s)"</span>
                        </div>
                        <ul class="mt-3 flex flex-col gap-1.5 text-sm">
                            for permission in &def.permissions {
                                <li>(describe_permission(permission)) " " <code class="text-muted-foreground">(permission)</code></li>
                            }
                        </ul>
                    } else {
                    <form method="post" action=(href!(update::update, Slug(slug.clone()))) class="flex flex-col gap-4">
                        <input type="hidden" name="role" value=(name)>
                        <div class="flex flex-wrap items-center justify-between gap-2">
                            <h2 class="text-base font-semibold">(&def.label) <code class="ml-2 text-sm font-normal text-muted-foreground">(name)</code></h2>
                            <span class="text-sm text-muted-foreground">(tenant.members_with_role(name)) " member(s)"</span>
                        </div>
                        field(
                            attrs: attributes! { class="max-w-xl" },
                            field_label(attrs: attributes! { for=(format!("label-{name}")) }, "Label")
                            input(attrs: attributes! {
                                id=(format!("label-{name}")) name="label" type="text"
                                maxlength=(MAX_ROLE_LABEL_LEN) required="" value=(&def.label)
                            })
                        )
                        <fieldset class="flex flex-col gap-1.5 text-sm">
                            <legend class="mb-2 text-sm font-medium">"Permissions"</legend>
                            for permission in PERMISSIONS {
                                <label class="flex items-center gap-2">
                                    checkbox(attrs: attributes! { name=(format!("{PERMISSION_FIELD_PREFIX}{permission}")) value="on" checked=(def.permissions.contains(*permission)) })
                                    <span>(describe_permission(permission)) " " <code class="text-muted-foreground">(permission)</code></span>
                                </label>
                            }
                        </fieldset>
                        <div>
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save role")
                        </div>
                    </form>
                    <form method="post" action=(href!(remove::remove, Slug(slug.clone()))) class="mt-4">
                        <input type="hidden" name="role" value=(name)>
                        button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Remove role")
                    </form>
                    }
                )
            }

            if is_owner {
            section_card(
                title: Some("Define a role"),
                <form method="post" action=(href!(define::define, Slug(slug.clone()))) class="flex flex-col gap-4">
                    <div class="grid gap-4 md:grid-cols-2">
                        field(
                            field_label(attrs: attributes! { for="new-role" }, "Role name")
                            input(attrs: attributes! {
                                id="new-role" name="role" type="text"
                                maxlength=(timada_tenancy::MAX_SLUG_LEN) required="" placeholder="support"
                            })
                            <p class="text-xs text-muted-foreground">"Lowercase letters, digits and dashes; it cannot be changed later."</p>
                        )
                        field(
                            field_label(attrs: attributes! { for="new-label" }, "Label")
                            input(attrs: attributes! {
                                id="new-label" name="label" type="text"
                                maxlength=(MAX_ROLE_LABEL_LEN) required="" placeholder="Support team"
                            })
                        )
                    </div>
                    <fieldset class="flex flex-col gap-1.5 text-sm">
                        <legend class="mb-2 text-sm font-medium">"Permissions"</legend>
                        for permission in PERMISSIONS {
                            <label class="flex items-center gap-2">
                                checkbox(attrs: attributes! { name=(format!("{PERMISSION_FIELD_PREFIX}{permission}")) value="on" })
                                <span>(describe_permission(permission)) " " <code class="text-muted-foreground">(permission)</code></span>
                            </label>
                        }
                    </fieldset>
                    <div>
                        button(attrs: attributes! { type="submit" }, "Define role")
                    </div>
                </form>
            )
            }
        )
    })
}
