use serde::Deserialize;
use timada_catalog::{CatalogError, MarginTiers};
use timada_tenancy::PERMISSION_PRODUCTS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{costs_url, parse_percent};
use crate::app::t::slug::products::{catalog, error_code};
use crate::app::t::slug::require_tenant;

#[derive(Deserialize)]
pub struct TiersForm {
    medium_from: String,
    high_from: String,
    /// The label's number, or empty for none.
    #[serde(default)]
    feed_label: String,
}

impl TiersForm {
    fn tiers(&self) -> Option<MarginTiers> {
        let feed_label = match self.feed_label.trim() {
            "" => None,
            label => Some(label.parse().ok()?),
        };
        Some(MarginTiers {
            medium_from_bps: parse_percent(&self.medium_from)?,
            high_from_bps: parse_percent(&self.high_from)?,
            feed_label,
        })
    }
}

/// Says how the shop grades its margins, and which label of the feed
/// carries the tier.
#[route(POST)]
pub async fn tiers(cx: &Cx, Form(form): Form<TiersForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let set = match form.tiers() {
        Some(tiers) => {
            catalog(&access)?
                .set_margin_tiers(&access.user_id, tiers)
                .await
        }
        None => Err(CatalogError::InvalidMarginTiers),
    };
    match set {
        Ok(()) => Ok(see_other(costs_url(cx, slug, None, Some("Tiers saved.")))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(costs_url(cx, slug, Some(error_code(&err)), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
