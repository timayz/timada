//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail.

use crate::value_object::{DeliveryDays, OptionSelection, VariantCost, VariantPrice};

/// A product of the tenant's catalog. Its id is a ULID.
#[evento::aggregate(name = "timada-catalog/Product")]
pub enum Product {
    Created {
        slug: String,
        name: String,
        created_by: String,
    },
    /// A new name and the slug that goes with it, which [`ProductSlug`] holds
    /// from then on.
    Renamed {
        name: String,
        slug: String,
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
    /// Final too, and more: the product leaves every listing as if it had
    /// never been. Only a product nobody ever ordered may be deleted (the
    /// host checks, the catalog cannot see the orders); its slug and codes
    /// are free again and its pictures' files are deleted.
    Deleted {
        deleted_by: String,
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
    /// A picture of the product: its two renditions, as the host rendered
    /// them from the upload ([`timada_core::Picture`]). The bytes live
    /// outside the store, named after the id; the host decides the address
    /// they are served from.
    ImageAdded {
        image_id: String,
        picture: timada_core::Picture,
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
    /// What a shopper reads for the option instead of its name (`Couleur`
    /// for `Color`). The name stays as an import gave it, so a sync still
    /// matches it; `None` goes back to the name.
    OptionLabelled {
        option_id: String,
        label: Option<String>,
        labelled_by: String,
    },
    /// What a shopper reads for one value of the option instead of the
    /// value itself (`Rouge` for `01 Red-A`); `None` goes back to the
    /// value. A value that leaves the option takes its label with it.
    OptionValueLabelled {
        option_id: String,
        value: String,
        label: Option<String>,
        labelled_by: String,
    },
    /// Shoppers stop choosing the option: the shop sells only the combinations
    /// taking `value` and shows them without it (where an imported product
    /// ships from, say). The other combinations stay, off sale, so an import
    /// still finds them all. Hiding it again moves what is on sale to another
    /// value.
    OptionHidden {
        option_id: String,
        value: String,
        hidden_by: String,
    },
    /// Shoppers choose the option again, and every combination it left off
    /// sale is back.
    OptionShown {
        option_id: String,
        shown_by: String,
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
    /// Several combinations priced at once, in `currency`: what one read of
    /// a supplier changed, in one event rather than one per combination, so
    /// a product whose supplier moves its prices daily stays far from the
    /// 65,535 versions its stream holds (#472).
    VariantsRepriced {
        prices: Vec<VariantPrice>,
        currency: String,
        repriced_by: String,
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
    /// How long delivery takes, set by the merchant or read from the
    /// supplier; `None` falls back to the shop's default.
    DeliveryDaysSet {
        days: Option<DeliveryDays>,
        set_by: String,
    },
    /// What search engines show for its page; empty falls back to the name
    /// and the description.
    SeoSet {
        meta_title: String,
        meta_description: String,
        set_by: String,
    },
    /// What tells the product apart from every other one sold anywhere: who
    /// makes it, the number under its barcode and the maker's own number
    /// for it. The two numbers are those of a product sold as is; one with
    /// variants gives each its own ([`VariantIdentified`]).
    /// `no_identifier` says it truly has none (made by hand, made to
    /// order), which a shopping feed must be told rather than left to
    /// guess.
    ///
    /// [`VariantIdentified`]: Product::VariantIdentified
    Identified {
        brand: Option<String>,
        gtin: Option<String>,
        mpn: Option<String>,
        no_identifier: bool,
        identified_by: String,
    },
    VariantIdentified {
        variant_id: String,
        gtin: Option<String>,
        mpn: Option<String>,
        identified_by: String,
    },
    /// Where a shopping feed files it: a category of Google's product
    /// taxonomy (its number or its path), and whom it is made for
    /// ([`AGE_GROUPS`](crate::AGE_GROUPS), [`GENDERS`](crate::GENDERS)).
    Classified {
        google_category: Option<String>,
        age_group: Option<String>,
        gender: Option<String>,
        classified_by: String,
    },
    /// What an option is to a feed and to search engines
    /// ([`OPTION_KINDS`](crate::OPTION_KINDS)); `None` for an option that is
    /// none of them. A product has at most one option of each kind.
    OptionKindSet {
        option_id: String,
        kind: Option<String>,
        set_by: String,
    },
    /// The free labels a merchant groups products by in ad campaigns
    /// (margin tier, origin, season): always
    /// [`FEED_LABELS`](crate::FEED_LABELS) of them, empty where there is
    /// none.
    FeedLabelsSet {
        labels: Vec<String>,
        set_by: String,
    },
    /// Whether its name and its description were written by a machine,
    /// which a shopping feed has to say.
    AiWritingDeclared {
        title: bool,
        description: bool,
        declared_by: String,
    },
    /// The price a reduction is announced against, struck through beside
    /// the price of a product sold as is; `None` announces none. In the
    /// product's currency. The command holds it to the lowest price the
    /// product was on sale at in the thirty days before its current one,
    /// which is what a reduction may be measured from.
    CompareAtSet {
        amount_cents: Option<u64>,
        set_by: String,
    },
    /// The same for one variant.
    VariantCompareAtSet {
        variant_id: String,
        amount_cents: Option<u64>,
        set_by: String,
    },
    /// Whether the shop's shopping feed carries it. A product is in the
    /// feed from the day it is published; one taken out stays in the shop
    /// and out of the ads.
    FeedListingSet {
        listed: bool,
        set_by: String,
    },
}

/// What the shop pays for the things one product sells: one stream per
/// product, with the deterministic id
/// [`product_cost_id`](crate::product_cost_id). Apart from the product's own
/// stream on purpose: a storefront reads that one, and what a shop pays its
/// supplier is no storefront's business; and a supplier's cost that moves
/// every week fills this stream, not the product's. Nothing here is ever
/// shown to a shopper: what a sale leaves is worked out from it.
#[evento::aggregate(name = "timada-catalog/ProductCost")]
pub enum ProductCost {
    /// What the shop pays for one of a product sold as is, in the product's
    /// currency; `None` when it is not known.
    CostSet {
        product_id: String,
        amount_cents: Option<u64>,
        set_by: String,
    },
    /// What the shop pays for one of each of these combinations, in one
    /// event: a merchant fills a table, a supplier's read moves several at
    /// once.
    VariantCostsSet {
        product_id: String,
        costs: Vec<VariantCost>,
        set_by: String,
    },
    /// What sending one and importing one cost the shop for this product,
    /// where they are not the shop's usual ([`SaleCosts`]): a supplier who
    /// ships it apart, goods that come from inside the EU and pay nothing at
    /// the border. `None` is the shop's.
    OwnCostsSet {
        product_id: String,
        shipping_cents: Option<u64>,
        import_cents: Option<u64>,
        set_by: String,
    },
}

/// What a sale costs the shop beyond the goods, and how its margins are
/// graded: one stream per tenant store, with the id
/// [`sale_costs_id`](crate::sale_costs_id). No stream means no such cost.
#[evento::aggregate(name = "timada-catalog/SaleCosts")]
pub enum SaleCosts {
    /// Replaces what was there. The amounts are in `currency`, and count
    /// for what is sold in it.
    SaleCostsSet {
        currency: String,
        /// What sending a parcel costs the shop.
        shipping_cents: u64,
        /// What the border charges for one kind of item in a parcel: the
        /// customs duty and the processing fee on goods shipped from
        /// outside the EU.
        import_cents: u64,
        /// The share of the shop's fixed costs a sale carries.
        fixed_cents: u64,
        /// What the payment provider keeps of the amount paid, in basis
        /// points (`150` is 1.5 %).
        payment_fee_bps: u32,
        /// And for each payment whatever its amount.
        payment_fee_cents: u64,
        set_by: String,
    },
    /// From which share of the price, in basis points, a margin is
    /// `medium` and `high` ([`MarginTier`](crate::MarginTier)), and which of
    /// the feed's labels carries the tier (from 0) so campaigns group
    /// products by it; `None` leaves the labels to the merchant.
    MarginTiersSet {
        medium_from_bps: u32,
        high_from_bps: u32,
        feed_label: Option<u8>,
        set_by: String,
    },
}

pub mod feed {
    /// The shop's shopping feed: one stream per tenant store, with the id
    /// [`feed_id`](crate::feed_id). The feed is read at an address nobody
    /// can guess, which `token` is: whoever has it reads the feed, so it is
    /// given to the feed's reader (Google Merchant Center) and to nobody
    /// else.
    #[evento::aggregate(name = "timada-catalog/Feed")]
    pub enum Feed {
        /// The feed answers at `token` from now on, and at no address it
        /// had before.
        Opened { token: String, opened_by: String },
        /// The feed answers nowhere.
        Closed { closed_by: String },
    }
}

/// The products a shop sold the most of lately, the best first: what a
/// listing sorts by when asked for the best sellers. One stream per shop
/// and calendar month, with the id
/// [`best_sellers_id`](crate::best_sellers_id), so none grows for ever; the
/// latest event of the latest month is the ranking. It names the products
/// and their order, never how many were sold: a storefront's key reads it.
#[evento::aggregate(name = "timada-catalog/BestSellers")]
pub enum BestSellers {
    /// The ranking whole, as the host worked it out from the shop's sales.
    Ranked {
        product_ids: Vec<String>,
        ranked_by: String,
    },
}

/// The options a shop's listings can be narrowed by ("Couleur", "Taille"),
/// in the order its filters show them: one stream per tenant store, with
/// the id [`listing_filters_id`](crate::listing_filters_id). A name no
/// option has is a fact of the products' tables ("Matière"), which the
/// storefront narrows by the same way. A shop that never set them leaves
/// the choice to its storefront.
#[evento::aggregate(name = "timada-catalog/ListingFilters")]
pub enum ListingFilters {
    /// The names whole, as the products' options or facts are named; empty
    /// offers no such filter.
    FiltersSet { names: Vec<String>, set_by: String },
}

/// The delivery time of a product that gives none: one stream per tenant
/// store, with the id [`delivery_default_id`](crate::delivery_default_id).
#[evento::aggregate(name = "timada-catalog/DeliveryDefault")]
pub enum DeliveryDefault {
    /// `None` announces no delivery time.
    DefaultDeliveryDaysSet {
        days: Option<DeliveryDays>,
        set_by: String,
    },
}

/// The index `sku → variant`: one stream per code, with the deterministic id
/// [`product_sku_id`](crate::product_sku_id). A code is the merchant's own
/// and has to mean one thing across the tenant.
pub mod sku {
    #[evento::aggregate(name = "timada-catalog/ProductSku")]
    pub enum ProductSku {
        /// `product_version` is the version of the product the claim was
        /// made at: the product's next write gives the variant the code or
        /// fails, so a product past it has used the claim or never will.
        Claimed {
            product_id: String,
            variant_id: String,
            product_version: u16,
        },
        Released,
    }
}

/// The index `slug → product`: one stream per slug, with the deterministic id
/// [`product_slug_id`](crate::product_slug_id).
#[evento::aggregate(name = "timada-catalog/ProductSlug")]
pub enum ProductSlug {
    /// `product_version` is the version of the product the claim was made
    /// at: the product's next write moves it to the slug or fails, so a
    /// product past it has used the claim or never will.
    Claimed {
        product_id: String,
        product_version: u16,
    },
    Released,
}

/// The category events, in their own module: the macro names an event struct
/// after its variant, and `Created` / `Renamed` / `Described` / `Archived`
/// already belong to the product above.
pub mod category {
    use crate::value_object::CategoryQuestion;

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
        /// What search engines show for its page; empty falls back to the
        /// name and the description.
        SeoSet {
            meta_title: String,
            meta_description: String,
            set_by: String,
        },
        /// The questions shoppers ask about it, each with its answer, as
        /// its page lists them below the products: the list whole, in
        /// order; empty takes them off.
        QuestionsSet {
            questions: Vec<CategoryQuestion>,
            set_by: String,
        },
        /// The products its page shows first, in this order, ahead of
        /// whatever order the rest follows: the list whole; empty leaves
        /// the page to its order.
        ProductsPinned {
            product_ids: Vec<String>,
            pinned_by: String,
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

/// The translations' events, in their own module: the macro names an event
/// struct after its variant, and `Described`, `SeoSet` and `Claimed` already
/// belong to the product above.
pub mod translation {
    use crate::value_object::{CategoryQuestion, ImageAlt};

    /// What a product or a category says in one more language of the shop:
    /// one stream per thing and language, with the deterministic id
    /// [`translation_id`](crate::translation_id). The thing's own stream
    /// keeps the shop's main language and everything that is no wording (a
    /// price, a picture, a place in the tree); what a translation leaves
    /// out is read in the main language. Apart from the thing's stream on
    /// purpose: wording ten languages again fills these streams, not the
    /// product's, which has its 65,535 versions to spend on prices and
    /// stock (#472).
    #[evento::aggregate(name = "timada-catalog/Translation")]
    pub enum Translation {
        /// The name in `language`, and the slug its address there goes by,
        /// which [`TranslationSlug`] holds from then on. The first event of
        /// a translation, and the first after one was dropped. `kind` is
        /// `product` or `category`, `subject_id` the thing's id.
        Named {
            kind: String,
            subject_id: String,
            language: String,
            name: String,
            slug: String,
            named_by: String,
        },
        /// Empty goes back to the main language's.
        Described {
            description: String,
            described_by: String,
        },
        /// The buying guide below a category's products; a product has
        /// none. Empty goes back to the main language's.
        BodyWritten { body: String, written_by: String },
        /// What search engines show for its page in this language; empty
        /// falls back to the translated name and description.
        SeoSet {
            meta_title: String,
            meta_description: String,
            set_by: String,
        },
        /// The questions a category's page answers, the list whole; empty
        /// goes back to the main language's.
        QuestionsSet {
            questions: Vec<CategoryQuestion>,
            set_by: String,
        },
        /// What a product's pictures show, said in this language, the list
        /// whole: a picture it leaves out is described as the main language
        /// does. A category has none.
        AltsSet { alts: Vec<ImageAlt>, set_by: String },
        /// The translation is gone, whole: the thing is read in the main
        /// language again, and its slug in this one is free.
        Dropped { dropped_by: String },
    }

    /// The index `(kind, language, slug) → thing`: one stream per slug of a
    /// language, with the deterministic id
    /// [`translation_slug_id`](crate::translation_slug_id).
    #[evento::aggregate(name = "timada-catalog/TranslationSlug")]
    pub enum TranslationSlug {
        /// `version` is the version of the translation the claim was made
        /// at: the translation's next write moves it to the slug or fails,
        /// so a translation past it has used the claim or never will. No
        /// release: a slug whose holder let go of it is taken over at
        /// once.
        Claimed { subject_id: String, version: u16 },
    }

    /// The languages a slug has an entry of [`TranslationSlug`] in: one
    /// stream per kind and slug, with the deterministic id
    /// [`slug_languages_id`](crate::slug_languages_id). The claim of a main
    /// slug reads it to find the translations that may hold the slug, in
    /// one read whatever the number of languages. A language is noted
    /// before its entry is claimed, once, and never taken back: an entry
    /// nobody holds any more is what its reader finds out.
    #[evento::aggregate(name = "timada-catalog/SlugLanguages")]
    pub enum SlugLanguages {
        Noted { language: String },
    }

    /// What the shop says in one more language for a word its products
    /// share: the name of an option (« Couleur »), one of its values
    /// (« Rose foncé »), the label or the value of a fact of their tables.
    /// One stream per language and word, with the deterministic id
    /// [`word_id`](crate::word_id): a word is worded once for the whole
    /// shop, whatever the number of products that carry it, and the
    /// filters of a listing, made of the same words, read the same entry.
    #[evento::aggregate(name = "timada-catalog/Word")]
    pub enum Word {
        /// `term` is the word as the main language writes it, `wording`
        /// what `language` reads for it.
        Worded {
            language: String,
            term: String,
            wording: String,
            worded_by: String,
        },
        /// The word is read as the main language writes it again.
        Unworded { unworded_by: String },
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

pub mod ordered {
    /// That somebody ordered a product: one stream per product, with the
    /// deterministic id [`product_ordered_id`](crate::product_ordered_id),
    /// written by the first order that sold it and never again. A product
    /// ordered once stays on record: it may be archived, not deleted.
    #[evento::aggregate(name = "timada-catalog/ProductOrdered")]
    pub enum ProductOrdered {
        /// `order_id` is the first order that sold it.
        Ordered {
            order_id: String,
            ordered_by: String,
        },
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
