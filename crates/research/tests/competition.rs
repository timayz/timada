mod common;

use common::*;
use timada_core::db::Db;
use timada_research::aggregator::{KeywordRow, ResultRow};
use timada_research::{
    FetchStatus, MAX_CHECKED_KEYWORDS, MAX_RESULTS, RESEARCH_ACTOR, ResearchError, Sale, SetInputs,
    domains_of_study, main_keywords, read_model_subscription, study_row,
};

async fn catch_up(executor: &evento::Turso, db: &Db) {
    read_model_subscription::<evento::Turso>(db.clone())
        .run_once(executor)
        .await
        .unwrap();
}

/// A keyword easy enough to keep, with a cheap click.
fn easy(keyword: &str, volume: u64) -> KeywordRow {
    KeywordRow {
        keyword: keyword.to_owned(),
        volume: Some(volume),
        cpc_usd_cents: Some(10),
        difficulty: Some(10),
        intent: Some("commercial".to_owned()),
    }
}

/// Three main keywords and ten long-tail ones: enough of both.
fn enough_rows() -> Vec<KeywordRow> {
    let mut rows = vec![
        easy("griffoir", 12_100),
        easy("griffoir chat", 5_400),
        easy("griffoir mural", 2_900),
    ];
    rows.extend((0..10).map(|n| easy(&format!("griffoir modèle {n}"), 400 - n)));
    rows
}

/// A sale that leaves something, and enough products: with
/// [`enough_rows`], every test passes but the competition.
fn inputs() -> SetInputs {
    SetInputs {
        sale: Sale {
            currency: "USD".into(),
            price_cents: Some(6_000),
            cost_cents: Some(1_500),
            shipping_cents: 400,
            import_cents: 500,
            vat_bps: 2_000,
            fixed_cents: 100,
            payment_fee_bps: 150,
            payment_fee_cents: 25,
        },
        supplier_products: Some(240),
        ..SetInputs::default()
    }
}

fn results(domains: &[&str]) -> Vec<ResultRow> {
    domains
        .iter()
        .enumerate()
        .map(|(at, domain)| ResultRow {
            position: at as u8 + 1,
            domain: (*domain).to_owned(),
            url: format!("https://{domain}/griffoirs"),
            title: format!("Griffoirs - {domain}"),
        })
        .collect()
}

fn keywords(list: &[&str]) -> Vec<String> {
    list.iter().map(|keyword| keyword.to_string()).collect()
}

/// Asks for the first results of `keyword`, takes the search on and
/// records `domains` for it, as the host's saga does.
async fn search(research: &Research, study_id: &str, keyword: &str, domains: &[&str]) {
    research
        .request_results(ALICE, study_id, &keywords(&[keyword]), true)
        .await
        .unwrap();
    assert!(
        research
            .start_results(RESEARCH_ACTOR, study_id, keyword)
            .await
            .unwrap()
    );
    assert!(
        research
            .record_results(RESEARCH_ACTOR, study_id, keyword, results(domains), 2_000)
            .await
            .unwrap()
    );
}

/// The sites found for a study, as `domain keywords positions`.
async fn found(db: &Db, study_id: &str) -> Vec<String> {
    domains_of_study(db, study_id)
        .await
        .unwrap()
        .into_iter()
        .map(|row| format!("{} {} [{}]", row.domain, row.keywords, row.positions))
        .collect()
}

#[tokio::test]
async fn the_first_results_are_asked_for_once_and_taken_on_by_one_node() {
    let research = command().await;
    let study = study(&research, &["griffoir"]).await;
    let mains = keywords(&["griffoir", "griffoir  chat", "griffoir"]);

    assert_eq!(
        research
            .request_results(ALICE, &study.id, &mains, false)
            .await
            .unwrap(),
        2,
        "a keyword named twice is asked for once"
    );
    let asked = research.require_owned(&study.id, ALICE).await.unwrap();
    assert_eq!(asked.checks.len(), 2);
    assert_eq!(asked.checks[1].keyword, "griffoir chat");
    assert!(
        asked
            .checks
            .iter()
            .all(|check| check.status == FetchStatus::Requested)
    );
    // On their way: nothing left to ask for, even again.
    for again in [false, true] {
        assert!(matches!(
            research
                .request_results(ALICE, &study.id, &mains, again)
                .await,
            Err(ResearchError::NothingToCheck)
        ));
    }

    assert!(
        research
            .start_results(RESEARCH_ACTOR, &study.id, "griffoir")
            .await
            .unwrap()
    );
    assert!(
        !research
            .start_results("another node", &study.id, "griffoir")
            .await
            .unwrap(),
        "taken on already"
    );
    let mut many = results(&[
        "WWW.Amazon.fr",
        "zooplus.fr",
        "not a site",
        "a.fr",
        "b.fr",
        "c.fr",
        "d.fr",
        "e.fr",
        "f.fr",
        "g.fr",
        "h.fr",
        "i.fr",
        "j.fr",
        "k.fr",
        "l.fr",
        "m.fr",
        "n.fr",
        "o.fr",
        "p.fr",
        "q.fr",
        "r.fr",
        "s.fr",
        "t.fr",
        "u.fr",
        "v.fr",
        "w.fr",
    ]);
    many[1].title = format!("  Griffoirs \n {}", "x".repeat(500));
    assert!(
        research
            .record_results(RESEARCH_ACTOR, &study.id, "griffoir", many, 2_000)
            .await
            .unwrap()
    );
    assert!(
        research
            .record_results_failure(
                RESEARCH_ACTOR,
                &study.id,
                "griffoir chat",
                "The keyword source did not answer; try again in a moment."
            )
            .await
            .unwrap()
    );
    let after = research.require_owned(&study.id, ALICE).await.unwrap();
    assert_eq!(after.checks[0].status, FetchStatus::Fetched);
    assert_eq!(after.checks[0].results as usize, MAX_RESULTS);
    assert!(after.checks[0].fetched_at.is_some());
    assert_eq!(after.checks[1].status, FetchStatus::Failed);
    assert!(after.checks[1].last_error.is_some());
    assert_eq!(after.cost_usd_micros, 2_000);

    // Only the one that failed is owed; again asks for both.
    assert_eq!(
        research
            .request_results(ALICE, &study.id, &mains, false)
            .await
            .unwrap(),
        1
    );
    research
        .record_results_failure(RESEARCH_ACTOR, &study.id, "griffoir chat", "again")
        .await
        .unwrap();
    assert_eq!(
        research
            .request_results(ALICE, &study.id, &mains, true)
            .await
            .unwrap(),
        2
    );

    // The events hold the site as its marks name it, and a cut title.
    let fetched = evento::read::<timada_research::aggregator::Study>(&study.id)
        .decode(&research.0)
        .await
        .unwrap()
        .into_iter()
        .find_map(|event| match event {
            timada_research::aggregator::StudyEvent::ResultsFetched(fetched) => Some(fetched),
            _ => None,
        })
        .unwrap();
    assert_eq!(fetched.results[0].domain, "amazon.fr");
    assert_eq!(fetched.results[1].title.chars().count(), 200);
    assert_eq!(
        fetched.results[2].domain, "a.fr",
        "what is no site is passed over"
    );
    assert_eq!(fetched.results[2].position, 4);
}

#[tokio::test]
async fn what_is_asked_for_is_checked() {
    let research = command().await;
    let study = study(&research, &["griffoir"]).await;
    assert!(matches!(
        research.request_results(ALICE, &study.id, &[], false).await,
        Err(ResearchError::NoMainKeyword)
    ));
    let many: Vec<String> = (0..=MAX_CHECKED_KEYWORDS)
        .map(|n| format!("griffoir {n}"))
        .collect();
    assert!(matches!(
        research
            .request_results(ALICE, &study.id, &many, false)
            .await,
        Err(ResearchError::TooManyChecked)
    ));
    assert!(matches!(
        research
            .request_results(MALLORY, &study.id, &keywords(&["griffoir"]), false)
            .await,
        Err(ResearchError::NotFound)
    ));
    // Nothing was asked for: nothing to take on, nothing to record.
    assert!(
        !research
            .start_results(RESEARCH_ACTOR, &study.id, "griffoir")
            .await
            .unwrap()
    );
    assert!(
        !research
            .record_results(
                RESEARCH_ACTOR,
                &study.id,
                "griffoir",
                results(&["a.fr"]),
                2_000
            )
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn a_mark_is_the_users_over_all_their_studies() {
    let research = command().await;
    assert!(research.load_marks(ALICE).await.unwrap().marks.is_empty());
    research
        .mark_domain(ALICE, " WWW.Amazon.FR ", false)
        .await
        .unwrap();
    research
        .mark_domain(ALICE, "zooplus.fr", true)
        .await
        .unwrap();
    let marks = research.load_marks(ALICE).await.unwrap();
    assert_eq!(marks.specialist("amazon.fr"), Some(false));
    assert_eq!(marks.specialist("zooplus.fr"), Some(true));
    assert_eq!(marks.specialist("cdiscount.com"), None);

    // The same again writes nothing; the other answer replaces it.
    research
        .mark_domain(ALICE, "amazon.fr", false)
        .await
        .unwrap();
    let again = research.load_marks(ALICE).await.unwrap();
    assert_eq!(again.aggregate_version, marks.aggregate_version);
    research
        .mark_domain(ALICE, "zooplus.fr", false)
        .await
        .unwrap();
    assert_eq!(
        research
            .load_marks(ALICE)
            .await
            .unwrap()
            .specialist("zooplus.fr"),
        Some(false)
    );

    assert!(matches!(
        research.mark_domain(ALICE, "not a site", true).await,
        Err(ResearchError::InvalidDomain)
    ));
    // Another user's marks are their own.
    assert!(research.load_marks(MALLORY).await.unwrap().marks.is_empty());
}

#[tokio::test]
async fn the_read_model_says_who_holds_the_first_results() {
    let (executor, db) = executor_with_pool().await;
    let research: Research = timada_research::Command(executor.clone());
    let study = study(&research, &["griffoir"]).await;
    fetch(&research, &study.id, "griffoir", enough_rows()).await;
    research
        .set_inputs(ALICE, &study.id, inputs())
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert_eq!(
        main_keywords(&db, &study.id).await.unwrap(),
        ["griffoir", "griffoir chat", "griffoir mural"]
    );
    let row = study_row(&db, &study.id).await.unwrap().unwrap();
    assert_eq!(row.competition, "not_checked");
    assert_eq!((row.searched_count, row.owed_count), (0, 3));
    assert_eq!(row.verdict, "incomplete", "only the competition is open");

    // Two of the three searched; a site twice in one search counts once,
    // at its best place.
    search(
        &research,
        &study.id,
        "griffoir",
        &["amazon.fr", "zooplus.fr", "amazon.fr", "blog.fr"],
    )
    .await;
    search(
        &research,
        &study.id,
        "griffoir chat",
        &["zooplus.fr", "cdiscount.com", "amazon.fr"],
    )
    .await;
    catch_up(&executor, &db).await;
    assert_eq!(
        found(&db, &study.id).await,
        [
            "amazon.fr 2 [1, 3]",
            "zooplus.fr 2 [1, 2]",
            "cdiscount.com 1 [2]",
            "blog.fr 1 [4]",
        ]
    );
    let row = study_row(&db, &study.id).await.unwrap().unwrap();
    assert_eq!((row.searched_count, row.owed_count), (2, 1));
    assert_eq!(row.competition, "not_checked");

    // The last one searched, the sites found for several still unmarked.
    search(&research, &study.id, "griffoir mural", &["leroymerlin.fr"]).await;
    research
        .mark_domain(ALICE, "amazon.fr", false)
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    let row = study_row(&db, &study.id).await.unwrap().unwrap();
    assert_eq!((row.searched_count, row.owed_count), (3, 0));
    assert_eq!(row.competition, "not_checked", "zooplus.fr is not marked");

    // A generalist: nobody holds the niche, and every test passes.
    research
        .mark_domain(ALICE, "zooplus.fr", false)
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    // Delivery may repeat: nothing drifts.
    catch_up(&executor, &db).await;
    let row = study_row(&db, &study.id).await.unwrap().unwrap();
    assert_eq!(row.competition, "passed");
    assert_eq!(row.verdict, "go");

    // A specialist after all, for two of the main keywords.
    research
        .mark_domain(ALICE, "zooplus.fr", true)
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    let row = study_row(&db, &study.id).await.unwrap().unwrap();
    assert_eq!(row.competition, "failed");
    assert_eq!(row.verdict, "no_go");

    // One of the two keywords dropped: the specialist is found for one
    // main keyword only, which decides nothing; two main keywords are not
    // enough any more, which the other tests say.
    research
        .drop_keyword(ALICE, &study.id, "griffoir chat")
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert_eq!(
        found(&db, &study.id).await,
        [
            "amazon.fr 1 [1]",
            "leroymerlin.fr 1 [1]",
            "zooplus.fr 1 [2]",
            "blog.fr 1 [4]",
        ]
    );
    let row = study_row(&db, &study.id).await.unwrap().unwrap();
    assert_eq!(row.competition, "passed");
    assert_eq!(row.verdict, "no_go");
    // Brought back, its results are still held: nothing is paid again.
    research
        .restore_keyword(ALICE, &study.id, "griffoir chat")
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    let row = study_row(&db, &study.id).await.unwrap().unwrap();
    assert_eq!((row.searched_count, row.owed_count), (3, 0));
    assert_eq!(row.competition, "failed");

    // A search again replaces what the keyword gave.
    search(&research, &study.id, "griffoir chat", &["cdiscount.com"]).await;
    catch_up(&executor, &db).await;
    let row = study_row(&db, &study.id).await.unwrap().unwrap();
    assert_eq!(row.competition, "passed");
    assert_eq!(row.verdict, "go");
}

#[tokio::test]
async fn a_mark_made_on_one_study_counts_on_the_next() {
    let (executor, db) = executor_with_pool().await;
    let research: Research = timada_research::Command(executor.clone());
    // The marks come first, before any study found the sites.
    research
        .mark_domain(ALICE, "amazon.fr", false)
        .await
        .unwrap();
    research
        .mark_domain(ALICE, "zooplus.fr", true)
        .await
        .unwrap();
    research
        .mark_domain(MALLORY, "zooplus.fr", false)
        .await
        .unwrap();
    catch_up(&executor, &db).await;

    let study = study(&research, &["griffoir"]).await;
    fetch(&research, &study.id, "griffoir", enough_rows()).await;
    for keyword in ["griffoir", "griffoir chat", "griffoir mural"] {
        search(&research, &study.id, keyword, &["amazon.fr", "zooplus.fr"]).await;
    }
    catch_up(&executor, &db).await;
    let row = study_row(&db, &study.id).await.unwrap().unwrap();
    assert_eq!(row.competition, "failed", "Alice's marks, not Mallory's");

    // Deleted, nothing of it is left.
    research.delete_study(ALICE, &study.id).await.unwrap();
    catch_up(&executor, &db).await;
    assert!(found(&db, &study.id).await.is_empty());
    assert!(study_row(&db, &study.id).await.unwrap().is_none());
    // A mark after it has no study to count on.
    research
        .mark_domain(ALICE, "zooplus.fr", false)
        .await
        .unwrap();
    catch_up(&executor, &db).await;
}
