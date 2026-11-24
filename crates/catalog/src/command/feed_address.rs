//! The shop's shopping feed: whether it answers, at which address, and
//! which of its labels says a product is kept out of ads.

use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};

use crate::aggregator::feed::{AdsLabelSet, Closed, Opened};
use crate::error::CatalogError;
use crate::value_object::{FEED_LABELS, feed_id, new_feed_token};

#[evento::projection(id = id, name = "timada-catalog/FeedState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct FeedState {
    pub id: String,
    /// What the feed's address ends with while it is open.
    pub token: Option<String>,
    /// Which label (from 0) says a product is kept out of ads.
    pub ads_label: Option<u8>,
}

pub fn feed_projection<E: Executor>() -> Projection<E, FeedState> {
    Projection::new::<crate::aggregator::feed::Feed>()
        .handler(on_feed_opened())
        .handler(on_feed_closed())
        .handler(on_feed_ads_label_set())
        .strict()
}

#[evento::handler]
async fn on_feed_opened(event: Event<Opened>, state: &mut FeedState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.token = Some(event.data.token.to_owned());
    Ok(())
}

#[evento::handler]
async fn on_feed_closed(event: Event<Closed>, state: &mut FeedState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.token = None;
    Ok(())
}

#[evento::handler]
async fn on_feed_ads_label_set(
    event: Event<AdsLabelSet>,
    state: &mut FeedState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.ads_label = event.data.label;
    Ok(())
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Opens the shop's shopping feed at a fresh address and returns what
    /// it ends with. A feed already open moves there: the address it had
    /// stops answering, which is how one that leaked is replaced.
    pub async fn open_feed(
        &self,
        actor: &str,
        routing_key: Option<String>,
    ) -> Result<String, CatalogError> {
        let token = new_feed_token();
        let mut write = match self.load_feed_state().await? {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(feed_id());
                write.original_version(0);
                write
            }
        };
        write
            .routing_key_opt(routing_key)
            .event(&Opened {
                token: token.clone(),
                opened_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(token)
    }

    /// Closes the shop's shopping feed: its address stops answering. A feed
    /// that is not open is a no-op.
    pub async fn close_feed(
        &self,
        actor: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let Some(state) = self
            .load_feed_state()
            .await?
            .filter(|state| state.token.is_some())
        else {
            return Ok(());
        };
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&Closed {
                closed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Says which label of the feed (from 0) carries
    /// [`OUT_OF_ADS_LABEL`](crate::OUT_OF_ADS_LABEL) on the products of a
    /// category kept out of ads; `None` says it nowhere. Refused on the
    /// label that says the margin's tier. The same is a no-op.
    pub async fn set_feed_ads_label(
        &self,
        actor: &str,
        label: Option<u8>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        if label.is_some_and(|label| usize::from(label) >= FEED_LABELS) {
            return Err(CatalogError::InvalidFeedLabel);
        }
        let state = self.load_feed_state().await?;
        if state.as_ref().and_then(|state| state.ads_label) == label {
            return Ok(());
        }
        if label.is_some() && self.load_sale_costs().await?.tiers().feed_label == label {
            return Err(CatalogError::FeedLabelTaken);
        }
        let mut write = match state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(feed_id());
                write.original_version(0);
                write
            }
        };
        write
            .routing_key_opt(routing_key)
            .event(&AdsLabelSet {
                label,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}

impl<E: Executor> super::Command<E> {
    /// Which label of the shop's feed says a product is kept out of ads.
    pub async fn load_feed_ads_label(&self) -> anyhow::Result<Option<u8>> {
        Ok(self
            .load_feed_state()
            .await?
            .and_then(|state| state.ads_label))
    }

    /// What the address of the shop's shopping feed ends with, while it is
    /// open.
    pub async fn load_feed_token(&self) -> anyhow::Result<Option<String>> {
        Ok(self.load_feed_state().await?.and_then(|state| state.token))
    }

    async fn load_feed_state(&self) -> anyhow::Result<Option<FeedState>> {
        feed_projection()
            .load(feed_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }
}
