use crate::value_object::{Brand, EnergyClass, Media, Spec, SpecKey};

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

    /// The product was filed under a category (again: it moves). The
    /// `category_path` it was created with is only a label from before
    /// categories were managed.
    ProductCategorised { category_id: String },
}

/// A node of the shop's category tree. Its id derives from its slug, which is
/// for ever: links to a category never break, whatever it is renamed to.
#[evento::aggregate(name = "timada-catalog/Category")]
pub enum Category {
    /// A category was opened, at the root or under a parent.
    CategoryCreated {
        slug: String,
        name: String,
        parent_id: Option<String>,
    },

    CategoryRenamed {
        name: String,
    },

    /// The text shown at the top of the category's page.
    CategoryDescribed {
        description: String,
    },

    /// The category — and everything under it — went elsewhere in the tree.
    CategoryMoved {
        parent_id: Option<String>,
    },

    /// Its rank among its siblings, lowest first.
    CategoryPositioned {
        position: u32,
    },

    /// The category left the storefront, with everything under it.
    CategoryArchived,

    /// Which lines of the technical sheet shoppers filter this category by,
    /// in the order shown — the whole list, replacing the one before. A
    /// category without a list of its own goes by its parent's.
    CategoryFacetsDefined {
        facets: Vec<SpecKey>,
    },
}
