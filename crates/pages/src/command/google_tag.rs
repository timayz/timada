use evento::{Executor, ProjectionAggregate, metadata::Event};

use crate::aggregator::{GoogleSaleValueSet, GoogleTagSet};
use crate::error::PageError;
use crate::value_object::{
    google_tag_id, normalize_ads_id, normalize_analytics_id, normalize_conversion_label,
};

/// What the storefront's Google tag is set up with. Nothing set, no tag.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoogleTagSettings {
    /// The Google Analytics measurement id (`G-XXXXXXXXXX`, or a combined
    /// tag's `GT-…`).
    pub analytics_id: Option<String>,
    /// The Google Ads conversion id (`AW-123456789`).
    pub ads_id: Option<String>,
    /// The labels of the Google Ads conversion actions, each counted only
    /// when given: a sale (the primary one), an item added to the cart and
    /// a checkout begun.
    pub purchase_label: Option<String>,
    pub add_to_cart_label: Option<String>,
    pub begin_checkout_label: Option<String>,
    /// A sale also tells Google Ads the buyer's e-mail address, phone and
    /// name hashed, and their address, when the buyer allowed advertising.
    pub enhanced_conversions: bool,
    /// A sale is worth its profit to Google Ads, not its total: the value
    /// of the conversion counted under `purchase_label`. Analytics is told
    /// the total either way, it reports revenue.
    pub profit_as_value: bool,
}

impl GoogleTagSettings {
    /// Whether the storefront has a tag to write.
    pub fn is_set(&self) -> bool {
        self.analytics_id.is_some() || self.ads_id.is_some()
    }
}

/// The write model of the shop's Google tag, and what a storefront copies
/// of it.
#[evento::projection(id = id, name = "timada-pages/GoogleTagState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct GoogleTagState {
    pub id: String,
    pub settings: GoogleTagSettings,
}

pub fn google_tag_projection<E: Executor>() -> evento::Projection<E, GoogleTagState> {
    evento::Projection::new::<crate::aggregator::GoogleTag>()
        .handler(on_google_tag_set())
        .handler(on_google_sale_value_set())
        .strict()
}

#[evento::handler]
async fn on_google_tag_set(
    event: Event<GoogleTagSet>,
    state: &mut GoogleTagState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.settings = GoogleTagSettings {
        analytics_id: event.data.analytics_id.clone(),
        ads_id: event.data.ads_id.clone(),
        purchase_label: event.data.purchase_label.clone(),
        add_to_cart_label: event.data.add_to_cart_label.clone(),
        begin_checkout_label: event.data.begin_checkout_label.clone(),
        enhanced_conversions: event.data.enhanced_conversions,
        profit_as_value: state.settings.profit_as_value,
    };
    Ok(())
}

#[evento::handler]
async fn on_google_sale_value_set(
    event: Event<GoogleSaleValueSet>,
    state: &mut GoogleTagState,
) -> anyhow::Result<()> {
    state.settings.profit_as_value = event.data.profit;
    Ok(())
}

impl<E: Executor> super::Command<E> {
    /// The shop's Google tag; `None` before it is first set.
    pub async fn load_google_tag(&self) -> anyhow::Result<Option<GoogleTagState>> {
        google_tag_projection()
            .load(google_tag_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The shop's Google tag as it is set up: nothing set before the first
    /// time.
    pub async fn google_tag(&self) -> anyhow::Result<GoogleTagSettings> {
        Ok(self
            .load_google_tag()
            .await?
            .map(|state| state.settings)
            .unwrap_or_default())
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Sets the storefront's Google tag up as `settings` say, whole: an id
    /// or a label left empty is taken off, and all of them empty takes the
    /// tag off the storefront. The ids and labels are checked for their
    /// shape only (they are written into the storefront's pages): whether
    /// Google knows them shows in Google's own tools. A conversion label
    /// and the enhanced conversions need the Google Ads id they belong to,
    /// and so does a sale valued at its profit. Settings that change
    /// nothing write nothing.
    pub async fn set_google_tag(
        &self,
        actor: &str,
        settings: GoogleTagSettings,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let settings = GoogleTagSettings {
            analytics_id: normalize_analytics_id(settings.analytics_id.as_deref())?,
            ads_id: normalize_ads_id(settings.ads_id.as_deref())?,
            purchase_label: normalize_conversion_label(settings.purchase_label.as_deref())?,
            add_to_cart_label: normalize_conversion_label(settings.add_to_cart_label.as_deref())?,
            begin_checkout_label: normalize_conversion_label(
                settings.begin_checkout_label.as_deref(),
            )?,
            enhanced_conversions: settings.enhanced_conversions,
            profit_as_value: settings.profit_as_value,
        };
        let for_ads = settings.purchase_label.is_some()
            || settings.add_to_cart_label.is_some()
            || settings.begin_checkout_label.is_some()
            || settings.enhanced_conversions
            || settings.profit_as_value;
        if for_ads && settings.ads_id.is_none() {
            return Err(PageError::GoogleAdsIdMissing);
        }
        let state = self.load_google_tag().await?;
        let was = state
            .as_ref()
            .map(|state| state.settings.clone())
            .unwrap_or_default();
        if was == settings {
            return Ok(());
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(google_tag_id());
                write.original_version(0);
                write
            }
        };
        write.routing_key_opt(routing_key);
        let tag = |settings: &GoogleTagSettings| GoogleTagSettings {
            profit_as_value: false,
            ..settings.clone()
        };
        if tag(&was) != tag(&settings) {
            write.event(&GoogleTagSet {
                analytics_id: settings.analytics_id,
                ads_id: settings.ads_id,
                purchase_label: settings.purchase_label,
                add_to_cart_label: settings.add_to_cart_label,
                begin_checkout_label: settings.begin_checkout_label,
                enhanced_conversions: settings.enhanced_conversions,
                set_by: actor.to_owned(),
            });
        }
        if was.profit_as_value != settings.profit_as_value {
            write.event(&GoogleSaleValueSet {
                profit: settings.profit_as_value,
                set_by: actor.to_owned(),
            });
        }
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(())
    }
}
