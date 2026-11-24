//! A study's category tree: the kept keywords its owner turned into the
//! categories of a shop to come, and what is checked of them before any is
//! created.
//!
//! A category is a keyword: its name and its slug are read off it, so no
//! keyword is used twice and no category is without one. The tree has two
//! levels, parent categories and their children. Nothing here reads a
//! store: [`review_tree`] is given the figures of the keywords, which are
//! the read model's.

use std::collections::HashMap;

use timada_catalog::{MAX_CATEGORY_NAME_LEN, is_reserved_slug, normalize_slug, slugify_or};

use crate::score::{Channel, StudyInputs, Thresholds, ratio, words};
use crate::value_object::USUAL_PARENTS;

/// One category of a study's tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeCategory {
    /// The keyword it is made of, as its row spells it.
    pub keyword: String,
    /// The keyword of the parent category it is under; `None` for a parent
    /// category.
    pub parent: Option<String>,
    /// Its own average price, when it is not the study's.
    pub price_cents: Option<u64>,
}

/// A study's category tree as its owner built it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    /// In the order they were placed.
    pub categories: Vec<TreeCategory>,
    /// The keywords parked for later, in the order they were.
    pub parked: Vec<String>,
    /// The niche's own words as the owner said them; `None` until they
    /// did, when the words every seed shares stand in.
    pub niche_words: Option<Vec<String>>,
}

impl Tree {
    pub fn category(&self, keyword: &str) -> Option<&TreeCategory> {
        self.categories
            .iter()
            .find(|category| category.keyword == keyword)
    }

    pub fn is_parked(&self, keyword: &str) -> bool {
        self.parked.iter().any(|parked| parked == keyword)
    }

    /// The parent categories, in the order they were placed.
    pub fn parents(&self) -> impl Iterator<Item = &TreeCategory> {
        self.categories
            .iter()
            .filter(|category| category.parent.is_none())
    }

    /// The children of the parent category of `keyword`, in the order they
    /// were placed.
    pub fn children_of<'a>(&'a self, keyword: &'a str) -> impl Iterator<Item = &'a TreeCategory> {
        self.categories
            .iter()
            .filter(move |category| category.parent.as_deref() == Some(keyword))
    }

    /// Every category as the tree reads: each parent, then its children.
    pub fn ordered(&self) -> Vec<&TreeCategory> {
        let mut ordered = Vec::with_capacity(self.categories.len());
        for parent in self.parents() {
            ordered.push(parent);
            ordered.extend(self.children_of(&parent.keyword));
        }
        ordered
    }

    /// Takes `keyword` out of the tree and out of the parked ones.
    pub(crate) fn take_out(&mut self, keyword: &str) {
        self.categories
            .retain(|category| category.keyword != keyword);
        self.parked.retain(|parked| parked != keyword);
    }
}

/// The figures of one keyword, as the read model holds them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeywordFigures {
    /// Searches a month.
    pub volume: Option<u64>,
    /// In US cents.
    pub cpc_usd_cents: Option<u64>,
    /// 0 (easy) to 100.
    pub difficulty: Option<u8>,
    /// Whether the score keeps it: a main keyword or a long-tail one.
    pub kept: bool,
}

/// What stands between a category of the tree and its creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeProblem {
    /// The study no longer keeps its keyword: dropped, filtered out, or
    /// gone with a new fetch.
    NotKept,
    /// Its name is longer than a category's may be.
    NameTooLong,
    /// Its keyword gives no slug a category can go by: nothing a slug is
    /// made of, or a name the shop keeps for its own pages.
    NoSlug,
    /// Another category of the tree, named by its keyword, has the same
    /// slug.
    SameSlug(String),
    /// Its name holds none of the niche's own words.
    NoNicheWord,
}

impl TreeProblem {
    /// A stable key, for a page to carry.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotKept => "not_kept",
            Self::NameTooLong => "name_too_long",
            Self::NoSlug => "no_slug",
            Self::SameSlug(_) => "same_slug",
            Self::NoNicheWord => "no_niche_word",
        }
    }
}

/// One category of the tree with what creating it would give, and what
/// stands in the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedCategory {
    pub keyword: String,
    pub parent: Option<String>,
    /// The keyword with a capital first letter.
    pub name: String,
    pub slug: String,
    /// `None` when the read model holds no row of the keyword.
    pub figures: Option<KeywordFigures>,
    /// Its own average price, when the owner gave it one.
    pub own_price_cents: Option<u64>,
    /// The price its clicks are read against: its own, else the study's.
    pub price_cents: Option<u64>,
    /// Its cost per click in thousandths of a cent of the sale's currency
    /// (of a US cent while no rate is known).
    pub cpc_millis: Option<u64>,
    /// That price over its cost per click, in hundredths.
    pub price_per_click: Option<u64>,
    /// Whether an ad on it can pay; `None` while the sale or its cost per
    /// click is not known.
    pub channel: Option<Channel>,
    pub problems: Vec<TreeProblem>,
}

impl ReviewedCategory {
    /// Whether campaigns are to leave it out: its price over its cost per
    /// click is below what the sale needs.
    pub fn keep_out_of_ads(&self) -> bool {
        self.channel == Some(Channel::SeoOnly)
    }
}

/// A study's tree, checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeReview {
    /// Each parent, then its children.
    pub categories: Vec<ReviewedCategory>,
    pub parents: usize,
    /// The words a name is checked against: the owner's, else the ones
    /// every seed shares. Empty when no name is checked.
    pub niche_words: Vec<String>,
}

impl TreeReview {
    /// How many categories have something in the way.
    pub fn held_back(&self) -> usize {
        self.categories
            .iter()
            .filter(|category| !category.problems.is_empty())
            .count()
    }

    /// Whether the tree can be created as it stands: it has a category,
    /// and nothing is in the way of any.
    pub fn is_ready(&self) -> bool {
        !self.categories.is_empty() && self.held_back() == 0
    }

    /// Whether it has more parent categories than a tree usually does.
    pub fn has_many_parents(&self) -> bool {
        self.parents > USUAL_PARENTS
    }
}

/// The name of the category a keyword makes: the keyword, its first letter
/// a capital.
pub fn category_name(keyword: &str) -> String {
    let mut letters = keyword.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

/// The slug of the category a keyword makes; empty when the keyword holds
/// nothing a slug is made of.
pub fn category_slug(keyword: &str) -> String {
    slugify_or(keyword, "")
}

/// A folded word without the `s` or the `x` of a plural, so `motos` and
/// `moto` are one. A word of three letters or fewer keeps its end.
fn singular(word: &str) -> &str {
    if word.chars().count() > 3 {
        word.strip_suffix(['s', 'x']).unwrap_or(word)
    } else {
        word
    }
}

/// The words of `text`, folded and singular.
fn stems(text: &str) -> Vec<String> {
    words(text)
        .iter()
        .map(|word| singular(word).to_owned())
        .collect()
}

/// The words every seed of a study shares, of three letters or more, as
/// the first seed spells them: what names the niche until its owner says.
/// `gants moto` and `casque moto` share `moto`.
pub fn shared_words<S: AsRef<str>>(seeds: &[S]) -> Vec<String> {
    let Some((first, others)) = seeds.split_first() else {
        return Vec::new();
    };
    let others: Vec<Vec<String>> = others.iter().map(|seed| stems(seed.as_ref())).collect();
    let mut shared: Vec<String> = Vec::new();
    for word in words(first.as_ref()) {
        let stem = singular(&word).to_owned();
        if word.chars().count() >= 3
            && others.iter().all(|other| other.contains(&stem))
            && !shared.iter().any(|kept| singular(kept) == stem)
        {
            shared.push(word);
        }
    }
    shared
}

/// Whether `name` holds one of the niche's words, whole, whatever its case,
/// its accents and its number. A niche named by several words in a row
/// (`arbre à chat`) is held when they are, in a row.
fn holds_niche_word(name: &str, niche_words: &[String]) -> bool {
    let name = stems(name);
    niche_words.iter().any(|word| {
        let word = stems(word);
        !word.is_empty() && name.windows(word.len()).any(|window| window == word)
    })
}

/// Checks a study's tree against the figures of its keywords: what each
/// category would be created as, whether campaigns are to leave it out,
/// and what stands in the way. `figures` holds a keyword's row when the
/// read model has one.
pub fn review_tree(
    tree: &Tree,
    seeds: &[String],
    inputs: &StudyInputs,
    thresholds: &Thresholds,
    figures: &HashMap<String, KeywordFigures>,
) -> TreeReview {
    let niche_words = match &tree.niche_words {
        Some(words) => words.clone(),
        None => shared_words(seeds),
    };
    let sale = inputs.sale_score(thresholds);
    let ordered = tree.ordered();
    let slugs: Vec<String> = ordered
        .iter()
        .map(|category| category_slug(&category.keyword))
        .collect();
    let categories = ordered
        .iter()
        .zip(&slugs)
        .map(|(category, slug)| {
            let name = category_name(&category.keyword);
            let row = figures.get(&category.keyword).copied();
            let price_cents = category.price_cents.or(inputs.sale.price_cents);
            let cpc_millis = row
                .and_then(|row| row.cpc_usd_cents)
                .map(|cents| inputs.cpc_millis(cents));
            let price_per_click = price_cents
                .zip(cpc_millis)
                .and_then(|(price, cpc)| ratio(price, cpc));
            // What is needed is the study's sale's to say; the category's
            // own price only changes what stands against it.
            let channel = sale.and_then(|sale| {
                let Some(needed) = sale.needed_price_per_click else {
                    return Some(Channel::SeoOnly);
                };
                cpc_millis.map(|_| {
                    if price_per_click.is_none_or(|ratio| ratio >= needed) {
                        Channel::Ads
                    } else {
                        Channel::SeoOnly
                    }
                })
            });
            let mut problems = Vec::new();
            if !row.is_some_and(|row| row.kept) {
                problems.push(TreeProblem::NotKept);
            }
            if name.chars().count() > MAX_CATEGORY_NAME_LEN {
                problems.push(TreeProblem::NameTooLong);
            }
            if normalize_slug(slug).is_err() || is_reserved_slug(slug) {
                problems.push(TreeProblem::NoSlug);
            } else if let Some(other) = ordered
                .iter()
                .zip(&slugs)
                .find(|(other, other_slug)| {
                    other.keyword != category.keyword && *other_slug == slug
                })
                .map(|(other, _)| other.keyword.clone())
            {
                problems.push(TreeProblem::SameSlug(other));
            }
            if !niche_words.is_empty() && !holds_niche_word(&name, &niche_words) {
                problems.push(TreeProblem::NoNicheWord);
            }
            ReviewedCategory {
                keyword: category.keyword.clone(),
                parent: category.parent.clone(),
                name,
                slug: slug.clone(),
                figures: row,
                own_price_cents: category.price_cents,
                price_cents,
                cpc_millis,
                price_per_click,
                channel,
                problems,
            }
        })
        .collect();
    TreeReview {
        categories,
        parents: tree.parents().count(),
        niche_words,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::Sale;

    fn tree(categories: &[(&str, Option<&str>)]) -> Tree {
        Tree {
            categories: categories
                .iter()
                .map(|(keyword, parent)| TreeCategory {
                    keyword: keyword.to_string(),
                    parent: parent.map(str::to_owned),
                    price_cents: None,
                })
                .collect(),
            ..Tree::default()
        }
    }

    fn kept(cpc_usd_cents: Option<u64>) -> KeywordFigures {
        KeywordFigures {
            volume: Some(1_200),
            cpc_usd_cents,
            difficulty: Some(12),
            kept: true,
        }
    }

    fn all_kept(tree: &Tree) -> HashMap<String, KeywordFigures> {
        tree.categories
            .iter()
            .map(|category| (category.keyword.clone(), kept(Some(50))))
            .collect()
    }

    /// A sale in US dollars at 100.00 that leaves 40.00: an ad pays from a
    /// price over a click's cost of (2.5 + 0.4) / 1.5 % = 193.34.
    fn inputs() -> StudyInputs {
        StudyInputs {
            sale: Sale {
                currency: "USD".to_owned(),
                price_cents: Some(10_000),
                cost_cents: Some(6_000),
                ..Sale::default()
            },
            ..StudyInputs::default()
        }
    }

    fn problems(review: &TreeReview, keyword: &str) -> Vec<TreeProblem> {
        review
            .categories
            .iter()
            .find(|category| category.keyword == keyword)
            .unwrap()
            .problems
            .clone()
    }

    #[test]
    fn a_category_is_named_and_addressed_by_its_keyword() {
        assert_eq!(category_name("gants moto été"), "Gants moto été");
        assert_eq!(category_name("écharpe"), "Écharpe");
        assert_eq!(category_slug("gants moto été"), "gants-moto-ete");
        assert_eq!(category_slug("手袋"), category_slug("手袋"));
        assert_eq!(category_slug("!!"), "");
    }

    #[test]
    fn the_words_every_seed_shares_name_the_niche() {
        assert_eq!(
            shared_words(&["gants moto", "casque motos", "blouson de moto"]),
            ["moto"]
        );
        assert_eq!(shared_words(&["arbre à chat"]), ["arbre", "chat"]);
        assert!(shared_words(&["arbre à chat", "griffoir"]).is_empty());
        assert!(shared_words::<&str>(&[]).is_empty());
    }

    #[test]
    fn the_tree_reads_each_parent_then_its_children() {
        let tree = tree(&[
            ("gants moto", None),
            ("casque moto", None),
            ("gants moto été", Some("gants moto")),
            ("casque moto intégral", Some("casque moto")),
            ("gants moto hiver", Some("gants moto")),
        ]);
        let review = review_tree(
            &tree,
            &["moto".to_owned()],
            &inputs(),
            &Thresholds::default(),
            &all_kept(&tree),
        );
        let read: Vec<&str> = review
            .categories
            .iter()
            .map(|category| category.keyword.as_str())
            .collect();
        assert_eq!(
            read,
            [
                "gants moto",
                "gants moto été",
                "gants moto hiver",
                "casque moto",
                "casque moto intégral"
            ]
        );
        assert_eq!(review.parents, 2);
        assert!(review.is_ready());
        assert!(!review.has_many_parents());
    }

    #[test]
    fn a_name_without_the_niches_word_is_held_back() {
        let tree = tree(&[
            ("Gants Motos", None),
            ("gants", None),
            ("motorisation", None),
        ]);
        let figures = all_kept(&tree);
        let seeds = ["gants moto".to_owned(), "casque moto".to_owned()];
        let review = review_tree(&tree, &seeds, &inputs(), &Thresholds::default(), &figures);
        assert_eq!(review.niche_words, ["moto"]);
        assert!(problems(&review, "Gants Motos").is_empty());
        assert_eq!(problems(&review, "gants"), [TreeProblem::NoNicheWord]);
        assert_eq!(
            problems(&review, "motorisation"),
            [TreeProblem::NoNicheWord],
            "a whole word, not a part of one"
        );
        assert_eq!(review.held_back(), 2);
        assert!(!review.is_ready());

        // The owner's words take the place of the seeds'; none checks no name.
        let said = Tree {
            niche_words: Some(vec!["Gant".to_owned()]),
            ..tree.clone()
        };
        let review = review_tree(&said, &seeds, &inputs(), &Thresholds::default(), &figures);
        assert!(problems(&review, "gants").is_empty());
        assert_eq!(
            problems(&review, "motorisation"),
            [TreeProblem::NoNicheWord]
        );
        let none = Tree {
            niche_words: Some(Vec::new()),
            ..tree
        };
        let review = review_tree(&none, &seeds, &inputs(), &Thresholds::default(), &figures);
        assert!(review.is_ready());
    }

    #[test]
    fn a_keyword_the_study_no_longer_keeps_is_held_back() {
        let tree = tree(&[
            ("gants moto", None),
            ("casque moto", None),
            ("bottes moto", None),
        ]);
        let mut figures = all_kept(&tree);
        figures.remove("casque moto");
        figures.get_mut("bottes moto").unwrap().kept = false;
        let review = review_tree(&tree, &[], &inputs(), &Thresholds::default(), &figures);
        assert!(problems(&review, "gants moto").is_empty());
        assert_eq!(problems(&review, "casque moto"), [TreeProblem::NotKept]);
        assert_eq!(problems(&review, "bottes moto"), [TreeProblem::NotKept]);
    }

    #[test]
    fn two_keywords_of_one_slug_and_a_slug_no_category_goes_by_are_held_back() {
        let long = format!("moto {}", "a".repeat(MAX_CATEGORY_NAME_LEN));
        let tree = tree(&[
            ("gants moto été", None),
            ("gants moto ete", None),
            ("blog", None),
            ("m", None),
            (long.as_str(), None),
        ]);
        let review = review_tree(
            &tree,
            &[],
            &inputs(),
            &Thresholds::default(),
            &all_kept(&tree),
        );
        assert_eq!(
            problems(&review, "gants moto été"),
            [TreeProblem::SameSlug("gants moto ete".to_owned())]
        );
        assert_eq!(
            problems(&review, "gants moto ete"),
            [TreeProblem::SameSlug("gants moto été".to_owned())]
        );
        assert_eq!(problems(&review, "blog"), [TreeProblem::NoSlug]);
        assert_eq!(problems(&review, "m"), [TreeProblem::NoSlug]);
        assert_eq!(problems(&review, &long), [TreeProblem::NameTooLong]);
    }

    #[test]
    fn a_category_whose_clicks_cost_too_much_is_kept_out_of_ads() {
        let mut tree = tree(&[
            ("gants moto", None),
            ("casque moto", None),
            ("bottes moto", None),
            ("blouson moto", None),
        ]);
        // 100.00 over a click at 0.50 is 200: above the 193.34 needed.
        // Over a click at 0.60 it is 166.66: below.
        let mut figures = all_kept(&tree);
        figures.insert("casque moto".to_owned(), kept(Some(60)));
        figures.insert("blouson moto".to_owned(), kept(None));
        // Its own price of 150.00 over a click at 0.60 is 250.
        figures.insert("bottes moto".to_owned(), kept(Some(60)));
        tree.categories[2].price_cents = Some(15_000);
        let review = review_tree(&tree, &[], &inputs(), &Thresholds::default(), &figures);
        let of = |keyword: &str| {
            review
                .categories
                .iter()
                .find(|category| category.keyword == keyword)
                .unwrap()
        };
        assert_eq!(of("gants moto").price_per_click, Some(20_000));
        assert_eq!(of("gants moto").channel, Some(Channel::Ads));
        assert!(!of("gants moto").keep_out_of_ads());
        assert_eq!(of("casque moto").price_per_click, Some(16_666));
        assert!(of("casque moto").keep_out_of_ads());
        assert_eq!(of("bottes moto").price_cents, Some(15_000));
        assert_eq!(of("bottes moto").price_per_click, Some(25_000));
        assert!(!of("bottes moto").keep_out_of_ads());
        assert_eq!(of("blouson moto").channel, None, "no cost per click known");
        assert!(!of("blouson moto").keep_out_of_ads());

        // Without a sale nothing is known, and nothing is marked.
        let unknown = review_tree(
            &tree,
            &[],
            &StudyInputs::default(),
            &Thresholds::default(),
            &figures,
        );
        assert!(unknown.categories.iter().all(|c| c.channel.is_none()));
        assert_eq!(unknown.categories[2].price_per_click, Some(25_000));
    }

    #[test]
    fn more_than_six_parents_is_said() {
        let keywords: Vec<String> = (0..=USUAL_PARENTS).map(|n| format!("moto {n}a")).collect();
        let placed: Vec<(&str, Option<&str>)> = keywords
            .iter()
            .map(|keyword| (keyword.as_str(), None))
            .collect();
        let tree = tree(&placed);
        let review = review_tree(
            &tree,
            &[],
            &inputs(),
            &Thresholds::default(),
            &all_kept(&tree),
        );
        assert!(review.has_many_parents());
        assert!(review.is_ready(), "said, not refused");
    }
}
