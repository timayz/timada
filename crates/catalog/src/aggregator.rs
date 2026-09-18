use crate::value_object::{Brand, EnergyClass, Media, Spec};

// The explicit name pins the on-disk identity: renaming the crate or the enum
// must never orphan stored events.
#[evento::aggregate(name = "timada-catalog/Product")]
pub enum Product {
    /// A product entered the catalog under a unique SKU.
    ProductCreated {
        sku: String,
        name: String,
        brand: Brand,
        category_path: Vec<String>,
        short_description: String,
        warranty_months: u16,
    },

    /// Marketing copy and the "caractéristiques principales" list were set.
    ProductDescribed {
        long_description: String,
        key_features: Vec<String>,
    },

    /// The "fiche technique" was (re)published.
    ProductSpecified { specs: Vec<Spec> },

    /// A gallery image or video was attached.
    ProductMediaAdded { media: Media },

    /// The EU energy label and its information sheet were attached.
    ProductEnergyLabelled {
        class: EnergyClass,
        info_sheet_url: String,
    },

    /// The product was withdrawn from the catalog.
    ProductArchived,
}
