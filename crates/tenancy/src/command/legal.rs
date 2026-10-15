use crate::error::TenancyError;
use crate::value_object::{
    MAX_LEGAL_CITY_LEN, MAX_LEGAL_FORM_LEN, MAX_LEGAL_LINE_LEN, MAX_LEGAL_NAME_LEN,
    MAX_LEGAL_POSTAL_CODE_LEN, normalize_email, normalize_legal_optional, normalize_legal_text,
    normalize_siret, normalize_vat_number,
};

/// Who a company is in law: what its invoices must say about it. The
/// commands take it as typed and the state holds it normalized.
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
    /// Registered for the EU One-Stop Shop: other EU countries' VAT from the
    /// first euro of sales to their consumers. Needs a `vat_number`.
    pub oss_registered: bool,
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

pub(crate) fn normalize_identity(identity: LegalIdentity) -> Result<LegalIdentity, TenancyError> {
    let siret = normalize_siret(&identity.siret)?;
    let vat_number = normalize_legal_optional(identity.vat_number.as_deref(), MAX_LEGAL_FORM_LEN)?
        .map(|number| normalize_vat_number(&number, &siret))
        .transpose()?;
    if identity.oss_registered && vat_number.is_none() {
        return Err(TenancyError::OssWithoutVatNumber);
    }
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
        oss_registered: identity.oss_registered,
    })
}
