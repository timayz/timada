use serde::Deserialize;
use timada_catalog::parse_amount;
use timada_discounts::{CreateDiscount, DiscountValue, parse_percent};
use timada_tenancy::PERMISSION_DISCOUNTS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::discount_id::discount_url;
use super::{discounts, discounts_url, error_code};
use crate::app::t::slug::require_tenant;
use crate::ui::parse_date;

#[derive(Deserialize)]
struct CreateForm {
    code: String,
    kind: String,
    value: String,
    #[serde(default)]
    currency: String,
    #[serde(default)]
    starts_on: String,
    #[serde(default)]
    ends_on: String,
    #[serde(default)]
    max_uses: String,
    #[serde(default)]
    once_per_email: Option<String>,
}

const DAY_SECS: u64 = 86_400;

impl CreateForm {
    /// The command the form describes, or the code of what is wrong with it.
    fn command(self) -> std::result::Result<CreateDiscount, &'static str> {
        let value = match self.kind.as_str() {
            "percent" => {
                DiscountValue::Percent(parse_percent(&self.value).map_err(|err| error_code(&err))?)
            }
            "amount" => DiscountValue::Amount {
                cents: parse_amount(&self.value).map_err(|_| "invalid_discount_amount")?,
                currency: self.currency,
            },
            _ => return Err("invalid_value"),
        };
        let day = |value: &str| match value.trim() {
            "" => Ok(None),
            value => parse_date(value).map(Some).ok_or("invalid_date"),
        };
        let starts_at = day(&self.starts_on)?;
        // The last day is included: the discount ends at the next midnight.
        let ends_at = day(&self.ends_on)?.map(|last| last + DAY_SECS);
        let max_uses = match self.max_uses.trim() {
            "" => None,
            max => Some(max.parse().map_err(|_| "invalid_max_uses")?),
        };
        Ok(CreateDiscount {
            code: self.code,
            value,
            starts_at,
            ends_at,
            max_uses,
            once_per_email: self.once_per_email.is_some(),
        })
    }
}

/// Creates a discount and lands on its page.
#[route(POST)]
pub async fn create(cx: &Cx, Form(form): Form<CreateForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_DISCOUNTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let cmd = match form.command() {
        Ok(cmd) => cmd,
        Err(code) => return Ok(see_other(discounts_url(cx, slug, Some(code), None))),
    };
    match discounts(&access)?
        .create_discount(&access.user_id, cmd)
        .await
    {
        Ok(discount) => Ok(see_other(discount_url(
            cx,
            slug,
            &discount.id,
            None,
            Some("Discount created."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(discounts_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
