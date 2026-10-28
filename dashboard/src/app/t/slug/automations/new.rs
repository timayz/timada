//! `/t/{slug}/automations/new`, for members with `automations:manage`: a
//! scenario named, then drawn on its own page.

use timada_tenancy::PERMISSION_AUTOMATIONS_MANAGE;
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

use super::scenario_id::scenario_url;
use super::{automations, automations_url};
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, TenantNav, require_tenant};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::ui::{Shell, document, notice, page_header, section_card};

#[topcoat::router::query_params(error = bad_request)]
struct NewQuery {
    error: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct NewForm {
    name: String,
}

#[page]
pub async fn new(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_AUTOMATIONS_MANAGE)
        .await?;
    let query = query_params::<NewQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let tenant = access.tenant;
    let title = format!("New scenario · Automations · {}", tenant.name);
    let action = href!(create, Slug(tenant.slug.clone())).resolve(cx);
    let back = automations_url(cx, &tenant.slug, None, None);
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "automations"),
            page_header(
                title: "New scenario",
                subtitle: Some("Name it; you draw its trigger and steps next. Nothing runs until it is published."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Automations"</a>
            )
            notice(error: query.error.as_deref())
            section_card(
                <form method="post" action=(action) class="flex max-w-xl flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="scenario-name" }, "Name")
                        input(attrs: attributes! {
                            id="scenario-name" name="name" required="" maxlength="80"
                            placeholder="Thank big orders" autocomplete="off"
                        })
                    )
                    <div>
                        button(attrs: attributes! { type="submit" }, "Create scenario")
                    </div>
                </form>
            )
        )
    })
}

#[route(POST)]
pub async fn create(cx: &Cx, Form(form): Form<NewForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_AUTOMATIONS_MANAGE)
        .await?;
    let slug = access.tenant.slug.clone();
    match automations(&access)?
        .create_scenario(&access.user_id, &form.name)
        .await
    {
        Ok(id) => Ok(see_other(scenario_url(cx, &slug, &id, None, None))),
        Err(err) if err.is_user_facing() => Ok(see_other(
            href!(new, Slug(slug))
                .query(Flash {
                    error: err.code(),
                    message: None,
                })
                .resolve(cx),
        )),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
