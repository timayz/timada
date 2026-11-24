//! `/t/{slug}/search-console`: what Search Console says of the shop's site,
//! for members with `pages:read`: its clicks, impressions, click rate and
//! average position over a period, then the queries it showed for and the
//! pages found, each page told for the category, product or article it is.
//! Read once a day through the shop's Google connection
//! (`crate::search_passes`), for the days Google has finished counting, so
//! a period ends on the last of those, two to three days back. Summed from
//! the read model; where the daily read stands is the watch's own stream.

mod landing;

use timada_google::{
    ACCESS_SEARCH_CONSOLE, MAX_SEARCH_ROWS, SearchFigures, day_before, last_search_day,
    search_totals, top_pages, top_queries,
};
use timada_tenancy::PERMISSION_PAGES_READ;
use topcoat::{
    Result,
    context::{Cx, context},
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use self::landing::{Landings, path_of};
use super::announcements::pages;
use super::google::google;
use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{
    LINK, Shell, document, empty_row, fact, facts, filter_bar, format_time, icons, page_header,
    section_card,
};

/// How many queries, and how many pages, a period lists.
const LISTED: i64 = 100;
/// The periods offered, in days up to the last one held, with what the
/// menu calls them. The longest is as far back as the queries and the
/// pages are kept.
const SPANS: [(u32, &str); 3] = [(7, "7 days"), (28, "28 days"), (90, "90 days")];
/// The span a page without a choice shows.
const DEFAULT_SPAN: u32 = 28;

#[topcoat::router::query_params(error = bad_request)]
struct SearchQuery {
    /// One of [`SPANS`], in days; anything else is the default.
    days: Option<String>,
}

/// The span the query asks for.
fn span_of(days: Option<&str>) -> u32 {
    days.and_then(|days| days.parse::<u32>().ok())
        .filter(|days| SPANS.iter().any(|(span, _)| span == days))
        .unwrap_or(DEFAULT_SPAN)
}

/// A count with its thousands apart: `12 480`.
fn count(n: i64) -> String {
    let digits = n.max(0).to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(digit);
    }
    out
}

/// Clicks per impression, as a percentage with one decimal.
fn click_rate(figures: &SearchFigures) -> String {
    match figures.click_rate_bps() {
        Some(bps) => format!("{}.{} %", bps / 100, (bps % 100) / 10),
        None => "None".to_owned(),
    }
}

/// The average position, with one decimal.
fn position(figures: &SearchFigures) -> String {
    match figures.position_tenths() {
        Some(tenths) => format!("{}.{}", tenths / 10, tenths % 10),
        None => "None".to_owned(),
    }
}

/// One query or one page of the period, as its row shows it.
struct Line {
    key: String,
    /// What the row reads: the query, or the page's path.
    shown: String,
    /// What a page is, and what it is called.
    kind: &'static str,
    kind_key: &'static str,
    name: String,
    clicks: i64,
    impressions: String,
    click_rate: String,
    position: String,
}

/// Where the shop stands with Search Console.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// No connection, or one that does not cover Search Console.
    NotConnected,
    /// Google no longer honours the connection.
    Lapsed,
    /// Connected, with no site chosen.
    NoSite,
    /// Read, or about to be.
    Read,
}

impl Standing {
    fn key(self) -> &'static str {
        match self {
            Self::NotConnected => "not-connected",
            Self::Lapsed => "lapsed",
            Self::NoSite => "no-site",
            Self::Read => "read",
        }
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_PAGES_READ).await?;
    let query = query_params::<SearchQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let span = span_of(query.days.as_deref());
    let command = google(cx);
    let connection = command
        .load_connection(access.tenant_id())
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|connection| connection.covers(ACCESS_SEARCH_CONSOLE));
    let site = connection.as_ref().and_then(|c| c.site.clone());
    let standing = match &connection {
        None => Standing::NotConnected,
        Some(connection) if connection.lapsed => Standing::Lapsed,
        Some(_) if site.is_none() => Standing::NoSite,
        Some(_) => Standing::Read,
    };
    // Where the daily read stands, for the site it is about.
    let watch = command
        .load_search_watch(access.tenant_id())
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|watch| Some(&watch.site) == site.as_ref());
    let db = context::<Db>(cx);
    let tenant_id = access.tenant_id().to_owned();
    // What a lapsed connection read before stays readable.
    let last = match &site {
        Some(site) => last_search_day(db, &tenant_id, site)
            .await
            .map_err(topcoat::Error::from_anyhow)?,
        None => None,
    };
    let period = last
        .as_ref()
        .map(|last| (day_before(last, span - 1), last.clone()));
    let mut totals = None;
    let mut queries = Vec::new();
    let mut found = Vec::new();
    if let (Some(site), Some((first, last))) = (&site, &period) {
        totals = Some(
            search_totals(db, &tenant_id, site, first, last)
                .await
                .map_err(topcoat::Error::from_anyhow)?,
        );
        let line = |key: String, shown: String, figures: SearchFigures| Line {
            key,
            shown,
            kind: "",
            kind_key: "",
            name: String::new(),
            clicks: figures.clicks,
            impressions: count(figures.impressions),
            click_rate: click_rate(&figures),
            position: position(&figures),
        };
        for row in top_queries(db, &tenant_id, site, first, last, LISTED)
            .await
            .map_err(topcoat::Error::from_anyhow)?
        {
            let figures = row.figures();
            queries.push(line(row.key.clone(), row.key, figures));
        }
        // The languages the storefront is read in: a page under `/en` is
        // the English one.
        let others = match pages(&access) {
            Ok(pages) => pages
                .languages()
                .await
                .map_err(topcoat::Error::from_anyhow)?
                .map(|languages| languages.others)
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let mut landings = Landings::new(db, &tenant_id, &others);
        for row in top_pages(db, &tenant_id, site, first, last, LISTED)
            .await
            .map_err(topcoat::Error::from_anyhow)?
        {
            let landing = landings
                .of(&row.key)
                .await
                .map_err(topcoat::Error::from_anyhow)?;
            let (kind, name) = landing.label();
            let mut line = line(row.key.clone(), path_of(&row.key), row.figures());
            line.kind = kind;
            line.kind_key = landing.key();
            line.name = name.to_owned();
            found.push(line);
        }
    }
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Search Console · {}", tenant.name);
    let google_page = href!(super::google::index, slug: slug.clone()).resolve(cx);
    let failure = watch
        .as_ref()
        .filter(|_| standing == Standing::Read)
        .and_then(|watch| Some((watch.last_error.clone()?, watch.next_at)));
    let checked_at = watch.as_ref().map_or(0, |watch| watch.checked_at);
    let figures = totals.map(|totals| totals.figures());
    let days_held = totals.map_or(0, |totals| totals.days);
    let (first, last) = period.clone().unwrap_or_default();
    let queries_note = format!(
        "What people typed, the most clicked first. Each day keeps its {MAX_SEARCH_ROWS} most clicked queries, and Google withholds the ones too few people typed: the rows add up to less than the totals."
    );
    let pages_note = format!(
        "The pages found, the most clicked first, each told for what it is in the shop now. A search that shows two of your pages counts for each: the rows may add up to more than the totals. Each day keeps its {MAX_SEARCH_ROWS} most clicked pages."
    );

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "search_console"),
            page_header(
                title: "Search Console",
                subtitle: Some("Where your shop shows on Google: the queries it shows for and the pages found, as Search Console counts them. Read once a day through the shop's Google account."),
            )

            <div data-search-console=(standing.key())>
                if standing == Standing::NotConnected {
                    section_card(
                        title: Some("Not connected"),
                        <p class="text-sm text-muted-foreground">
                            "Connect the Google account that owns your shop's site in Search Console, on the "
                            <a href=(&google_page) class=(LINK)>"Google account"</a>
                            " page, and its queries and pages are read here from the next pass on."
                        </p>
                    )
                }
                if standing == Standing::Lapsed {
                    alert(
                        variant: AlertVariant::Destructive,
                        attrs: attributes! { role="alert" class="mb-6" data-search-lapsed="" },
                        icon(data: icons::ERROR)
                        alert_description(
                            "Google no longer honours the shop's connection: nothing new is read until you "
                            <a href=(&google_page) class=(LINK)>"connect again"</a>
                            ". What was read before stays below."
                        )
                    )
                }
                if standing == Standing::NoSite {
                    section_card(
                        title: Some("No site chosen"),
                        <p class="text-sm text-muted-foreground">
                            "The shop is connected to Google. Say which of the account's Search Console sites is this shop's on the "
                            <a href=(&google_page) class=(LINK)>"Google account"</a>
                            " page."
                        </p>
                    )
                }
                if let Some((reason, next_at)) = &failure {
                    alert(
                        variant: AlertVariant::Warning,
                        attrs: attributes! { role="alert" class="mb-6" data-search-failure="" },
                        icon(data: icons::WARNING)
                        alert_description(
                            "The last read of Search Console failed: " (reason) ". It is tried again from "
                            <time datetime=(*next_at)>(format_time(*next_at))</time> "."
                        )
                    )
                }
            </div>

            if let Some(site) = &site {
                section_card(
                    filter_bar(
                        <label class="text-sm" for="search-days">"Period"</label>
                        select(
                            attrs: attributes! { id="search-days" name="days" class="w-44" },
                            for (days, label) in SPANS {
                                <option value=(days) selected=(span == days)>"Last " (label)</option>
                            }
                        )
                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Show")
                    )
                    if period.is_some() {
                        <p class="text-sm text-muted-foreground" data-search-period="">
                            "From " <time datetime=(&first)>(&first)</time> " to " <time datetime=(&last)>(&last)</time>
                            ", both included: the last day Google has finished counting for "
                            <code>(site)</code> ", and the days before it. Days are Google's (Pacific time)."
                        </p>
                    } else if standing == Standing::Read && failure.is_none() {
                        <p class="text-sm text-muted-foreground" data-search-waiting="">
                            "Nothing read yet for " <code>(site)</code>
                            if checked_at > 0 {
                                ": Google has no finished day with a search that showed it. A new site takes a few days to appear."
                            } else {
                                ": the first read is on its way, with the last 90 days."
                            }
                        </p>
                    } else {
                        <p class="text-sm text-muted-foreground">"Nothing read yet for " <code>(site)</code> "."</p>
                    }
                    if checked_at > 0 {
                        <p class="mt-2 text-sm text-muted-foreground">
                            "Last read on " <time datetime=(checked_at) data-search-checked="">(format_time(checked_at))</time> "."
                        </p>
                    }
                )
            }

            if let Some(figures) = &figures {
                section_card(
                    title: Some("Totals"),
                    description: Some("The site's own numbers in web search. The average position counts each day for its impressions."),
                    attrs: attributes! { data-search-totals="" },
                    facts(
                        fact(term: "Clicks", attrs: attributes! { data-search-clicks=(figures.clicks) }, (count(figures.clicks)))
                        fact(term: "Impressions", attrs: attributes! { data-search-impressions=(figures.impressions) }, (count(figures.impressions)))
                        fact(term: "Click rate", attrs: attributes! { data-search-rate="" }, (click_rate(figures)))
                        fact(term: "Average position", attrs: attributes! { data-search-position="" }, (position(figures)))
                        fact(term: "Days with numbers", attrs: attributes! { data-search-days=(days_held) }, (days_held) " of " (span))
                    )
                )

                section_card(
                    title: Some("Queries"),
                    description: Some(&queries_note),
                    table(
                        attrs: attributes! { data-search-queries="" },
                        table_header(
                            table_row(
                                table_head("Query")
                                table_head(attrs: attributes! { class="text-right" }, "Clicks")
                                table_head(attrs: attributes! { class="text-right" }, "Impressions")
                                table_head(attrs: attributes! { class="text-right" }, "Click rate")
                                table_head(attrs: attributes! { class="text-right" }, "Position")
                            )
                        )
                        table_body(
                            if queries.is_empty() {
                                empty_row(colspan: 5, text: "No query told for these days.")
                            }
                            for row in &queries {
                                table_row(
                                    attrs: attributes! { data-search-query=(&row.key) },
                                    table_cell((&row.shown))
                                    table_cell(attrs: attributes! { class="text-right tabular-nums" data-clicks=(row.clicks) }, (count(row.clicks)))
                                    table_cell(attrs: attributes! { class="text-right tabular-nums" }, (&row.impressions))
                                    table_cell(attrs: attributes! { class="text-right tabular-nums" }, (&row.click_rate))
                                    table_cell(attrs: attributes! { class="text-right tabular-nums" data-position="" }, (&row.position))
                                )
                            }
                        )
                    )
                )

                section_card(
                    title: Some("Pages"),
                    description: Some(&pages_note),
                    table(
                        attrs: attributes! { data-search-pages="" },
                        table_header(
                            table_row(
                                table_head("Page")
                                table_head("What it is")
                                table_head(attrs: attributes! { class="text-right" }, "Clicks")
                                table_head(attrs: attributes! { class="text-right" }, "Impressions")
                                table_head(attrs: attributes! { class="text-right" }, "Click rate")
                                table_head(attrs: attributes! { class="text-right" }, "Position")
                            )
                        )
                        table_body(
                            if found.is_empty() {
                                empty_row(colspan: 6, text: "No page told for these days.")
                            }
                            for row in &found {
                                table_row(
                                    attrs: attributes! { data-search-page=(&row.shown) },
                                    table_cell(<a href=(&row.key) class=(LINK) rel="noreferrer">(&row.shown)</a>)
                                    table_cell(
                                        attrs: attributes! { data-search-kind=(row.kind_key) },
                                        (row.kind)
                                        if !row.name.is_empty() { ": " (&row.name) }
                                    )
                                    table_cell(attrs: attributes! { class="text-right tabular-nums" data-clicks=(row.clicks) }, (count(row.clicks)))
                                    table_cell(attrs: attributes! { class="text-right tabular-nums" }, (&row.impressions))
                                    table_cell(attrs: attributes! { class="text-right tabular-nums" }, (&row.click_rate))
                                    table_cell(attrs: attributes! { class="text-right tabular-nums" }, (&row.position))
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
    fn a_span_is_one_of_those_offered() {
        assert_eq!(span_of(Some("7")), 7);
        assert_eq!(span_of(Some("90")), 90);
        assert_eq!(span_of(Some("365")), DEFAULT_SPAN);
        assert_eq!(span_of(Some("x")), DEFAULT_SPAN);
        assert_eq!(span_of(None), DEFAULT_SPAN);
    }

    #[test]
    fn figures_read_as_a_person_writes_them() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(12_480), "12\u{202f}480");
        assert_eq!(count(1_234_567), "1\u{202f}234\u{202f}567");
        let figures = SearchFigures {
            clicks: 26,
            impressions: 400,
            weighted_position_milli: 1_000_000,
        };
        assert_eq!(click_rate(&figures), "6.5 %");
        assert_eq!(position(&figures), "2.5");
        let none = SearchFigures::default();
        assert_eq!(click_rate(&none), "None");
        assert_eq!(position(&none), "None");
    }
}
