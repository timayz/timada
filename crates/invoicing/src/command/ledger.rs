//! A company's EU distance sales, counted across its tenants. The ledger
//! lives in the platform store, where the company is: an invoice of any of
//! its tenants appends one `Counted` there, at the version the ledger was
//! read at, before taking its number in its tenant's book. Two tenants
//! selling at once cannot both count against a stale total, and a retry
//! finds its sale counted and gets the decision it got.

use std::collections::BTreeMap;

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::Counted;
use crate::error::InvoicingError;
use crate::tax::{Decision, DistanceSales, EU_DISTANCE_THRESHOLD_CENTS, Regime};
use crate::value_object::ledger_id;

use super::{ATTEMPTS, chunk_size};

#[evento::projection(id = id, name = "timada-invoicing/LedgerState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct LedgerState {
    pub id: String,
    /// The decision each counted sale got, by [`sale_key`] (tenant and
    /// invoice: document ids are only unique within a tenant's store).
    pub counted: BTreeMap<String, Decision>,
    /// The year's total as of the last sale counted.
    pub distance: DistanceSales,
}

pub fn ledger_projection<E: Executor>() -> Projection<E, LedgerState> {
    Projection::new::<crate::aggregator::DistanceLedger>()
        .handler(on_counted())
        .strict()
}

#[evento::handler]
async fn on_counted(event: Event<Counted>, ledger: &mut LedgerState) -> anyhow::Result<()> {
    let data = &event.data;
    let distance = DistanceSales {
        sales_cents: data.sales_cents,
        passed: data.passed,
    };
    ledger.id = event.aggregate_id.to_owned();
    ledger.distance = distance;
    ledger.counted.insert(
        sale_key(&data.tenant_id, &data.document_id),
        Decision {
            regime: Regime::parse(&data.regime).unwrap_or_default(),
            tax_country: data.tax_country.to_owned(),
            rates: data.rates.to_owned(),
            distance,
        },
    );
    Ok(())
}

/// What a sale is counted under.
fn sale_key(tenant_id: &str, document_id: &str) -> String {
    format!("{tenant_id}/{document_id}")
}

/// The ledgers of the platform store.
pub struct Ledger<E: Executor>(pub E);

/// The chunk a sale goes in, as read for one attempt.
struct OpenChunk {
    id: String,
    state: Option<LedgerState>,
}

impl<E: Executor> Ledger<E> {
    /// The decision of an EU distance sale of `company_id` in `year`:
    /// `decide` is given the year's total before it, and the decision is
    /// counted. A sale counted already gets its decision back.
    pub async fn count<F>(
        &self,
        company_id: &str,
        year: i64,
        document_id: &str,
        tenant_id: &str,
        decide: F,
    ) -> Result<Decision, InvoicingError>
    where
        F: Fn(DistanceSales) -> Decision,
    {
        for _ in 0..ATTEMPTS {
            let (open, previous) = self.open_chunk(company_id, year).await?;
            if let Some(decision) = [open.state.as_ref(), previous.as_ref()]
                .into_iter()
                .flatten()
                .find_map(|ledger| ledger.counted.get(&sale_key(tenant_id, document_id)))
            {
                return Ok(decision.clone());
            }
            let before = match open.state.as_ref().or(previous.as_ref()) {
                Some(ledger) => ledger.distance,
                None => self.start_of(company_id, year).await?,
            };
            let decision = decide(before);
            let committed = evento::append(&open.id)
                .original_version(
                    open.state
                        .as_ref()
                        .map_or(Ok(0), |ledger| ledger.aggregate_version())?,
                )
                .event(&Counted {
                    document_id: document_id.to_owned(),
                    tenant_id: tenant_id.to_owned(),
                    regime: decision.regime.as_str().to_owned(),
                    tax_country: decision.tax_country.clone(),
                    rates: decision.rates.clone(),
                    sales_cents: decision.distance.sales_cents,
                    passed: decision.distance.passed,
                })
                .commit(&self.0)
                .await;
            match committed {
                Ok(_) => return Ok(decision),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(InvoicingError::Busy)
    }

    /// The year's total as of the last sale counted.
    pub async fn total(
        &self,
        company_id: &str,
        year: i64,
    ) -> Result<DistanceSales, InvoicingError> {
        let (open, previous) = self.open_chunk(company_id, year).await?;
        match open.state.or(previous) {
            Some(ledger) => Ok(ledger.distance),
            None => self.start_of(company_id, year).await,
        }
    }

    /// A year before its first sale: nothing yet, past the threshold all
    /// year if the year before was.
    async fn start_of(&self, company_id: &str, year: i64) -> Result<DistanceSales, InvoicingError> {
        let (open, previous) = self.open_chunk(company_id, year - 1).await?;
        let passed = open
            .state
            .or(previous)
            .is_some_and(|ledger| ledger.distance.sales_cents > EU_DISTANCE_THRESHOLD_CENTS);
        Ok(DistanceSales {
            sales_cents: 0,
            passed,
        })
    }

    async fn open_chunk(
        &self,
        company_id: &str,
        year: i64,
    ) -> Result<(OpenChunk, Option<LedgerState>), InvoicingError> {
        let mut previous = None;
        let mut chunk = 0;
        loop {
            let id = ledger_id(company_id, year, chunk);
            let state = ledger_projection().load(&id).execute(&self.0).await?;
            let full = match &state {
                Some(ledger) => ledger.aggregate_version()? >= chunk_size(),
                None => false,
            };
            if !full {
                return Ok((OpenChunk { id, state }, previous));
            }
            previous = state;
            chunk += 1;
        }
    }
}
