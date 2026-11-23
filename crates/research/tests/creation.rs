mod common;

use common::*;
use timada_core::db::Db;
use timada_research::{
    CreateIn, PlanCategory, RESEARCH_ACTOR, ResearchError, category_name, category_slug,
    read_model_subscription, study_of_shop, study_row,
};

async fn catch_up(executor: &evento::Turso, db: &Db) {
    read_model_subscription::<evento::Turso>(db.clone())
        .run_once(executor)
        .await
        .unwrap();
}

fn plan(keywords: &[&str]) -> Vec<PlanCategory> {
    keywords
        .iter()
        .map(|keyword| PlanCategory {
            keyword: keyword.to_string(),
            name: category_name(keyword),
            slug: category_slug(keyword),
            meta_title: category_name(keyword),
            out_of_ads: *keyword == "casque moto",
        })
        .collect()
}

/// A study with two parents and a child under the first.
async fn planted(research: &Research) -> String {
    let study = study(research, &["gants moto"]).await;
    let id = study.id.clone();
    research
        .place_category(ALICE, &id, "gants moto", None)
        .await
        .unwrap();
    research
        .place_category(ALICE, &id, "casque moto", None)
        .await
        .unwrap();
    research
        .place_category(ALICE, &id, "gants moto été", Some("gants moto"))
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn a_request_plans_each_category_under_an_id_of_its_own() {
    let research = command().await;
    let id = planted(&research).await;
    let asked = research
        .request_creation(
            ALICE,
            &id,
            CreateIn::NewShop("  Gants de motard ".into()),
            &plan(&["gants moto", "gants moto été", "casque moto"]),
        )
        .await
        .unwrap();
    assert_eq!(asked, 3);
    let study = research.load_study(&id).await.unwrap().unwrap();
    let creation = study.creation.clone().unwrap();
    assert_eq!(creation.shop_name.as_deref(), Some("Gants de motard"));
    assert!(!creation.tenant_id.is_empty());
    assert!(study.shop.is_none(), "linked once the shop exists");
    let [gloves, summer, helmet] = creation.planned.as_slice() else {
        panic!("{:?}", creation.planned);
    };
    assert_eq!(
        (
            gloves.name.as_str(),
            gloves.slug.as_str(),
            gloves.parent_id.as_deref()
        ),
        ("Gants moto", "gants-moto", None)
    );
    assert_eq!(
        summer.parent_id.as_deref(),
        Some(gloves.category_id.as_str())
    );
    assert_eq!(
        (gloves.position, helmet.position, summer.position),
        (0, 1, 0)
    );
    assert!(helmet.out_of_ads && !gloves.out_of_ads);
    assert!(creation.is_pending(creation.requested_at));
    assert!(!creation.is_settled());

    // While the host is at it, nothing more is asked for.
    assert!(matches!(
        research
            .request_creation(
                ALICE,
                &id,
                CreateIn::Shop(creation.tenant_id.clone()),
                &plan(&["casque moto"])
            )
            .await,
        Err(ResearchError::CreationPending)
    ));

    // What came of each: created, refused, and the study remembers.
    let request = creation.request.as_str();
    assert!(
        research
            .link_shop(RESEARCH_ACTOR, &id, &creation.tenant_id)
            .await
            .unwrap()
    );
    assert!(
        research
            .record_category(RESEARCH_ACTOR, &id, request, "gants moto")
            .await
            .unwrap()
    );
    assert!(
        !research
            .record_category(RESEARCH_ACTOR, &id, request, "gants moto")
            .await
            .unwrap(),
        "said once"
    );
    assert!(
        research
            .record_category_refusal(
                RESEARCH_ACTOR,
                &id,
                request,
                "casque moto",
                "this address is taken"
            )
            .await
            .unwrap()
    );
    assert!(
        research
            .record_category(RESEARCH_ACTOR, &id, request, "gants moto été")
            .await
            .unwrap()
    );
    assert!(
        !research
            .record_category(RESEARCH_ACTOR, &id, "another", "casque moto")
            .await
            .unwrap()
    );
    let study = research.load_study(&id).await.unwrap().unwrap();
    assert_eq!(study.shop.as_deref(), Some(creation.tenant_id.as_str()));
    assert_eq!(
        study
            .created
            .iter()
            .map(|c| c.keyword.as_str())
            .collect::<Vec<_>>(),
        ["gants moto", "gants moto été"]
    );
    assert_eq!(
        study.created_category("gants moto").unwrap().category_id,
        gloves.category_id
    );
    let settled = study.creation.unwrap();
    assert!(settled.is_settled());
    assert_eq!(
        settled.refused,
        [("casque moto".to_owned(), "this address is taken".to_owned())]
    );
}

#[tokio::test]
async fn what_is_left_is_asked_for_again_in_the_same_shop() {
    let research = command().await;
    let id = planted(&research).await;
    research
        .request_creation(
            ALICE,
            &id,
            CreateIn::Shop("shop-1".into()),
            &plan(&["gants moto", "casque moto"]),
        )
        .await
        .unwrap();
    let first = research
        .load_study(&id)
        .await
        .unwrap()
        .unwrap()
        .creation
        .unwrap();
    research
        .link_shop(RESEARCH_ACTOR, &id, "shop-1")
        .await
        .unwrap();
    research
        .record_category(RESEARCH_ACTOR, &id, &first.request, "gants moto")
        .await
        .unwrap();
    research
        .record_category_refusal(RESEARCH_ACTOR, &id, &first.request, "casque moto", "taken")
        .await
        .unwrap();

    // One shop for a study.
    for target in [
        CreateIn::Shop("shop-2".into()),
        CreateIn::NewShop("Another".into()),
    ] {
        assert!(matches!(
            research
                .request_creation(ALICE, &id, target, &plan(&["casque moto"]))
                .await,
            Err(ResearchError::AnotherShop)
        ));
    }
    assert!(matches!(
        research
            .request_creation(
                ALICE,
                &id,
                CreateIn::NewShop(" ".into()),
                &plan(&["casque moto"])
            )
            .await,
        Err(ResearchError::InvalidShop)
    ));
    assert!(matches!(
        research
            .request_creation(
                ALICE,
                &id,
                CreateIn::Shop("shop-1".into()),
                &plan(&["bottes moto"])
            )
            .await,
        Err(ResearchError::NotInTree)
    ));
    assert!(matches!(
        research
            .request_creation(
                MALLORY,
                &id,
                CreateIn::Shop("shop-1".into()),
                &plan(&["casque moto"])
            )
            .await,
        Err(ResearchError::NotFound)
    ));

    // The created one is passed over; the refused one keeps the id it was
    // planned under, and the child goes under the category that exists.
    let asked = research
        .request_creation(
            ALICE,
            &id,
            CreateIn::Shop("shop-1".into()),
            &plan(&["gants moto", "gants moto été", "casque moto"]),
        )
        .await
        .unwrap();
    assert_eq!(asked, 2);
    let study = research.load_study(&id).await.unwrap().unwrap();
    let second = study.creation.clone().unwrap();
    assert_ne!(second.request, first.request);
    let helmet = |creation: &timada_research::Creation| {
        creation
            .planned
            .iter()
            .find(|p| p.keyword == "casque moto")
            .unwrap()
            .category_id
            .clone()
    };
    assert_eq!(helmet(&second), helmet(&first));
    let summer = second
        .planned
        .iter()
        .find(|p| p.keyword == "gants moto été")
        .unwrap();
    assert_eq!(
        summer.parent_id.as_deref(),
        Some(
            study
                .created_category("gants moto")
                .unwrap()
                .category_id
                .as_str()
        )
    );

    // Nothing could be created: said once, and the request is settled.
    assert!(
        research
            .fail_creation(RESEARCH_ACTOR, &id, &second.request, "no such shop")
            .await
            .unwrap()
    );
    assert!(
        !research
            .fail_creation(RESEARCH_ACTOR, &id, &second.request, "again")
            .await
            .unwrap()
    );
    let study = research.load_study(&id).await.unwrap().unwrap();
    assert_eq!(
        study.creation.unwrap().failed.as_deref(),
        Some("no such shop")
    );

    // A child whose parent is neither created nor asked for is passed over.
    research
        .place_category(ALICE, &id, "bottes moto", None)
        .await
        .unwrap();
    research
        .place_category(ALICE, &id, "bottes moto cuir", Some("bottes moto"))
        .await
        .unwrap();
    assert!(matches!(
        research
            .request_creation(
                ALICE,
                &id,
                CreateIn::Shop("shop-1".into()),
                &plan(&["bottes moto cuir"])
            )
            .await,
        Err(ResearchError::NothingToCreate)
    ));
}

#[tokio::test]
async fn a_shop_finds_the_study_it_started_from() {
    let (executor, db) = executor_with_pool().await;
    let research = timada_research::Command(executor.clone());
    let id = planted(&research).await;
    research
        .request_creation(
            ALICE,
            &id,
            CreateIn::Shop("shop-1".into()),
            &plan(&["gants moto"]),
        )
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert!(study_of_shop(&db, "shop-1").await.unwrap().is_none());
    assert!(study_of_shop(&db, "").await.unwrap().is_none());

    let request = research
        .load_study(&id)
        .await
        .unwrap()
        .unwrap()
        .creation
        .unwrap()
        .request;
    research
        .link_shop(RESEARCH_ACTOR, &id, "shop-1")
        .await
        .unwrap();
    research
        .record_category(RESEARCH_ACTOR, &id, &request, "gants moto")
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    let row = study_of_shop(&db, "shop-1").await.unwrap().unwrap();
    assert_eq!(row.id, id);
    assert_eq!((row.tenant_id.as_str(), row.created_count), ("shop-1", 1));
    assert_eq!(study_row(&db, &id).await.unwrap().unwrap().created_count, 1);
}
