//! `/t/{slug}/search-console/categories`: where the shop's categories stand
//! in search, for members with `pages:read`, so ranking work goes where it
//! pays: the categories whose best query sits on the second page of results
//! (positions 11 to 20), each with that query, how it moved week by week,
//! and what the catalog says can be done about it (its products, its text,
//! when it last changed); the check a new shop is owed ninety days after
//! Google first showed it; and the queries the shop shows for through no
//! category, candidates for a new one or for an article.
//!
//! Worked out on each visit ([`ranking`](super::ranking)) from what the
//! daily read holds: which page showed for which query, a week at a time.

use std::collections::HashMap;

use timada_core::civil::parse_date;
use timada_google::{
    ACCESS_SEARCH_CONSOLE, MAX_PAGE_QUERY_ROWS, SearchFigures, day_before, first_search_day,
    last_search_day, page_queries, search_totals,
};
use timada_tenancy::PERMISSION_PAGES_READ;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{href, page},
    view::{View, attributes, view},
};

use super::landing::{Landing, Landings, path_of};
use super::ranking::{Band, RECENT_WEEKS, Trend, WEEKS, change_percent, rank};
use super::{count, position, tabs};
use crate::app::t::slug::announcements::pages;
use crate::app::t::slug::categories::category_id;
use crate::app::t::slug::google::google;
use crate::app::t::slug::{TenantAccess, TenantNav, require_tenant, slug};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{
    LINK, Shell, document, empty_row, fact, facts, format_time, page_header, page_tabs,
    section_card,
};

/// How many days after Google first showed the shop its check is due.
const CHECK_AFTER_DAYS: u64 = 90;
/// For how many days after that first one the check is shown: past them
/// the shop is no new shop.
const CHECK_SHOWN_DAYS: u64 = 180;
/// How many categories the check wants in positions 1 to 20.
const CHECK_CATEGORIES: usize = 2;
/// How many days the check compares, against as many before them.
const CHECK_SPAN: u32 = 28;

/// What the catalog says of a category.
#[derive(timada_core::db::FromRow)]
struct CategoryFacts {
    id: String,
    name: String,
    /// The products its storefront page lists.
    subtree_count: i64,
    has_text: bool,
    updated_at: i64,
}

/// One category on the second page of results, as its row shows it.
struct Row {
    id: String,
    name: String,
    edit: String,
    query: String,
    impressions: i64,
    position: String,
    /// "Up 2.4", "Down 1.0", "Steady"; empty when one week only showed.
    moved: String,
    moved_key: &'static str,
    /// A week at a time, the oldest first; a dash for a week not shown.
    week_positions: String,
    week_impressions: String,
    products: i64,
    has_text: bool,
    updated_at: i64,
}

/// One query no category showed for, as its row shows it.
struct Loose {
    query: String,
    impressions: i64,
    position: String,
    page: String,
    path: String,
    kind: &'static str,
    kind_key: &'static str,
    name: String,
}

/// The check a new shop is owed.
struct Check {
    first: String,
    /// The day the check is due.
    due: String,
    /// Whether that day has come.
    is_due: bool,
    /// How many days since the first one, that one counted.
    day: u64,
    placed: usize,
    trend: Trend,
    recent: i64,
    before: i64,
    change: Option<i64>,
}

impl Check {
    /// `early` before the check is due; then `met` when both hold, `unmet`
    /// otherwise.
    fn key(&self) -> &'static str {
        if !self.is_due {
            "early"
        } else if self.placed >= CHECK_CATEGORIES && self.trend == Trend::Rising {
            "met"
        } else {
            "unmet"
        }
    }
}

/// How a query moved, in words, and as a data attribute reads it.
fn moved(climbed_tenths: Option<i64>) -> (String, &'static str) {
    let places = |tenths: i64| format!("{}.{}", tenths / 10, tenths % 10);
    match climbed_tenths {
        None => (String::new(), ""),
        Some(0) => ("Steady".to_owned(), "steady"),
        Some(tenths) if tenths > 0 => (format!("Up {}", places(tenths)), "up"),
        Some(tenths) => (format!("Down {}", places(-tenths)), "down"),
    }
}

/// A row of weekly figures, the oldest first.
fn series(weeks: &[SearchFigures], shown: impl Fn(&SearchFigures) -> String) -> String {
    weeks
        .iter()
        .map(|week| {
            if week.impressions > 0 {
                shown(week)
            } else {
                "–".to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_PAGES_READ).await?;
    let nav = TenantNav::of(cx, &access).await?;
    let site = google(cx)
        .load_connection(access.tenant_id())
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|connection| connection.covers(ACCESS_SEARCH_CONSOLE))
        .and_then(|connection| connection.site);
    let db = context::<Db>(cx);
    let tenant_id = access.tenant_id().to_owned();
    let slug = access.tenant.slug.clone();
    let last = match &site {
        Some(site) => last_search_day(db, &tenant_id, site)
            .await
            .map_err(topcoat::Error::from_anyhow)?,
        None => None,
    };

    let mut rows = Vec::new();
    let mut loose = Vec::new();
    let mut check = None;
    // How many categories stand on the first page, the second, further.
    let mut bands = [0usize; 3];
    let mut live = 0;
    if let (Some(site), Some(last)) = (&site, &last) {
        // The weeks followed, the oldest first, the latest ending on the
        // last day held.
        let mut weeks = Vec::with_capacity(WEEKS);
        for back in (0..WEEKS as u32).rev() {
            let end = day_before(last, back * 7);
            let lines = page_queries(
                db,
                &tenant_id,
                site,
                &day_before(&end, 6),
                &end,
                7 * MAX_PAGE_QUERY_ROWS as i64,
            )
            .await
            .map_err(topcoat::Error::from_anyhow)?;
            weeks.push(lines);
        }
        let others = match pages(&access) {
            Ok(pages) => pages
                .languages()
                .await
                .map_err(topcoat::Error::from_anyhow)?
                .map(|languages| languages.others)
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        // What each page is, each address read once.
        let mut landings = Landings::new(db, &tenant_id, &others);
        let mut told: HashMap<String, Landing> = HashMap::new();
        for line in weeks.iter().flatten() {
            if !told.contains_key(&line.page) {
                let landing = landings
                    .of(&line.page)
                    .await
                    .map_err(topcoat::Error::from_anyhow)?;
                told.insert(line.page.clone(), landing);
            }
        }
        let category_of: HashMap<String, String> = told
            .iter()
            .filter_map(|(page, landing)| match landing {
                Landing::Category { id, .. } => Some((page.clone(), id.clone())),
                _ => None,
            })
            .collect();
        let ranked = rank(&weeks, &category_of);

        let catalog: Vec<CategoryFacts> = db
            .fetch_all(
                "SELECT id, name, subtree_count, description <> '' AS has_text, updated_at
                 FROM catalog_categories
                 WHERE tenant_id = ?1 AND archived = 0 AND draft = 0",
                (tenant_id.as_str(),),
            )
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        live = catalog.len();
        let catalog: HashMap<&str, &CategoryFacts> = catalog
            .iter()
            .map(|category| (category.id.as_str(), category))
            .collect();
        for category in &ranked.categories {
            let Some(band) = Band::of(&category.recent) else {
                continue;
            };
            let Some(facts) = catalog.get(category.category_id.as_str()) else {
                continue;
            };
            bands[band as usize] += 1;
            if band != Band::Second {
                continue;
            }
            let (moved, moved_key) = moved(category.climbed_tenths());
            rows.push(Row {
                id: facts.id.clone(),
                name: facts.name.clone(),
                edit: href!(category_id::index, slug: slug.clone(), category_id::category_id: facts.id.clone())
                    .resolve(cx),
                query: category.query.clone(),
                impressions: category.recent.impressions,
                position: position(&category.recent),
                moved,
                moved_key,
                week_positions: series(&category.weeks, position),
                week_impressions: series(&category.weeks, |week| count(week.impressions)),
                products: facts.subtree_count,
                has_text: facts.has_text,
                updated_at: facts.updated_at,
            });
        }
        for query in ranked.loose {
            let landing = told.get(&query.page).cloned().unwrap_or(Landing::Other);
            let (kind, name) = landing.label();
            loose.push(Loose {
                impressions: query.figures.impressions,
                position: position(&query.figures),
                path: path_of(&query.page),
                kind,
                kind_key: landing.key(),
                name: name.to_owned(),
                page: query.page,
                query: query.query,
            });
        }

        let first = first_search_day(db, &tenant_id, site)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        let today = crate::auth::now();
        if let Some(first) = first
            && let Some(first_at) = parse_date(&first)
            && today.saturating_sub(first_at) < CHECK_SHOWN_DAYS * 86_400
        {
            let recent_from = day_before(last, CHECK_SPAN - 1);
            let recent = search_totals(db, &tenant_id, site, &recent_from, last)
                .await
                .map_err(topcoat::Error::from_anyhow)?
                .impressions;
            let before = search_totals(
                db,
                &tenant_id,
                site,
                &day_before(last, 2 * CHECK_SPAN - 1),
                &day_before(last, CHECK_SPAN),
            )
            .await
            .map_err(topcoat::Error::from_anyhow)?
            .impressions;
            let due_at = first_at + CHECK_AFTER_DAYS * 86_400;
            check = Some(Check {
                first,
                due: timada_core::civil::format_date(due_at),
                is_due: today >= due_at,
                day: today.saturating_sub(first_at) / 86_400 + 1,
                placed: bands[Band::First as usize] + bands[Band::Second as usize],
                trend: Trend::of(recent, before),
                recent,
                before,
                change: change_percent(recent, before),
            });
        }
    }
    let shown: usize = bands.iter().sum();
    let TenantAccess { tenant, .. } = access;
    let title = format!("Categories in search · {}", tenant.name);
    let tabs = tabs(cx, &slug, "categories");
    let overview = href!(super::index, slug: slug.clone()).resolve(cx);
    let page_two_note = format!(
        "The categories whose best query (the one their page was shown for the most over the last {} days) sits in positions 11 to 20: a little work on one of them moves it to the first page. The weeks are the last {WEEKS}, the oldest first.",
        RECENT_WEEKS * 7
    );
    let loose_note = format!(
        "What people typed over the last {} days that showed your shop through a product, an article or the home page, and no category: candidates for a new category or for an article. The most shown first, with the page Google answered with.",
        RECENT_WEEKS * 7
    );

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "search_console"),
            page_header(
                title: "Categories in search",
                subtitle: Some("Where each category stands on Google, by the query its page is shown for the most, as Search Console counts it."),
            )
            page_tabs(label: "Search Console", tabs: &tabs)

            if last.is_none() {
                section_card(
                    attrs: attributes! { data-ranking-empty="" },
                    <p class="text-sm text-muted-foreground">
                        "Nothing is read from Search Console yet. The "
                        <a href=(&overview) class=(LINK)>"overview"</a>
                        " says where the shop's connection and its daily read stand."
                    </p>
                )
            }

            if let Some(check) = &check {
                section_card(
                    title: Some("New shop check"),
                    description: Some("Ninety days after Google first showed the shop: are two categories in positions 1 to 20, and are impressions rising?"),
                    attrs: attributes! { data-check=(check.key()) },
                    <p class="text-sm">
                        if !check.is_due {
                            "Too early to tell: the check is due on "
                            <time datetime=(&check.due)>(&check.due)</time> "."
                        } else if check.key() == "met" {
                            "Both hold: the shop is on course."
                        } else {
                            "Not there yet: see what is missing below."
                        }
                    </p>
                    facts(
                        fact(term: "First shown", attrs: attributes! { data-check-first=(&check.first) },
                            <time datetime=(&check.first)>(&check.first)</time>
                            " (day " (check.day) "), as far back as Search Console was read"
                        )
                        fact(term: "Categories placed", attrs: attributes! { data-check-placed=(check.placed) },
                            (check.placed) " in positions 1 to 20, of " (CHECK_CATEGORIES) " wanted"
                        )
                        fact(term: "Impressions", attrs: attributes! { data-check-trend=(check.trend.key()) },
                            (check.trend.label()) ": " (count(check.recent)) " over the last " (CHECK_SPAN)
                            " days, " (count(check.before)) " over the " (CHECK_SPAN) " before"
                            if let Some(change) = check.change {
                                " (" if change > 0 { "+" } (change) " %)"
                            }
                        )
                    )
                )
            }

            if last.is_some() {
                section_card(
                    title: Some("On the second page"),
                    description: Some(&page_two_note),
                    attrs: attributes! { data-ranking="" },
                    <p class="text-sm text-muted-foreground" data-ranking-bands="">
                        "Of " <span data-live=(live)>(live)</span> " live categories: "
                        <span data-band-first=(bands[0])>(bands[0])</span> " in positions 1 to 10, "
                        <span data-band-second=(bands[1])>(bands[1])</span> " in 11 to 20, "
                        <span data-band-beyond=(bands[2])>(bands[2])</span> " further, "
                        (live.saturating_sub(shown)) " not shown."
                    </p>
                    table(
                        attrs: attributes! { data-ranking-categories="" },
                        table_header(
                            table_row(
                                table_head("Category")
                                table_head("Best query")
                                table_head(attrs: attributes! { class="text-right" }, "Impressions")
                                table_head(attrs: attributes! { class="text-right" }, "Position")
                                table_head("Week by week")
                                table_head(attrs: attributes! { class="text-right" }, "Products")
                                table_head("Text")
                                table_head("Last changed")
                            )
                        )
                        table_body(
                            if rows.is_empty() {
                                empty_row(colspan: 8, text: "No category's best query sits in positions 11 to 20.")
                            }
                            for row in &rows {
                                table_row(
                                    attrs: attributes! { data-ranking-category=(&row.id) },
                                    table_cell(<a href=(&row.edit) class=(LINK)>(&row.name)</a>)
                                    table_cell(attrs: attributes! { data-ranking-query="" }, (&row.query))
                                    table_cell(attrs: attributes! { class="text-right tabular-nums" data-impressions=(row.impressions) }, (count(row.impressions)))
                                    table_cell(attrs: attributes! { class="text-right tabular-nums" data-position="" }, (&row.position))
                                    table_cell(
                                        attrs: attributes! { class="text-xs tabular-nums" },
                                        if !row.moved.is_empty() {
                                            <p class="text-sm" data-moved=(row.moved_key)>(&row.moved)</p>
                                        }
                                        <p data-week-positions="">"Position: " (&row.week_positions)</p>
                                        <p data-week-impressions="">"Impressions: " (&row.week_impressions)</p>
                                    )
                                    table_cell(attrs: attributes! { class="text-right tabular-nums" data-products=(row.products) }, (row.products))
                                    table_cell(
                                        attrs: attributes! { data-text=(if row.has_text { "yes" } else { "no" }) },
                                        if row.has_text { "Written" } else { "None" }
                                    )
                                    table_cell(<time datetime=(row.updated_at)>(format_time(row.updated_at as u64))</time>)
                                )
                            }
                        )
                    )
                )

                section_card(
                    title: Some("Queries without a category"),
                    description: Some(&loose_note),
                    table(
                        attrs: attributes! { data-ranking-loose="" },
                        table_header(
                            table_row(
                                table_head("Query")
                                table_head(attrs: attributes! { class="text-right" }, "Impressions")
                                table_head(attrs: attributes! { class="text-right" }, "Position")
                                table_head("Page shown")
                                table_head("What it is")
                            )
                        )
                        table_body(
                            if loose.is_empty() {
                                empty_row(colspan: 5, text: "Every query told showed a category.")
                            }
                            for row in &loose {
                                table_row(
                                    attrs: attributes! { data-loose-query=(&row.query) },
                                    table_cell((&row.query))
                                    table_cell(attrs: attributes! { class="text-right tabular-nums" data-impressions=(row.impressions) }, (count(row.impressions)))
                                    table_cell(attrs: attributes! { class="text-right tabular-nums" data-position="" }, (&row.position))
                                    table_cell(<a href=(&row.page) class=(LINK) rel="noreferrer">(&row.path)</a>)
                                    table_cell(
                                        attrs: attributes! { data-search-kind=(row.kind_key) },
                                        (row.kind)
                                        if !row.name.is_empty() { ": " (&row.name) }
                                    )
                                )
                            }
                        )
                    )
                )
            }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_move_reads_in_places_up_or_down() {
        assert_eq!(moved(Some(24)), ("Up 2.4".to_owned(), "up"));
        assert_eq!(moved(Some(-10)), ("Down 1.0".to_owned(), "down"));
        assert_eq!(moved(Some(0)), ("Steady".to_owned(), "steady"));
        assert_eq!(moved(None), (String::new(), ""));
    }

    #[test]
    fn a_week_not_shown_reads_as_a_dash() {
        let week = |impressions: i64| SearchFigures {
            clicks: 0,
            impressions,
            weighted_position_milli: impressions * 12_500,
        };
        let weeks = [week(0), week(40), week(1_200)];
        assert_eq!(series(&weeks, position), "– · 12.5 · 12.5");
        assert_eq!(
            series(&weeks, |week| count(week.impressions)),
            "– · 40 · 1\u{202f}200"
        );
    }
}
