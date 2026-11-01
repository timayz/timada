//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail.

use crate::value_object::OptionSelection;

/// A product of the tenant's catalog. Its id is a ULID.
#[evento::aggregate(name = "timada-catalog/Product")]
pub enum Product {
    Created {
        slug: String,
        name: String,
        created_by: String,
    },
    Renamed {
        name: String,
        renamed_by: String,
    },
    Described {
        description: String,
        described_by: String,
    },
    /// The price in minor units of an ISO-4217 currency (`1250` + `EUR` is
    /// 12.50 €).
    Priced {
        amount_cents: u64,
        currency: String,
        priced_by: String,
    },
    Published {
        published_by: String,
    },
    Unpublished {
        unpublished_by: String,
    },
    /// Final: an archived product takes no more edits and its slug is free
    /// again.
    Archived {
        archived_by: String,
    },
    /// The product joins a category of the tenant (see [`category`]).
    Categorized {
        category_id: String,
        categorized_by: String,
    },
    Uncategorized {
        category_id: String,
        uncategorized_by: String,
    },
    /// A picture of the product. The bytes live outside the store, under the
    /// id and extension recorded here; the host decides the address they are
    /// served from.
    ImageAdded {
        image_id: String,
        /// Lowercase, no dot: `jpg`, `png`, `webp`, `gif`.
        ext: String,
        mime: String,
        /// In bytes.
        size: u64,
        width: Option<u32>,
        height: Option<u32>,
        /// What the picture shows, for a reader who cannot see it.
        alt: String,
        added_by: String,
    },
    ImageAltChanged {
        image_id: String,
        alt: String,
        changed_by: String,
    },
    /// The whole order, every image of the product exactly once. The first
    /// is the one a listing shows.
    ImagesReordered {
        image_ids: Vec<String>,
        reordered_by: String,
    },
    ImageRemoved {
        image_id: String,
        removed_by: String,
    },
    /// A way the product varies, with the values it takes: `Colour` being
    /// beige, blue or grey. A product has at most three.
    OptionDefined {
        option_id: String,
        name: String,
        values: Vec<String>,
        defined_by: String,
    },
    OptionRenamed {
        option_id: String,
        name: String,
        renamed_by: String,
    },
    OptionValuesChanged {
        option_id: String,
        values: Vec<String>,
        changed_by: String,
    },
    OptionRemoved {
        option_id: String,
        removed_by: String,
    },
    /// One combination of the options, with its own price: what a shopper
    /// actually buys. `selections` names a value for every option, once.
    VariantDefined {
        variant_id: String,
        selections: Vec<OptionSelection>,
        /// The merchant's own code for it, unique across the tenant.
        sku: Option<String>,
        amount_cents: u64,
        currency: String,
        /// One of the product's pictures, shown when this variant is picked.
        image_id: Option<String>,
        defined_by: String,
    },
    VariantPriced {
        variant_id: String,
        amount_cents: u64,
        currency: String,
        priced_by: String,
    },
    VariantSkuChanged {
        variant_id: String,
        sku: Option<String>,
        changed_by: String,
    },
    VariantImaged {
        variant_id: String,
        image_id: Option<String>,
        imaged_by: String,
    },
    VariantRemoved {
        variant_id: String,
        removed_by: String,
    },
    /// A fact about the product, for the table beside its description:
    /// `Material` is `Cotton`. Setting a name that is already there replaces
    /// its value.
    AttributeSet {
        name: String,
        value: String,
        set_by: String,
    },
    AttributeRemoved {
        name: String,
        removed_by: String,
    },
    /// What kind of goods it is for VAT ([`TAX_CATEGORIES`](crate::TAX_CATEGORIES)),
    /// for every variant alike: each country taxes a kind at its own rate.
    /// `None` is `standard`.
    TaxCategorySet {
        category: Option<String>,
        set_by: String,
    },
}

/// The index `sku → variant`: one stream per code, with the deterministic id
/// [`product_sku_id`](crate::product_sku_id). A code is the merchant's own
/// and has to mean one thing across the tenant.
pub mod sku {
    #[evento::aggregate(name = "timada-catalog/ProductSku")]
    pub enum ProductSku {
        Claimed {
            product_id: String,
            variant_id: String,
        },
        Released,
    }
}

/// The index `slug → product`: one stream per slug, with the deterministic id
/// [`product_slug_id`](crate::product_slug_id).
#[evento::aggregate(name = "timada-catalog/ProductSlug")]
pub enum ProductSlug {
    Claimed { product_id: String },
    Released,
}

/// The category events, in their own module: the macro names an event struct
/// after its variant, and `Created` / `Renamed` / `Described` / `Archived`
/// already belong to the product above.
pub mod category {
    /// A node of the tenant's category tree: a product belongs to any number
    /// of them, a category has one parent or none. Its id is a ULID.
    #[evento::aggregate(name = "timada-catalog/Category")]
    pub enum Category {
        Created {
            slug: String,
            name: String,
            /// `None` for a top-level category.
            parent_id: Option<String>,
            created_by: String,
        },
        Renamed {
            name: String,
            renamed_by: String,
        },
        Described {
            description: String,
            described_by: String,
        },
        /// The longer text the category's page shows below its products.
        BodyWritten {
            body: String,
            written_by: String,
        },
        /// Re-parented; `None` makes it top-level.
        Moved {
            parent_id: Option<String>,
            moved_by: String,
        },
        /// Where it sorts among its siblings, lowest first.
        Positioned {
            position: u32,
            positioned_by: String,
        },
        /// Final: no more edits and its slug is free again. Its children keep
        /// their parent id and products keep their membership; both are
        /// simply hidden where only live categories are shown.
        Archived {
            archived_by: String,
        },
        /// Left out of the shop's menu, with everything under it. Its page,
        /// products and place in the tree are untouched.
        HiddenFromMenu {
            hidden_by: String,
        },
        /// Back in the shop's menu.
        ShownInMenu {
            shown_by: String,
        },
    }

    /// The index `slug → category`: one stream per slug, with the
    /// deterministic id [`category_slug_id`](crate::category_slug_id).
    #[evento::aggregate(name = "timada-catalog/CategorySlug")]
    pub enum CategorySlug {
        Claimed { category_id: String },
        Released,
    }
}

/// The stock events, in their own module: the macro names an event struct
/// after its variant, and `Released` already belongs to the indexes above.
pub mod stock {
    use crate::value_object::{CarriedHold, CarriedTake};

    /// How many of one thing the shop holds: a product without variants, or
    /// one variant of a product. A thing is counted in generations: one
    /// stream per generation, with the deterministic id
    /// [`stock_item_id`](crate::stock_item_id) of the thing and the
    /// generation, each holding at most [`STOCK_CHUNK`](crate::STOCK_CHUNK)
    /// events, so a thing that sells for years never runs out of stream
    /// versions. [`StockPointer`] names the current generation. A thing
    /// without a stream (or untracked) sells without limit.
    #[evento::aggregate(name = "timada-catalog/StockItem")]
    pub enum StockItem {
        /// The first event of every generation after the first: what the
        /// full generation before it ended with. `holds` are the ones not
        /// lapsed when it was opened, `recent` the last orders that took or
        /// gave back some of it (so a repeated delivery is still told
        /// apart).
        Opened {
            product_id: String,
            variant_id: Option<String>,
            tracked: bool,
            on_hand: u32,
            holds: Vec<CarriedHold>,
            recent: Vec<CarriedTake>,
            opened_by: String,
        },
        /// The shop counts it from now on, starting at `on_hand`.
        Tracked {
            product_id: String,
            variant_id: Option<String>,
            on_hand: u32,
            tracked_by: String,
        },
        /// Sells without limit again; the checkouts it held are let go.
        Untracked { untracked_by: String },
        /// Counted again: what is on the shelf now.
        OnHandSet { on_hand: u32, set_by: String },
        /// Held for a checkout until `expires_at` (unix seconds, excluded),
        /// unless its order spends it first. Reserving again for the same
        /// cart replaces the hold.
        Reserved {
            cart_id: String,
            quantity: u32,
            expires_at: u64,
            reserved_by: String,
        },
        Released {
            cart_id: String,
            released_by: String,
        },
        /// Sold: the paid order of a checkout takes `quantity` off the shelf.
        Consumed {
            cart_id: String,
            order_id: String,
            quantity: u32,
            consumed_by: String,
        },
        /// Back on the shelf: the order that took them was cancelled.
        Restored {
            order_id: String,
            quantity: u32,
            restored_by: String,
        },
    }

    /// Which generation of a thing's stock is current: one stream per thing,
    /// with the deterministic id
    /// [`stock_pointer_id`](crate::stock_pointer_id). No stream means
    /// generation 0. The writer that finds a generation full opens the next
    /// one, then rolls the pointer; a roll is one event per
    /// [`STOCK_CHUNK`](crate::STOCK_CHUNK) stock events.
    #[evento::aggregate(name = "timada-catalog/StockPointer")]
    pub enum StockPointer {
        Rolled { generation: u32, rolled_by: String },
    }
}

pub mod stock_take {
    use crate::value_object::TakenLine;

    /// What one paid order took off the shelves: one stream per order, with
    /// the deterministic id [`stock_take_id`](crate::stock_take_id), written
    /// once the stock streams are. It is what makes taking, and giving back
    /// on cancellation, happen once however often the order is delivered,
    /// long after the stock generations forgot the order.
    #[evento::aggregate(name = "timada-catalog/StockTake")]
    pub enum StockTake {
        /// `lines` are the counted things only: an untracked one took
        /// nothing.
        Taken {
            cart_id: String,
            lines: Vec<TakenLine>,
            taken_by: String,
        },
        /// Everything it took is back on the shelves.
        Restored { restored_by: String },
    }
}
