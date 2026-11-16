use evento::{Executor, ProjectionAggregate, metadata::Event};

use crate::aggregator::{
    ConversionRefused, ConversionsTokenRemoved, ConversionsTokenSet, MetaPixelSet,
};
use crate::error::PageError;
use crate::value_object::{MAX_META_REASON_LEN, meta_pixel_id, normalize_meta_pixel_id};

/// What a change of the pixel's settings does to the Conversions API token.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum TokenChange {
    /// As it is.
    #[default]
    Keep,
    /// A new one, sealed by the host.
    Set(Vec<u8>),
    Remove,
}

/// The write model of the shop's Meta pixel, and what a storefront copies
/// of it (the pixel's id).
#[evento::projection(id = id, name = "timada-pages/MetaPixelState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct MetaPixelState {
    pub id: String,
    /// `None`: no pixel on the storefront.
    pub pixel_id: Option<String>,
    /// The Conversions API token, sealed; `None`: sales are told by the
    /// browser only.
    pub token_sealed: Option<Vec<u8>>,
    /// Why Meta refused the last sale sent under the token, when it did
    /// since the pixel or the token last changed.
    pub refusal: Option<String>,
}

pub fn meta_pixel_projection<E: Executor>() -> evento::Projection<E, MetaPixelState> {
    evento::Projection::new::<crate::aggregator::MetaPixel>()
        .handler(on_meta_pixel_set())
        .handler(on_conversions_token_set())
        .handler(on_conversions_token_removed())
        .handler(on_conversion_refused())
        .strict()
}

#[evento::handler]
async fn on_meta_pixel_set(
    event: Event<MetaPixelSet>,
    state: &mut MetaPixelState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.pixel_id = event.data.pixel_id.clone();
    if state.pixel_id.is_none() {
        state.token_sealed = None;
    }
    state.refusal = None;
    Ok(())
}

#[evento::handler]
async fn on_conversions_token_set(
    event: Event<ConversionsTokenSet>,
    state: &mut MetaPixelState,
) -> anyhow::Result<()> {
    state.token_sealed = Some(event.data.token_sealed.clone());
    state.refusal = None;
    Ok(())
}

#[evento::handler]
async fn on_conversions_token_removed(
    _event: Event<ConversionsTokenRemoved>,
    state: &mut MetaPixelState,
) -> anyhow::Result<()> {
    state.token_sealed = None;
    state.refusal = None;
    Ok(())
}

#[evento::handler]
async fn on_conversion_refused(
    event: Event<ConversionRefused>,
    state: &mut MetaPixelState,
) -> anyhow::Result<()> {
    state.refusal = Some(event.data.reason.clone());
    Ok(())
}

impl<E: Executor> super::Command<E> {
    /// The shop's Meta pixel; `None` before it is first set.
    pub async fn load_meta_pixel(&self) -> anyhow::Result<Option<MetaPixelState>> {
        meta_pixel_projection()
            .load(meta_pixel_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Sets the storefront's Meta pixel to `pixel_id` (checked for its
    /// shape only: it is written into the storefront's pages), and changes
    /// the Conversions API token as `token` says. An id left empty takes
    /// the pixel off, and the token with it; a token needs a pixel. What
    /// changes nothing writes nothing.
    pub async fn set_meta_pixel(
        &self,
        actor: &str,
        pixel_id: Option<&str>,
        token: TokenChange,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let pixel_id = normalize_meta_pixel_id(pixel_id)?;
        let state = self.load_meta_pixel().await?;
        let had_token = state
            .as_ref()
            .is_some_and(|state| state.token_sealed.is_some());
        // Taken off with the pixel, by the fold.
        let token = match (token, &pixel_id) {
            (TokenChange::Set(_), None) => return Err(PageError::MetaPixelMissing),
            (_, None) => TokenChange::Keep,
            (TokenChange::Remove, Some(_)) if !had_token => TokenChange::Keep,
            (token, Some(_)) => token,
        };
        let was = state.as_ref().and_then(|state| state.pixel_id.clone());
        if was == pixel_id && token == TokenChange::Keep {
            return Ok(());
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(meta_pixel_id());
                write.original_version(0);
                write
            }
        };
        write.routing_key_opt(routing_key);
        if was != pixel_id {
            write.event(&MetaPixelSet {
                pixel_id,
                set_by: actor.to_owned(),
            });
        }
        match token {
            TokenChange::Keep => {}
            TokenChange::Set(token_sealed) => {
                write.event(&ConversionsTokenSet {
                    token_sealed,
                    set_by: actor.to_owned(),
                });
            }
            TokenChange::Remove => {
                write.event(&ConversionsTokenRemoved {
                    removed_by: actor.to_owned(),
                });
            }
        }
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(())
    }

    /// Records that Meta refused the sale of `order_id` sent under the
    /// token, in its words (cut to [`MAX_META_REASON_LEN`] characters), for
    /// the merchant to read beside the token. The same reason as the one
    /// already standing writes nothing, and so does a shop without a token
    /// (it was taken off meanwhile).
    pub async fn refuse_meta_conversion(
        &self,
        actor: &str,
        order_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let Some(state) = self.load_meta_pixel().await? else {
            return Ok(());
        };
        let reason: String = reason
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(MAX_META_REASON_LEN)
            .collect();
        if state.token_sealed.is_none() || state.refusal.as_deref() == Some(reason.as_str()) {
            return Ok(());
        }
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&ConversionRefused {
                order_id: order_id.to_owned(),
                reason,
                refused_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
