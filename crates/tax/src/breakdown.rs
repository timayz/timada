use bitcode::{Decode, Encode};
use timada_core::{Money, MoneyError};

/// One line of the VAT summary of an order or an invoice: everything charged
/// at one rate. Persisted in `OrderTaxed` / `InvoiceTaxed`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct VatLine {
    /// Basis points; `0` for what is charged without VAT.
    pub rate_bp: u16,
    /// Pre-tax amount ("HT").
    pub base: Money,
    pub vat: Money,
    /// `base + vat`: what was charged at this rate ("TTC").
    pub total: Money,
}

/// An amount charged, VAT of `rate_bp` included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Charged {
    pub total: Money,
    pub rate_bp: u16,
}

/// The VAT summary of what was charged. `goods` are the order lines,
/// `reduction` a price reduction on the goods (a promo code — a voucher is a
/// means of payment and reduces nothing here), `fees` delivery and handling.
///
/// The reduction is spread over the goods pro rata of each rate, the last
/// rate taking the rounding remainder, so the lines add up to exactly what
/// was charged. Lines come out sorted by rate, highest first.
pub fn vat_breakdown(
    goods: &[Charged],
    reduction: Option<&Money>,
    fees: &[Charged],
) -> Result<Vec<VatLine>, MoneyError> {
    let currency = goods
        .iter()
        .chain(fees)
        .next()
        .map_or_else(|| Money::EUR.to_owned(), |c| c.total.currency.clone());

    // Totals per rate, goods first so the reduction can be spread over them.
    let mut per_rate: Vec<(u16, i64)> = Vec::new();
    let add = |per_rate: &mut Vec<(u16, i64)>, rate_bp: u16, minor: i64| match per_rate
        .iter_mut()
        .find(|(rate, _)| *rate == rate_bp)
    {
        Some((_, total)) => *total += minor,
        None => per_rate.push((rate_bp, minor)),
    };
    for charged in goods {
        charged.total.same_currency(&Money::zero(&currency))?;
        add(&mut per_rate, charged.rate_bp, charged.total.minor);
    }

    if let Some(reduction) = reduction.filter(|r| r.is_positive()) {
        reduction.same_currency(&Money::zero(&currency))?;
        let goods_total: i64 = per_rate.iter().map(|(_, total)| *total).sum();
        let reduction = reduction.minor.min(goods_total);
        let mut left = reduction;
        let rates = per_rate.len();
        for (index, (_, total)) in per_rate.iter_mut().enumerate() {
            let share = if index + 1 == rates {
                left
            } else if goods_total > 0 {
                (i128::from(reduction) * i128::from(*total) / i128::from(goods_total)) as i64
            } else {
                0
            };
            *total -= share;
            left -= share;
        }
    }

    for charged in fees {
        charged.total.same_currency(&Money::zero(&currency))?;
        add(&mut per_rate, charged.rate_bp, charged.total.minor);
    }

    per_rate.retain(|(_, total)| *total != 0);
    per_rate.sort_by_key(|(rate_bp, _)| std::cmp::Reverse(*rate_bp));
    Ok(per_rate
        .into_iter()
        .map(|(rate_bp, minor)| {
            let total = Money::new(minor, &currency);
            let base = total.excl_tax(rate_bp);
            VatLine {
                rate_bp,
                vat: Money::new(total.minor - base.minor, &currency),
                base,
                total,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn charged(minor: i64, rate_bp: u16) -> Charged {
        Charged {
            total: Money::eur(minor),
            rate_bp,
        }
    }

    #[test]
    fn vat_is_broken_out_per_rate_and_adds_up() -> Result<(), MoneyError> {
        // A monitor at 20 %, a book at 5,5 %, delivery at 20 %.
        let lines = vat_breakdown(
            &[charged(23_990, 2_000), charged(2_110, 550)],
            None,
            &[charged(590, 2_000)],
        )?;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].rate_bp, 2_000);
        assert_eq!(lines[0].total, Money::eur(24_580));
        assert_eq!(lines[0].base, Money::eur(20_483));
        assert_eq!(lines[0].vat, Money::eur(4_097));
        assert_eq!(lines[1].rate_bp, 550);
        assert_eq!(lines[1].base, Money::eur(2_000));
        assert_eq!(lines[1].vat, Money::eur(110));
        let charged_in_all: i64 = lines.iter().map(|l| l.total.minor).sum();
        assert_eq!(charged_in_all, 23_990 + 2_110 + 590);
        Ok(())
    }

    #[test]
    fn a_reduction_is_spread_over_the_goods_only() -> Result<(), MoneyError> {
        // 10,00 off 100,00 at 20 % + 50,00 at 5,5 %; the fee is untouched.
        let lines = vat_breakdown(
            &[charged(10_000, 2_000), charged(5_000, 550)],
            Some(&Money::eur(1_000)),
            &[charged(590, 2_000)],
        )?;
        // 666 off the first rate, the remaining 334 off the last one.
        assert_eq!(lines[0].total, Money::eur(10_000 - 666 + 590));
        assert_eq!(lines[1].total, Money::eur(5_000 - 334));
        let charged_in_all: i64 = lines.iter().map(|l| l.total.minor).sum();
        assert_eq!(charged_in_all, 15_000 - 1_000 + 590);
        Ok(())
    }

    #[test]
    fn an_export_has_a_single_line_without_vat() -> Result<(), MoneyError> {
        let lines = vat_breakdown(&[charged(9_996, 0)], None, &[charged(1_996, 0)])?;
        assert_eq!(
            lines,
            [VatLine {
                rate_bp: 0,
                base: Money::eur(11_992),
                vat: Money::eur(0),
                total: Money::eur(11_992),
            }]
        );
        Ok(())
    }
}
