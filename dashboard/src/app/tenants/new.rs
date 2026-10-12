//! `/tenants/new`: any signed-in user creates a tenant and owns it.

use serde::Deserialize;
use timada_tenancy::{CreateTenant, MAX_NAME_LEN, MAX_SLUG_LEN, MIN_SLUG_LEN};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, page, query_params, route,
    },
    view::{View, view},
};

use super::{error_code, tenancy};
use crate::app::t::slug::tenant_url;
use crate::auth::require_user;
use crate::ui::{document, notice};

#[topcoat::router::query_params(error = bad_request)]
struct NewQuery {
    error: Option<String>,
    /// What was typed before an error, to fill the form again.
    slug: Option<String>,
    name: Option<String>,
}

#[derive(serde::Serialize)]
struct Prefill<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slug: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
}

pub fn new_tenant_url(
    cx: &Cx,
    error: Option<&str>,
    slug: Option<&str>,
    name: Option<&str>,
) -> String {
    href!(new).query(Prefill { error, slug, name }).resolve(cx)
}

#[page]
pub async fn new(cx: &Cx) -> Result<impl View> {
    require_user(cx).await?;
    let query = query_params::<NewQuery>(cx)?;

    Ok(view! {
        document(
            title: "Create a tenant",
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Create a tenant"</h1>
                <nav class="flex items-center gap-4 text-sm">
                    <a class="underline" href=(href!(crate::app::home))>"Dashboard"</a>
                </nav>
            </header>
            notice(error: query.error.as_deref())

            <form method="post" action=(href!(submit)) class="space-y-3 rounded border border-neutral-200 bg-white p-4">
                <label class="block text-sm" for="name">"Name"</label>
                <input id="name" name="name" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                    type="text" maxlength=(MAX_NAME_LEN) required="" value=(query.name.as_deref().unwrap_or_default())>
                <label class="block text-sm" for="slug">"Slug"</label>
                <input id="slug" name="slug" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                    type="text" minlength=(MIN_SLUG_LEN) maxlength=(MAX_SLUG_LEN) pattern="[A-Za-z0-9][A-Za-z0-9-]*[A-Za-z0-9]"
                    autocapitalize="off" spellcheck="false" required="" value=(query.slug.as_deref().unwrap_or_default())>
                <p class="text-xs text-neutral-500">"The address of the tenant's pages: /t/" <em>"slug"</em> ". Lowercase letters, digits and dashes; it cannot be changed later."</p>
                <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Create tenant"</button>
            </form>
        )
    })
}

#[derive(Deserialize)]
struct NewForm {
    slug: String,
    name: String,
}

#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<NewForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    match tenancy(cx)
        .create_tenant(CreateTenant {
            slug: form.slug.clone(),
            name: form.name.clone(),
            owner_user_id: user.id.clone(),
        })
        .await
    {
        Ok(tenant) => Ok(see_other(tenant_url(
            cx,
            &tenant.slug,
            None,
            Some("Tenant created."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(new_tenant_url(
            cx,
            Some(error_code(&err)),
            Some(&form.slug),
            Some(&form.name),
        ))),
        Err(err) => Err(err.into()),
    }
}
