//! The renewal of a company's AliExpress access, one per token.
//!
//! One node claims the renewal, asks AliExpress, and records the tokens it
//! got on the claim before the company hears of them: the tokens are the
//! only copy AliExpress handed out, so a node that stops after the answer,
//! or a company write that fails, must not lose them. The next node to find
//! the access due applies what the claim recorded instead of asking again,
//! and takes over a claim whose node went quiet before recording anything.
//! The claim lives in the platform's own streams, next to the company, not
//! under any tenant.

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::refresh::{Claimed, Renewed, TokenRefresh};
use crate::value_object::{RenewedTokens, TOKEN_CLAIM_STALE_SECS, token_refresh_id};

/// How many times a renewal's write is tried again when another node wrote
/// the same claim at the same moment.
const CLAIM_ATTEMPTS: usize = 5;

/// Where the renewal of one token stands.
#[evento::projection(id = id, name = "timada-aliexpress/TokenRefreshState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct TokenRefreshState {
    pub id: String,
    /// When the latest claim was made (unix seconds).
    pub claimed_at: u64,
    /// What AliExpress answered, once recorded.
    pub renewed: Option<RenewedTokens>,
}

pub fn token_refresh_projection<E: Executor>() -> Projection<E, TokenRefreshState> {
    Projection::new::<TokenRefresh>()
        .handler(on_claimed())
        .handler(on_renewed())
        .strict()
}

#[evento::handler]
async fn on_claimed(event: Event<Claimed>, state: &mut TokenRefreshState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.claimed_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_renewed(event: Event<Renewed>, state: &mut TokenRefreshState) -> anyhow::Result<()> {
    state
        .renewed
        .get_or_insert_with(|| event.data.tokens.clone());
    Ok(())
}

/// What the node asking to renew a token is to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenRenewal {
    /// It took the renewal on: it asks AliExpress, then records what it got
    /// ([`record_token_renewal`]).
    Claimed,
    /// Another node took it on a moment ago.
    Held,
    /// AliExpress answered already: these are the tokens the company is
    /// owed, to apply without asking again.
    Renewed(RenewedTokens),
}

/// The renewal of `company_id`'s token lapsing at `expires_at`, as it
/// stands.
pub async fn load_token_refresh<E: Executor>(
    executor: &E,
    company_id: &str,
    expires_at: u64,
) -> anyhow::Result<Option<TokenRefreshState>> {
    token_refresh_projection()
        .load(token_refresh_id(company_id, expires_at))
        .execute(executor)
        .await
}

/// Takes the renewal of `company_id`'s token lapsing at `expires_at` on, at
/// `now` (unix seconds): see [`TokenRenewal`]. A claim older than
/// [`TOKEN_CLAIM_STALE_SECS`] with nothing recorded is taken over.
pub async fn claim_token_refresh<E: Executor>(
    executor: &E,
    actor: &str,
    company_id: &str,
    expires_at: u64,
    now: u64,
) -> anyhow::Result<TokenRenewal> {
    let claimed = Claimed {
        claimed_by: actor.to_owned(),
    };
    for _ in 0..CLAIM_ATTEMPTS {
        let written = match load_token_refresh(executor, company_id, expires_at).await? {
            Some(state) => {
                if let Some(tokens) = state.renewed {
                    return Ok(TokenRenewal::Renewed(tokens));
                }
                if now < state.claimed_at.saturating_add(TOKEN_CLAIM_STALE_SECS) {
                    return Ok(TokenRenewal::Held);
                }
                state.write()?.event(&claimed).commit(executor).await
            }
            None => {
                evento::append(token_refresh_id(company_id, expires_at))
                    .original_version(0)
                    .event(&claimed)
                    .commit(executor)
                    .await
            }
        };
        match written {
            Ok(_) => return Ok(TokenRenewal::Claimed),
            // Another node claimed or recorded meanwhile: decide again.
            Err(WriteError::InvalidOriginalVersion) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(TokenRenewal::Held)
}

/// Records the tokens AliExpress answered the claim of `company_id`'s token
/// lapsing at `expires_at` with. The first tokens recorded stand: a node
/// that took the claim over and was answered too records nothing more.
pub async fn record_token_renewal<E: Executor>(
    executor: &E,
    actor: &str,
    company_id: &str,
    expires_at: u64,
    tokens: &RenewedTokens,
) -> anyhow::Result<()> {
    for _ in 0..CLAIM_ATTEMPTS {
        let state = load_token_refresh(executor, company_id, expires_at)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no claim on the renewal of {company_id}'s token"))?;
        if state.renewed.is_some() {
            return Ok(());
        }
        let written = state
            .write()?
            .event(&Renewed {
                tokens: tokens.clone(),
                renewed_by: actor.to_owned(),
            })
            .commit(executor)
            .await;
        match written {
            Ok(_) => return Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {}
            Err(err) => return Err(err.into()),
        }
    }
    anyhow::bail!("the renewal of {company_id}'s token kept changing while being recorded")
}
