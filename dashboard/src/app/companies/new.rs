//! `/companies/new`: any signed-in user registers a company and owns it.
//! Coming from a tenant's invoicing page (`?tenant=slug`), the tenant is
//! linked to it right away, in the company's plain series.

use timada_tenancy::LegalIdentity;
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

use super::{IdentityForm, company_id, identity_fields};
use crate::app::t::slug::invoicing::invoicing_url;
use crate::app::tenants::{error_code, tenancy};
use crate::auth::require_user;
use crate::ui::{document, notice};

#[topcoat::router::query_params(error = bad_request)]
struct NewQuery {
    error: Option<String>,
    /// The tenant to link once the company exists.
    tenant: Option<String>,
}

#[derive(serde::Serialize)]
struct NewLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tenant: Option<&'a str>,
}

pub fn new_company_url(cx: &Cx, error: Option<&str>, tenant: Option<&str>) -> String {
    href!(new).query(NewLink { error, tenant }).resolve(cx)
}

#[page]
pub async fn new(cx: &Cx) -> Result<impl View> {
    require_user(cx).await?;
    let query = query_params::<NewQuery>(cx)?;
    let legal = LegalIdentity {
        country: "FR".to_owned(),
        ..LegalIdentity::default()
    };
    let tenant = query.tenant.clone().unwrap_or_default();
    Ok(view! {
        document(
            title: "Register a company",
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Register a company"</h1>
                <nav class="flex items-center gap-4 text-sm">
                    <a class="underline" href=(href!(super::index))>"Companies"</a>
                </nav>
            </header>
            notice(error: query.error.as_deref())
            <form method="post" action=(href!(create)) class="space-y-6">
                <input type="hidden" name="tenant" value=(&tenant)>
                identity_fields(legal: &legal)
                <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Register"</button>
            </form>
        )
    })
}

#[derive(serde::Deserialize)]
pub struct NewForm {
    #[serde(default)]
    tenant: String,
    #[serde(flatten)]
    identity: IdentityForm,
}

#[route(POST)]
pub async fn create(cx: &Cx, Form(form): Form<NewForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    let tenant = (!form.tenant.is_empty()).then_some(form.tenant.as_str());
    let identity = match form.identity.identity() {
        Ok(identity) => identity,
        Err(code) => return Ok(see_other(new_company_url(cx, Some(code), tenant))),
    };
    let gateway = tenancy(cx);
    let company = match gateway.create_company(&user.id, identity).await {
        Ok(company) => company,
        Err(err) if err.is_user_facing() => {
            return Ok(see_other(new_company_url(
                cx,
                Some(error_code(&err)),
                tenant,
            )));
        }
        Err(err) => return Err(err.into()),
    };
    let Some(slug) = tenant else {
        return Ok(see_other(
            href!(company_id::index, company_id::CompanyId(company.id.clone())).resolve(cx),
        ));
    };
    let Some(state) = gateway
        .load_tenant_by_slug(slug)
        .await
        .map_err(topcoat::Error::from_anyhow)?
    else {
        return Ok(see_other(super::companies_url(
            cx,
            None,
            Some("Company registered."),
        )));
    };
    match gateway
        .link_tenant(&user.id, &company.id, &state.id, None)
        .await
    {
        Ok(()) => Ok(see_other(invoicing_url(
            cx,
            slug,
            None,
            Some("Company registered and linked."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(invoicing_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
