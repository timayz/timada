use serde::Deserialize;
use timada_tenancy::{LegalIdentity, PERMISSION_TENANT_MANAGE};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::invoicing_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

#[derive(Deserialize)]
pub struct LegalForm {
    legal_name: String,
    legal_form: String,
    share_capital: String,
    siret: String,
    rcs_city: String,
    contact_email: String,
    line1: String,
    line2: String,
    postal_code: String,
    city: String,
    country: String,
    vat_number: String,
    default_vat_rate_bps: String,
}

/// Saves the legal identity; the crate checks the SIRET and the VAT number.
#[route(POST)]
pub async fn save(cx: &Cx, Form(form): Form<LegalForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_TENANT_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let share_capital_cents = match form.share_capital.trim() {
        "" => None,
        capital => match timada_catalog::parse_amount(capital) {
            Ok(cents) => Some(cents),
            Err(_) => {
                return Ok(see_other(invoicing_url(
                    cx,
                    slug,
                    Some("invalid_share_capital"),
                    None,
                )));
            }
        },
    };
    let Ok(default_vat_rate_bps) = form.default_vat_rate_bps.trim().parse::<u32>() else {
        return Ok(see_other(invoicing_url(
            cx,
            slug,
            Some("invalid_vat_rate"),
            None,
        )));
    };
    let identity = LegalIdentity {
        legal_name: form.legal_name,
        legal_form: form.legal_form,
        share_capital_cents,
        siret: form.siret,
        rcs_city: Some(form.rcs_city),
        line1: form.line1,
        line2: Some(form.line2),
        postal_code: form.postal_code,
        city: form.city,
        country: form.country,
        contact_email: form.contact_email,
        vat_number: Some(form.vat_number),
        default_vat_rate_bps: Some(default_vat_rate_bps),
    };
    match tenancy(cx)
        .set_legal_identity(&access.user_id, &access.tenant.id, identity)
        .await
    {
        Ok(()) => Ok(see_other(invoicing_url(
            cx,
            slug,
            None,
            Some("Legal identity saved."),
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
