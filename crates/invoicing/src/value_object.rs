//! Identifiers, the persisted value types, the VAT arithmetic and the
//! French way of writing money, rates and dates.

use std::collections::BTreeMap;

/// How many documents one book stream holds before the series carries on in
/// the next; far below the 65 535 events a stream can take, so loading a
/// book stays cheap.
pub const BOOK_CHUNK: u16 = 1000;
/// The prefix of the invoice series (`F2026-000042`).
pub const INVOICE_PREFIX: &str = "F";
/// The prefix of the credit-note series (`A2026-000003`, for *avoir*).
pub const CREDIT_NOTE_PREFIX: &str = "A";
/// The layout a document is drawn with. Recorded in the document: a new
/// layout is a new version, and a document keeps the one it was issued with.
pub const TEMPLATE_VERSION: u16 = 1;
/// How a storefront order was paid.
pub const PAYMENT_METHOD_CARD: &str = "card";
/// What a merchant under the VAT exemption for small businesses writes on
/// its invoices.
pub const FRANCHISE_MENTION: &str = "TVA non applicable, art. 293 B du CGI";

/// The index stream of an order's invoice.
pub fn invoice_id(order_id: &str) -> String {
    evento::hash_ids(vec!["invoice", order_id])
}

/// The index stream of the credit note for one refund.
pub fn credit_note_id(refund_id: &str) -> String {
    evento::hash_ids(vec!["credit-note", refund_id])
}

/// The stream of one chunk of a series (`F2026`, chunk 0, 1…).
pub fn book_id(series: &str, chunk: u32) -> String {
    evento::hash_ids(vec![
        "invoice-book".to_owned(),
        series.to_owned(),
        chunk.to_string(),
    ])
}

/// The series of a document issued at `at` (Unix seconds): its prefix and
/// the calendar year, in UTC.
pub fn series_of(prefix: &str, at: u64) -> String {
    format!("{prefix}{}", civil_date(at).0)
}

/// What a document's number reads as: `F2026-000042`.
pub fn document_label(series: &str, number: u64) -> String {
    format!("{series}-{number:06}")
}

/// A postal address as a document prints it. Frozen in `events.lock`: the
/// fields never change.
#[derive(Debug, Clone, Default, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct Address {
    /// The person or company.
    pub name: String,
    pub line1: String,
    pub line2: Option<String>,
    pub postal_code: String,
    pub city: String,
    /// ISO 3166-1 alpha-2, uppercase.
    pub country: String,
}

/// The merchant as the law asks a document to name it, copied when it was
/// issued. Frozen in `events.lock`: the fields never change.
#[derive(Debug, Clone, Default, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct Seller {
    /// The registered name and address.
    pub address: Address,
    pub legal_form: String,
    /// In euro cents.
    pub share_capital_cents: Option<u64>,
    pub siret: String,
    pub rcs_city: Option<String>,
    pub contact_email: String,
    /// `None` under the VAT exemption for small businesses.
    pub vat_number: Option<String>,
}

impl Seller {
    /// The first nine digits of the SIRET.
    pub fn siren(&self) -> &str {
        &self.siret[..9.min(self.siret.len())]
    }
}

/// One line of an invoice, as it was sold. Frozen in `events.lock`: the
/// fields never change.
#[derive(Debug, Clone, Default, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct InvoiceLine {
    pub name: String,
    pub variant_label: Option<String>,
    pub sku: Option<String>,
    pub quantity: u32,
    /// The price of one, VAT included, in minor units.
    pub unit_amount_cents: u64,
    /// In basis points (`550` is 5.5 %); `None` when no VAT is charged.
    pub vat_rate_bps: Option<u32>,
}

impl InvoiceLine {
    /// The price of the whole line, VAT included.
    pub fn total_cents(&self) -> u64 {
        self.unit_amount_cents * u64::from(self.quantity)
    }

    /// The price of one without VAT, rounded to the cent: what the line
    /// shows. The totals by rate are computed on the sums, not from this.
    pub fn unit_excl_vat_cents(&self) -> u64 {
        match self.vat_rate_bps {
            Some(rate) => excl_vat(self.unit_amount_cents, rate),
            None => self.unit_amount_cents,
        }
    }
}

/// What a document says about one VAT rate: the base it applies to and the
/// tax. Frozen in `events.lock`: the fields never change.
#[derive(Debug, Clone, Default, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct VatSubtotal {
    /// In basis points.
    pub rate_bps: u32,
    /// Without VAT, in minor units.
    pub base_cents: u64,
    pub vat_cents: u64,
}

impl VatSubtotal {
    /// The sum, VAT included.
    pub fn total_cents(&self) -> u64 {
        self.base_cents + self.vat_cents
    }
}

/// The price without VAT of `incl` (VAT included at `rate_bps`), rounded to
/// the nearest cent, halves up.
pub fn excl_vat(incl: u64, rate_bps: u32) -> u64 {
    let divisor = 10_000 + u64::from(rate_bps);
    (incl * 10_000 + divisor / 2) / divisor
}

/// Splits sums VAT included by rate into base and tax. Each rate is worked
/// out on its whole sum (rounding once per rate, never per line), and the
/// tax is what remains of the sum, so base plus tax is always the sum.
pub fn vat_breakdown<I>(sums: I) -> Vec<VatSubtotal>
where
    I: IntoIterator<Item = (u32, u64)>,
{
    let mut by_rate: BTreeMap<u32, u64> = BTreeMap::new();
    for (rate, incl) in sums {
        *by_rate.entry(rate).or_default() += incl;
    }
    by_rate
        .into_iter()
        .rev()
        .map(|(rate_bps, incl)| {
            let base_cents = excl_vat(incl, rate_bps);
            VatSubtotal {
                rate_bps,
                base_cents,
                vat_cents: incl - base_cents,
            }
        })
        .collect()
}

/// The VAT of an invoice's lines; empty when none carries a rate.
pub fn lines_breakdown(lines: &[InvoiceLine]) -> Vec<VatSubtotal> {
    vat_breakdown(
        lines
            .iter()
            .filter_map(|line| line.vat_rate_bps.map(|rate| (rate, line.total_cents()))),
    )
}

/// Spreads `amount` (VAT included) over the rates of an invoice in
/// proportion to what each was for, the cents left by rounding going to the
/// largest remainders, so the parts add up to `amount` exactly; each part is
/// then split into base and tax.
pub fn prorate(invoice: &[VatSubtotal], amount: u64) -> Vec<VatSubtotal> {
    let total: u64 = invoice.iter().map(VatSubtotal::total_cents).sum();
    if total == 0 {
        return Vec::new();
    }
    let mut parts: Vec<(u32, u64, u128)> = invoice
        .iter()
        .map(|subtotal| {
            let exact = u128::from(amount) * u128::from(subtotal.total_cents());
            let share = (exact / u128::from(total)) as u64;
            (subtotal.rate_bps, share, exact % u128::from(total))
        })
        .collect();
    let mut left = amount - parts.iter().map(|(_, share, _)| share).sum::<u64>();
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by(|a, b| parts[*b].2.cmp(&parts[*a].2));
    for index in order {
        if left == 0 {
            break;
        }
        parts[index].1 += 1;
        left -= 1;
    }
    vat_breakdown(parts.into_iter().map(|(rate, share, _)| (rate, share)))
}

/// `1250` as `12,50 €`: what a French reader expects, the thousands
/// separated by a no-break space.
pub fn format_amount_fr(cents: u64, currency: &str) -> String {
    let whole = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, digit) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push('\u{a0}');
        }
        grouped.push(digit);
    }
    let symbol = match currency {
        "EUR" => "€",
        "USD" => "$",
        "GBP" => "£",
        other => other,
    };
    format!("{grouped},{:02}\u{a0}{symbol}", cents % 100)
}

/// `2000` as `20 %`, `550` as `5,5 %`.
pub fn format_rate_fr(rate_bps: u32) -> String {
    let whole = rate_bps / 100;
    match rate_bps % 100 {
        0 => format!("{whole}\u{a0}%"),
        part if part.is_multiple_of(10) => format!("{whole},{}\u{a0}%", part / 10),
        part => format!("{whole},{part:02}\u{a0}%"),
    }
}

/// Unix seconds as `14/10/2026`, the UTC day.
pub fn format_date_fr(at: u64) -> String {
    let (year, month, day) = civil_date(at);
    format!("{day:02}/{month:02}/{year}")
}

/// The UTC calendar date of Unix seconds (Howard Hinnant's algorithm).
pub fn civil_date(at: u64) -> (i64, u32, u32) {
    let days = (at / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(unit: u64, quantity: u32, rate: Option<u32>) -> InvoiceLine {
        InvoiceLine {
            name: "x".into(),
            quantity,
            unit_amount_cents: unit,
            vat_rate_bps: rate,
            ..Default::default()
        }
    }

    #[test]
    fn vat_is_worked_out_once_per_rate() {
        // 3 × 0.99 at 20 % rounds once on 2.97, not three times on 0.99.
        let lines = [
            line(99, 3, Some(2000)),
            line(1250, 2, Some(2000)),
            line(1999, 1, Some(550)),
        ];
        let breakdown = lines_breakdown(&lines);
        assert_eq!(
            breakdown,
            vec![
                VatSubtotal {
                    rate_bps: 2000,
                    base_cents: 2331,
                    vat_cents: 466
                },
                VatSubtotal {
                    rate_bps: 550,
                    base_cents: 1895,
                    vat_cents: 104
                },
            ]
        );
        let total: u64 = breakdown.iter().map(VatSubtotal::total_cents).sum();
        assert_eq!(
            total,
            lines.iter().map(InvoiceLine::total_cents).sum::<u64>()
        );
        assert_eq!(lines[1].unit_excl_vat_cents(), 1042);
        assert!(lines_breakdown(&[line(1000, 1, None)]).is_empty());
    }

    #[test]
    fn a_partial_credit_is_spread_over_the_rates_to_the_cent() {
        let invoice = vat_breakdown([(2000, 2000), (550, 1000)]);
        let part = prorate(&invoice, 1001);
        let total: u64 = part.iter().map(VatSubtotal::total_cents).sum();
        assert_eq!(total, 1001);
        assert_eq!(part[0].total_cents(), 667);
        assert_eq!(part[1].total_cents(), 334);
        // The whole amount gives the invoice back exactly.
        assert_eq!(prorate(&invoice, 3000), invoice);
    }

    #[test]
    fn numbers_money_and_dates_read_the_french_way() {
        assert_eq!(document_label("F2026", 42), "F2026-000042");
        assert_eq!(format_amount_fr(1250, "EUR"), "12,50\u{a0}€");
        assert_eq!(
            format_amount_fr(123_456_789, "EUR"),
            "1\u{a0}234\u{a0}567,89\u{a0}€"
        );
        assert_eq!(format_rate_fr(550), "5,5\u{a0}%");
        assert_eq!(format_rate_fr(2000), "20\u{a0}%");
        // 2026-10-14T21:00:00Z
        assert_eq!(format_date_fr(1_792_011_600), "14/10/2026");
        assert_eq!(series_of(INVOICE_PREFIX, 1_792_011_600), "F2026");
        assert_eq!(civil_date(0), (1970, 1, 1));
        assert_eq!(civil_date(951_782_400), (2000, 2, 29));
    }
}
