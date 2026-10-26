use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{StorefrontAddressCleared, StorefrontAddressSet};
use crate::error::OrdersError;
use crate::value_object::{normalize_storefront_url, storefront_id};

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Where the tenant's storefront lives (`https://shop.example`), if the
    /// merchant said.
    pub async fn storefront_address(&self) -> anyhow::Result<Option<String>> {
        Ok(self.load_storefront().await?.and_then(|state| state.url))
    }

    /// The merchant says where the storefront lives, or `None` to forget it.
    /// The same address again is a no-op.
    pub async fn set_storefront_address(
        &self,
        actor: &str,
        url: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<(), OrdersError> {
        let url = url.map(normalize_storefront_url).transpose()?;
        let state = self.load_storefront().await?;
        if state.as_ref().and_then(|state| state.url.as_ref()) == url.as_ref() {
            return Ok(());
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(storefront_id());
                write.original_version(0);
                write
            }
        };
        write.routing_key_opt(routing_key);
        match url {
            Some(url) => write.event(&StorefrontAddressSet {
                url,
                set_by: actor.to_owned(),
            }),
            None => write.event(&StorefrontAddressCleared {
                cleared_by: actor.to_owned(),
            }),
        };
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(())
    }
}
