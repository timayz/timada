//! `/tenants/new`: any signed-in user creates a tenant and owns it. Only the
//! name is asked; the address (slug) derives from it.

use serde::Deserialize;
use timada_tenancy::{CreateTenant, MAX_NAME_LEN};
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
use crate::ui::{Shell, document, notice};

#[topcoat::router::query_params(error = bad_request)]
struct NewQuery {
    error: Option<String>,
    /// What was typed before an error, to fill the form again.
    name: Option<String>,
}

#[derive(serde::Serialize)]
struct Prefill<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
}

pub fn new_tenant_url(cx: &Cx, error: Option<&str>, name: Option<&str>) -> String {
    href!(new).query(Prefill { error, name }).resolve(cx)
}

#[page]
pub async fn new(cx: &Cx) -> Result<impl View> {
    require_user(cx).await?;
    let query = query_params::<NewQuery>(cx)?;

    Ok(view! {
        document(
            title: "Create a tenant",
            shell: Shell::page(""),
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Create a tenant"</h1>
            </header>
            notice(error: query.error.as_deref())

            <form method="post" action=(href!(submit)) class="space-y-3 rounded border border-neutral-200 bg-white p-4">
                <label class="block text-sm" for="name">"Name"</label>
                <input id="name" name="name" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                    type="text" maxlength=(MAX_NAME_LEN) required="" value=(query.name.as_deref().unwrap_or_default())>
                <p class="text-xs text-neutral-500">"The tenant's pages live at /t/" <em>"name-as-a-slug"</em> ", derived from the name (a short suffix is added when that address is taken). The address cannot be changed later."</p>
                <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Create tenant"</button>
            </form>
        )
    })
}

#[derive(Deserialize)]
struct NewForm {
    name: String,
}

#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<NewForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    match tenancy(cx)
        .create_tenant(CreateTenant {
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
            Some(&form.name),
        ))),
        Err(err) => Err(err.into()),
    }
}
