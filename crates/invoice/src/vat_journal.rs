//! The VAT of what was invoiced, line by line, and the return it adds up to.
//!
//! `invoice_vat_journal` holds one row per VAT rate of each **issued**
//! invoice, and — negative — of each credit note, its amount spread over the
//! invoice's rates. Fed by the `invoice-vat-journal` subscription.
//! [`vat_report`] reads a calendar quarter out of it: the shop's own VAT,
//! the one-stop-shop return by member state and rate, and exports.
//!
//! Dates are UTC, like every date of the framework: an invoice issued in the
//! first hour of a quarter, Paris time, belongs to the quarter before.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_tax::{TaxTreatment, VatLine};

use crate::{
    aggregator::{CreditNoteIssued, InvoiceIssued, InvoiceVoided},
    query::{load_credit_note, load_invoice},
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const VAT_JOURNAL_SUBSCRIPTION: &str = "invoice-vat-journal";

/// Not strict: it follows two aggregates, and only what makes or unmakes VAT.
pub fn vat_journal_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(VAT_JOURNAL_SUBSCRIPTION)
        .handler(journal_on_invoice_issued())
        .handler(unjournal_on_invoice_voided())
        .handler(journal_on_credit_note_issued())
        .handler(convert_on_order_rate_pinned())
}

/// A calendar quarter: what a VAT return covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VatPeriod {
    pub year: i32,
    /// 1 to 4.
    pub quarter: u8,
}

/// Days from 1970-01-01 to a civil date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

impl VatPeriod {
    pub fn new(year: i32, quarter: u8) -> Option<Self> {
        ((1..=4).contains(&quarter) && (1970..=9999).contains(&year))
            .then_some(Self { year, quarter })
    }

    /// The quarter a moment (Unix seconds, UTC) falls in.
    pub fn of(unix_secs: u64) -> Self {
        let (year, month, _) = timada_core::format::civil_date(unix_secs);
        Self {
            year: year as i32,
            quarter: (month - 1) / 3 + 1,
        }
    }

    /// `2026-T3`, as URLs and file names spell it.
    pub fn code(&self) -> String {
        format!("{}-T{}", self.year, self.quarter)
    }

    pub fn parse(code: &str) -> Option<Self> {
        let (year, quarter) = code.split_once("-T")?;
        Self::new(year.parse().ok()?, quarter.parse().ok()?)
    }

    /// "3e trimestre 2026".
    pub fn label(&self) -> String {
        let rank = if self.quarter == 1 {
            "1er"
        } else {
            &format!("{}e", self.quarter)
        };
        format!("{rank} trimestre {}", self.year)
    }

    pub fn previous(&self) -> Self {
        match self.quarter {
            1 => Self {
                year: self.year - 1,
                quarter: 4,
            },
            quarter => Self {
                year: self.year,
                quarter: quarter - 1,
            },
        }
    }

    pub fn next(&self) -> Self {
        match self.quarter {
            4 => Self {
                year: self.year + 1,
                quarter: 1,
            },
            quarter => Self {
                year: self.year,
                quarter: quarter + 1,
            },
        }
    }

    /// `[start, end)` in Unix seconds.
    pub fn bounds(&self) -> (i64, i64) {
        let start = |period: &Self| {
            days_from_civil(
                i64::from(period.year),
                i64::from(period.quarter - 1) * 3 + 1,
                1,
            ) * 86_400
        };
        (start(self), start(&self.next()))
    }
}

/// The part of a credit note that falls on one VAT rate of its invoice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreditShare {
    pub rate_bp: u16,
    pub base_minor: i64,
    pub vat_minor: i64,
}

/// Spreads a credit note's amount (all taxes included) over the VAT rates of
/// its invoice, in proportion to what each rate was charged; the cents left
/// by the rounding go to the largest shares, so the parts add up to the
/// amount. Each share is then split back into base and VAT at its rate.
pub fn apportion_credit(vat_lines: &[VatLine], amount_minor: i64) -> Vec<CreditShare> {
    let charged: i64 = vat_lines.iter().map(|line| line.total.minor).sum();
    if charged <= 0 || amount_minor <= 0 {
        return Vec::new();
    }
    let mut shares: Vec<(usize, i64, i64)> = vat_lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let exact = i128::from(amount_minor) * i128::from(line.total.minor);
            let floor = (exact / i128::from(charged)) as i64;
            let remainder = (exact % i128::from(charged)) as i64;
            (index, floor, remainder)
        })
        .collect();
    let mut left = amount_minor - shares.iter().map(|(_, floor, _)| floor).sum::<i64>();
    let mut by_remainder: Vec<usize> = (0..shares.len()).collect();
    by_remainder.sort_by_key(|&at| (std::cmp::Reverse(shares[at].2), at));
    for at in by_remainder {
        if left == 0 {
            break;
        }
        shares[at].1 += 1;
        left -= 1;
    }
    shares
        .into_iter()
        .filter(|(_, share, _)| *share != 0)
        .map(|(index, share, _)| {
            let rate = i128::from(vat_lines[index].rate_bp);
            // VAT inside an all-taxes-included amount, rounded half up.
            let vat =
                ((i128::from(share) * rate * 2 + (10_000 + rate)) / ((10_000 + rate) * 2)) as i64;
            CreditShare {
                rate_bp: vat_lines[index].rate_bp,
                base_minor: share - vat,
                vat_minor: vat,
            }
        })
        .collect()
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

/// What a journal row says of the document it belongs to.
struct Document<'a> {
    kind: &'static str,
    id: &'a str,
    number: &'a str,
    invoice_id: &'a str,
    order_id: &'a str,
    issued_at: u64,
    invoice_issued_at: u64,
    zone_code: &'a str,
    treatment: &'a str,
    country_code: &'a str,
    currency: &'a str,
    /// The VAT number of the business the sale was reverse-charged to.
    buyer_vat_number: Option<&'a str>,
}

/// `(rate, base, VAT)`; no rate: an invoice that carries no VAT breakdown.
type JournalRow = (Option<u16>, i64, i64);

/// Rewrites a document's rows.
async fn write_rows(
    db: &SqlitePool,
    document: &Document<'_>,
    rows: &[JournalRow],
) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM invoice_vat_journal WHERE document_id = ?")
        .bind(document.id)
        .execute(&mut *tx)
        .await?;
    for (rate_bp, base_minor, vat_minor) in rows {
        sqlx::query(
            "INSERT INTO invoice_vat_journal
                (document_kind, document_id, document_number, invoice_id, order_id, issued_at,
                 invoice_issued_at, zone_code, treatment, country_code, rate_bp, base_minor,
                 vat_minor, currency, buyer_vat_number)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(document.kind)
        .bind(document.id)
        .bind(document.number)
        .bind(document.invoice_id)
        .bind(document.order_id)
        .bind(document.issued_at as i64)
        .bind(document.invoice_issued_at as i64)
        .bind(document.zone_code)
        .bind(document.treatment)
        .bind(document.country_code)
        .bind(rate_bp.map(i64::from))
        .bind(base_minor)
        .bind(vat_minor)
        .bind(document.currency)
        .bind(document.buyer_vat_number)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The rows as the books hold them: a document in another currency goes in
/// at the rate pinned on its order — the one its own VAT mention was stated
/// at — and comes out in the currency of the books. A document whose order
/// has no rate (yet) keeps its currency: the report leaves it aside and says
/// so, rather than add pounds to euros.
async fn in_books<E: Executor>(
    executor: &E,
    order_id: &str,
    currency: &str,
    rows: Vec<JournalRow>,
) -> anyhow::Result<(Vec<JournalRow>, String)> {
    let rate = timada_order::load_order_details(executor, order_id)
        .await?
        .and_then(|order| order.exchange_rate)
        .filter(|rate| rate.currency == currency);
    let Some(rate) = rate else {
        return Ok((rows, currency.to_owned()));
    };
    let mut converted = Vec::with_capacity(rows.len());
    for (rate_bp, base_minor, vat_minor) in rows {
        let base = rate.to_base(&timada_core::Money::new(base_minor, currency))?;
        let vat = rate.to_base(&timada_core::Money::new(vat_minor, currency))?;
        converted.push((rate_bp, base.minor, vat.minor));
    }
    Ok((converted, rate.base))
}

/// Where the goods went: the member state of consumption. The order knows;
/// an order that cannot be read leaves the invoice's billing country.
async fn destination<E: Executor>(
    executor: &E,
    order_id: &str,
    fallback: &str,
) -> anyhow::Result<String> {
    Ok(timada_order::load_order_details(executor, order_id)
        .await?
        .map_or_else(
            || fallback.to_uppercase(),
            |order| order.delivery_address.country_code.to_uppercase(),
        ))
}

#[evento::subscription]
async fn journal_on_invoice_issued<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<InvoiceIssued>,
) -> anyhow::Result<()> {
    let Some(invoice) = load_invoice(ctx.executor, &event.aggregate_id).await? else {
        anyhow::bail!("invoice {} issued but cannot be loaded", event.aggregate_id);
    };
    let country = destination(
        ctx.executor,
        &invoice.order_id,
        &invoice.billing_address.country_code,
    )
    .await?;
    let (zone_code, treatment, rows): (&str, &str, Vec<JournalRow>) = match &invoice.tax {
        Some(tax) => (
            &tax.zone_code,
            regime(&invoice).map_or(tax.treatment.as_str(), |(_, regime)| regime),
            tax.vat_lines
                .iter()
                .map(|line| (Some(line.rate_bp), line.base.minor, line.vat.minor))
                .collect(),
        ),
        // From before tax zones: the total, its VAT unknown.
        None => ("", "unknown", vec![(None, invoice.total.minor, 0)]),
    };
    let (rows, currency) = in_books(
        ctx.executor,
        &invoice.order_id,
        &invoice.total.currency,
        rows,
    )
    .await?;
    write_rows(
        &pool(ctx)?,
        &Document {
            kind: "invoice",
            id: &invoice.id,
            number: &event.data.invoice_number,
            invoice_id: &invoice.id,
            order_id: &invoice.order_id,
            issued_at: event.timestamp,
            invoice_issued_at: event.timestamp,
            zone_code,
            treatment,
            country_code: &country,
            currency: &currency,
            buyer_vat_number: invoice
                .reverse_charge
                .as_ref()
                .and(invoice.company.as_ref())
                .map(|company| company.vat_number.as_str()),
        },
        &rows,
    )
    .await
}

/// Only an invoice without anything to pay is voided once issued; whatever
/// it had in the journal leaves it.
#[evento::subscription]
async fn unjournal_on_invoice_voided<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<InvoiceVoided>,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM invoice_vat_journal WHERE document_id = ?")
        .bind(&event.aggregate_id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}

/// A rate pinned on an order *after* its documents were journalled (no rate
/// could be had when it was placed): the rows waiting in the order's own
/// currency go to the books now. Rows already converted are in the base
/// currency and are left alone, so a redelivery changes nothing — and the
/// usual case, a rate pinned with the order, finds no row at all.
#[evento::subscription]
async fn convert_on_order_rate_pinned<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<timada_order::aggregator::OrderRatePinned>,
) -> anyhow::Result<()> {
    let db = pool(ctx)?;
    let rate = &event.data.rate;
    let waiting: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT rowid, base_minor, vat_minor FROM invoice_vat_journal
         WHERE order_id = ? AND currency = ?",
    )
    .bind(&event.aggregate_id)
    .bind(&rate.currency)
    .fetch_all(&db)
    .await?;
    for (rowid, base_minor, vat_minor) in waiting {
        let base = rate.to_base(&timada_core::Money::new(base_minor, &rate.currency))?;
        let vat = rate.to_base(&timada_core::Money::new(vat_minor, &rate.currency))?;
        sqlx::query(
            "UPDATE invoice_vat_journal SET base_minor = ?, vat_minor = ?, currency = ?
             WHERE rowid = ?",
        )
        .bind(base.minor)
        .bind(vat.minor)
        .bind(&rate.base)
        .bind(rowid)
        .execute(&db)
        .await?;
    }
    Ok(())
}

#[evento::subscription]
async fn journal_on_credit_note_issued<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CreditNoteIssued>,
) -> anyhow::Result<()> {
    let Some(note) = load_credit_note(ctx.executor, &event.aggregate_id).await? else {
        anyhow::bail!("credit note {} cannot be loaded", event.aggregate_id);
    };
    let Some(invoice) = load_invoice(ctx.executor, &note.invoice_id).await? else {
        anyhow::bail!("credit note {} has no invoice", note.id);
    };
    let country = destination(
        ctx.executor,
        &invoice.order_id,
        &invoice.billing_address.country_code,
    )
    .await?;
    let (zone_code, treatment, rows): (&str, &str, Vec<JournalRow>) = match &invoice.tax {
        Some(tax) => (
            &tax.zone_code,
            regime(&invoice).map_or(tax.treatment.as_str(), |(_, regime)| regime),
            apportion_credit(&tax.vat_lines, note.amount.minor)
                .into_iter()
                .map(|share| (Some(share.rate_bp), -share.base_minor, -share.vat_minor))
                .collect(),
        ),
        None => ("", "unknown", vec![(None, -note.amount.minor, 0)]),
    };
    let (rows, currency) =
        in_books(ctx.executor, &invoice.order_id, &note.amount.currency, rows).await?;
    write_rows(
        &pool(ctx)?,
        &Document {
            kind: "credit_note",
            id: &note.id,
            number: &note.credit_note_number,
            invoice_id: &invoice.id,
            order_id: &invoice.order_id,
            issued_at: note.issued_at,
            invoice_issued_at: invoice.issued_at.unwrap_or(note.issued_at),
            zone_code,
            treatment,
            country_code: &country,
            currency: &currency,
            buyer_vat_number: invoice
                .reverse_charge
                .as_ref()
                .and(invoice.company.as_ref())
                .map(|company| company.vat_number.as_str()),
        },
        &rows,
    )
    .await
}

/// How the journal names the regime of an intra-community supply: taxed like
/// an export, reported apart.
const REVERSE_CHARGE: &str = "reverse_charge";

/// The journal's regime for an invoice: its treatment, unless the sale was
/// reverse-charged.
fn regime(invoice: &crate::query::InvoiceView) -> Option<(&str, &'static str)> {
    invoice.tax.as_ref().map(|tax| {
        let regime = if invoice.reverse_charge.is_some() {
            REVERSE_CHARGE
        } else {
            tax.treatment.as_str()
        };
        (tax.zone_code.as_str(), regime)
    })
}

/// Base and VAT at one rate, for one country.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VatReportLine {
    /// ISO 3166-1 alpha-2, upper case: where the goods were delivered.
    pub country_code: String,
    pub rate_bp: u16,
    pub base_minor: i64,
    pub vat_minor: i64,
}

/// What one business of another member state bought without VAT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntraCommunityLine {
    pub country_code: String,
    pub buyer_vat_number: String,
    pub base_minor: i64,
}

/// VAT given back in this quarter on sales declared in an earlier one: the
/// one-stop-shop return corrects the earlier period, per member state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VatCorrection {
    pub corrected: VatPeriod,
    pub country_code: String,
    /// Negative: VAT that goes back.
    pub vat_minor: i64,
}

/// A quarter's VAT, as three returns read it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VatReport {
    pub currency: String,
    /// The shop's own VAT, by rate, credit notes of the quarter deducted
    /// whatever quarter their invoice was in — as a domestic return does.
    pub domestic: Vec<VatReportLine>,
    /// The one-stop-shop return: by member state of consumption and rate, net
    /// of the credit notes on invoices of the *same* quarter.
    pub oss: Vec<VatReportLine>,
    /// Credit notes of the quarter on one-stop-shop invoices of earlier ones.
    pub oss_corrections: Vec<VatCorrection>,
    /// Intra-community supplies — sold without VAT to businesses of other
    /// member states, which account for it at home: the pre-tax total per
    /// member state and buyer, credit notes deducted. What the recapitulative
    /// statement of customers is filled from.
    pub intra_community: Vec<IntraCommunityLine>,
    /// Sold without VAT outside the Union's VAT area: the pre-tax total.
    pub exports_base_minor: i64,
    /// Invoices from before tax zones, which carry no breakdown: how many,
    /// and their total, all taxes included.
    pub unbroken: (i64, i64),
    /// Documents of the quarter left out because they are in another
    /// currency than the report's: sales in a foreign currency whose order
    /// has no exchange rate pinned. Pin one (`timada_order`'s
    /// `pin_exchange_rate`) and the journal takes them in.
    pub unconverted: i64,
}

impl VatReport {
    pub fn oss_vat_minor(&self) -> i64 {
        self.oss.iter().map(|line| line.vat_minor).sum::<i64>()
            + self
                .oss_corrections
                .iter()
                .map(|correction| correction.vat_minor)
                .sum::<i64>()
    }

    pub fn domestic_vat_minor(&self) -> i64 {
        self.domestic.iter().map(|line| line.vat_minor).sum()
    }
}

/// The VAT of a quarter, in the currency of the books: the journal converts
/// what is sold in another one at the rate pinned on the order. Documents it
/// could not convert are counted in [`VatReport::unconverted`]; the report's
/// currency is the one with the most rows.
pub async fn vat_report(db: &SqlitePool, period: VatPeriod) -> sqlx::Result<VatReport> {
    let (start, end) = period.bounds();
    let currency: Option<String> = sqlx::query_scalar(
        "SELECT currency FROM invoice_vat_journal
         WHERE issued_at >= ?1 AND issued_at < ?2
         GROUP BY currency ORDER BY COUNT(*) DESC, currency LIMIT 1",
    )
    .bind(start)
    .bind(end)
    .fetch_optional(db)
    .await?;
    let Some(currency) = currency else {
        return Ok(VatReport::default());
    };

    let lines = |treatment: TaxTreatment, within_quarter_only: bool| {
        let currency = currency.clone();
        async move {
            let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
                "SELECT country_code, rate_bp, SUM(base_minor), SUM(vat_minor)
                 FROM invoice_vat_journal
                 WHERE issued_at >= ?1 AND issued_at < ?2 AND treatment = ?3 AND currency = ?4
                   AND rate_bp IS NOT NULL AND (?5 = 0 OR invoice_issued_at >= ?1)
                 GROUP BY country_code, rate_bp
                 ORDER BY country_code, rate_bp",
            )
            .bind(start)
            .bind(end)
            .bind(treatment.as_str())
            .bind(&currency)
            .bind(within_quarter_only)
            .fetch_all(db)
            .await?;
            Ok::<_, sqlx::Error>(
                rows.into_iter()
                    .map(
                        |(country_code, rate_bp, base_minor, vat_minor)| VatReportLine {
                            country_code,
                            rate_bp: rate_bp as u16,
                            base_minor,
                            vat_minor,
                        },
                    )
                    .collect::<Vec<_>>(),
            )
        }
    };
    let domestic = lines(TaxTreatment::Domestic, false).await?;
    let oss = lines(TaxTreatment::DestinationVat, true).await?;
    let exports_base_minor: i64 = lines(TaxTreatment::Export, false)
        .await?
        .iter()
        .map(|line| line.base_minor)
        .sum();

    let intra_community: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT country_code, COALESCE(buyer_vat_number, ''), SUM(base_minor)
         FROM invoice_vat_journal
         WHERE issued_at >= ?1 AND issued_at < ?2 AND treatment = ?3 AND currency = ?4
         GROUP BY country_code, buyer_vat_number
         ORDER BY country_code, buyer_vat_number",
    )
    .bind(start)
    .bind(end)
    .bind(REVERSE_CHARGE)
    .bind(&currency)
    .fetch_all(db)
    .await?;

    // SQLite's date functions are UTC, like `VatPeriod::of`.
    let corrections: Vec<(i32, i64, String, i64)> = sqlx::query_as(
        "SELECT CAST(strftime('%Y', invoice_issued_at, 'unixepoch') AS INTEGER) AS year,
                (CAST(strftime('%m', invoice_issued_at, 'unixepoch') AS INTEGER) + 2) / 3 AS quarter,
                country_code, SUM(vat_minor)
         FROM invoice_vat_journal
         WHERE issued_at >= ?1 AND issued_at < ?2 AND treatment = ?3 AND currency = ?4
           AND document_kind = 'credit_note' AND invoice_issued_at < ?1
         GROUP BY year, quarter, country_code
         ORDER BY year, quarter, country_code",
    )
    .bind(start)
    .bind(end)
    .bind(TaxTreatment::DestinationVat.as_str())
    .bind(&currency)
    .fetch_all(db)
    .await?;

    let unconverted: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT document_id) FROM invoice_vat_journal
         WHERE issued_at >= ?1 AND issued_at < ?2 AND currency <> ?3",
    )
    .bind(start)
    .bind(end)
    .bind(&currency)
    .fetch_one(db)
    .await?;
    let unbroken: (i64, Option<i64>) = sqlx::query_as(
        "SELECT COUNT(DISTINCT CASE WHEN document_kind = 'invoice' THEN document_id END),
                SUM(base_minor)
         FROM invoice_vat_journal
         WHERE issued_at >= ?1 AND issued_at < ?2 AND rate_bp IS NULL AND currency = ?3",
    )
    .bind(start)
    .bind(end)
    .bind(&currency)
    .fetch_one(db)
    .await?;

    Ok(VatReport {
        currency,
        domestic,
        oss,
        oss_corrections: corrections
            .into_iter()
            .map(|(year, quarter, country_code, vat_minor)| VatCorrection {
                corrected: VatPeriod {
                    year,
                    quarter: quarter as u8,
                },
                country_code,
                vat_minor,
            })
            .collect(),
        intra_community: intra_community
            .into_iter()
            .map(
                |(country_code, buyer_vat_number, base_minor)| IntraCommunityLine {
                    country_code,
                    buyer_vat_number,
                    base_minor,
                },
            )
            .collect(),
        exports_base_minor,
        unbroken: (unbroken.0, unbroken.1.unwrap_or(0)),
        unconverted,
    })
}

fn decimal(minor: i64) -> String {
    let sign = if minor < 0 { "-" } else { "" };
    format!("{sign}{}.{:02}", minor.abs() / 100, minor.abs() % 100)
}

/// The one-stop-shop return of a [`VatReport`] as CSV: a `sale` row per
/// member state and rate, a `correction` row per corrected quarter and member
/// state. Amounts are decimal, with a point; the rate is a percentage.
pub fn oss_csv(period: VatPeriod, report: &VatReport) -> String {
    let mut csv = String::from(
        "kind,period,corrected_period,member_state,vat_rate,taxable_amount,vat_amount,currency\n",
    );
    for line in &report.oss {
        csv.push_str(&format!(
            "sale,{},,{},{},{},{},{}\n",
            period.code(),
            line.country_code,
            decimal(i64::from(line.rate_bp)),
            decimal(line.base_minor),
            decimal(line.vat_minor),
            report.currency
        ));
    }
    for correction in &report.oss_corrections {
        csv.push_str(&format!(
            "correction,{},{},{},,,{},{}\n",
            period.code(),
            correction.corrected.code(),
            correction.country_code,
            decimal(correction.vat_minor),
            report.currency
        ));
    }
    csv
}

#[cfg(test)]
mod tests {
    use timada_core::Money;

    use super::*;

    fn line(rate_bp: u16, total: i64) -> VatLine {
        let vat = total * i64::from(rate_bp) / (10_000 + i64::from(rate_bp));
        VatLine {
            rate_bp,
            base: Money::eur(total - vat),
            vat: Money::eur(vat),
            total: Money::eur(total),
        }
    }

    #[test]
    fn quarters_have_bounds_codes_and_neighbours() {
        let q3 = VatPeriod::of(1_789_000_000); // 2026-09-09
        assert_eq!(
            q3,
            VatPeriod {
                year: 2026,
                quarter: 3
            }
        );
        assert_eq!(q3.code(), "2026-T3");
        assert_eq!(VatPeriod::parse("2026-T3"), Some(q3));
        assert_eq!(q3.label(), "3e trimestre 2026");
        assert_eq!(
            VatPeriod {
                year: 2027,
                quarter: 1
            }
            .label(),
            "1er trimestre 2027"
        );
        for bad in ["2026-T5", "2026-T0", "2026", "T3", "x-T1", "1500-T1"] {
            assert_eq!(VatPeriod::parse(bad), None, "{bad}");
        }

        // 2026-07-01 and 2026-10-01, 00:00 UTC.
        assert_eq!(q3.bounds(), (1_782_864_000, 1_790_812_800));
        let (start, end) = q3.bounds();
        assert_eq!(VatPeriod::of(start as u64), q3);
        assert_eq!(VatPeriod::of(end as u64 - 1), q3);
        assert_eq!(VatPeriod::of(end as u64), q3.next());
        let q4 = q3.next();
        assert_eq!(
            q4.next(),
            VatPeriod {
                year: 2027,
                quarter: 1
            }
        );
        assert_eq!(q4.next().previous(), q4);
        assert_eq!(
            VatPeriod {
                year: 2027,
                quarter: 1
            }
            .previous(),
            q4
        );
    }

    #[test]
    fn a_credit_note_is_spread_over_the_rates_of_its_invoice() {
        // One rate: the whole amount, split back into base and VAT.
        assert_eq!(
            apportion_credit(&[line(2_000, 12_000)], 6_000),
            [CreditShare {
                rate_bp: 2_000,
                base_minor: 5_000,
                vat_minor: 1_000
            }]
        );
        // Two rates, in proportion; the parts add up to the amount.
        let shares = apportion_credit(&[line(2_000, 10_000), line(550, 5_000)], 1_000);
        assert_eq!(
            shares,
            [
                CreditShare {
                    rate_bp: 2_000,
                    base_minor: 556,
                    vat_minor: 111
                },
                CreditShare {
                    rate_bp: 550,
                    base_minor: 316,
                    vat_minor: 17
                },
            ]
        );
        assert_eq!(
            shares
                .iter()
                .map(|s| s.base_minor + s.vat_minor)
                .sum::<i64>(),
            1_000
        );
        // Exports carry no VAT; the cent the rounding leaves is never lost.
        assert_eq!(
            apportion_credit(&[line(0, 9_999)], 9_999),
            [CreditShare {
                rate_bp: 0,
                base_minor: 9_999,
                vat_minor: 0
            }]
        );
        let thirds = apportion_credit(&[line(2_000, 100), line(1_000, 100), line(550, 100)], 100);
        assert_eq!(
            thirds
                .iter()
                .map(|s| s.base_minor + s.vat_minor)
                .sum::<i64>(),
            100
        );
        assert!(apportion_credit(&[], 100).is_empty());
        assert!(apportion_credit(&[line(2_000, 100)], 0).is_empty());
    }

    #[test]
    fn amounts_are_written_with_a_point() {
        assert_eq!(decimal(123_456), "1234.56");
        assert_eq!(decimal(-5), "-0.05");
        assert_eq!(decimal(2_000), "20.00");
    }
}
