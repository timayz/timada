//! `/research/{study_id}`: one study, for its owner: its seeds and where
//! the fetch of each stands, and the keywords the fetches gave, the most
//! searched first. Where a fetch stands is read from the study's stream;
//! the keywords come from the shared read model, a moment behind.

pub mod delete;
pub mod fetch;
pub mod rename;
pub mod seeds;

use timada_core::page::{Cursor, PageFrom};
use timada_research::{
    Command, FetchStatus, MAX_SEEDS, ResearchError, SeedState, SpendCap, StudyState,
    keywords_of_study, spend_day, spend_day_ends,
};
use topcoat::{
    Result,
    context::{Cx, context},
    icon::icon,
    router::{error::not_found, href, module_param, page, query_params},
    view::{View, attributes, view},
};

use super::research;
use crate::auth::require_user;
use crate::cluster::LocalReads;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::input::input;
use crate::components::pagination::{
    pagination, pagination_content, pagination_item, pagination_next, pagination_previous,
};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::config::Config;
use crate::db::Db;
use crate::keywords::Keywords;
use crate::research::{Spending, cap_reached, format_usd};
use crate::ui::{
    Shell, Tone, document, empty_row, format_date, icons, notice, page_header, section_card,
    status_badge,
};

module_param!(pub study_id);

/// The page shows this many keywords at once.
const PAGE_SIZE: i64 = 100;

#[topcoat::router::query_params(error = bad_request)]
struct StudyQuery {
    /// The cursor of the row the page starts after (less searched keywords).
    after: Option<String>,
    /// The cursor of the row the page ends before (more searched keywords).
    before: Option<String>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct StudyLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
}

pub fn study_url(cx: &Cx, study_id: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, study_id: study_id.to_owned())
        .query(StudyLink {
            after: None,
            before: None,
            error,
            message,
        })
        .resolve(cx)
}

/// The study in the URL and the signed-in user, who must own it (else 404:
/// a study is nobody else's business).
pub(crate) async fn require_owned(cx: &Cx) -> Result<(StudyState, String)> {
    let user = require_user(cx).await?;
    match research(cx).require_owned(study_id(cx), &user.id).await {
        Ok(study) => Ok((study, user.id.clone())),
        Err(ResearchError::NotFound) => Err(not_found().into()),
        Err(err) => Err(err.into()),
    }
}

/// A seed's row, as the page words it.
struct SeedLine {
    seed: String,
    /// `fetching`, `fetched`, `failed`, `stalled` or `never`: what a test
    /// reads.
    state: &'static str,
    label: &'static str,
    tone: Tone,
    keywords: u32,
    /// The day of the last fetch that gave its rows.
    fetched: Option<String>,
    error: Option<String>,
    /// What the button that asks for its keywords says; none while they are
    /// on their way, or so recent that asking again would give them back.
    action: Option<&'static str>,
}

impl SeedLine {
    /// `reuse_secs`: for how long a seed's keywords answer a new fetch of
    /// it.
    fn of(seed: &SeedState, now: u64, reuse_secs: u64) -> Self {
        let fresh = seed
            .fetched_at
            .is_some_and(|at| now.saturating_sub(at) < reuse_secs);
        let (state, label, tone, action) = if seed.is_fetching(now) {
            ("fetching", "Fetching…", Tone::Info, None)
        } else {
            match seed.status {
                FetchStatus::Fetched => (
                    "fetched",
                    "Fetched",
                    Tone::Success,
                    (!fresh).then_some("Fetch again"),
                ),
                FetchStatus::Failed => ("failed", "Failed", Tone::Danger, Some("Try again")),
                FetchStatus::Never => ("never", "Not fetched", Tone::Neutral, Some("Fetch")),
                // Asked for long ago and never answered: the node that took
                // it on stopped.
                FetchStatus::Requested | FetchStatus::Fetching => {
                    ("stalled", "No answer", Tone::Warning, Some("Try again"))
                }
            }
        };
        Self {
            seed: seed.seed.clone(),
            state,
            label,
            tone,
            keywords: seed.keywords,
            fetched: seed.fetched_at.map(format_date),
            error: seed.last_error.clone(),
            action,
        }
    }
}

/// A figure the source may have nothing for: never shown as a zero.
fn figure(value: Option<i64>) -> String {
    value.map_or_else(|| "–".to_owned(), |value| value.to_string())
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let (study, user_id) = require_owned(cx).await?;
    let query = query_params::<StudyQuery>(cx)?;
    let after = query.after.as_deref().and_then(Cursor::parse);
    let before = query.before.as_deref().and_then(Cursor::parse);
    let listed = keywords_of_study(
        context::<Db>(cx),
        &study.id,
        PageFrom::of(after.as_ref(), before.as_ref()),
        PAGE_SIZE,
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    let source = &context::<Keywords>(cx).0;
    let enabled = source.is_some();
    let now = crate::auth::now();
    let config = &context::<Config>(cx).research;
    let spending = Spending::new(config);
    // What the day's fetches cost the user so far, and the cap the next one
    // would pass: read from this node's file, as the saga that decides
    // reads it again through the cluster.
    let (spent, capped) = match source {
        Some(source) => {
            let day = Command(context::<LocalReads>(cx).0.clone())
                .load_spend_day(&spend_day(now))
                .await
                .map_err(topcoat::Error::from_anyhow)?;
            let capped = day.refusal(spending.caps, &user_id, source.most_usd_micros(), now);
            (Some(day.charged_to(&user_id)), capped)
        }
        None => (None, None),
    };
    let capped_text = capped.map(|cap| {
        let reused = match config.reuse_days {
            0 => String::new(),
            1 => " A seed fetched in the last day is still answered.".to_owned(),
            days => format!(" A seed fetched in the last {days} days is still answered."),
        };
        format!("{}{reused}", cap_reached(cap, spend_day_ends(now)))
    });
    let capped_by = capped.map(|cap| match cap {
        SpendCap::User => "user",
        SpendCap::Platform => "platform",
    });
    let spent_text = spent.map(|spent| {
        format!(
            "Your fetches cost {} today (UTC), of the {} a day may cost.",
            format_usd(spent),
            format_usd(spending.caps.user_usd_micros)
        )
    });
    let seeds_description = match config.reuse_days {
        0 => "Each seed is fetched with the keywords that contain its words, up to 1,000 of them. A fetch is one paid call to the keyword source, and replaces what an earlier fetch of the seed gave.".to_owned(),
        days => format!("Each seed is fetched with the keywords that contain its words, up to 1,000 of them. A fetch is one paid call to the keyword source, unless the seed was fetched in the last {days} day{} for the same country and language, by you or anyone: its keywords are then those, at no cost.", if days == 1 { "" } else { "s" }),
    };
    let id = study.id.clone();
    let page_link = |after: Option<&str>, before: Option<&str>| {
        href!(index, study_id: id.clone())
            .query(StudyLink {
                after,
                before,
                error: None,
                message: None,
            })
            .resolve(cx)
    };
    let more_searched = listed
        .newer
        .as_ref()
        .map(|cursor| page_link(None, Some(&cursor.param())));
    let less_searched = listed
        .older
        .as_ref()
        .map(|cursor| page_link(Some(&cursor.param()), None));
    let paged = more_searched.is_some() || less_searched.is_some();
    let rows = listed.rows;
    let seeds: Vec<SeedLine> = study
        .seeds
        .iter()
        .map(|seed| SeedLine::of(seed, now, spending.reuse_secs))
        .collect();
    let fetching = seeds.iter().any(|seed| seed.state == "fetching");
    let owed = study.seeds.iter().filter(|seed| seed.is_owed(now)).count();
    let fetch_owed = match owed {
        1 => "Fetch the keywords of 1 seed".to_owned(),
        n => format!("Fetch the keywords of {n} seeds"),
    };
    let can_add = study.seeds.len() < MAX_SEEDS;
    let total = study.keywords();
    let keywords_title = match total {
        1 => "1 keyword".to_owned(),
        n => format!("{n} keywords"),
    };
    let title = format!("{} · Research", study.name);
    let place = format!("Searched in {} · {}", study.country, study.language);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::page("research"),
            page_header(
                title: &study.name,
                subtitle: Some(&place),
                h1_attrs: attributes! { data-study-name=(&study.name) },
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            if !enabled {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" class="mb-4" data-research-disabled="" },
                    icon(data: icons::WARNING)
                    alert_description("Keyword research is not set up on this platform: the operator has to give it a keyword source before keywords can be fetched.")
                )
            }
            if let (Some(text), Some(by)) = (&capped_text, capped_by) {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" class="mb-4" data-spend-capped=(by) },
                    icon(data: icons::WARNING)
                    alert_description((text))
                )
            }
            if fetching {
                alert(
                    variant: AlertVariant::Info,
                    attrs: attributes! { class="mb-4" data-study-fetching="" },
                    icon(data: icons::INFO)
                    alert_description(
                        "Keywords are being fetched, which takes a few seconds. "
                        <a class="font-medium underline underline-offset-4" href=(href!(index, study_id: id.clone()))>"Look again"</a>
                    )
                )
            }

            section_card(
                title: Some("Seed keywords"),
                description: Some(&seeds_description),
                table(
                    table_header(
                        table_row(
                            table_head("Seed")
                            table_head("Keywords")
                            table_head("Fetched")
                            table_head("Status")
                            table_head(<span class="sr-only">"Fetch"</span>)
                        )
                    )
                    table_body(
                        for seed in &seeds {
                            table_row(
                                attrs: attributes! { data-seed=(&seed.seed) data-seed-status=(seed.state) },
                                table_cell(attrs: attributes! { class="font-medium" }, (&seed.seed))
                                table_cell(attrs: attributes! { class="tabular-nums" data-seed-keywords=(seed.keywords) },
                                    if seed.fetched.is_some() { (seed.keywords) } else { "–" }
                                )
                                table_cell(
                                    if let Some(day) = &seed.fetched {
                                        <time datetime=(day)>(day)</time>
                                    } else {
                                        <span class="text-muted-foreground">"Never"</span>
                                    }
                                )
                                table_cell(
                                    status_badge(tone: seed.tone, (seed.label))
                                    if let Some(error) = &seed.error {
                                        <p class="mt-1 max-w-md text-sm whitespace-normal text-destructive-text" data-seed-error="">(error)</p>
                                    }
                                )
                                table_cell(
                                    attrs: attributes! { class="text-right" },
                                    if let Some(action) = seed.action.filter(|_| enabled) {
                                        <form method="post" action=(href!(fetch::request, study_id: id.clone()))>
                                            <input type="hidden" name="seed" value=(&seed.seed)>
                                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, (action))
                                        </form>
                                    }
                                )
                            )
                        }
                    )
                )
                if let (Some(text), Some(spent)) = (&spent_text, spent) {
                    <p class="mt-3 text-sm text-muted-foreground" data-spent-today=(spent)>(text)</p>
                }
                <div class="mt-4 flex flex-wrap items-start justify-between gap-3">
                    if can_add {
                        <form method="post" action=(href!(seeds::add_seed, study_id: id.clone())) class="flex flex-wrap gap-2">
                            <label class="sr-only" for="new-seed">"A seed keyword to add"</label>
                            input(attrs: attributes! { id="new-seed" name="seed" required="" maxlength="80" placeholder="Another seed keyword" class="sm:w-72" })
                            button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, icon(data: icons::PLUS) "Add the seed")
                        </form>
                    }
                    if enabled && owed > 0 {
                        <form method="post" action=(href!(fetch::request, study_id: id.clone()))>
                            button(attrs: attributes! { type="submit" data-fetch-owed=(owed) }, (&fetch_owed))
                        </form>
                    }
                </div>
            )

            section_card(
                title: Some(&keywords_title),
                description: Some("As the last fetch of each seed gave them, the most searched first. A figure the source has nothing for stays empty. The cost per click is in US dollars whatever the country."),
                attrs: attributes! { id="keywords" data-study-keywords=(total) },
                table(
                    table_header(
                        table_row(
                            table_head("Keyword")
                            table_head("Searches a month")
                            table_head("Cost per click")
                            table_head("Difficulty")
                            table_head("Intent")
                            table_head("Seed")
                            table_head("Fetched")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 7, text: "No keyword yet: fetch the keywords of a seed.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-keyword=(&row.keyword) },
                                table_cell(attrs: attributes! { class="font-medium" }, (&row.keyword))
                                table_cell(attrs: attributes! { class="tabular-nums" data-volume="" }, (figure(row.volume)))
                                table_cell(attrs: attributes! { class="tabular-nums" data-cpc="" },
                                    if let Some(cents) = row.cpc_usd_cents {
                                        "$" (timada_core::format_amount(cents as u64))
                                    } else {
                                        "–"
                                    }
                                )
                                table_cell(attrs: attributes! { class="tabular-nums" data-difficulty="" }, (figure(row.difficulty)))
                                table_cell(attrs: attributes! { data-intent="" }, (row.intent.as_deref().unwrap_or("–")))
                                table_cell(attrs: attributes! { class="text-muted-foreground" }, (&row.seed))
                                table_cell(<time datetime=(format_date(row.fetched_at as u64))>(format_date(row.fetched_at as u64))</time>)
                            )
                        }
                    )
                )
                if paged {
                    pagination(
                        attrs: attributes! { class="mt-4" },
                        pagination_content(
                            if let Some(more_searched) = more_searched {
                                pagination_item(pagination_previous(label: "More searched", attrs: attributes! { href=(more_searched) rel="prev" }))
                            }
                            if let Some(less_searched) = less_searched {
                                pagination_item(pagination_next(label: "Less searched", attrs: attributes! { href=(less_searched) rel="next" }))
                            }
                        )
                    )
                }
            )

            section_card(
                title: Some("The study"),
                <form method="post" action=(href!(rename::rename, study_id: id.clone())) class="flex flex-wrap gap-2">
                    <label class="sr-only" for="study-name">"Name"</label>
                    input(attrs: attributes! { id="study-name" name="name" required="" maxlength="80" value=(&study.name) class="sm:w-72" })
                    button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, "Rename")
                </form>
                <form method="post" action=(href!(delete::delete, study_id: id.clone())) class="mt-4">
                    <p class="mb-2 text-sm text-muted-foreground">"Deleting the study removes it and its keywords from your list for good."</p>
                    button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Delete the study")
                </form>
            )
        )
    })
}
