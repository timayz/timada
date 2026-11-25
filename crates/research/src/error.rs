use evento::WriteError;

use crate::value_object::{
    MAX_CHECKED_KEYWORDS, MAX_HAND_SET, MAX_LISTED_WORDS, MAX_MARKS, MAX_NAME_LEN, MAX_NICHE_WORDS,
    MAX_PARKED, MAX_REFUSED, MAX_SEED_LEN, MAX_SEEDS, MAX_TREE_CATEGORIES, MAX_WORD_LEN,
};

#[derive(Debug, thiserror::Error)]
pub enum ResearchError {
    #[error("a study's name is 1 to {MAX_NAME_LEN} characters")]
    InvalidName,
    #[error("a seed keyword is 1 to {MAX_SEED_LEN} characters")]
    InvalidSeed,
    #[error("a study starts from one seed keyword at least")]
    NoSeed,
    #[error("a study holds at most {MAX_SEEDS} seed keywords")]
    TooManySeeds,
    #[error("give a two-letter country and a language code")]
    InvalidPlace,
    /// Unknown, deleted, or another user's: a study is nobody else's
    /// business, so the three read the same.
    #[error("study not found")]
    NotFound,
    #[error("the study has no such seed keyword")]
    UnknownSeed,
    #[error("these keywords are being fetched")]
    Fetching,
    #[error("every seed keyword has its keywords, or is being fetched")]
    NothingToFetch,
    #[error("the sale is in a currency the platform does not know")]
    InvalidCurrency,
    #[error("a rate is between 0 and 100 %")]
    InvalidRate,
    #[error("a brand or an excluded word is 1 to {MAX_WORD_LEN} characters")]
    InvalidWord,
    #[error(
        "a study lists at most {MAX_LISTED_WORDS} brands and {MAX_LISTED_WORDS} excluded words"
    )]
    TooManyWords,
    #[error("no such keyword")]
    InvalidKeyword,
    #[error(
        "a study holds at most {MAX_HAND_SET} keywords dropped by hand, and as many brought back"
    )]
    TooManyHandSet,
    #[error("the study was changed too many times: start another")]
    TooManyChanges,
    #[error("the study has no main keyword whose first results to look at")]
    NoMainKeyword,
    #[error("the first results of at most {MAX_CHECKED_KEYWORDS} keywords are looked at")]
    TooManyChecked,
    #[error("every main keyword has its first results, or they are being fetched")]
    NothingToCheck,
    #[error("a site is named by its domain, like example.com")]
    InvalidDomain,
    #[error("at most {MAX_MARKS} sites are marked: no more can be")]
    TooManyMarks,
    #[error("a category goes under one of the tree's parent categories")]
    UnknownParent,
    #[error("this category has children: move them or take them out first")]
    CategoryHasChildren,
    #[error("a tree holds at most {MAX_TREE_CATEGORIES} categories")]
    TooManyCategories,
    #[error("at most {MAX_PARKED} keywords are parked")]
    TooManyParked,
    #[error("at most {MAX_REFUSED} keywords are refused")]
    TooManyRefused,
    #[error("this keyword is not in the tree")]
    NotInTree,
    #[error("a niche is named by at most {MAX_NICHE_WORDS} words")]
    TooManyNicheWords,
    #[error("name the shop to create, or pick one of yours")]
    InvalidShop,
    #[error("this study's categories are created in the shop it is linked to")]
    AnotherShop,
    #[error("the categories are being created")]
    CreationPending,
    #[error("every category that can be created is")]
    NothingToCreate,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ResearchError {
    /// Whether the message can be shown to the person who made the request
    /// (as opposed to an infrastructure failure).
    pub fn is_user_facing(&self) -> bool {
        match self {
            // A write the store is not taking right now is told to the person.
            Self::Write(err) => timada_core::write_refusal(err).is_some(),
            Self::Other(_) => false,
            _ => true,
        }
    }

    /// Whether the store failed rather than the request: a saga fails its
    /// pass on it and tries again.
    pub fn is_store_fault(&self) -> bool {
        match self {
            Self::Write(err) => timada_core::is_store_write_fault(err),
            Self::Other(_) => true,
            _ => false,
        }
    }

    /// A stable code for a page to carry in a URL; `None` for an
    /// infrastructure failure.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::InvalidName => "invalid_study_name",
            Self::InvalidSeed => "invalid_seed",
            Self::NoSeed => "study_without_seed",
            Self::TooManySeeds => "too_many_seeds",
            Self::InvalidPlace => "invalid_study_place",
            Self::NotFound => "study_not_found",
            Self::UnknownSeed => "unknown_seed",
            Self::Fetching => "keywords_fetching",
            Self::NothingToFetch => "nothing_to_fetch",
            Self::InvalidCurrency => "invalid_study_currency",
            Self::InvalidRate => "invalid_study_rate",
            Self::InvalidWord => "invalid_study_word",
            Self::TooManyWords => "too_many_study_words",
            Self::InvalidKeyword => "invalid_keyword",
            Self::TooManyHandSet => "too_many_hand_set_keywords",
            Self::TooManyChanges => "study_changed_too_often",
            Self::NoMainKeyword => "no_main_keyword",
            Self::TooManyChecked => "too_many_checked_keywords",
            Self::NothingToCheck => "nothing_to_check",
            Self::InvalidDomain => "invalid_domain",
            Self::TooManyMarks => "too_many_marks",
            Self::UnknownParent => "unknown_parent_category",
            Self::CategoryHasChildren => "tree_category_has_children",
            Self::TooManyCategories => "too_many_tree_categories",
            Self::TooManyParked => "too_many_parked_keywords",
            Self::TooManyRefused => "too_many_refused_keywords",
            Self::NotInTree => "keyword_not_in_tree",
            Self::TooManyNicheWords => "too_many_niche_words",
            Self::InvalidShop => "invalid_study_shop",
            Self::AnotherShop => "study_has_another_shop",
            Self::CreationPending => "categories_being_created",
            Self::NothingToCreate => "nothing_to_create",
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}
