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
    view::{View, attributes, view},
};

use super::{error_code, tenancy};
use crate::app::t::slug::tenant_url;
use crate::auth::require_user;
use crate::components::button::button;
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::ui::{Shell, document, notice, page_header, section_card};

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
            page_header(title: "Create a tenant", subtitle: Some("A shop of your own on the platform; you become its owner."))
            notice(error: query.error.as_deref())

            section_card(
                title: Some("Name"),
                description: Some("What members and the platform see the tenant under. It can be renamed later."),
                attrs: attributes! { class="max-w-xl" },
                <form method="post" action=(href!(submit)) class="flex flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="name" }, "Name")
                        input(attrs: attributes! {
                            id="name" name="name" type="text" maxlength=(MAX_NAME_LEN) required=""
                            value=(query.name.as_deref().unwrap_or_default())
                        })
                        field_description(
                            "The tenant's pages live at /t/" <em>"name-as-a-slug"</em> ", derived from the name (a short suffix is added when that address is taken). The address cannot be changed later."
                        )
                    )
                    <div>
                        button(attrs: attributes! { type="submit" }, "Create tenant")
                    </div>
                </form>
            )
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
