//! From a study's tree to draft categories in a shop: the owner asks, the
//! host creates the shop when one is asked for, then each category, and
//! what came of each is recorded on the study, which remembers its shop
//! and the categories it created there.
//!
//! The request carries the whole plan: each category's id in the shop is
//! chosen here, so the host that is delivered a request twice, or that
//! stopped half-way, creates each category once. What a category is
//! created as (its name, its slug, whether campaigns leave it out) is the
//! caller's to work out from the tree's review, which reads the read model.

use evento::Executor;
use timada_core::unix_now;
use ulid::Ulid;

use crate::aggregator::{
    CategoryCreated, CategoryNotCreated, CreationFailed, CreationRequested, PlannedCategory,
    ShopLinked,
};
use crate::error::ResearchError;
use crate::value_object::{MAX_NAME_LEN, MAX_REASON_LEN, MAX_STUDY_EVENTS};

use super::{Command, Step};

/// One category of the tree as the caller wants it created.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanCategory {
    pub keyword: String,
    pub name: String,
    pub slug: String,
    pub meta_title: String,
    pub out_of_ads: bool,
}

/// Where a study's tree is to be created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateIn {
    /// A shop the owner may manage the catalog of, by its id: the caller
    /// checked that they may.
    Shop(String),
    /// A shop to create first, theirs, under this name.
    NewShop(String),
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Asks for `categories`, of the study's tree, to be created as draft
    /// categories, parents before their children. One already created is
    /// passed over, as is a child whose parent is neither created nor
    /// asked for. A study has one shop: once it has it, only there. How
    /// many categories were asked for.
    pub async fn request_creation(
        &self,
        actor_user_id: &str,
        study_id: &str,
        target: CreateIn,
        categories: &[PlanCategory],
        routing_key: Option<String>,
    ) -> Result<usize, ResearchError> {
        let target = match target {
            CreateIn::Shop(tenant_id) if !tenant_id.trim().is_empty() => {
                CreateIn::Shop(tenant_id.trim().to_owned())
            }
            CreateIn::NewShop(name)
                if !name.trim().is_empty() && name.trim().chars().count() <= MAX_NAME_LEN =>
            {
                CreateIn::NewShop(name.trim().to_owned())
            }
            _ => return Err(ResearchError::InvalidShop),
        };
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            let (tenant_id, shop_name) = match (&target, &study.shop) {
                (CreateIn::Shop(tenant_id), Some(shop)) if tenant_id != shop => {
                    return Err(ResearchError::AnotherShop);
                }
                (CreateIn::NewShop(_), Some(_)) => return Err(ResearchError::AnotherShop),
                (CreateIn::Shop(tenant_id), _) => (tenant_id.clone(), None),
                (CreateIn::NewShop(name), None) => {
                    (Ulid::generate().to_string(), Some(name.clone()))
                }
            };
            if study
                .creation
                .as_ref()
                .is_some_and(|creation| creation.is_pending(unix_now()))
            {
                return Err(ResearchError::CreationPending);
            }
            // A category asked for before in this shop keeps the id it was
            // given: created then and never recorded, it is found again.
            let earlier = study
                .creation
                .as_ref()
                .filter(|creation| creation.tenant_id == tenant_id);
            let mut planned: Vec<PlannedCategory> = Vec::new();
            for category in categories {
                let Some(placed) = study.tree.category(&category.keyword) else {
                    return Err(ResearchError::NotInTree);
                };
                if study.is_created(&placed.keyword)
                    || planned.iter().any(|p| p.keyword == placed.keyword)
                {
                    continue;
                }
                let parent_id = match &placed.parent {
                    None => None,
                    Some(parent) => {
                        let id = study
                            .created_category(parent)
                            .map(|created| created.category_id.clone())
                            .or_else(|| {
                                planned
                                    .iter()
                                    .find(|p| p.keyword == *parent)
                                    .map(|p| p.category_id.clone())
                            });
                        match id {
                            Some(id) => Some(id),
                            None => continue,
                        }
                    }
                };
                let siblings = study
                    .tree
                    .categories
                    .iter()
                    .filter(|other| other.parent == placed.parent);
                let position = siblings
                    .take_while(|other| other.keyword != placed.keyword)
                    .count() as u32;
                let category_id = earlier
                    .and_then(|creation| {
                        creation
                            .planned
                            .iter()
                            .find(|p| p.keyword == placed.keyword)
                    })
                    .map_or_else(|| Ulid::generate().to_string(), |p| p.category_id.clone());
                planned.push(PlannedCategory {
                    keyword: placed.keyword.clone(),
                    category_id,
                    parent_id,
                    name: category.name.clone(),
                    slug: category.slug.clone(),
                    meta_title: category.meta_title.clone(),
                    out_of_ads: category.out_of_ads,
                    position,
                });
            }
            if planned.is_empty() {
                return Err(ResearchError::NothingToCreate);
            }
            // The request, the shop, then what came of each category.
            let room = usize::from(MAX_STUDY_EVENTS.saturating_sub(study.aggregate_version));
            if planned.len() + 2 > room {
                return Err(ResearchError::TooManyChanges);
            }
            let asked = planned.len();
            write.event(&CreationRequested {
                request: Ulid::generate().to_string(),
                tenant_id,
                shop_name,
                categories: planned,
                requested_by: actor_user_id.to_owned(),
            });
            Ok(Step::Write(asked))
        })
        .await
    }

    /// Links the study to the shop its categories are created in; `false`
    /// when the study is gone. Already linked to it is a no-op.
    pub async fn link_shop(
        &self,
        actor: &str,
        study_id: &str,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<bool, ResearchError> {
        let linked = self
            .settle(study_id, routing_key, |study, write| {
                if study.shop.as_deref() == Some(tenant_id) {
                    return Ok(Step::Leave(true));
                }
                write.event(&ShopLinked {
                    tenant_id: tenant_id.to_owned(),
                    linked_by: actor.to_owned(),
                });
                Ok(Step::Write(true))
            })
            .await?;
        Ok(linked.unwrap_or(false))
    }

    /// Records that the category `request` planned for `keyword` exists in
    /// the shop; `false` when the request is no longer the study's last,
    /// or said so already.
    pub async fn record_category(
        &self,
        actor: &str,
        study_id: &str,
        request: &str,
        keyword: &str,
        routing_key: Option<String>,
    ) -> Result<bool, ResearchError> {
        let recorded = self
            .settle(study_id, routing_key, |study, write| {
                let planned = study
                    .creation
                    .as_ref()
                    .filter(|creation| creation.request == request)
                    .and_then(|creation| creation.left().find(|p| p.keyword == keyword));
                let Some(planned) = planned else {
                    return Ok(Step::Leave(false));
                };
                write.event(&CategoryCreated {
                    request: request.to_owned(),
                    keyword: planned.keyword.clone(),
                    category_id: planned.category_id.clone(),
                    slug: planned.slug.clone(),
                    created_by: actor.to_owned(),
                });
                Ok(Step::Write(true))
            })
            .await?;
        Ok(recorded.unwrap_or(false))
    }

    /// Records why the shop refused the category `request` planned for
    /// `keyword`; `false` when the request is no longer the study's last,
    /// or said what came of the keyword already.
    pub async fn record_category_refusal(
        &self,
        actor: &str,
        study_id: &str,
        request: &str,
        keyword: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<bool, ResearchError> {
        let reason: String = reason.chars().take(MAX_REASON_LEN).collect();
        let recorded = self
            .settle(study_id, routing_key, |study, write| {
                let left = study
                    .creation
                    .as_ref()
                    .filter(|creation| creation.request == request)
                    .is_some_and(|creation| creation.left().any(|p| p.keyword == keyword));
                if !left {
                    return Ok(Step::Leave(false));
                }
                write.event(&CategoryNotCreated {
                    request: request.to_owned(),
                    keyword: keyword.to_owned(),
                    reason: reason.clone(),
                    failed_by: actor.to_owned(),
                });
                Ok(Step::Write(true))
            })
            .await?;
        Ok(recorded.unwrap_or(false))
    }

    /// Records why nothing of `request` could be created; `false` when the
    /// request is no longer the study's last, or is settled.
    pub async fn fail_creation(
        &self,
        actor: &str,
        study_id: &str,
        request: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<bool, ResearchError> {
        let reason: String = reason.chars().take(MAX_REASON_LEN).collect();
        let recorded = self
            .settle(study_id, routing_key, |study, write| {
                let open = study
                    .creation
                    .as_ref()
                    .is_some_and(|creation| creation.request == request && !creation.is_settled());
                if !open {
                    return Ok(Step::Leave(false));
                }
                write.event(&CreationFailed {
                    request: request.to_owned(),
                    reason: reason.clone(),
                    failed_by: actor.to_owned(),
                });
                Ok(Step::Write(true))
            })
            .await?;
        Ok(recorded.unwrap_or(false))
    }
}
