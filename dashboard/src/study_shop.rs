//! Creates what a study's owner asked for from its category tree: the shop
//! first, when a new one was asked for, then each category as a draft in
//! it, parents before their children. What came of each is recorded on the
//! study, which is where its owner reads it. Every node of the work group
//! runs it.
//!
//! Each step is made once whoever makes it and however often: the shop and
//! each category go by an id the request chose, and creating under an id
//! that exists does nothing. So a request delivered twice, to two nodes or
//! after one stopped half-way, ends on the same shop and the same
//! categories, and a category created by a node that stopped before saying
//! so is said by the next pass.
//!
//! A refusal is the category's outcome, not the pass's: a slug taken
//! meanwhile, a parent that was refused. It is recorded with its reason,
//! and the owner asks again for what is left. If the categories fail after
//! the shop exists, the shop stays. Only the store failing fails the pass.

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_catalog::{CatalogError, DraftCategory};
use timada_research::aggregator::CreationRequested;
use timada_research::{Command, RESEARCH_ACTOR};
use timada_tenancy::{CreateTenant, PERMISSION_PRODUCTS_MANAGE, TenancyError, TenantState};

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/study-categories";

/// What the owner reads when the shop is gone or no longer theirs to fill.
const NOT_THEIRS: &str = "You may not manage this shop's catalog, or it is suspended.";

/// What a child reads when the category above it was not created.
const NO_PARENT: &str = "The category above it was not created.";

/// The store, to write with.
#[derive(Clone)]
struct Store(Evento);

pub fn subscription<E: Executor>(store: Evento) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Store(store))
        .handler(on_creation_requested())
        // A saga: it follows the node's own work group. The studies live in
        // the platform's tenant.
        .tenant("")
        .retry(5)
}

/// The shop a request names: the one to create, made once under the id the
/// request chose, or the one that exists. `Err` with what the owner reads
/// when there is none they may fill.
async fn shop_of(
    store: &Evento,
    request: &CreationRequested,
) -> anyhow::Result<Result<TenantState, String>> {
    let tenancy = timada_tenancy::Command(store.clone());
    let tenant = match &request.shop_name {
        Some(name) => {
            let made = tenancy
                .create_tenant_under(
                    &request.tenant_id,
                    CreateTenant {
                        name: name.clone(),
                        owner_user_id: request.requested_by.clone(),
                    },
                )
                .await;
            match made {
                Ok(tenant) => tenant,
                Err(TenancyError::Write(err)) if timada_core::is_store_write_fault(&err) => {
                    return Err(err.into());
                }
                Err(TenancyError::Other(err)) => return Err(err),
                Err(err) => return Ok(Err(format!("The shop was not created: {err}."))),
            }
        }
        None => match tenancy.load_tenant(&request.tenant_id).await? {
            Some(tenant) => tenant,
            None => return Ok(Err(NOT_THEIRS.to_owned())),
        },
    };
    if tenant.is_suspended() || !tenant.can(&request.requested_by, PERMISSION_PRODUCTS_MANAGE) {
        return Ok(Err(NOT_THEIRS.to_owned()));
    }
    Ok(Ok(tenant))
}

#[evento::subscription]
async fn on_creation_requested<E: Executor>(
    context: &Context<'_, E>,
    event: Event<CreationRequested>,
) -> anyhow::Result<()> {
    let store: Store = context.extract();
    let studies = Command(store.0.clone());
    let study_id = event.aggregate_id.as_str();
    let request = event.data.request.as_str();
    let Some(study) = studies.load_study(study_id).await? else {
        return Ok(());
    };
    // A later request took its place, or another node settled it.
    let Some(creation) = study
        .creation
        .as_ref()
        .filter(|creation| creation.request == request && !creation.is_settled())
    else {
        return Ok(());
    };
    let tenant = match shop_of(&store.0, &event.data).await? {
        Ok(tenant) => tenant,
        Err(reason) => {
            tracing::info!(study = %study_id, %request, %reason, "categories not created");
            studies
                .fail_creation(RESEARCH_ACTOR, study_id, request, &reason)
                .await?;
            return Ok(());
        }
    };
    studies
        .link_shop(RESEARCH_ACTOR, study_id, &tenant.id)
        .await?;
    let catalog = timada_catalog::Command::new(store.0.clone(), &tenant.id);
    let (mut created, mut refused) = (0, 0);
    for planned in creation.left() {
        let made = catalog
            .create_draft_category(
                &event.data.requested_by,
                DraftCategory {
                    id: planned.category_id.clone(),
                    name: planned.name.clone(),
                    slug: planned.slug.clone(),
                    parent_id: planned.parent_id.clone(),
                    meta_title: planned.meta_title.clone(),
                    out_of_ads: planned.out_of_ads,
                    position: planned.position,
                },
            )
            .await;
        match made {
            // Made now, or by a pass that stopped before saying so.
            Ok(_) => {
                studies
                    .record_category(RESEARCH_ACTOR, study_id, request, &planned.keyword)
                    .await?;
                created += 1;
            }
            Err(err) if err.is_store_fault() => return Err(err.into()),
            Err(err) => {
                let reason = match (&err, &planned.parent_id) {
                    (CatalogError::CategoryNotFound, Some(_)) => NO_PARENT.to_owned(),
                    _ => format!("The shop refused it: {err}."),
                };
                studies
                    .record_category_refusal(
                        RESEARCH_ACTOR,
                        study_id,
                        request,
                        &planned.keyword,
                        &reason,
                    )
                    .await?;
                refused += 1;
            }
        }
    }
    tracing::info!(study = %study_id, tenant = %tenant.id, created, refused, "a study's categories created as drafts");
    Ok(())
}

#[cfg(test)]
mod tests {
    use evento::Turso;
    use timada_research::{CreateIn, CreateStudy, PlanCategory, StudyState};

    use super::*;

    const ALICE: &str = "user-alice";
    const MALLORY: &str = "user-mallory";

    async fn store() -> Evento {
        Evento::new(Turso::in_memory_concurrent().await.unwrap())
    }

    /// A study of Alice's with two parents and a child under the first.
    async fn planted(store: &Evento) -> StudyState {
        let research = Command(store.clone());
        let study = research
            .create_study(
                ALICE,
                CreateStudy {
                    name: "Gants moto".to_owned(),
                    seeds: vec!["gants moto".to_owned()],
                    country: "FR".to_owned(),
                    language: "fr".to_owned(),
                },
            )
            .await
            .unwrap();
        for (keyword, parent) in [
            ("gants moto", None),
            ("casque moto", None),
            ("gants moto été", Some("gants moto")),
        ] {
            research
                .place_category(ALICE, &study.id, keyword, parent)
                .await
                .unwrap();
        }
        study
    }

    fn plan(keywords: &[&str]) -> Vec<PlanCategory> {
        keywords
            .iter()
            .map(|keyword| PlanCategory {
                keyword: keyword.to_string(),
                name: timada_research::category_name(keyword),
                slug: timada_research::category_slug(keyword),
                meta_title: timada_research::category_name(keyword),
                out_of_ads: *keyword == "casque moto",
            })
            .collect()
    }

    const ALL: [&str; 3] = ["gants moto", "gants moto été", "casque moto"];

    async fn pass(store: &Evento) {
        subscription::<Evento>(store.clone())
            .retry(0)
            .run_once(store)
            .await
            .unwrap();
    }

    async fn reload(store: &Evento, study: &StudyState) -> StudyState {
        Command(store.clone())
            .load_study(&study.id)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn a_shop_is_created_from_a_study_with_its_tree_as_drafts() {
        let store = store().await;
        let study = planted(&store).await;
        Command(store.clone())
            .request_creation(
                ALICE,
                &study.id,
                CreateIn::NewShop("Gants de motard".into()),
                &plan(&ALL),
            )
            .await
            .unwrap();
        pass(&store).await;

        let study = reload(&store, &study).await;
        let creation = study.creation.clone().unwrap();
        assert!(creation.is_settled());
        assert_eq!(creation.failed, None);
        assert!(creation.refused.is_empty(), "{:?}", creation.refused);
        let tenant_id = study.shop.clone().expect("linked to its shop");
        assert_eq!(tenant_id, creation.tenant_id);
        let tenant = timada_tenancy::Command(store.clone())
            .load_tenant(&tenant_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tenant.name, "Gants de motard");
        assert!(tenant.is_owner(ALICE));

        let catalog = timada_catalog::Command::new(store.clone(), &tenant_id);
        let gloves = catalog
            .load_category_by_slug("gants-moto")
            .await
            .unwrap()
            .unwrap();
        assert!(gloves.draft, "nothing is published");
        assert_eq!(gloves.name, "Gants moto");
        assert_eq!(gloves.meta_title, "Gants moto");
        assert_eq!(gloves.created_by, ALICE);
        assert_eq!(
            gloves.id,
            study.created_category("gants moto").unwrap().category_id
        );
        let summer = catalog
            .load_category_by_slug("gants-moto-ete")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summer.parent_id.as_deref(), Some(gloves.id.as_str()));
        assert!(summer.draft && !summer.out_of_ads);
        let helmet = catalog
            .load_category_by_slug("casque-moto")
            .await
            .unwrap()
            .unwrap();
        assert!(helmet.out_of_ads, "the mark reaches the shop's category");
        assert_eq!(helmet.position, 1);
        assert_eq!(study.created.len(), 3);

        // Delivered again, nothing more is made.
        let version = study.aggregate_version;
        pass(&store).await;
        assert_eq!(reload(&store, &study).await.aggregate_version, version);
    }

    #[tokio::test]
    async fn a_taken_slug_leaves_its_category_and_its_children_to_create() {
        let store = store().await;
        let study = planted(&store).await;
        let tenant = timada_tenancy::Command(store.clone())
            .create_tenant(CreateTenant {
                name: "Acme".into(),
                owner_user_id: ALICE.into(),
            })
            .await
            .unwrap();
        let catalog = timada_catalog::Command::new(store.clone(), &tenant.id);
        let held = catalog
            .create_category(
                ALICE,
                timada_catalog::CreateCategory {
                    name: "Gants moto".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let research = Command(store.clone());
        research
            .request_creation(
                ALICE,
                &study.id,
                CreateIn::Shop(tenant.id.clone()),
                &plan(&ALL),
            )
            .await
            .unwrap();
        pass(&store).await;

        let after = reload(&store, &study).await;
        assert_eq!(after.shop.as_deref(), Some(tenant.id.as_str()));
        let creation = after.creation.clone().unwrap();
        assert!(creation.is_settled());
        assert_eq!(creation.created, ["casque moto"]);
        let refused: Vec<&str> = creation.refused.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(refused, ["gants moto", "gants moto été"]);
        assert!(
            creation.refused[0].1.contains("refused"),
            "{:?}",
            creation.refused
        );
        assert_eq!(creation.refused[1].1, NO_PARENT);

        // The slug freed, what is left is created under the ids it had.
        catalog.archive_category(ALICE, &held.id).await.unwrap();
        research
            .request_creation(
                ALICE,
                &study.id,
                CreateIn::Shop(tenant.id.clone()),
                &plan(&ALL),
            )
            .await
            .unwrap();
        pass(&store).await;
        let done = reload(&store, &study).await;
        assert_eq!(done.created.len(), 3);
        assert!(done.creation.unwrap().refused.is_empty());
    }

    #[tokio::test]
    async fn a_shop_its_asker_may_not_fill_gets_nothing() {
        let store = store().await;
        let theirs = timada_tenancy::Command(store.clone())
            .create_tenant(CreateTenant {
                name: "Mallory's".into(),
                owner_user_id: MALLORY.into(),
            })
            .await
            .unwrap();
        let research = Command(store.clone());
        for tenant_id in [theirs.id.clone(), "no-such-shop".to_owned()] {
            let study = planted(&store).await;
            research
                .request_creation(ALICE, &study.id, CreateIn::Shop(tenant_id), &plan(&ALL))
                .await
                .unwrap();
            pass(&store).await;
            let after = reload(&store, &study).await;
            assert_eq!(after.creation.unwrap().failed.as_deref(), Some(NOT_THEIRS));
            assert!(after.shop.is_none());
            assert!(after.created.is_empty());
        }
        let catalog = timada_catalog::Command::new(store.clone(), &theirs.id);
        assert!(
            catalog
                .load_category_by_slug("gants-moto")
                .await
                .unwrap()
                .is_none()
        );
    }
}
