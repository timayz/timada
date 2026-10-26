use serde::Deserialize;
use timada_catalog::parse_amount;
use timada_discounts::CreateOffer;
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

use super::offers_url;
use crate::app::t::slug::discounts::{discounts, error_code};
use crate::app::t::slug::require_tenant;
use crate::ui::parse_date;

#[derive(Deserialize)]
struct CreateForm {
    /// `product` or `product:variant`.
    item: String,
    price: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    requires_product_id: String,
    #[serde(default)]
    min_subtotal: String,
    #[serde(default)]
    starts_on: String,
    #[serde(default)]
    ends_on: String,
}

const DAY_SECS: u64 = 86_400;

impl CreateForm {
    /// The command the form describes, without its currency (the
    /// product's), or the code of what is wrong with it.
    fn command(self) -> std::result::Result<CreateOffer, &'static str> {
        let (product_id, variant_id) = match self.item.trim().split_once(':') {
            Some((product, variant)) => (product.to_owned(), Some(variant.to_owned())),
            None => (self.item.trim().to_owned(), None),
        };
        let price_cents = parse_amount(&self.price).map_err(|_| "invalid_offer_price")?;
        let min_subtotal_cents = match self.min_subtotal.trim() {
            "" => None,
            min => Some(parse_amount(min).map_err(|_| "invalid_min_subtotal")?),
        };
        let day = |value: &str| match value.trim() {
            "" => Ok(None),
            value => parse_date(value).map(Some).ok_or("invalid_date"),
        };
        let starts_at = day(&self.starts_on)?;
        // The last day is included: the offer ends at the next midnight.
        let ends_at = day(&self.ends_on)?.map(|last| last + DAY_SECS);
        Ok(CreateOffer {
            product_id,
            variant_id,
            price_cents,
            currency: String::new(),
            title: Some(self.title),
            requires_product_id: Some(self.requires_product_id),
            min_subtotal_cents,
            starts_at,
            ends_at,
        })
    }
}

/// Creates an offer, priced in its product's currency, and comes back to
/// the list.
#[route(POST)]
pub async fn create(cx: &Cx, Form(form): Form<CreateForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_DISCOUNTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let mut cmd = match form.command() {
        Ok(cmd) => cmd,
        Err(code) => return Ok(see_other(offers_url(cx, slug, Some(code), None))),
    };
    let product = timada_catalog::Command::new(access.executor()?.clone(), access.tenant_id())
        .load_product(&cmd.product_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    // A product gone meanwhile is refused by the command.
    cmd.currency = product
        .as_ref()
        .and_then(|product| product.currency())
        .unwrap_or("EUR")
        .to_owned();
    match discounts(&access)?.create_offer(&access.user_id, cmd).await {
        Ok(_) => Ok(see_other(offers_url(
            cx,
            slug,
            None,
            Some("Offer created."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(offers_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
