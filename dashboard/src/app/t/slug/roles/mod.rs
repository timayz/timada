//! `/t/{slug}/roles`: the tenant's custom roles and what each one may do,
//! for members with `roles:manage`. `owner` is listed read-only.

pub mod define;
pub mod remove;
pub mod update;

use std::collections::HashMap;

use timada_tenancy::{MAX_ROLE_LABEL_LEN, PERMISSION_ROLES_MANAGE, PERMISSIONS, ROLE_OWNER};
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::ui::{describe_permission, document, notice};

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
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Roles · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Roles"</h1>
                <nav class="flex items-center gap-4 text-sm">
                    <a class="underline" href=(href!(super::index, Slug(slug.clone())))>(&tenant.name)</a>
                    if nav.members {
                        <a class="underline" href=(href!(super::members::index, Slug(slug.clone())))>"Members"</a>
                    }
                    if nav.api_keys {
                        <a class="underline" href=(href!(super::api_keys::index, Slug(slug.clone())))>"API keys"</a>
                    }
                    <a class="underline" href=(href!(crate::app::home))>"Dashboard"</a>
                </nav>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4" data-role=(ROLE_OWNER)>
                <h2 class="text-lg font-medium">"Owner" <code class="ml-2 text-sm font-normal text-neutral-500">(ROLE_OWNER)</code></h2>
                <p class="mt-1 text-sm text-neutral-500">
                    "Built in: holds every permission, and alone hands out or takes back owner seats. "
                    (tenant.owners()) " member(s)."
                </p>
            </section>

            for (name, def) in &tenant.roles {
                <section class="mb-8 rounded border border-neutral-200 bg-white p-4" data-role=(name)>
                    <form method="post" action=(href!(update::update, Slug(slug.clone()))) class="space-y-3">
                        <input type="hidden" name="role" value=(name)>
                        <div class="flex items-center justify-between">
                            <h2 class="text-lg font-medium">(&def.label) <code class="ml-2 text-sm font-normal text-neutral-500">(name)</code></h2>
                            <span class="text-sm text-neutral-500">(tenant.members_with_role(name)) " member(s)"</span>
                        </div>
                        <label class="block text-sm" for=(format!("label-{name}"))>"Label"</label>
                        <input id=(format!("label-{name}")) name="label" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                            type="text" maxlength=(MAX_ROLE_LABEL_LEN) required="" value=(&def.label)>
                        <fieldset class="space-y-1 text-sm">
                            <legend class="mb-1 text-sm">"Permissions"</legend>
                            for permission in PERMISSIONS {
                                <label class="flex items-center gap-2">
                                    <input type="checkbox" name=(format!("{PERMISSION_FIELD_PREFIX}{permission}")) value="on" checked=(def.permissions.contains(*permission))>
                                    <span>(describe_permission(permission)) " " <code class="text-neutral-500">(permission)</code></span>
                                </label>
                            }
                        </fieldset>
                        <button class="rounded border border-neutral-300 px-3 py-1 text-sm hover:bg-neutral-100">"Save role"</button>
                    </form>
                    <form method="post" action=(href!(remove::remove, Slug(slug.clone()))) class="mt-3">
                        <input type="hidden" name="role" value=(name)>
                        <button class="rounded border border-red-700 px-3 py-1 text-sm text-red-700 hover:bg-red-50">"Remove role"</button>
                    </form>
                </section>
            }

            <section class="rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Define a role"</h2>
                <form method="post" action=(href!(define::define, Slug(slug.clone()))) class="space-y-3">
                    <label class="block text-sm" for="new-role">"Role name"</label>
                    <input id="new-role" name="role" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                        type="text" maxlength=(timada_tenancy::MAX_SLUG_LEN) required="" placeholder="support">
                    <p class="text-xs text-neutral-500">"Lowercase letters, digits and dashes; it cannot be changed later."</p>
                    <label class="block text-sm" for="new-label">"Label"</label>
                    <input id="new-label" name="label" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                        type="text" maxlength=(MAX_ROLE_LABEL_LEN) required="" placeholder="Support team">
                    <fieldset class="space-y-1 text-sm">
                        <legend class="mb-1 text-sm">"Permissions"</legend>
                        for permission in PERMISSIONS {
                            <label class="flex items-center gap-2">
                                <input type="checkbox" name=(format!("{PERMISSION_FIELD_PREFIX}{permission}")) value="on">
                                <span>(describe_permission(permission)) " " <code class="text-neutral-500">(permission)</code></span>
                            </label>
                        }
                    </fieldset>
                    <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Define role"</button>
                </form>
            </section>
        )
    })
}
