//! What the shop's shipping methods are called in its other languages.

use std::collections::BTreeMap;

use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};
use timada_core::languages::is_language;

use crate::aggregator::{MethodNamed, ShippingNames};
use crate::error::OrderError;
use crate::value_object::{MAX_SHIPPING_NAME_LEN, shipping_names_id};

/// The write model of a language's names, and what a storefront copies of
/// it.
#[evento::projection(id = id, name = "timada-orders/ShippingNamesState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ShippingNamesState {
    pub id: String,
    pub language: String,
    /// A method's id, then what the language calls it: only the methods
    /// named there.
    pub names: BTreeMap<String, String>,
}

pub fn shipping_names_projection<E: Executor>() -> Projection<E, ShippingNamesState> {
    Projection::new::<ShippingNames>()
        .handler(on_method_named())
        .strict()
}

#[evento::handler]
async fn on_method_named(
    event: Event<MethodNamed>,
    state: &mut ShippingNamesState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.language = event.data.language.to_owned();
    if event.data.name.is_empty() {
        state.names.remove(&event.data.method_id);
    } else {
        state
            .names
            .insert(event.data.method_id.to_owned(), event.data.name.to_owned());
    }
    Ok(())
}

/// A language code from the list, lowercased.
fn normalize_language(code: &str) -> Result<String, OrderError> {
    let code = code.trim().to_ascii_lowercase();
    if is_language(&code) {
        Ok(code)
    } else {
        Err(OrderError::InvalidLanguage)
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// What `language` calls the shop's shipping methods, by method id:
    /// only the ones named there, a method the shop removed since among
    /// them.
    pub async fn shipping_names(&self, language: &str) -> anyhow::Result<BTreeMap<String, String>> {
        Ok(shipping_names_projection()
            .load(shipping_names_id(language))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?
            .map(|state| state.names)
            .unwrap_or_default())
    }

    /// Says what `language` calls a shipping method the shop offers; an
    /// empty `name` goes back to the main language's, and the same one is
    /// a no-op. A language is one of the list, whether the storefront is
    /// read in it yet or not.
    pub async fn name_shipping_method(
        &self,
        actor: &str,
        language: &str,
        method_id: &str,
        name: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let language = normalize_language(language)?;
        let name = name.trim();
        if name.chars().count() > MAX_SHIPPING_NAME_LEN {
            return Err(OrderError::InvalidShippingName);
        }
        let offered = self
            .load_shipping_methods()
            .await?
            .is_some_and(|state| state.method(method_id).is_some());
        if !offered {
            return Err(OrderError::ShippingMethodNotFound);
        }
        let state = shipping_names_projection()
            .load(shipping_names_id(&language))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?;
        let current = state
            .as_ref()
            .and_then(|state| state.names.get(method_id))
            .map_or("", String::as_str);
        if current == name {
            return Ok(());
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(shipping_names_id(&language));
                write.original_version(0);
                write
            }
        };
        write.routing_key_opt(routing_key).event(&MethodNamed {
            language,
            method_id: method_id.to_owned(),
            name: name.to_owned(),
            named_by: actor.to_owned(),
        });
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(())
    }
}
