//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail.

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
    }

    /// The index `slug → category`: one stream per slug, with the
    /// deterministic id [`category_slug_id`](crate::category_slug_id).
    #[evento::aggregate(name = "timada-catalog/CategorySlug")]
    pub enum CategorySlug {
        Claimed { category_id: String },
        Released,
    }
}
