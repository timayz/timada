//! Tax zones: which VAT treatment a sale gets, depending on where it is
//! delivered. A library, not a bounded context — there are no events here.
//! [`TaxZones`] is a value the host builds and hands to the checkout, like a
//! `ReturnPolicy`; what a given order was taxed with is recorded *by the
//! order* (`OrderTaxed`) and its invoice (`InvoiceTaxed`), using the two
//! persisted types of this crate, [`TaxTreatment`] and [`VatLine`].
//!
//! Prices are listed tax-inclusive at the shop's domestic rate. From there:
//!
//! - [`TaxTreatment::Domestic`] — charged as listed; the VAT inside is broken
//!   out per rate.
//! - [`TaxTreatment::Export`] — the domestic VAT is taken off and the customer
//!   pays the pre-tax price. French overseas territories are exports for VAT
//!   (CGI art. 294), like any country outside the EU (art. 262 I); local
//!   taxes are the customer's business on arrival.
//! - [`TaxTreatment::DestinationVat`] — pre-tax price plus the VAT of the
//!   destination (EU one-stop shop). One zone per member state, with its
//!   [`DestinationRates`]: the country's standard rate, and the host's
//!   mapping from the rates products are listed with to the country's reduced
//!   ones. Opt-in through [`TaxZones::france_with_eu_oss`]; the default zones
//!   stay France and its overseas territories.
//!
//! A business of another member state buys without the seller's VAT when its
//! VAT number is valid there: [`VatNumber`] reads one, a
//! [`VatNumberValidator`] asks the registry.
//!
//! A sale in another currency than the shop's books is converted *for the
//! books only*, at a rate pinned when the order is placed: [`PinnedRate`],
//! from an [`ExchangeRates`] source.

mod breakdown;
mod business;
#[cfg(feature = "ecb")]
mod ecb;
mod exchange;
mod vat_number;
#[cfg(feature = "vies")]
mod vies;
mod zone;

pub use breakdown::*;
pub use business::*;
#[cfg(feature = "ecb")]
pub use ecb::*;
pub use exchange::*;
pub use vat_number::*;
#[cfg(feature = "vies")]
pub use vies::*;
pub use zone::*;
