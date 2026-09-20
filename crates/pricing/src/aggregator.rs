use timada_core::Money;

use crate::value_object::InstallmentOffer;

// The explicit name pins the on-disk identity: renaming the crate or the enum
// must never orphan stored events.
#[evento::aggregate(name = "timada-pricing/ProductPrice")]
pub enum ProductPrice {
    /// A product was given a tax-inclusive price in one currency — the shop's
    /// base currency, the one `ProductPriceChanged` goes on changing.
    ProductPriceListed {
        product_id: String,
        price_incl_tax: Money,
        vat_rate_bp: u16,
        eco_participation: Money,
    },

    /// The tax-inclusive price changed (same currency as listed).
    ProductPriceChanged { price_incl_tax: Money },

    /// The éco-participation contribution changed.
    EcoParticipationChanged { eco_participation: Money },

    /// A "payez en Nx" offer was attached (replaces any previous one).
    InstallmentOfferAttached { offer: InstallmentOffer },

    /// The price was withdrawn; the product can no longer be sold.
    ProductPriceWithdrawn,

    /// The product was given — or given another — tax-inclusive price in a
    /// currency other than the one it was listed in. Set by the operator,
    /// never computed: the same VAT rate applies, nothing is converted.
    CurrencyPriceSet { price_incl_tax: Money },

    /// The product is no longer sold in that currency.
    CurrencyPriceRemoved { currency: String },
}
