//! What the dashboard adds to the catalog's margins: the VAT in a price.
//! The catalog knows what things cost and the invoicing what the law
//! taxes; a margin needs both, before any sale says where it goes, so it
//! is worked out with the rate of a sale at home.

use std::collections::BTreeMap;

use timada_catalog::{FeedMargin, SaleCostsState, TAX_CATEGORIES};
use timada_invoicing::tax::{TaxProfile, home_rate_bps};
use timada_tenancy::TenantState;

use crate::cluster::Executor;

/// The tenant as far as VAT goes: exempt until the company it sells as has
/// a VAT number.
pub async fn tax_profile(store: &Executor, tenant: &TenantState) -> anyhow::Result<TaxProfile> {
    let legal = timada_tenancy::Command(store.clone())
        .load_company_of(tenant)
        .await?
        .and_then(|company| company.identity);
    Ok(legal.map_or_else(TaxProfile::default, |legal| TaxProfile {
        vat_registered: legal.is_vat_registered(),
        oss_registered: legal.oss_registered,
    }))
}

/// The VAT in the price of each kind of goods sold at home, in basis
/// points.
pub fn home_vat(profile: TaxProfile) -> BTreeMap<String, u32> {
    TAX_CATEGORIES
        .iter()
        .map(|(category, _)| ((*category).to_owned(), home_rate_bps(profile, category)))
        .collect()
}

/// How the tenant's feed grades its items' margins, when the shop gave one
/// of the feed's labels to them.
pub async fn feed_margin(
    store: &Executor,
    tenant: &TenantState,
    costs: &SaleCostsState,
) -> anyhow::Result<Option<FeedMargin>> {
    let tiers = costs.tiers();
    let Some(label) = tiers.feed_label else {
        return Ok(None);
    };
    Ok(Some(FeedMargin {
        label: usize::from(label),
        tiers,
        costs: costs.costs.clone(),
        vat_bps: home_vat(tax_profile(store, tenant).await?),
    }))
}
