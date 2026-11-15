//! The shop's shopping feed: whether it answers, and at which address.

use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};

use crate::aggregator::feed::{Closed, Opened};
use crate::error::CatalogError;
use crate::value_object::{feed_id, new_feed_token};

#[evento::projection(id = id, name = "timada-catalog/FeedState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct FeedState {
    pub id: String,
    /// What the feed's address ends with while it is open.
    pub token: Option<String>,
}

pub fn feed_projection<E: Executor>() -> Projection<E, FeedState> {
    Projection::new::<crate::aggregator::feed::Feed>()
        .handler(on_feed_opened())
        .handler(on_feed_closed())
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

impl<E: Executor> super::Command<E> {
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
