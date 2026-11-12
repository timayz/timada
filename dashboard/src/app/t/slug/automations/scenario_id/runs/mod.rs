//! `/t/{slug}/automations/{scenario_id}/runs`: what a published scenario
//! did, run by run, for members with `automations:read`; newest first, from
//! the shared read model (a moment behind), a page at a time from where the
//! page beside it stopped. Runs that ended leave the list after 90 days.

pub mod run_id;

use std::collections::HashMap;

use timada_automations::{
    Definition, RunFilter, RunStatus, catalog, count_runs, published_definition, runs_of_scenario,
};
use timada_core::page::{Cursor, PageFrom};
use timada_tenancy::PERMISSION_AUTOMATIONS_READ;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::super::labels::{run_status_label, run_status_tone, step_label, subject_kind};
use super::{ScenarioId, require_scenario, scenario_url};
use crate::app::t::slug::{Slug, TenantNav, require_tenant};
use crate::automations::runner::subject_link;
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{
    LINK, Shell, cursor_pager, document, empty_row, filter_bar, format_time, notice, page_header,
    section_card, status_badge,
};

const PAGE_SIZE: i64 = 50;
const ALL: &str = "all";

#[topcoat::router::query_params(error = bad_request)]
struct RunsQuery {
    status: Option<String>,
    /// The cursor of the row the page starts after (older runs).
    after: Option<String>,
    /// The cursor of the row the page ends before (newer runs).
    before: Option<String>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct RunsLink<'a> {
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<&'a str>,
}

/// One row as the page shows it.
struct Row {
    href: String,
    started_at: i64,
    about: &'static str,
    subject: String,
    status: RunStatus,
    step: String,
    reason: String,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_AUTOMATIONS_READ)
        .await?;
    let query = query_params::<RunsQuery>(cx)?;
    let scenario = require_scenario(&access, cx).await?;
    let status = query.status.as_deref().and_then(RunStatus::parse);
    let after = query.after.as_deref().and_then(Cursor::parse);
    let before = query.before.as_deref().and_then(Cursor::parse);
    let from = PageFrom::of(after.as_ref(), before.as_ref());
    let nav = TenantNav::of(cx, &access).await?;
    let tenant = access.tenant;
    let pool = app_context::<Db>(cx);
    let filter = RunFilter { status };
    let total = count_runs(pool, &tenant.id, &scenario.id, &filter)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let listed = runs_of_scenario(pool, &tenant.id, &scenario.id, &filter, from, PAGE_SIZE)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let found = listed.rows;
    // The versions the runs listed started on, each read once; one this
    // build cannot read names no step.
    let mut definitions: HashMap<i64, Option<Definition>> = HashMap::new();
    for row in &found {
        if definitions.contains_key(&row.scenario_version) {
            continue;
        }
        let definition =
            published_definition(pool, &tenant.id, &scenario.id, row.scenario_version as u32)
                .await
                .map_err(topcoat::Error::from_anyhow)?
                .and_then(|json| Definition::from_json(&json).ok());
        definitions.insert(row.scenario_version, definition);
    }
    let slug = tenant.slug.clone();
    let rows: Vec<Row> = found
        .iter()
        .map(|row| {
            let subject = catalog::trigger_kind(&row.trigger).map(|kind| kind.subject);
            let definition = definitions
                .get(&row.scenario_version)
                .and_then(Option::as_ref);
            let step = row
                .step
                .as_deref()
                .and_then(|id| definition?.find(id).map(|step| step_label(&step.kind)))
                .unwrap_or("—");
            Row {
                href: href!(
                    run_id::index,
                    Slug(slug.clone()),
                    ScenarioId(scenario.id.clone()),
                    run_id::RunId(row.id.clone())
                )
                .resolve(cx),
                started_at: row.started_at,
                about: subject.map_or("—", subject_kind),
                subject: subject.map_or_else(String::new, |subject| {
                    subject_link("", &slug, subject, &row.subject_id)
                }),
                status: row.status(),
                step: step.to_owned(),
                reason: row.reason.clone().unwrap_or_default(),
            }
        })
        .collect();
    let status_value = status.map_or(ALL, RunStatus::as_str);
    let page_link = {
        let slug = slug.clone();
        let id = scenario.id.clone();
        move |after: Option<&str>, before: Option<&str>| {
            href!(index, Slug(slug.clone()), ScenarioId(id.clone()))
                .query(RunsLink {
                    status: status_value,
                    after,
                    before,
                })
                .resolve(cx)
        }
    };
    let newer = listed
        .newer
        .map(|cursor| page_link(None, Some(&cursor.param())));
    let older = listed
        .older
        .map(|cursor| page_link(Some(&cursor.param()), None));
    let summary = match total {
        1 => "1 run".to_owned(),
        n => format!("{n} runs"),
    };
    let title = format!("Runs · {} · Automations · {}", scenario.name, tenant.name);
    let back = scenario_url(cx, &slug, &scenario.id, None, None);
    let subtitle = format!("What “{}” did, run by run.", scenario.name);
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "automations"),
            page_header(
                title: "Runs",
                subtitle: Some(subtitle.as_str()),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to the scenario"</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            section_card(
                description: Some("Each run is one event that started the scenario. A run that waits shows when it goes on; one that failed says why."),
                filter_bar(
                    <label class="sr-only" for="runs-status">"Status"</label>
                    select(
                        attrs: attributes! { id="runs-status" name="status" class="w-48" },
                        <option value=(ALL) selected=(status.is_none())>"Every run"</option>
                        for option in RunStatus::ALL {
                            <option value=(option.as_str()) selected=(status == Some(option))>(run_status_label(option))</option>
                        }
                    )
                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Filter")
                )
                <p class="mb-4 text-sm text-muted-foreground" data-run-count=(total)>(total) " run(s)"</p>
                table(
                    table_header(
                        table_row(
                            table_head("Started")
                            table_head("About")
                            table_head("Status")
                            table_head("At")
                            table_head("Note")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 5, text: "No run here.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-run=(row.status.as_str()) },
                                table_cell(<a class=(LINK) href=(&row.href)><time datetime=(row.started_at)>(format_time(row.started_at as u64))</time></a>)
                                table_cell(
                                    if row.subject.is_empty() {
                                        (row.about)
                                    } else {
                                        <a class=(LINK) href=(&row.subject)>(row.about)</a>
                                    }
                                )
                                table_cell(status_badge(tone: run_status_tone(row.status), (run_status_label(row.status))))
                                table_cell((&row.step))
                                table_cell(<span class="text-sm text-muted-foreground">(&row.reason)</span>)
                            )
                        }
                    )
                )
                cursor_pager(summary: &summary, newer: newer, older: older)
            )
        )
    })
}
