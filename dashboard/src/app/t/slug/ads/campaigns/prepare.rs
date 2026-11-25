use serde::Deserialize;
use timada_google::{
    CAMPAIGN_AD_GROUPS, CAMPAIGN_BID, CAMPAIGN_COUNTRIES, CAMPAIGN_DAILY_BUDGET,
    CAMPAIGN_DESCRIPTIONS, CAMPAIGN_HEADLINES, CAMPAIGN_KEYWORDS, CAMPAIGN_NAME, CAMPAIGN_PHONE,
    CAMPAIGN_SHOPPING, CAMPAIGN_START, CAMPAIGN_TABLET, campaign_fields, plan_of,
};
use timada_proposals::{KIND_ADS_CAMPAIGN, waiting_about};
use timada_tenancy::PERMISSION_ADS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, not_found, see_other},
        route,
    },
};

use super::{campaigns_url, known};
use crate::app::t::slug::proposals::proposal_id::proposal_url;
use crate::app::t::slug::require_tenant;
use crate::auth::now;
use crate::campaigns::{SOURCE, ad_groups_of, ready};
use crate::db::Db;
use crate::proposals::{Draft, Refusal};

#[derive(Deserialize)]
pub struct CampaignForm {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    daily_budget: String,
    #[serde(default)]
    bid: String,
    #[serde(default)]
    countries: String,
    #[serde(default)]
    phone_adjustment: String,
    #[serde(default)]
    tablet_adjustment: String,
    #[serde(default)]
    start: String,
    #[serde(default)]
    ad_groups: String,
    #[serde(default)]
    keywords: String,
    #[serde(default)]
    headlines: String,
    #[serde(default)]
    descriptions: String,
}

impl CampaignForm {
    fn value(&self, field: &str) -> &str {
        match field {
            CAMPAIGN_NAME => &self.name,
            CAMPAIGN_DAILY_BUDGET => &self.daily_budget,
            CAMPAIGN_BID => &self.bid,
            CAMPAIGN_COUNTRIES => &self.countries,
            CAMPAIGN_PHONE => &self.phone_adjustment,
            CAMPAIGN_TABLET => &self.tablet_adjustment,
            CAMPAIGN_START => &self.start,
            CAMPAIGN_AD_GROUPS => &self.ad_groups,
            CAMPAIGN_KEYWORDS => &self.keywords,
            CAMPAIGN_HEADLINES => &self.headlines,
            CAMPAIGN_DESCRIPTIONS => &self.descriptions,
            _ => "",
        }
    }
}

/// Prepares a campaign as a proposal, with the settings the member typed:
/// nothing is sent to Google Ads until a member approves it. Refused, with
/// what is missing, while the shop is not ready.
#[route(POST)]
pub async fn prepare(cx: &Cx, Form(form): Form<CampaignForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_ADS_MANAGE).await?;
    let kind = known(&form.kind).ok_or_else(not_found)?;
    let slug = access.tenant.slug.as_str();
    let store = access.executor()?;
    let db = context::<Db>(cx);
    let failed = topcoat::Error::from_anyhow;
    let refused = |code: &str| Ok(see_other(campaigns_url(cx, slug, Some(code), None)));
    let ready = ready(store, db, &access.tenant, kind)
        .await
        .map_err(failed)?;
    if !ready.is_ready() {
        return refused("campaign_not_ready");
    }
    // One at a time: a second would be approved into the first's place.
    if waiting_about(db, access.tenant_id(), KIND_ADS_CAMPAIGN, kind)
        .await
        .map_err(failed)?
        .is_some()
    {
        return refused("campaign_waits");
    }
    let values: Vec<(String, String)> = campaign_fields(kind)
        .iter()
        .map(|field| ((*field).to_owned(), form.value(field).to_owned()))
        .collect();
    let mut reason = match kind {
        CAMPAIGN_SHOPPING => "The shop's first campaign: Standard Shopping with manual bids, one ad group a category through the feed's label.".to_owned(),
        _ => "A Search campaign on the shop's own name.".to_owned(),
    };
    if kind == CAMPAIGN_SHOPPING {
        // Only the shop's own categories that may be advertised: a line
        // typed by hand is not taken on trust.
        let categories = timada_catalog::categories_of_tenant(db, access.tenant_id())
            .await
            .map_err(failed)?;
        let (allowed, left_out) = ad_groups_of(&categories);
        let plan = match plan_of(
            kind,
            values
                .iter()
                .map(|(field, value)| (field.as_str(), value.as_str())),
        ) {
            Ok(plan) => plan,
            Err(err) => return refused(err.code().unwrap_or("google_invalid_campaign")),
        };
        if plan
            .ad_groups
            .iter()
            .any(|group| !allowed.iter().any(|(id, _)| *id == group.category_id))
        {
            return refused("google_invalid_ad_groups");
        }
        reason.push_str(&format!(" {} ad groups.", plan.ad_groups.len()));
        if !left_out.is_empty() {
            reason.push_str(&format!(
                " Kept out of ads, so left out: {}.",
                left_out.join(", ")
            ));
        }
    }
    let prepared = crate::proposals::prepare(
        store,
        access.tenant_id(),
        &access.user_id,
        Draft {
            kind: KIND_ADS_CAMPAIGN.to_owned(),
            subject_id: kind.to_owned(),
            values,
            reason,
            source: SOURCE.to_owned(),
            valid_until: None,
        },
        now(),
    )
    .await;
    match prepared {
        Ok(id) => Ok(see_other(proposal_url(
            cx,
            slug,
            &id,
            None,
            Some(
                "The campaign is prepared. Read it, change what you want, and approve it to create it in Google Ads.",
            ),
        ))),
        Err(Refusal::Other(err)) => Err(failed(err)),
        Err(err) => match err.code() {
            Some(code) => refused(code),
            None => Err(topcoat::Error::from_anyhow(anyhow::anyhow!(err))),
        },
    }
}
