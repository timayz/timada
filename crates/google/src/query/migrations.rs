//! Schema of the read model, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "google";

/// `google_connections`: one row per shop whose connection reads Search
/// Console and has a site chosen: where the daily pass looks for shops.
/// `google_search_watch`: where each shop's pass stands, and from when the
/// next is due (`next_at`); `site` is the one it last looked at, so a shop
/// whose site changed is due at once.
/// `google_search_days`: what a site did on a day. `google_search_queries`
/// and `google_search_pages`: the most clicked queries and pages of a day.
/// `google_search_page_queries`: the queries each page showed for on a day,
/// its most shown. A position is in thousandths. The four are keyed by shop,
/// site and day, so a period is one range of the key, and a site's rows are
/// left behind, to age out, when the shop chooses another.
/// `google_ads_accounts`: one row per shop whose connection reads Google Ads
/// and has an account chosen: where the daily pass looks for shops.
/// `google_ads_watch`: where each shop's pass stands. `google_ads_calls`:
/// how many questions each shop's passes asked Google Ads on a day (UTC),
/// which the platform's daily allowance is counted in.
/// `google_ads_days`: what an account cost and brought on a day, added up
/// from `google_ads_campaigns` (a campaign on a kind of device).
/// `google_ads_ad_groups`, `google_ads_products` and `google_ads_terms`:
/// the day's ad groups, products and searches. Money is in millionths of
/// the account's currency, conversions in thousandths.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[
        Migration::up(
            "0001_search",
            "CREATE TABLE google_connections (
            tenant_id TEXT NOT NULL PRIMARY KEY,
            site TEXT NOT NULL,
            lapsed INTEGER NOT NULL
        );
        CREATE TABLE google_search_watch (
            tenant_id TEXT NOT NULL PRIMARY KEY,
            site TEXT NOT NULL,
            next_at INTEGER NOT NULL
        );
        CREATE TABLE google_search_days (
            tenant_id TEXT NOT NULL,
            site TEXT NOT NULL,
            day TEXT NOT NULL,
            clicks INTEGER NOT NULL,
            impressions INTEGER NOT NULL,
            position_milli INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, site, day)
        );
        CREATE TABLE google_search_queries (
            tenant_id TEXT NOT NULL,
            site TEXT NOT NULL,
            day TEXT NOT NULL,
            query TEXT NOT NULL,
            clicks INTEGER NOT NULL,
            impressions INTEGER NOT NULL,
            position_milli INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, site, day, query)
        );
        CREATE TABLE google_search_pages (
            tenant_id TEXT NOT NULL,
            site TEXT NOT NULL,
            day TEXT NOT NULL,
            page TEXT NOT NULL,
            clicks INTEGER NOT NULL,
            impressions INTEGER NOT NULL,
            position_milli INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, site, day, page)
        );",
        ),
        Migration::up(
            "0002_page_queries",
            "CREATE TABLE google_search_page_queries (
                tenant_id TEXT NOT NULL,
                site TEXT NOT NULL,
                day TEXT NOT NULL,
                page TEXT NOT NULL,
                query TEXT NOT NULL,
                clicks INTEGER NOT NULL,
                impressions INTEGER NOT NULL,
                position_milli INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, site, day, page, query)
            );",
        ),
        Migration::up(
            "0003_ads",
            "CREATE TABLE google_ads_accounts (
                tenant_id TEXT NOT NULL PRIMARY KEY,
                customer_id TEXT NOT NULL,
                lapsed INTEGER NOT NULL
            );
            CREATE TABLE google_ads_watch (
                tenant_id TEXT NOT NULL PRIMARY KEY,
                customer_id TEXT NOT NULL,
                next_at INTEGER NOT NULL
            );
            CREATE TABLE google_ads_calls (
                day TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                calls INTEGER NOT NULL,
                PRIMARY KEY (day, tenant_id)
            );
            CREATE TABLE google_ads_days (
                tenant_id TEXT NOT NULL,
                customer_id TEXT NOT NULL,
                day TEXT NOT NULL,
                currency TEXT NOT NULL,
                cost_micros INTEGER NOT NULL,
                impressions INTEGER NOT NULL,
                clicks INTEGER NOT NULL,
                conversions_milli INTEGER NOT NULL,
                value_micros INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, customer_id, day)
            );
            CREATE TABLE google_ads_campaigns (
                tenant_id TEXT NOT NULL,
                customer_id TEXT NOT NULL,
                day TEXT NOT NULL,
                campaign_id TEXT NOT NULL,
                device TEXT NOT NULL,
                name TEXT NOT NULL,
                cost_micros INTEGER NOT NULL,
                impressions INTEGER NOT NULL,
                clicks INTEGER NOT NULL,
                conversions_milli INTEGER NOT NULL,
                value_micros INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, customer_id, day, campaign_id, device)
            );
            CREATE TABLE google_ads_ad_groups (
                tenant_id TEXT NOT NULL,
                customer_id TEXT NOT NULL,
                day TEXT NOT NULL,
                ad_group_id TEXT NOT NULL,
                campaign_id TEXT NOT NULL,
                name TEXT NOT NULL,
                cost_micros INTEGER NOT NULL,
                impressions INTEGER NOT NULL,
                clicks INTEGER NOT NULL,
                conversions_milli INTEGER NOT NULL,
                value_micros INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, customer_id, day, ad_group_id)
            );
            CREATE TABLE google_ads_products (
                tenant_id TEXT NOT NULL,
                customer_id TEXT NOT NULL,
                day TEXT NOT NULL,
                item_id TEXT NOT NULL,
                ad_group_id TEXT NOT NULL,
                campaign_id TEXT NOT NULL,
                cost_micros INTEGER NOT NULL,
                impressions INTEGER NOT NULL,
                clicks INTEGER NOT NULL,
                conversions_milli INTEGER NOT NULL,
                value_micros INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, customer_id, day, item_id, ad_group_id)
            );
            CREATE TABLE google_ads_terms (
                tenant_id TEXT NOT NULL,
                customer_id TEXT NOT NULL,
                day TEXT NOT NULL,
                term TEXT NOT NULL,
                ad_group_id TEXT NOT NULL,
                campaign_id TEXT NOT NULL,
                cost_micros INTEGER NOT NULL,
                impressions INTEGER NOT NULL,
                clicks INTEGER NOT NULL,
                conversions_milli INTEGER NOT NULL,
                value_micros INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, customer_id, day, term, ad_group_id)
            );",
        ),
    ],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscription fills them again.
pub const TABLES: &[&str] = &[
    "google_connections",
    "google_search_watch",
    "google_search_days",
    "google_search_queries",
    "google_search_pages",
    "google_search_page_queries",
    "google_ads_accounts",
    "google_ads_watch",
    "google_ads_calls",
    "google_ads_days",
    "google_ads_campaigns",
    "google_ads_ad_groups",
    "google_ads_products",
    "google_ads_terms",
];
