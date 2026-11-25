//! The category tree a study's owner builds by hand from its kept
//! keywords: a keyword becomes a parent category, a child of one, is
//! parked for later, or is refused so it is not offered again. Nothing
//! here fetches, and nothing is created in a
//! shop: the tree is the study's until its owner asks for it.
//!
//! Which keywords the score keeps is the read model's to say: the caller
//! names a keyword it read as kept, and [`review_tree`](crate::review_tree)
//! says so when one stops being.

use evento::Executor;

use crate::aggregator::{
    CategoryParked, CategoryPlaced, CategoryPriced, CategoryRefused, CategoryRemoved, NicheWordsSet,
};
use crate::error::ResearchError;
use crate::value_object::{
    MAX_NICHE_WORDS, MAX_PARKED, MAX_REFUSED, MAX_TREE_CATEGORIES, normalize_keyword,
    normalize_words,
};

use super::inputs::room_for_a_change;
use super::{Command, Step, StudyState};

/// Refuses to move a parent category that still has children.
fn without_children(study: &StudyState, keyword: &str) -> Result<(), ResearchError> {
    if study.tree.children_of(keyword).next().is_some() {
        return Err(ResearchError::CategoryHasChildren);
    }
    Ok(())
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Puts a keyword in the tree: as a parent category without `parent`,
    /// or under the parent category `parent` names by its keyword. It
    /// leaves where it stood, in the tree or parked, and keeps the price
    /// it was given. A parent category with children goes under no other.
    /// Where it already stands is a no-op.
    pub async fn place_category(
        &self,
        actor_user_id: &str,
        study_id: &str,
        keyword: &str,
        parent: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<(), ResearchError> {
        let keyword = normalize_keyword(keyword)?;
        let parent = parent
            .map(str::trim)
            .filter(|parent| !parent.is_empty())
            .map(normalize_keyword)
            .transpose()?;
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            let placed = study.tree.category(&keyword);
            if placed.is_some_and(|category| category.parent == parent) {
                return Ok(Step::Leave(()));
            }
            if let Some(parent) = &parent {
                let is_parent = study
                    .tree
                    .category(parent)
                    .is_some_and(|category| category.parent.is_none());
                if !is_parent || *parent == keyword {
                    return Err(ResearchError::UnknownParent);
                }
                without_children(study, &keyword)?;
            }
            if placed.is_none() && study.tree.categories.len() >= MAX_TREE_CATEGORIES {
                return Err(ResearchError::TooManyCategories);
            }
            room_for_a_change(study)?;
            write.event(&CategoryPlaced {
                keyword: keyword.clone(),
                parent: parent.clone(),
                placed_by: actor_user_id.to_owned(),
            });
            Ok(Step::Write(()))
        })
        .await
    }

    /// Parks a keyword for later, out of the tree; its own price goes with
    /// its place. One already parked is a no-op.
    pub async fn park_category(
        &self,
        actor_user_id: &str,
        study_id: &str,
        keyword: &str,
        routing_key: Option<String>,
    ) -> Result<(), ResearchError> {
        let keyword = normalize_keyword(keyword)?;
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            if study.tree.is_parked(&keyword) {
                return Ok(Step::Leave(()));
            }
            without_children(study, &keyword)?;
            if study.tree.parked.len() >= MAX_PARKED {
                return Err(ResearchError::TooManyParked);
            }
            room_for_a_change(study)?;
            write.event(&CategoryParked {
                keyword: keyword.clone(),
                parked_by: actor_user_id.to_owned(),
            });
            Ok(Step::Write(()))
        })
        .await
    }

    /// Refuses a keyword as a category: out of the tree and of the parked
    /// ones, and no longer among what is left to choose. The score keeps
    /// it as it did. One already refused is a no-op.
    pub async fn refuse_category(
        &self,
        actor_user_id: &str,
        study_id: &str,
        keyword: &str,
        routing_key: Option<String>,
    ) -> Result<(), ResearchError> {
        let keyword = normalize_keyword(keyword)?;
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            if study.tree.is_refused(&keyword) {
                return Ok(Step::Leave(()));
            }
            without_children(study, &keyword)?;
            if study.tree.refused.len() >= MAX_REFUSED {
                return Err(ResearchError::TooManyRefused);
            }
            room_for_a_change(study)?;
            write.event(&CategoryRefused {
                keyword: keyword.clone(),
                refused_by: actor_user_id.to_owned(),
            });
            Ok(Step::Write(()))
        })
        .await
    }

    /// Takes a keyword out of the tree, out of the parked ones or out of
    /// the refused ones; one that is in none is a no-op.
    pub async fn remove_category(
        &self,
        actor_user_id: &str,
        study_id: &str,
        keyword: &str,
        routing_key: Option<String>,
    ) -> Result<(), ResearchError> {
        let keyword = normalize_keyword(keyword)?;
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            if study.tree.category(&keyword).is_none()
                && !study.tree.is_parked(&keyword)
                && !study.tree.is_refused(&keyword)
            {
                return Ok(Step::Leave(()));
            }
            without_children(study, &keyword)?;
            room_for_a_change(study)?;
            write.event(&CategoryRemoved {
                keyword: keyword.clone(),
                removed_by: actor_user_id.to_owned(),
            });
            Ok(Step::Write(()))
        })
        .await
    }

    /// Gives a category of the tree an average price of its own, in the
    /// currency of the study's sale; `None`, or nothing, goes back to the
    /// study's. The same is a no-op.
    pub async fn price_category(
        &self,
        actor_user_id: &str,
        study_id: &str,
        keyword: &str,
        price_cents: Option<u64>,
        routing_key: Option<String>,
    ) -> Result<(), ResearchError> {
        let keyword = normalize_keyword(keyword)?;
        let price_cents = price_cents.filter(|cents| *cents > 0);
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            let Some(category) = study.tree.category(&keyword) else {
                return Err(ResearchError::NotInTree);
            };
            if category.price_cents == price_cents {
                return Ok(Step::Leave(()));
            }
            room_for_a_change(study)?;
            write.event(&CategoryPriced {
                keyword: keyword.clone(),
                price_cents,
                priced_by: actor_user_id.to_owned(),
            });
            Ok(Step::Write(()))
        })
        .await
    }

    /// Says the niche's own words, one of which every category's name is
    /// to hold; none checks no name. The same list is a no-op.
    pub async fn set_niche_words(
        &self,
        actor_user_id: &str,
        study_id: &str,
        words: &[String],
        routing_key: Option<String>,
    ) -> Result<(), ResearchError> {
        let words = normalize_words(words)?;
        if words.len() > MAX_NICHE_WORDS {
            return Err(ResearchError::TooManyNicheWords);
        }
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            if study.tree.niche_words.as_ref() == Some(&words) {
                return Ok(Step::Leave(()));
            }
            room_for_a_change(study)?;
            write.event(&NicheWordsSet {
                words: words.clone(),
                set_by: actor_user_id.to_owned(),
            });
            Ok(Step::Write(()))
        })
        .await
    }
}
