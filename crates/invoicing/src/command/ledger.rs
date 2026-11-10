//! A company's EU distance sales, counted across its tenants. The ledger
//! lives in the default tenant of the store, where the company is: an
//! invoice of any of its tenants appends one `Counted` there, at the version
//! the ledger was read at, before taking its number in its tenant's book.
//! Two tenants selling at once cannot both count against a stale total, and
//! a retry finds its sale counted and gets the decision it got.
//!
//! A year's ledger is written in chunks like a book (see the module doc of
//! [`super`]): the pointer of the company and year names the open chunk,
//! the writer that finds it full opens the next with `Opened`, carrying the
//! total, then rolls the pointer. A retry looks for its sale from the chunk
//! open when its document was reserved to the open one.

use std::collections::BTreeMap;

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::ledger::{
    Converted, Counted, DistanceLedger, LedgerPointer, Opened, Rolled,
};
use crate::error::InvoicingError;
use crate::tax::{Decision, DistanceSales, EU_DISTANCE_THRESHOLD_CENTS, Regime};
use crate::value_object::{ledger_id, ledger_pointer_id};

use super::{ATTEMPTS, chunk_size};

#[evento::projection(id = id, name = "timada-invoicing/LedgerState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct LedgerState {
    pub id: String,
    /// The decision each counted sale got, by [`sale_key`] (tenant and
    /// invoice: document ids are only unique within a tenant's store).
    pub counted: BTreeMap<String, Decision>,
    /// The year's total as of the last sale counted (the one the chunk
    /// before ended with for a chunk with none yet).
    pub distance: DistanceSales,
}

pub fn ledger_projection<E: Executor>() -> Projection<E, LedgerState> {
    Projection::new::<DistanceLedger>()
        .handler(on_opened())
        .handler(on_counted())
        .handler(on_converted())
        .strict()
}

#[evento::handler]
async fn on_opened(event: Event<Opened>, ledger: &mut LedgerState) -> anyhow::Result<()> {
    ledger.id = event.aggregate_id.to_owned();
    ledger.distance = DistanceSales {
        sales_cents: event.data.sales_cents,
        passed: event.data.passed,
    };
    Ok(())
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
            euro_rate: None,
        },
    );
    Ok(())
}

#[evento::handler]
async fn on_converted(event: Event<Converted>, ledger: &mut LedgerState) -> anyhow::Result<()> {
    // Written right after the `Counted` it belongs to.
    let data = &event.data;
    if let Some(decision) = ledger
        .counted
        .get_mut(&sale_key(&data.tenant_id, &data.document_id))
    {
        decision.euro_rate = Some(data.rate.to_owned());
    }
    Ok(())
}

/// Which chunk of a company's ledger of a year is open.
#[evento::projection(id = id, name = "timada-invoicing/LedgerPointerState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct LedgerPointerState {
    pub id: String,
    pub chunk: u32,
}

pub fn ledger_pointer_projection<E: Executor>() -> Projection<E, LedgerPointerState> {
    Projection::new::<LedgerPointer>()
        .handler(on_rolled())
        .strict()
}

#[evento::handler]
async fn on_rolled(event: Event<Rolled>, pointer: &mut LedgerPointerState) -> anyhow::Result<()> {
    pointer.id = event.aggregate_id.to_owned();
    pointer.chunk = pointer.chunk.max(event.data.chunk);
    Ok(())
}

/// What a sale is counted under.
fn sale_key(tenant_id: &str, document_id: &str) -> String {
    format!("{tenant_id}/{document_id}")
}

/// The ledgers of the platform: the default tenant of the store.
pub struct Ledger<E: Executor>(pub E);

/// The chunk a sale goes in, as read for one attempt.
struct OpenChunk {
    /// The pointer, `None` while chunk 0 is open.
    pointer: Option<LedgerPointerState>,
    /// The chunk the pointer names (0 without one).
    chunk: u32,
    /// The stream of that chunk.
    id: String,
    /// Its state, `None` when nothing was ever written to it.
    state: Option<LedgerState>,
}

impl OpenChunk {
    /// Whether nothing more may be written to it.
    fn full(&self) -> anyhow::Result<bool> {
        match &self.state {
            Some(ledger) => Ok(ledger.aggregate_version()? >= chunk_size()),
            None => Ok(false),
        }
    }

    /// The version a write to it is made at.
    fn version(&self) -> anyhow::Result<u16> {
        self.state
            .as_ref()
            .map_or(Ok(0), |ledger| ledger.aggregate_version())
    }
}

impl<E: Executor> Ledger<E> {
    /// The decision of an EU distance sale of `company_id` in `year`:
    /// `decide` is given the year's total before it, and the decision is
    /// counted. A sale counted already gets its decision back; it is looked
    /// for from chunk `from_chunk` (the one open when its document was
    /// reserved, see [`Self::open_chunk_number`]) to the open one.
    pub async fn count<F>(
        &self,
        company_id: &str,
        year: i64,
        document_id: &str,
        tenant_id: &str,
        from_chunk: u32,
        decide: F,
    ) -> Result<Decision, InvoicingError>
    where
        F: Fn(DistanceSales) -> Decision,
    {
        let key = sale_key(tenant_id, document_id);
        for _ in 0..ATTEMPTS {
            let open = self.open_chunk(company_id, year).await?;
            if let Some(decision) = self
                .find_counted(company_id, year, &open, from_chunk, &key)
                .await?
            {
                return Ok(decision);
            }
            let before = match open.state.as_ref() {
                Some(ledger) => ledger.distance,
                None => self.start_of(company_id, year).await?,
            };
            let decision = decide(before);
            let mut write = evento::append(&open.id);
            write.original_version(open.version()?).event(&Counted {
                document_id: document_id.to_owned(),
                tenant_id: tenant_id.to_owned(),
                regime: decision.regime.as_str().to_owned(),
                tax_country: decision.tax_country.clone(),
                rates: decision.rates.clone(),
                sales_cents: decision.distance.sales_cents,
                passed: decision.distance.passed,
            });
            if let Some(rate) = &decision.euro_rate {
                write.event(&Converted {
                    document_id: document_id.to_owned(),
                    tenant_id: tenant_id.to_owned(),
                    rate: rate.clone(),
                });
            }
            let committed = write.commit(&self.0).await;
            match committed {
                Ok(_) => return Ok(decision),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(InvoicingError::Busy)
    }

    /// The chunk of the ledger of `company_id` and `year` the pointer names
    /// now: where a sale counted from now on goes, or a later one. What a
    /// document notes when reserved, so its retries know where to look.
    pub async fn open_chunk_number(
        &self,
        company_id: &str,
        year: i64,
    ) -> Result<u32, InvoicingError> {
        Ok(ledger_pointer_projection()
            .load(ledger_pointer_id(company_id, year))
            .execute(&self.0)
            .await?
            .map_or(0, |pointer| pointer.chunk))
    }

    /// The year's total as of the last sale counted.
    pub async fn total(
        &self,
        company_id: &str,
        year: i64,
    ) -> Result<DistanceSales, InvoicingError> {
        match self.read_open_chunk(company_id, year).await?.state {
            Some(ledger) => Ok(ledger.distance),
            None => self.start_of(company_id, year).await,
        }
    }

    /// A year before its first sale: nothing yet, past the threshold all
    /// year if the year before was.
    async fn start_of(&self, company_id: &str, year: i64) -> Result<DistanceSales, InvoicingError> {
        let passed = self
            .read_open_chunk(company_id, year - 1)
            .await?
            .state
            .is_some_and(|ledger| ledger.distance.sales_cents > EU_DISTANCE_THRESHOLD_CENTS);
        Ok(DistanceSales {
            sales_cents: 0,
            passed,
        })
    }

    /// The decision of the sale `key`, when it is counted: looked for from
    /// chunk `from_chunk` to the open one (`open`, read already).
    async fn find_counted(
        &self,
        company_id: &str,
        year: i64,
        open: &OpenChunk,
        from_chunk: u32,
        key: &str,
    ) -> Result<Option<Decision>, InvoicingError> {
        for chunk in from_chunk..=open.chunk {
            let found = if chunk == open.chunk {
                open.state
                    .as_ref()
                    .and_then(|ledger| ledger.counted.get(key).cloned())
            } else {
                ledger_projection()
                    .load(ledger_id(company_id, year, chunk))
                    .execute(&self.0)
                    .await?
                    .and_then(|ledger| ledger.counted.get(key).cloned())
            };
            if found.is_some() {
                return Ok(found);
            }
        }
        Ok(None)
    }

    /// The chunk the next sale goes in: the one the pointer names, rolled
    /// past while full.
    async fn open_chunk(&self, company_id: &str, year: i64) -> Result<OpenChunk, InvoicingError> {
        loop {
            let open = self.read_open_chunk(company_id, year).await?;
            if !open.full()? {
                return Ok(open);
            }
            self.roll(company_id, year, open).await?;
        }
    }

    /// The chunk the pointer names, as it is now.
    async fn read_open_chunk(
        &self,
        company_id: &str,
        year: i64,
    ) -> Result<OpenChunk, InvoicingError> {
        let pointer = ledger_pointer_projection()
            .load(ledger_pointer_id(company_id, year))
            .execute(&self.0)
            .await?;
        let chunk = pointer.as_ref().map_or(0, |pointer| pointer.chunk);
        let id = ledger_id(company_id, year, chunk);
        let state = ledger_projection().load(&id).execute(&self.0).await?;
        if chunk > 0 && state.is_none() {
            // Its `Opened` is committed before the pointer names it: a read
            // that sees the one without the other is behind, and counting
            // from nothing would forget the year's total.
            return Err(anyhow::anyhow!(
                "chunk {chunk} of the ledger of {company_id} for {year} is named but not opened"
            )
            .into());
        }
        Ok(OpenChunk {
            pointer,
            chunk,
            id,
            state,
        })
    }

    /// Opens the chunk after the full one `open`, from the total it ended
    /// with, then points at it. Another writer doing the same at the same
    /// moment carries the same total, so either write losing its race is
    /// the work done.
    async fn roll(
        &self,
        company_id: &str,
        year: i64,
        open: OpenChunk,
    ) -> Result<(), InvoicingError> {
        let Some(full) = open.state else {
            return Ok(());
        };
        let next = open.chunk + 1;
        let opened = evento::append(ledger_id(company_id, year, next))
            .original_version(0)
            .event(&Opened {
                sales_cents: full.distance.sales_cents,
                passed: full.distance.passed,
            })
            .commit(&self.0)
            .await;
        match opened {
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => {}
            Err(err) => return Err(err.into()),
        }
        let rolled = Rolled { chunk: next };
        let pointed = match open.pointer {
            Some(pointer) => pointer.write()?.event(&rolled).commit(&self.0).await,
            None => {
                evento::append(ledger_pointer_id(company_id, year))
                    .original_version(0)
                    .event(&rolled)
                    .commit(&self.0)
                    .await
            }
        };
        match pointed {
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}
