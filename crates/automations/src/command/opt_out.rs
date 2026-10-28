use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::mail_claim::Claimed;
use crate::aggregator::opt_out::{OptedOut, Withdrawn};
use crate::error::AutomationError;
use crate::value_object::{mail_claim_id, normalize_email, opt_out_id, opt_out_id_of_token};

/// The write model of an address on the opt-out list.
#[evento::projection(id = id, name = "timada-automations/OptOutState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct OptOutState {
    pub id: String,
    /// On the list (not withdrawn since).
    pub opted_out: bool,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

pub fn opt_out_projection<E: Executor>() -> Projection<E, OptOutState> {
    Projection::new::<crate::aggregator::opt_out::OptOut>()
        .handler(on_opted_out())
        .handler(on_withdrawn())
        .strict()
}

#[evento::handler]
async fn on_opted_out(event: Event<OptedOut>, state: &mut OptOutState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.opted_out = true;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_withdrawn(event: Event<Withdrawn>, state: &mut OptOutState) -> anyhow::Result<()> {
    state.opted_out = false;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Puts an address on the shop's opt-out list (a member, at the
    /// customer's request); on it already is a no-op.
    pub async fn opt_out_address(
        &self,
        actor: &str,
        email: &str,
        routing_key: Option<String>,
    ) -> Result<(), AutomationError> {
        let email = normalize_email(email).ok_or(AutomationError::InvalidEmail)?;
        self.put_on_list(actor, &opt_out_id(&email), routing_key)
            .await
    }

    /// Puts the address a mail's link names on the list: `key` is the
    /// host's ([`opt_out_token`](crate::opt_out_token)). On it already is a
    /// no-op; a link that does not check out is
    /// [`AutomationError::InvalidLink`].
    pub async fn opt_out_with_link(
        &self,
        key: &[u8; 32],
        actor: &str,
        token: &str,
        routing_key: Option<String>,
    ) -> Result<(), AutomationError> {
        let id =
            opt_out_id_of_token(key, &self.tenant, token).ok_or(AutomationError::InvalidLink)?;
        self.put_on_list(actor, &id, routing_key).await
    }

    /// Takes an address off the list: it gets the shop's automated mails
    /// again. Whether it was on it.
    pub async fn withdraw_opt_out(
        &self,
        actor: &str,
        email: &str,
        routing_key: Option<String>,
    ) -> Result<bool, AutomationError> {
        let email = normalize_email(email).ok_or(AutomationError::InvalidEmail)?;
        let Some(state) = self.load_opt_out(&opt_out_id(&email)).await? else {
            return Ok(false);
        };
        if !state.opted_out {
            return Ok(false);
        }
        match state
            .write()?
            .routing_key_opt(routing_key)
            .event(&Withdrawn {
                withdrawn_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await
        {
            Ok(_) => Ok(true),
            // Moved at the same moment: as it stands now.
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Takes a scenario's one mail of the day to an address, before it goes
    /// out: whether this caller took it (`false`: mailed already today).
    pub async fn claim_mail(
        &self,
        actor: &str,
        scenario_id: &str,
        email: &str,
        day: u64,
        routing_key: Option<String>,
    ) -> Result<bool, AutomationError> {
        let email = normalize_email(email).ok_or(AutomationError::InvalidEmail)?;
        let claimed = evento::append(mail_claim_id(scenario_id, &email, day))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Claimed {
                scenario_id: scenario_id.to_owned(),
                claimed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match claimed {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }
}

impl<E: Executor> super::Command<E> {
    async fn load_opt_out(&self, id: &str) -> anyhow::Result<Option<OptOutState>> {
        opt_out_projection()
            .load(id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    async fn put_on_list(
        &self,
        actor: &str,
        id: &str,
        routing_key: Option<String>,
    ) -> Result<(), AutomationError> {
        // Twice at most: a write that lost the race reads the stream again.
        for _ in 0..2 {
            let state = self.load_opt_out(id).await?;
            if state.as_ref().is_some_and(|state| state.opted_out) {
                return Ok(());
            }
            let mut write = match &state {
                Some(state) => state.write()?,
                None => {
                    let mut write = evento::append(id);
                    write.original_version(0);
                    write
                }
            };
            let written = write
                .routing_key_opt(routing_key.clone())
                .event(&OptedOut {
                    opted_out_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match written {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// Whether an address is on the shop's opt-out list.
    pub async fn is_opted_out(&self, email: &str) -> anyhow::Result<bool> {
        let Some(email) = normalize_email(email) else {
            return Ok(false);
        };
        Ok(self
            .load_opt_out(&opt_out_id(&email))
            .await?
            .is_some_and(|state| state.opted_out))
    }
}
