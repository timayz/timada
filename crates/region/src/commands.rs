//! Create and update regions — admin-only commands.

use evento::ProjectionAggregate as _;
use timada_core::{Currency, Executor};

use crate::aggregate::{RegionCountry, RegionCreated, RegionUpdated};
use crate::view::load_region;

/// Why a region write was refused.
#[derive(Debug, thiserror::Error)]
pub enum RegionError {
    #[error("{0}")]
    Invalid(String),
    #[error("this region does not exist")]
    UnknownRegion,
    #[error(transparent)]
    Storage(#[from] anyhow::Error),
}

/// Create a region and return its id.
#[tracing::instrument(skip(executor, countries))]
pub async fn create_region(
    executor: &Executor,
    name: &str,
    currency: Currency,
    countries: Vec<RegionCountry>,
) -> Result<String, RegionError> {
    let (name, countries) = validate(name, countries)?;

    let region_id = evento::create()
        .event(&RegionCreated {
            name,
            currency,
            countries,
        })
        .commit(executor)
        .await
        .map_err(anyhow::Error::from)?;

    tracing::info!(%region_id, "region created");
    Ok(region_id)
}

/// Replace a region's name and country list. The currency never changes —
/// see [`RegionCreated`](crate::aggregate::Region).
#[tracing::instrument(skip(executor, countries))]
pub async fn update_region(
    executor: &Executor,
    region_id: &str,
    name: &str,
    countries: Vec<RegionCountry>,
) -> Result<(), RegionError> {
    let (name, countries) = validate(name, countries)?;

    let Some(region) = load_region(executor, region_id).await? else {
        return Err(RegionError::UnknownRegion);
    };

    region
        .write()?
        .event(&RegionUpdated { name, countries })
        .commit(executor)
        .await
        .map_err(anyhow::Error::from)?;

    tracing::info!(%region_id, "region updated");
    Ok(())
}

/// Trim the name, uppercase the codes, refuse blanks and duplicates.
fn validate(
    name: &str,
    countries: Vec<RegionCountry>,
) -> Result<(String, Vec<RegionCountry>), RegionError> {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err(RegionError::Invalid("a region needs a name".into()));
    }
    if countries.is_empty() {
        return Err(RegionError::Invalid(
            "a region needs at least one country".into(),
        ));
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut normalized = Vec::with_capacity(countries.len());
    for country in countries {
        let code = country.code.trim().to_uppercase();
        if code.is_empty() {
            return Err(RegionError::Invalid("a country code is empty".into()));
        }
        if !seen.insert(code.clone()) {
            return Err(RegionError::Invalid(format!(
                "country {code} is listed twice"
            )));
        }
        normalized.push(RegionCountry {
            code,
            tax_rate_bps: country.tax_rate_bps,
        });
    }

    Ok((name, normalized))
}
