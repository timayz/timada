use evento::{Executor, ProjectionAggregate};

use crate::aggregator::LegalIdentitySet;
use crate::error::TenancyError;
use crate::value_object::{
    MAX_LEGAL_CITY_LEN, MAX_LEGAL_FORM_LEN, MAX_LEGAL_LINE_LEN, MAX_LEGAL_NAME_LEN,
    MAX_LEGAL_POSTAL_CODE_LEN, PERMISSION_TENANT_MANAGE, normalize_email, normalize_legal_optional,
    normalize_legal_text, normalize_siret, normalize_vat_number, normalize_vat_rate,
};

/// Who a merchant is in law: what its invoices must say about it. The
/// command takes it as typed and the state holds it normalized.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegalIdentity {
    pub legal_name: String,
    pub legal_form: String,
    /// In euro cents.
    pub share_capital_cents: Option<u64>,
    pub siret: String,
    pub rcs_city: Option<String>,
    pub line1: String,
    pub line2: Option<String>,
    pub postal_code: String,
    pub city: String,
    /// ISO 3166-1 alpha-2, uppercase.
    pub country: String,
    pub contact_email: String,
    /// `None` under the VAT exemption for small businesses.
    pub vat_number: Option<String>,
    /// The rate of a product that names none, in basis points; set exactly
    /// when `vat_number` is.
    pub default_vat_rate_bps: Option<u32>,
}

impl LegalIdentity {
    /// The first nine digits of the SIRET.
    pub fn siren(&self) -> &str {
        &self.siret[..9.min(self.siret.len())]
    }

    /// Whether the merchant charges VAT (as opposed to the exemption).
    pub fn is_vat_registered(&self) -> bool {
        self.vat_number.is_some()
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Sets (or replaces) the legal identity. Needs `tenant:manage`; the
    /// same identity is a no-op. A VAT number needs a default rate and the
    /// exemption needs none.
    pub async fn set_legal_identity(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        identity: LegalIdentity,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let identity = normalize_identity(identity)?;
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_TENANT_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if tenant.legal.as_ref() == Some(&identity) {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&LegalIdentitySet {
                legal_name: identity.legal_name,
                legal_form: identity.legal_form,
                share_capital_cents: identity.share_capital_cents,
                siret: identity.siret,
                rcs_city: identity.rcs_city,
                line1: identity.line1,
                line2: identity.line2,
                postal_code: identity.postal_code,
                city: identity.city,
                country: identity.country,
                contact_email: identity.contact_email,
                vat_number: identity.vat_number,
                default_vat_rate_bps: identity.default_vat_rate_bps,
                set_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}

fn normalize_identity(identity: LegalIdentity) -> Result<LegalIdentity, TenancyError> {
    let siret = normalize_siret(&identity.siret)?;
    let (vat_number, default_vat_rate_bps) = match (
        normalize_legal_optional(identity.vat_number.as_deref(), MAX_LEGAL_FORM_LEN)?,
        identity.default_vat_rate_bps,
    ) {
        (None, _) => (None, None),
        (Some(number), Some(rate)) => (
            Some(normalize_vat_number(&number, &siret)?),
            Some(normalize_vat_rate(rate)?),
        ),
        (Some(_), None) => return Err(TenancyError::InvalidVatRate),
    };
    let country = identity.country.trim().to_ascii_uppercase();
    if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(TenancyError::InvalidLegalIdentity);
    }
    Ok(LegalIdentity {
        legal_name: normalize_legal_text(&identity.legal_name, MAX_LEGAL_NAME_LEN)?,
        legal_form: normalize_legal_text(&identity.legal_form, MAX_LEGAL_FORM_LEN)?,
        share_capital_cents: identity.share_capital_cents,
        siret,
        rcs_city: normalize_legal_optional(identity.rcs_city.as_deref(), MAX_LEGAL_CITY_LEN)?,
        line1: normalize_legal_text(&identity.line1, MAX_LEGAL_LINE_LEN)?,
        line2: normalize_legal_optional(identity.line2.as_deref(), MAX_LEGAL_LINE_LEN)?,
        postal_code: normalize_legal_text(&identity.postal_code, MAX_LEGAL_POSTAL_CODE_LEN)?,
        city: normalize_legal_text(&identity.city, MAX_LEGAL_CITY_LEN)?,
        country,
        contact_email: normalize_email(&identity.contact_email)?,
        vat_number,
        default_vat_rate_bps,
    })
}
