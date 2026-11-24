#![allow(dead_code)]

use timada_google::{ACCESS_SEARCH_CONSOLE, Command, Connect};

pub type Google = Command<evento::Turso>;

pub const ALICE: &str = "user-alice";
pub const SHOP: &str = "tenant-shop";

pub async fn command() -> Google {
    Command(evento::Turso::in_memory_concurrent().await.unwrap())
}

/// What `account` granted: Search Console, under `token`.
pub fn grant(account: &str, token: &[u8]) -> Connect {
    Connect {
        refresh_token_sealed: token.to_vec(),
        account: account.to_owned(),
        accesses: vec![ACCESS_SEARCH_CONSOLE.to_owned()],
    }
}
