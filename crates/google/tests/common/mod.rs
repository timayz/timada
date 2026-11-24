#![allow(dead_code)]

use timada_google::aggregator::SearchRow;
use timada_google::{ACCESS_SEARCH_CONSOLE, Command, Connect, DayReport, SearchNumbers};

pub type Google = Command<evento::Turso>;

pub const ALICE: &str = "user-alice";
pub const SHOP: &str = "tenant-shop";

pub async fn command() -> Google {
    Command(evento::Turso::in_memory_concurrent().await.unwrap())
}

/// A fresh in-memory event store with the read-model schema applied, and
/// the read-model database in it.
pub async fn executor_with_pool() -> (evento::Turso, timada_core::db::Db) {
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_google::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), timada_core::db::Db::from(store))
}

/// A query or a page with its numbers.
pub fn row(key: &str, clicks: u64, impressions: u64, position_milli: u32) -> SearchRow {
    SearchRow {
        key: key.to_owned(),
        clicks,
        impressions,
        position_milli,
    }
}

/// A day of 10 clicks over 200 impressions at position 8, with two queries
/// and one page.
pub fn day(day: &str) -> DayReport {
    DayReport {
        day: day.to_owned(),
        numbers: SearchNumbers {
            clicks: 10,
            impressions: 200,
            position_milli: 8_000,
        },
        queries: vec![
            row("arbre à chat", 6, 100, 4_000),
            row("griffoir", 2, 50, 12_000),
        ],
        pages: vec![row("https://shop.example/arbres-a-chat", 9, 180, 7_500)],
    }
}

/// What `account` granted: Search Console, under `token`.
pub fn grant(account: &str, token: &[u8]) -> Connect {
    Connect {
        refresh_token_sealed: token.to_vec(),
        account: account.to_owned(),
        accesses: vec![ACCESS_SEARCH_CONSOLE.to_owned()],
    }
}
