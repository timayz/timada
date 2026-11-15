use evento::{Executor, ProjectionAggregate, metadata::Event};

use crate::aggregator::{RedirectAdded, RedirectRemoved};
use crate::error::PageError;
use crate::value_object::{
    MAX_REDIRECTS, new_redirect_id, normalize_redirect_from, normalize_redirect_to, redirects_id,
};

/// An old address of the shop and where it sends to now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Redirect {
    pub id: String,
    /// The old path (`/products/blue-mug`), with no query.
    pub from: String,
    /// The path that answers now (`/p/blue-mug`).
    pub to: String,
}

impl Redirect {
    /// The path of `to`, without its query or fragment: what another
    /// redirect must not start from.
    fn target_path(&self) -> &str {
        path_of(&self.to)
    }
}

/// A path without its query or fragment.
fn path_of(to: &str) -> &str {
    to.split(['?', '#']).next().unwrap_or(to)
}

/// The write model of the shop's redirects, and what a storefront copies of
/// them.
#[evento::projection(id = id, name = "timada-pages/RedirectsState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct RedirectsState {
    pub id: String,
    /// In the order they were added.
    pub redirects: Vec<Redirect>,
}

pub fn redirects_projection<E: Executor>() -> evento::Projection<E, RedirectsState> {
    evento::Projection::new::<crate::aggregator::Redirects>()
        .handler(on_redirect_added())
        .handler(on_redirect_removed())
        .strict()
}

#[evento::handler]
async fn on_redirect_added(
    event: Event<RedirectAdded>,
    state: &mut RedirectsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.redirects.push(Redirect {
        id: event.data.redirect_id.clone(),
        from: event.data.from.clone(),
        to: event.data.to.clone(),
    });
    Ok(())
}

#[evento::handler]
async fn on_redirect_removed(
    event: Event<RedirectRemoved>,
    state: &mut RedirectsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state
        .redirects
        .retain(|redirect| redirect.id != event.data.redirect_id);
    Ok(())
}

impl<E: Executor> super::Command<E> {
    /// The shop's redirects; `None` before the first is added.
    pub async fn load_redirects(&self) -> anyhow::Result<Option<RedirectsState>> {
        redirects_projection()
            .load(redirects_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The shop's redirects, in the order they were added.
    pub async fn redirects(&self) -> anyhow::Result<Vec<Redirect>> {
        Ok(self
            .load_redirects()
            .await?
            .map(|state| state.redirects)
            .unwrap_or_default())
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Sends the old path `from` to `to` from now on; returns the
    /// redirect's id. Refused when `from` already sends somewhere, and when
    /// it would make a chain: `to` being an old path itself, or `from`
    /// being where another redirect sends.
    pub async fn add_redirect(
        &self,
        actor: &str,
        from: &str,
        to: &str,
        routing_key: Option<String>,
    ) -> Result<String, PageError> {
        let from = normalize_redirect_from(from)?;
        let to = normalize_redirect_to(to)?;
        if path_of(&to) == from {
            return Err(PageError::RedirectChain);
        }
        let state = self.load_redirects().await?;
        let redirects = state
            .as_ref()
            .map_or(&[][..], |state| state.redirects.as_slice());
        if redirects.len() >= MAX_REDIRECTS {
            return Err(PageError::TooManyRedirects);
        }
        if redirects.iter().any(|redirect| redirect.from == from) {
            return Err(PageError::RedirectExists);
        }
        if redirects
            .iter()
            .any(|redirect| redirect.from == path_of(&to) || redirect.target_path() == from)
        {
            return Err(PageError::RedirectChain);
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(redirects_id());
                write.original_version(0);
                write
            }
        };
        let id = new_redirect_id();
        write
            .routing_key_opt(routing_key)
            .event(&RedirectAdded {
                redirect_id: id.clone(),
                from,
                to,
                added_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(id)
    }

    /// Stops a redirect: its old path answers as any unknown one again.
    pub async fn remove_redirect(
        &self,
        actor: &str,
        redirect_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let state = self
            .load_redirects()
            .await?
            .filter(|state| state.redirects.iter().any(|r| r.id == redirect_id))
            .ok_or(PageError::RedirectNotFound)?;
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&RedirectRemoved {
                redirect_id: redirect_id.to_owned(),
                removed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
