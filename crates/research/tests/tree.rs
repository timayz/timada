mod common;

use common::*;
use std::collections::HashSet;

use timada_core::db::Db;
use timada_core::page::PageFrom;
use timada_research::aggregator::KeywordRow;
use timada_research::{
    KeywordFigures, MAX_NICHE_WORDS, ResearchError, Thresholds, TreeProblem, kept_keywords_without,
    keyword_figures, read_model_subscription, review_tree,
};

async fn catch_up(executor: &evento::Turso, db: &Db) {
    read_model_subscription::<evento::Turso>(db.clone())
        .run_once(executor)
        .await
        .unwrap();
}

/// A keyword easy enough to keep.
fn easy(keyword: &str, volume: u64) -> KeywordRow {
    KeywordRow {
        keyword: keyword.to_owned(),
        volume: Some(volume),
        cpc_usd_cents: Some(50),
        difficulty: Some(10),
        intent: Some("commercial".to_owned()),
    }
}

fn shape(study: &timada_research::StudyState) -> Vec<(&str, Option<&str>)> {
    study
        .tree
        .ordered()
        .into_iter()
        .map(|category| (category.keyword.as_str(), category.parent.as_deref()))
        .collect()
}

#[tokio::test]
async fn a_keyword_becomes_a_parent_a_child_or_is_parked() {
    let research = command().await;
    let study = study(&research, &["gants moto"]).await;
    let id = study.id.as_str();

    research
        .place_category(ALICE, id, " Gants  moto ", None)
        .await
        .unwrap();
    research
        .place_category(ALICE, id, "casque moto", None)
        .await
        .unwrap();
    research
        .place_category(ALICE, id, "gants moto été", Some("Gants moto"))
        .await
        .unwrap();
    research
        .place_category(ALICE, id, "casque moto intégral", Some("casque moto"))
        .await
        .unwrap();
    research
        .park_category(ALICE, id, "bottes moto")
        .await
        .unwrap();
    let study = research.load_study(id).await.unwrap().unwrap();
    assert_eq!(
        shape(&study),
        [
            ("Gants moto", None),
            ("gants moto été", Some("Gants moto")),
            ("casque moto", None),
            ("casque moto intégral", Some("casque moto")),
        ]
    );
    assert_eq!(study.tree.parked, ["bottes moto"]);

    // The same again writes nothing.
    let version = study.aggregate_version;
    research
        .place_category(ALICE, id, "casque moto", None)
        .await
        .unwrap();
    research
        .park_category(ALICE, id, "bottes moto")
        .await
        .unwrap();
    research
        .remove_category(ALICE, id, "never placed")
        .await
        .unwrap();
    let study = research.load_study(id).await.unwrap().unwrap();
    assert_eq!(study.aggregate_version, version);

    // A child moves to another parent and keeps its price; a parked keyword
    // comes into the tree; one in the tree is parked.
    research
        .price_category(ALICE, id, "gants moto été", Some(4_500))
        .await
        .unwrap();
    research
        .place_category(ALICE, id, "gants moto été", Some("casque moto"))
        .await
        .unwrap();
    research
        .place_category(ALICE, id, "bottes moto", Some("casque moto"))
        .await
        .unwrap();
    research
        .park_category(ALICE, id, "casque moto intégral")
        .await
        .unwrap();
    let study = research.load_study(id).await.unwrap().unwrap();
    assert_eq!(
        shape(&study),
        [
            ("Gants moto", None),
            ("casque moto", None),
            ("gants moto été", Some("casque moto")),
            ("bottes moto", Some("casque moto")),
        ]
    );
    assert_eq!(study.tree.parked, ["casque moto intégral"]);
    assert_eq!(
        study.tree.category("gants moto été").unwrap().price_cents,
        Some(4_500)
    );

    research
        .remove_category(ALICE, id, "bottes moto")
        .await
        .unwrap();
    research
        .remove_category(ALICE, id, "casque moto intégral")
        .await
        .unwrap();
    let study = research.load_study(id).await.unwrap().unwrap();
    assert_eq!(study.tree.categories.len(), 3);
    assert!(study.tree.parked.is_empty());
}

#[tokio::test]
async fn the_tree_has_two_levels_and_a_parent_keeps_its_children() {
    let research = command().await;
    let study = study(&research, &["gants moto"]).await;
    let id = study.id.as_str();
    research
        .place_category(ALICE, id, "gants moto", None)
        .await
        .unwrap();
    research
        .place_category(ALICE, id, "gants moto été", Some("gants moto"))
        .await
        .unwrap();
    research
        .place_category(ALICE, id, "casque moto", None)
        .await
        .unwrap();

    // Under a child, under itself, under a keyword that is no category.
    for (keyword, parent) in [
        ("gants moto cuir", "gants moto été"),
        ("casque moto", "casque moto"),
        ("casque moto", "bottes moto"),
    ] {
        assert!(
            matches!(
                research
                    .place_category(ALICE, id, keyword, Some(parent))
                    .await,
                Err(ResearchError::UnknownParent)
            ),
            "{keyword} under {parent}"
        );
    }
    // A parent with children goes nowhere until they do.
    assert!(matches!(
        research
            .place_category(ALICE, id, "gants moto", Some("casque moto"))
            .await,
        Err(ResearchError::CategoryHasChildren)
    ));
    assert!(matches!(
        research.park_category(ALICE, id, "gants moto").await,
        Err(ResearchError::CategoryHasChildren)
    ));
    assert!(matches!(
        research.remove_category(ALICE, id, "gants moto").await,
        Err(ResearchError::CategoryHasChildren)
    ));
    research
        .remove_category(ALICE, id, "gants moto été")
        .await
        .unwrap();
    research
        .place_category(ALICE, id, "gants moto", Some("casque moto"))
        .await
        .unwrap();

    // A price is a category's; nobody else touches the tree.
    assert!(matches!(
        research
            .price_category(ALICE, id, "bottes moto", Some(1))
            .await,
        Err(ResearchError::NotInTree)
    ));
    assert!(matches!(
        research
            .place_category(MALLORY, id, "bottes moto", None)
            .await,
        Err(ResearchError::NotFound)
    ));
    research
        .price_category(ALICE, id, "gants moto", Some(0))
        .await
        .unwrap();
    let study = research.load_study(id).await.unwrap().unwrap();
    assert_eq!(study.tree.category("gants moto").unwrap().price_cents, None);
}

#[tokio::test]
async fn the_niche_is_named_by_its_seeds_until_its_owner_says() {
    let (executor, db) = executor_with_pool().await;
    let research = timada_research::Command(executor.clone());
    let study = study(&research, &["gants moto", "casque moto"]).await;
    let id = study.id.as_str();
    fetch(
        &research,
        id,
        "gants moto",
        vec![
            easy("gants moto", 5_000),
            easy("gants moto été", 400),
            easy("gants", 9_000),
            easy("gants moto pas cher", 40),
        ],
    )
    .await;
    catch_up(&executor, &db).await;
    for keyword in [
        "gants moto",
        "gants",
        "gants moto pas cher",
        "gants moto cuir",
    ] {
        research
            .place_category(ALICE, id, keyword, None)
            .await
            .unwrap();
    }
    research
        .place_category(ALICE, id, "gants moto été", Some("gants moto"))
        .await
        .unwrap();

    let review = async |research: &Research| {
        let study = research.load_study(id).await.unwrap().unwrap();
        let keywords: Vec<String> = study
            .tree
            .categories
            .iter()
            .map(|category| category.keyword.clone())
            .collect();
        let figures = keyword_figures(&db, id, &keywords).await.unwrap();
        let seeds: Vec<String> = study.seeds.iter().map(|seed| seed.seed.clone()).collect();
        (
            review_tree(
                &study.tree,
                &seeds,
                &study.inputs,
                &Thresholds::default(),
                &figures,
            ),
            figures,
        )
    };
    let (checked, figures) = review(&research).await;
    assert_eq!(
        figures["gants moto été"],
        KeywordFigures {
            volume: Some(400),
            cpc_usd_cents: Some(50),
            difficulty: Some(10),
            kept: true,
        }
    );
    assert!(!figures["gants moto pas cher"].kept, "volume too low");
    assert!(!figures.contains_key("gants moto cuir"), "never fetched");
    assert_eq!(checked.niche_words, ["moto"]);
    let problems = |keyword: &str| {
        checked
            .categories
            .iter()
            .find(|category| category.keyword == keyword)
            .unwrap()
            .problems
            .clone()
    };
    assert!(problems("gants moto").is_empty());
    assert!(problems("gants moto été").is_empty());
    assert_eq!(problems("gants"), [TreeProblem::NoNicheWord]);
    assert_eq!(problems("gants moto pas cher"), [TreeProblem::NotKept]);
    assert_eq!(problems("gants moto cuir"), [TreeProblem::NotKept]);
    assert_eq!(checked.held_back(), 3);

    research
        .set_niche_words(ALICE, id, &[" Gants ".to_owned(), "gants".to_owned()])
        .await
        .unwrap();
    let (checked, _) = review(&research).await;
    assert_eq!(checked.niche_words, ["Gants", "gants"]);
    assert_eq!(checked.held_back(), 2);

    let many: Vec<String> = (0..=MAX_NICHE_WORDS).map(|n| format!("mot{n}")).collect();
    assert!(matches!(
        research.set_niche_words(ALICE, id, &many).await,
        Err(ResearchError::TooManyNicheWords)
    ));
}

/// What is left to choose from: the kept keywords, main and long-tail in
/// one list, the most searched first, without the ones the tree took.
#[tokio::test]
async fn the_keywords_left_to_choose_are_the_kept_ones_the_tree_has_not() {
    let (executor, db) = executor_with_pool().await;
    let research = timada_research::Command(executor.clone());
    let study = study(&research, &["gants moto"]).await;
    let id = study.id.as_str();
    // Seven main keywords, seven long-tail ones, and one too rare to keep.
    let mut rows: Vec<KeywordRow> = (0..7)
        .flat_map(|n| {
            [
                easy(&format!("gants moto main {n}"), 2_000 + n),
                easy(&format!("gants moto tail {n}"), 200 + n),
            ]
        })
        .collect();
    rows.push(easy("gants moto rare", 40));
    fetch(&research, id, "gants moto", rows).await;
    catch_up(&executor, &db).await;

    let taken: HashSet<String> = [
        "gants moto main 6",
        "gants moto main 5",
        "gants moto main 2",
        "gants moto tail 6",
        "gants moto tail 0",
    ]
    .map(str::to_owned)
    .into();
    let expected = [
        "gants moto main 4",
        "gants moto main 3",
        "gants moto main 1",
        "gants moto main 0",
        "gants moto tail 5",
        "gants moto tail 4",
        "gants moto tail 3",
        "gants moto tail 2",
        "gants moto tail 1",
    ];
    let keywords = |rows: &[timada_research::KeywordRecord]| -> Vec<String> {
        rows.iter().map(|row| row.keyword.clone()).collect()
    };

    let all = kept_keywords_without(&db, id, &taken, PageFrom::First, 50)
        .await
        .unwrap();
    assert_eq!(keywords(&all.rows), expected);
    assert!(all.newer.is_none() && all.older.is_none());

    // Two a page, down to the last and back up: the taken ones are stepped
    // over, whichever way the list is walked.
    let mut down = Vec::new();
    let mut page = kept_keywords_without(&db, id, &taken, PageFrom::First, 2)
        .await
        .unwrap();
    let mut pages = vec![keywords(&page.rows)];
    loop {
        down.extend(keywords(&page.rows));
        let Some(older) = page.older.clone() else {
            break;
        };
        page = kept_keywords_without(&db, id, &taken, PageFrom::Older(&older), 2)
            .await
            .unwrap();
        pages.push(keywords(&page.rows));
    }
    assert_eq!(down, expected);
    assert_eq!(pages.len(), 5);
    while let Some(newer) = page.newer.clone() {
        pages.pop();
        page = kept_keywords_without(&db, id, &taken, PageFrom::Newer(&newer), 2)
            .await
            .unwrap();
        assert_eq!(Some(&keywords(&page.rows)), pages.last());
    }
    assert_eq!(pages.len(), 1);

    // With nothing taken the list is every kept keyword.
    let none = kept_keywords_without(&db, id, &HashSet::new(), PageFrom::First, 50)
        .await
        .unwrap();
    assert_eq!(none.rows.len(), 14);
}

/// A refused keyword is out of the tree and of the parked ones until its
/// owner brings it back or places it after all.
#[tokio::test]
async fn a_refused_keyword_is_set_aside_until_it_is_brought_back() {
    let research = command().await;
    let study = study(&research, &["gants moto"]).await;
    let id = study.id.as_str();
    research
        .place_category(ALICE, id, "gants moto", None)
        .await
        .unwrap();
    research
        .place_category(ALICE, id, "gants moto été", Some("gants moto"))
        .await
        .unwrap();
    research
        .park_category(ALICE, id, "bottes moto")
        .await
        .unwrap();
    for keyword in ["gants moto été", "bottes moto", "gants de ski"] {
        research.refuse_category(ALICE, id, keyword).await.unwrap();
    }
    let study = research.load_study(id).await.unwrap().unwrap();
    assert_eq!(shape(&study), [("gants moto", None)]);
    assert!(study.tree.parked.is_empty());
    assert_eq!(
        study.tree.refused,
        ["gants moto été", "bottes moto", "gants de ski"]
    );

    // The same again writes nothing, and a stranger refuses nothing.
    let version = study.aggregate_version;
    research
        .refuse_category(ALICE, id, "gants de ski")
        .await
        .unwrap();
    assert!(
        research
            .refuse_category(MALLORY, id, "gants moto")
            .await
            .is_err()
    );
    let study = research.load_study(id).await.unwrap().unwrap();
    assert_eq!(study.aggregate_version, version);

    // Brought back, placed after all, or parked: no longer refused.
    research
        .remove_category(ALICE, id, "gants de ski")
        .await
        .unwrap();
    research
        .place_category(ALICE, id, "gants moto été", None)
        .await
        .unwrap();
    research
        .park_category(ALICE, id, "bottes moto")
        .await
        .unwrap();
    let study = research.load_study(id).await.unwrap().unwrap();
    assert!(study.tree.refused.is_empty());
    assert_eq!(study.tree.parked, ["bottes moto"]);
    assert_eq!(
        shape(&study),
        [("gants moto", None), ("gants moto été", None)]
    );

    // A parent category keeps its children: it is not refused over them.
    research
        .place_category(ALICE, id, "gants moto hiver", Some("gants moto"))
        .await
        .unwrap();
    assert!(matches!(
        research.refuse_category(ALICE, id, "gants moto").await,
        Err(ResearchError::CategoryHasChildren)
    ));
}
