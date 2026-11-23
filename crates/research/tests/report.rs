mod common;

use common::*;
use timada_core::db::Db;
use timada_core::page::PageFrom;
use timada_research::aggregator::KeywordRow;
use timada_research::{
    DUPLICATE, MAX_LISTED_WORDS, ResearchError, Sale, SetInputs, Thresholds, Verdict, judge,
    keywords_of_outcome, outcome_counts, read_model_subscription, study_row,
};

async fn catch_up(executor: &evento::Turso, db: &Db) {
    read_model_subscription::<evento::Turso>(db.clone())
        .run_once(executor)
        .await
        .unwrap();
}

/// A keyword easy enough to keep, with a click at `cpc_usd_cents`.
fn easy(keyword: &str, volume: u64, cpc_usd_cents: u64) -> KeywordRow {
    KeywordRow {
        keyword: keyword.to_owned(),
        volume: Some(volume),
        cpc_usd_cents: Some(cpc_usd_cents),
        difficulty: Some(10),
        intent: Some("commercial".to_owned()),
    }
}

/// The sale of the score's own tests: 60.00 leaves 23.85, breaks even at
/// 2.52 and needs a price / CPC of 194.67.
fn inputs() -> SetInputs {
    SetInputs {
        sale: Sale {
            currency: "eur".into(),
            price_cents: Some(6_000),
            cost_cents: Some(1_500),
            shipping_cents: 400,
            import_cents: 500,
            vat_bps: 2_000,
            fixed_cents: 100,
            payment_fee_bps: 150,
            payment_fee_cents: 25,
        },
        ..SetInputs::default()
    }
}

/// The keywords of one outcome, the most searched first.
async fn of_outcome(db: &Db, study_id: &str, outcome: &str) -> Vec<String> {
    keywords_of_outcome(db, study_id, outcome, PageFrom::First, 1_000)
        .await
        .unwrap()
        .rows
        .into_iter()
        .map(|row| row.keyword)
        .collect()
}

#[tokio::test]
async fn the_owner_says_what_the_study_is_scored_from() {
    let research = command().await;
    let study = study(&research, &["griffoir"]).await;
    assert!(study.inputs_set_at.is_none());

    let said = SetInputs {
        brands: vec![
            " Trixie ".into(),
            "trixie".into(),
            "".into(),
            "Trixie".into(),
        ],
        excluded_words: vec!["diy".into()],
        supplier_products: Some(240),
        specialist_first: Some(false),
        usd_micros: Some(1_085_000),
        ..inputs()
    };
    research
        .set_inputs(ALICE, &study.id, said.clone())
        .await
        .unwrap();
    let loaded = research.require_owned(&study.id, ALICE).await.unwrap();
    assert_eq!(loaded.inputs.sale.currency, "EUR");
    assert_eq!(loaded.inputs.sale.price_cents, Some(6_000));
    assert_eq!(loaded.inputs.brands, ["Trixie", "trixie"]);
    assert_eq!(loaded.inputs.excluded_words, ["diy"]);
    assert_eq!(loaded.inputs.supplier_products, Some(240));
    assert_eq!(loaded.inputs.specialist_first, Some(false));
    assert_eq!(loaded.inputs.usd_micros, Some(1_085_000));
    assert!(loaded.inputs_set_at.is_some());

    // The same again writes nothing.
    research.set_inputs(ALICE, &study.id, said).await.unwrap();
    let again = research.require_owned(&study.id, ALICE).await.unwrap();
    assert_eq!(again.aggregate_version, loaded.aggregate_version);

    // Nobody else's to say.
    assert!(matches!(
        research.set_inputs(MALLORY, &study.id, inputs()).await,
        Err(ResearchError::NotFound)
    ));
    assert!(matches!(
        research.drop_keyword(MALLORY, &study.id, "griffoir").await,
        Err(ResearchError::NotFound)
    ));
}

#[tokio::test]
async fn what_the_owner_says_is_checked() {
    let research = command().await;
    let study = study(&research, &["griffoir"]).await;
    let mut unknown = inputs();
    unknown.sale.currency = "XXX".into();
    assert!(matches!(
        research.set_inputs(ALICE, &study.id, unknown).await,
        Err(ResearchError::InvalidCurrency)
    ));
    let mut taxed = inputs();
    taxed.sale.vat_bps = 10_001;
    assert!(matches!(
        research.set_inputs(ALICE, &study.id, taxed).await,
        Err(ResearchError::InvalidRate)
    ));
    let many = SetInputs {
        brands: (0..=MAX_LISTED_WORDS)
            .map(|n| format!("brand {n}"))
            .collect(),
        ..inputs()
    };
    assert!(matches!(
        research.set_inputs(ALICE, &study.id, many).await,
        Err(ResearchError::TooManyWords)
    ));
    let long = SetInputs {
        excluded_words: vec!["w".repeat(81)],
        ..inputs()
    };
    assert!(matches!(
        research.set_inputs(ALICE, &study.id, long).await,
        Err(ResearchError::InvalidWord)
    ));
    assert!(matches!(
        research.drop_keyword(ALICE, &study.id, "  ").await,
        Err(ResearchError::InvalidKeyword)
    ));
}

#[tokio::test]
async fn a_keyword_is_dropped_or_brought_back_by_hand() {
    let research = command().await;
    let study = study(&research, &["griffoir"]).await;
    research
        .drop_keyword(ALICE, &study.id, "griffoir carton")
        .await
        .unwrap();
    let dropped = research.require_owned(&study.id, ALICE).await.unwrap();
    assert!(dropped.inputs.dropped.contains("griffoir carton"));
    // Twice is once.
    research
        .drop_keyword(ALICE, &study.id, "griffoir carton")
        .await
        .unwrap();
    let again = research.require_owned(&study.id, ALICE).await.unwrap();
    assert_eq!(again.aggregate_version, dropped.aggregate_version);

    research
        .restore_keyword(ALICE, &study.id, "griffoir carton")
        .await
        .unwrap();
    let restored = research.require_owned(&study.id, ALICE).await.unwrap();
    assert!(restored.inputs.dropped.is_empty());
    assert!(restored.inputs.restored.contains("griffoir carton"));

    research
        .drop_keyword(ALICE, &study.id, "griffoir carton")
        .await
        .unwrap();
    let dropped = research.require_owned(&study.id, ALICE).await.unwrap();
    assert!(dropped.inputs.restored.is_empty());
    assert!(dropped.inputs.dropped.contains("griffoir carton"));
}

#[tokio::test]
async fn a_fetch_scores_the_study_and_saving_scores_it_again_without_one() {
    let (executor, db) = executor_with_pool().await;
    let research = timada_research::Command(executor.clone());
    let study = study(&research, &["griffoir", "arbre à chat"]).await;
    fetch(
        &research,
        &study.id,
        "griffoir",
        vec![
            easy("griffoir", 12_100, 20),
            easy("griffoir carton", 880, 10),
            easy("griffoirs carton", 320, 10),
            easy("griffoir trixie", 400, 10),
            easy("comment fabriquer un griffoir", 700, 5),
            easy("griffoir mural", 90, 10),
            KeywordRow {
                difficulty: Some(80),
                ..easy("griffoir chat", 5_000, 30)
            },
        ],
    )
    .await;
    // The second seed gives one keyword the first gave too, less searched.
    fetch(
        &research,
        &study.id,
        "arbre à chat",
        vec![
            easy("arbre à chat", 40_000, 25),
            easy("griffoir", 9_000, 20),
        ],
    )
    .await;
    catch_up(&executor, &db).await;

    assert_eq!(
        of_outcome(&db, &study.id, "main").await,
        ["arbre à chat", "griffoir"]
    );
    assert_eq!(
        of_outcome(&db, &study.id, "long_tail").await,
        ["griffoir carton", "griffoir trixie"]
    );
    let variants = keywords_of_outcome(&db, &study.id, "variant", PageFrom::First, 10)
        .await
        .unwrap()
        .rows;
    assert_eq!(variants.len(), 1);
    assert_eq!(variants[0].keyword, "griffoirs carton");
    assert_eq!(variants[0].variant_of.as_deref(), Some("griffoir carton"));
    assert_eq!(
        of_outcome(&db, &study.id, "informational").await,
        ["comment fabriquer un griffoir"]
    );
    assert_eq!(
        of_outcome(&db, &study.id, "too_difficult").await,
        ["griffoir chat"]
    );
    assert_eq!(
        of_outcome(&db, &study.id, "low_volume").await,
        ["griffoir mural"]
    );
    assert_eq!(of_outcome(&db, &study.id, DUPLICATE).await, ["griffoir"]);
    let mut counts = outcome_counts(&db, &study.id).await.unwrap();
    counts.sort();
    assert_eq!(
        counts,
        [
            ("duplicate".to_owned(), 1),
            ("informational".to_owned(), 1),
            ("long_tail".to_owned(), 2),
            ("low_volume".to_owned(), 1),
            ("main".to_owned(), 2),
            ("too_difficult".to_owned(), 1),
            ("variant".to_owned(), 1),
        ]
    );
    let row = study_row(&db, &study.id).await.unwrap().unwrap();
    assert_eq!(row.verdict, "no_go", "two main keywords are not three");
    assert_eq!(row.scored_keywords, 8);
    assert_eq!((row.main_count, row.long_tail_count), (2, 2));
    assert_eq!(row.kept_volume, 40_000 + 12_100 + 880 + 400);
    assert_eq!(row.break_even_roas, None);

    // What the owner says scores the study again: no fetch.
    let before = research.require_owned(&study.id, ALICE).await.unwrap();
    research
        .set_inputs(
            ALICE,
            &study.id,
            SetInputs {
                brands: vec!["Trixie".into()],
                usd_micros: Some(1_000_000),
                ..inputs()
            },
        )
        .await
        .unwrap();
    research
        .drop_keyword(ALICE, &study.id, "griffoir carton")
        .await
        .unwrap();
    research
        .restore_keyword(ALICE, &study.id, "griffoir mural")
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    // Delivery may repeat: nothing drifts.
    catch_up(&executor, &db).await;
    let after = research.require_owned(&study.id, ALICE).await.unwrap();
    assert_eq!(after.cost_usd_micros, before.cost_usd_micros);

    assert_eq!(
        of_outcome(&db, &study.id, "brand").await,
        ["griffoir trixie"]
    );
    assert_eq!(
        of_outcome(&db, &study.id, "excluded").await,
        ["griffoir carton"]
    );
    // Its leader gone, the other spelling stands for itself.
    assert_eq!(
        of_outcome(&db, &study.id, "long_tail").await,
        ["griffoirs carton", "griffoir mural"]
    );
    assert!(of_outcome(&db, &study.id, "variant").await.is_empty());
    let row = study_row(&db, &study.id).await.unwrap().unwrap();
    assert_eq!(row.verdict, "no_go", "two long-tail keywords are not ten");
    assert_eq!(row.currency, "EUR");
    assert_eq!(row.break_even_roas, Some(252));
    assert_eq!(row.needed_price_per_click, Some(19_467));
    assert!(row.price_per_click.is_some());

    // The page reads the tests again from the row and the study.
    let judgement = judge(&row.tally(), &after.inputs, &Thresholds::default());
    assert_eq!(judgement.verdict, Verdict::NoGo);
    assert_eq!(
        judgement.price_per_click.map(|ratio| ratio as i64),
        row.price_per_click
    );
}
