use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::{LinkIssued, RequestDelaySet, RequestMailed, RequestsDisabled};
use crate::error::ReviewError;
use crate::value_object::{
    DEFAULT_REQUEST_DELAY_DAYS, MAX_REQUEST_DELAY_DAYS, REVIEW_LINK_TTL_SECS, generate_token,
    review_link_id, review_request_id, review_requests_id,
};

use super::ReviewRequestsState;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// The merchant says how many days after fulfilment the shop mails its
    /// buyers, or `None` to mail them never. The same setting again is a
    /// no-op.
    pub async fn set_review_requests(
        &self,
        actor: &str,
        days: Option<u8>,
        routing_key: Option<String>,
    ) -> Result<(), ReviewError> {
        if days.is_some_and(|days| days > MAX_REQUEST_DELAY_DAYS) {
            return Err(ReviewError::RequestDelayTooLong);
        }
        let state = self.load_review_requests().await?;
        let current = state.as_ref().map_or(
            Some(DEFAULT_REQUEST_DELAY_DAYS),
            ReviewRequestsState::delay_days,
        );
        if current == days {
            return Ok(());
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(review_requests_id());
                write.original_version(0);
                write
            }
        };
        write.routing_key_opt(routing_key);
        match days {
            Some(days) => write.event(&RequestDelaySet {
                days,
                set_by: actor.to_owned(),
            }),
            None => write.event(&RequestsDisabled {
                disabled_by: actor.to_owned(),
            }),
        };
        write.commit(&self.0).await?;
        Ok(())
    }

    /// Takes the sending of an order's review request, before it goes out.
    /// Returns whether this caller took it: `false` when it was mailed
    /// already, by another node say.
    pub async fn claim_review_request(
        &self,
        order_id: &str,
        email: &str,
        actor: &str,
        routing_key: Option<String>,
    ) -> Result<bool, ReviewError> {
        let claimed = evento::append(review_request_id(order_id))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&RequestMailed {
                order_id: order_id.to_owned(),
                email: email.to_owned(),
                mailed_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await;
        match claimed {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// A fresh token that opens `order_id`'s review forms until
    /// [`REVIEW_LINK_TTL_SECS`] after `now`. Only its hash is stored: the
    /// token itself is in the returned value and nowhere else.
    pub async fn issue_review_link(
        &self,
        order_id: &str,
        actor: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<String, ReviewError> {
        let token = generate_token();
        evento::append(review_link_id(&token))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&LinkIssued {
                order_id: order_id.to_owned(),
                expires_at: now + REVIEW_LINK_TTL_SECS,
                issued_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(token)
    }
}

impl<E: Executor> super::Command<E> {
    /// The shop's policy, the default when the merchant never set one.
    pub async fn review_requests(&self) -> anyhow::Result<ReviewRequestsState> {
        Ok(self.load_review_requests().await?.unwrap_or_default())
    }

    /// The order a mailed token opens, while it is still valid at `now`.
    pub async fn resolve_review_link(
        &self,
        token: &str,
        now: u64,
    ) -> anyhow::Result<Option<String>> {
        if token.is_empty() {
            return Ok(None);
        }
        Ok(self
            .load_review_link(&review_link_id(token))
            .await?
            .filter(|link| link.is_valid_at(now))
            .map(|link| link.order_id))
    }
}
