//! What an address form offers in a shop that forwards to AliExpress: the
//! dashboard answers from AliExpress's own lists, so that the province and the
//! town picked go through when the order is placed there.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use topcoat_core::context::Cx;
use topcoat_core::error::Result;

use crate::context::store;
use crate::store::Store;

/// The dashboard may have to read a country's list first, and France's weighs
/// over 2 MB.
const PLACES_TIMEOUT: Duration = Duration::from_secs(95);

/// All empty when the form should ask for no province.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Places {
    /// The country's provinces, as AliExpress lists them.
    pub provinces: Vec<String>,
    /// The one the postal code names.
    pub province: Option<String>,
    /// The cities of the province asked for, else of that one. None for a
    /// province without a list, whose town is typed.
    pub cities: Vec<PlaceCity>,
}

/// A city a form lists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceCity {
    /// `Other` for a town the list does not have, to type.
    pub name: String,
    /// The towns to pick from once this city is picked, if it has any.
    pub towns: Vec<String>,
}

impl Store {
    /// See [`places`].
    pub async fn places(
        &self,
        country: &str,
        postal_code: &str,
        province: &str,
    ) -> anyhow::Result<Places> {
        let Some(api) = self.api() else {
            return Ok(Places::default());
        };
        let places = api
            .client
            .get(format!("{}/store/places", api.url))
            .bearer_auth(&api.token)
            .query(&[
                ("country", country),
                ("postal_code", postal_code),
                ("province", province),
            ])
            .timeout(PLACES_TIMEOUT)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(places)
    }
}

/// What the form offers for an address in `country`; nothing to offer when
/// the dashboard could not say, as the form works without.
pub async fn places(cx: &Cx, country: &str, postal_code: &str, province: &str) -> Result<Places> {
    match store(cx).places(country, postal_code, province).await {
        Ok(places) => Ok(places),
        Err(err) => {
            tracing::warn!(error = %err, country, "the address form's places not read");
            Ok(Places::default())
        }
    }
}
