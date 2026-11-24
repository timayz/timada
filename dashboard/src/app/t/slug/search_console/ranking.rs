//! Where the shop's categories stand in search, worked out from which page
//! showed for which query, a week at a time.
//!
//! A category's best query is the one its page was shown for the most over
//! the last [`RECENT_WEEKS`] weeks: the query Google answers with it, more
//! than any other. Where that query sits says where the category stands:
//! on the first page of results, on the second, or further. A category
//! served in two languages is one category, its pages counted together.
//!
//! A query no category's page was shown for is a loose one: something the
//! shop shows for through a product, an article or its home page, and a
//! candidate for a category or an article of its own.

use std::cmp::Reverse;
use std::collections::HashMap;

use timada_google::{PageQueryLine, SearchFigures};

/// How many weeks back a query is followed.
pub const WEEKS: usize = 6;
/// How many of them, the latest, say where a category stands now.
pub const RECENT_WEEKS: usize = 4;
/// How many loose queries are listed: the most shown.
pub const LOOSE_LISTED: usize = 50;

/// Where an average position falls, to the nearest whole one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    /// Positions 1 to 10: the first page of results.
    First,
    /// Positions 11 to 20: the second.
    Second,
    /// Further than 20.
    Beyond,
}

impl Band {
    /// The band of what was shown; `None` for what never was.
    pub fn of(figures: &SearchFigures) -> Option<Self> {
        (figures.impressions > 0).then(|| {
            match figures.weighted_position_milli / figures.impressions {
                ..10_500 => Self::First,
                10_500..20_500 => Self::Second,
                _ => Self::Beyond,
            }
        })
    }
}

fn add(to: &mut SearchFigures, more: &SearchFigures) {
    to.clicks += more.clicks;
    to.impressions += more.impressions;
    to.weighted_position_milli += more.weighted_position_milli;
}

/// A category that showed in search, by its best query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryRank {
    pub category_id: String,
    /// The query its pages were shown for the most over the recent weeks.
    pub query: String,
    /// What the category did for that query over the recent weeks.
    pub recent: SearchFigures,
    /// The same, a week at a time, the oldest first.
    pub weeks: [SearchFigures; WEEKS],
}

impl CategoryRank {
    /// By how many tenths of a position the query climbed from the first
    /// week it showed in to the last (negative: it fell); `None` when it
    /// showed in one week only.
    pub fn climbed_tenths(&self) -> Option<i64> {
        let mut shown = self.weeks.iter().filter_map(SearchFigures::position_tenths);
        let first = shown.next()?;
        let last = shown.next_back()?;
        Some(first - last)
    }
}

/// A query the shop showed for through no category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LooseQuery {
    pub query: String,
    /// The page shown for it the most.
    pub page: String,
    /// What that page did for it over the recent weeks.
    pub figures: SearchFigures,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ranked {
    /// The categories shown over the recent weeks, the most shown first.
    pub categories: Vec<CategoryRank>,
    /// The loose queries, the most shown first, [`LOOSE_LISTED`] at most.
    pub loose: Vec<LooseQuery>,
}

/// Reads `weeks` ([`WEEKS`] of them at most, the oldest first, each the
/// queries its pages showed for) into where each category stands.
/// `category_of` gives the category a page is, by its address; a page
/// absent from it is no category.
pub fn rank(weeks: &[Vec<PageQueryLine>], category_of: &HashMap<String, String>) -> Ranked {
    let weeks = &weeks[weeks.len().saturating_sub(WEEKS)..];
    // The weeks given end on the latest: a short list fills the last slots.
    let offset = WEEKS - weeks.len();
    let recent_from = WEEKS - RECENT_WEEKS;
    let mut by_category: HashMap<&str, HashMap<&str, [SearchFigures; WEEKS]>> = HashMap::new();
    // By query, over the recent weeks: what each page that is no category
    // did, and whether a category showed for it.
    let mut by_query: HashMap<&str, (bool, HashMap<&str, SearchFigures>)> = HashMap::new();
    for (at, lines) in weeks.iter().enumerate() {
        let week = at + offset;
        for line in lines.iter().filter(|line| line.impressions > 0) {
            let category = category_of.get(&line.page);
            if let Some(category) = category {
                let series = by_category
                    .entry(category.as_str())
                    .or_default()
                    .entry(line.query.as_str())
                    .or_default();
                add(&mut series[week], &line.figures());
            }
            if week >= recent_from {
                let seen = by_query.entry(line.query.as_str()).or_default();
                match category {
                    Some(_) => seen.0 = true,
                    None => add(
                        seen.1.entry(line.page.as_str()).or_default(),
                        &line.figures(),
                    ),
                }
            }
        }
    }

    let mut categories: Vec<CategoryRank> = by_category
        .into_iter()
        .filter_map(|(category_id, queries)| {
            queries
                .into_iter()
                .map(|(query, weeks)| {
                    let mut recent = SearchFigures::default();
                    for week in &weeks[recent_from..] {
                        add(&mut recent, week);
                    }
                    (query, recent, weeks)
                })
                .filter(|(_, recent, _)| recent.impressions > 0)
                // The most shown; then the most clicked, the best placed,
                // and by name so two readings agree.
                .max_by_key(|(query, recent, _)| {
                    (
                        recent.impressions,
                        recent.clicks,
                        Reverse(recent.weighted_position_milli),
                        Reverse(*query),
                    )
                })
                .map(|(query, recent, weeks)| CategoryRank {
                    category_id: category_id.to_owned(),
                    query: query.to_owned(),
                    recent,
                    weeks,
                })
        })
        .collect();
    categories.sort_by(|a, b| {
        (Reverse(a.recent.impressions), &a.category_id)
            .cmp(&(Reverse(b.recent.impressions), &b.category_id))
    });

    let mut loose: Vec<LooseQuery> = by_query
        .into_iter()
        .filter(|(_, (in_category, _))| !in_category)
        .filter_map(|(query, (_, pages))| {
            pages
                .into_iter()
                .max_by_key(|(page, figures)| (figures.impressions, Reverse(*page)))
                .map(|(page, figures)| LooseQuery {
                    query: query.to_owned(),
                    page: page.to_owned(),
                    figures,
                })
        })
        .collect();
    loose.sort_by(|a, b| {
        (Reverse(a.figures.impressions), &a.query).cmp(&(Reverse(b.figures.impressions), &b.query))
    });
    loose.truncate(LOOSE_LISTED);
    Ranked { categories, loose }
}

/// Which way a count went from one period to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    Rising,
    Flat,
    Falling,
}

impl Trend {
    /// `recent` against `before`: a tenth more is rising, a tenth less is
    /// falling, anything between is flat.
    pub fn of(recent: i64, before: i64) -> Self {
        if recent > before && recent * 10 >= before * 11 {
            Self::Rising
        } else if recent < before && recent * 10 <= before * 9 {
            Self::Falling
        } else {
            Self::Flat
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Rising => "rising",
            Self::Flat => "flat",
            Self::Falling => "falling",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Rising => "Rising",
            Self::Flat => "Flat",
            Self::Falling => "Falling",
        }
    }
}

/// By how many percent `recent` differs from `before`; `None` when there
/// was nothing before.
pub fn change_percent(recent: i64, before: i64) -> Option<i64> {
    (before > 0).then(|| ((recent - before) * 100 + before / 2).div_euclid(before))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREES: &str = "https://shop.example/arbres-a-chat";
    const TREES_EN: &str = "https://shop.example/en/cat-trees";
    const TOYS: &str = "https://shop.example/jouets";
    const PRODUCT: &str = "https://shop.example/p/plumeau";
    const HOME: &str = "https://shop.example/";

    fn line(page: &str, query: &str, impressions: i64, position: i64) -> PageQueryLine {
        PageQueryLine {
            page: page.to_owned(),
            query: query.to_owned(),
            clicks: 0,
            impressions,
            weighted_position_milli: impressions * position * 1_000,
        }
    }

    fn categories() -> HashMap<String, String> {
        [(TREES, "c1"), (TREES_EN, "c1"), (TOYS, "c2")]
            .into_iter()
            .map(|(page, id)| (page.to_owned(), id.to_owned()))
            .collect()
    }

    #[test]
    fn a_position_falls_on_the_page_it_rounds_to() {
        let at = |milli: i64| {
            Band::of(&SearchFigures {
                clicks: 0,
                impressions: 10,
                weighted_position_milli: milli * 10,
            })
        };
        assert_eq!(at(1_000), Some(Band::First));
        assert_eq!(at(10_499), Some(Band::First));
        assert_eq!(at(10_500), Some(Band::Second));
        assert_eq!(at(20_499), Some(Band::Second));
        assert_eq!(at(20_500), Some(Band::Beyond));
        assert_eq!(Band::of(&SearchFigures::default()), None);
    }

    #[test]
    fn a_category_stands_where_its_most_shown_query_sits() {
        let mut weeks = vec![Vec::new(); WEEKS];
        // Six weeks of one query climbing, a rarer one far better placed.
        for (week, position) in [19, 18, 16, 15, 13, 12].into_iter().enumerate() {
            weeks[week].push(line(TREES, "arbre à chat", 100, position));
            weeks[week].push(line(TREES, "arbre à chat design", 5, 2));
        }
        // The same category in English counts with it.
        weeks[5].push(line(TREES_EN, "arbre à chat", 100, 10));
        // Shown before the recent weeks only: not standing anywhere now.
        weeks[0].push(line(TOYS, "jouet chat", 500, 3));
        let ranked = rank(&weeks, &categories());
        assert_eq!(ranked.categories.len(), 1);
        let trees = &ranked.categories[0];
        assert_eq!(trees.category_id, "c1");
        assert_eq!(trees.query, "arbre à chat");
        // Weeks 3 to 6: 16, 15, 13 over 100 each, then 12 and 10 over 100.
        assert_eq!(trees.recent.impressions, 500);
        assert_eq!(trees.recent.position_tenths(), Some(132));
        assert_eq!(Band::of(&trees.recent), Some(Band::Second));
        assert_eq!(trees.weeks[0].position_tenths(), Some(190));
        assert_eq!(trees.weeks[5].impressions, 200);
        assert_eq!(trees.weeks[5].position_tenths(), Some(110));
        assert_eq!(trees.climbed_tenths(), Some(80));
        assert!(ranked.loose.is_empty());
    }

    #[test]
    fn categories_come_the_most_shown_first_and_ties_read_the_same() {
        let mut weeks = vec![Vec::new(); WEEKS];
        weeks[5] = vec![
            line(TOYS, "jouet chat", 300, 4),
            line(TREES, "b", 50, 12),
            line(TREES, "a", 50, 12),
        ];
        let ranked = rank(&weeks, &categories());
        let read: Vec<_> = ranked
            .categories
            .iter()
            .map(|rank| (rank.category_id.as_str(), rank.query.as_str()))
            .collect();
        assert_eq!(read, vec![("c2", "jouet chat"), ("c1", "a")]);
        // One week only: nothing to say of how it moved.
        assert_eq!(ranked.categories[0].climbed_tenths(), None);
        // Fewer weeks than followed are the latest ones.
        let short = rank(&weeks[4..], &categories());
        assert_eq!(short.categories, ranked.categories);
    }

    #[test]
    fn a_query_no_category_showed_for_is_loose() {
        let mut weeks = vec![Vec::new(); WEEKS];
        weeks[4] = vec![
            line(PRODUCT, "plumeau chat", 40, 8),
            line(HOME, "plumeau chat", 10, 30),
            line(HOME, "boutique chat", 90, 6),
            // A category answers this one, the home page beside it.
            line(TREES, "arbre à chat", 100, 12),
            line(HOME, "arbre à chat", 500, 25),
        ];
        weeks[5] = vec![line(PRODUCT, "plumeau chat", 30, 6)];
        // Too old to count.
        weeks[1] = vec![line(PRODUCT, "griffoir", 900, 5)];
        let ranked = rank(&weeks, &categories());
        let read: Vec<_> = ranked
            .loose
            .iter()
            .map(|loose| {
                (
                    loose.query.as_str(),
                    loose.page.as_str(),
                    loose.figures.impressions,
                )
            })
            .collect();
        assert_eq!(
            read,
            vec![("boutique chat", HOME, 90), ("plumeau chat", PRODUCT, 70)]
        );
        // 8 over 40 and 6 over 30.
        assert_eq!(ranked.loose[1].figures.position_tenths(), Some(71));

        let many: Vec<_> = (0..LOOSE_LISTED as i64 + 5)
            .map(|n| line(HOME, &format!("query {n:03}"), 100 - n, 9))
            .collect();
        let ranked = rank(&[many], &categories());
        assert_eq!(ranked.loose.len(), LOOSE_LISTED);
        assert_eq!(ranked.loose[0].query, "query 000");
    }

    #[test]
    fn a_count_rises_or_falls_by_a_tenth_or_stays_flat() {
        assert_eq!(Trend::of(110, 100), Trend::Rising);
        assert_eq!(Trend::of(109, 100), Trend::Flat);
        assert_eq!(Trend::of(91, 100), Trend::Flat);
        assert_eq!(Trend::of(90, 100), Trend::Falling);
        assert_eq!(Trend::of(1, 0), Trend::Rising);
        assert_eq!(Trend::of(0, 0), Trend::Flat);
        assert_eq!(Trend::of(0, 5), Trend::Falling);
        assert_eq!(change_percent(150, 100), Some(50));
        assert_eq!(change_percent(75, 100), Some(-25));
        assert_eq!(change_percent(10, 0), None);
    }
}
