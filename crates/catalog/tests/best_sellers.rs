mod common;

use common::*;
use timada_catalog::{MAX_BEST_SELLERS, best_sellers_id};

/// 2026-11-16 12:00 UTC, and the same hour a month later.
const NOVEMBER: u64 = 1_794_830_400;
const DECEMBER: u64 = NOVEMBER + 30 * 86_400;

fn ids(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[tokio::test]
async fn a_ranking_is_written_when_it_changes() {
    let cmd = command().await;
    // Nothing sold, nothing said.
    assert!(!cmd.rank_best_sellers(ALICE, &[], NOVEMBER).await.unwrap());
    let ranked = ids(&["mat", "mug", "mat"]);
    assert!(
        cmd.rank_best_sellers(ALICE, &ranked, NOVEMBER)
            .await
            .unwrap()
    );
    let state = cmd
        .load_best_sellers(&best_sellers_id(NOVEMBER))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.product_ids, ids(&["mat", "mug"]));
    // The same again costs nothing; another order is written.
    assert!(
        !cmd.rank_best_sellers(ALICE, &ranked, NOVEMBER + 3600)
            .await
            .unwrap()
    );
    assert!(
        cmd.rank_best_sellers(ALICE, &ids(&["mug", "mat"]), NOVEMBER + 7200)
            .await
            .unwrap()
    );

    let many: Vec<String> = (0..MAX_BEST_SELLERS + 5).map(|n| format!("p{n}")).collect();
    cmd.rank_best_sellers(ALICE, &many, NOVEMBER).await.unwrap();
    let state = cmd
        .load_best_sellers(&best_sellers_id(NOVEMBER))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.product_ids.len(), MAX_BEST_SELLERS);
}

#[tokio::test]
async fn each_month_has_its_stream_and_an_empty_ranking_is_said_once() {
    let cmd = command().await;
    assert_ne!(best_sellers_id(NOVEMBER), best_sellers_id(DECEMBER));
    cmd.rank_best_sellers(ALICE, &ids(&["mat"]), NOVEMBER)
        .await
        .unwrap();
    // The month after, nothing sold any more: told once, on its stream.
    assert!(cmd.rank_best_sellers(ALICE, &[], DECEMBER).await.unwrap());
    assert!(!cmd.rank_best_sellers(ALICE, &[], DECEMBER).await.unwrap());
    let december = cmd
        .load_best_sellers(&best_sellers_id(DECEMBER))
        .await
        .unwrap()
        .unwrap();
    assert!(december.product_ids.is_empty());
    // The month after that opens on nothing, and nothing is written.
    let january = DECEMBER + 31 * 86_400;
    assert!(!cmd.rank_best_sellers(ALICE, &[], january).await.unwrap());
    assert!(
        cmd.load_best_sellers(&best_sellers_id(january))
            .await
            .unwrap()
            .is_none()
    );
}
