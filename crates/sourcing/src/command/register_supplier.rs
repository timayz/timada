use evento::Executor;
use timada_core::currency::is_currency_code;

use crate::{aggregator::SupplierRegistered, connector::SupplierConnectors, error::SourcingError};

use super::supplier_id;

#[derive(Debug, Clone)]
pub struct RegisterSupplier {
    /// What the supplier is known by, for ever: the id derives from it.
    pub slug: String,
    pub name: String,
    /// The key of the connector that talks to it, or `"manual"`.
    pub connector: String,
    /// What it quotes its costs in.
    pub currency: String,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Takes on a supplier. The id derives from the slug, so a second
    /// supplier under the same one is refused atomically by the store.
    ///
    /// `connectors` is what the host plugged in: a supplier is not taken on
    /// under a connector nobody can answer for. Pass
    /// `&SupplierConnectors::default()` only where there is genuinely no
    /// registry to check against.
    pub async fn register_supplier(
        &self,
        cmd: RegisterSupplier,
        connectors: &SupplierConnectors,
        routing_key: Option<String>,
    ) -> Result<String, SourcingError> {
        let slug = timada_core::slug::slugify(&cmd.slug);
        if slug.is_empty() {
            return Err(SourcingError::Required("slug"));
        }
        if cmd.name.trim().is_empty() {
            return Err(SourcingError::Required("name"));
        }
        let connector = cmd.connector.trim().to_owned();
        if connector.is_empty() {
            return Err(SourcingError::Required("connector"));
        }
        if !connectors.is_empty() && connectors.of(&connector).is_none() {
            return Err(SourcingError::ConnectorUnknown(connector));
        }
        let currency = cmd.currency.trim().to_uppercase();
        if !is_currency_code(&currency) {
            return Err(SourcingError::InvalidCurrency(currency));
        }

        let id = supplier_id(&slug);
        let result = evento::append(&id)
            .routing_key_opt(routing_key)
            .event(&SupplierRegistered {
                slug: slug.clone(),
                name: cmd.name.trim().to_owned(),
                connector,
                currency,
            })
            .commit(self.executor)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(supplier_id = %id, %slug, "supplier registered");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => {
                Err(SourcingError::AlreadyRegistered(slug))
            }
            Err(err) => Err(err.into()),
        }
    }
}
