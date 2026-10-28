//! `/t/{slug}/automations/{scenario_id}/runs/{run_id}`: one run, step by
//! step, from its own stream (so it is never behind), against the version
//! of the scenario it started on. Members with `automations:manage` cancel
//! a run under way ([`cancel`]).

pub mod cancel;

use timada_automations::{RunStatus, catalog};
use timada_tenancy::{PERMISSION_AUTOMATIONS_MANAGE, PERMISSION_AUTOMATIONS_READ};
use topcoat::{
    Result,
    context::Cx,
    router::{error::not_found, href, module_param, page, path_param, query_params},
    view::{View, attributes, view},
};

use super::super::super::automations;
use super::super::super::labels::{
    outcome_label, run_status_label, run_status_tone, step_label, subject_kind, trigger_label,
};
use super::super::{ScenarioId, require_scenario};
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, TenantNav, require_tenant};
use crate::automations::runner::subject_link;
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::ui::{
    LINK, Shell, document, fact, facts, format_time, notice, page_header, section_card,
    status_badge,
};

module_param!(pub run_id);

#[topcoat::router::query_params(error = bad_request)]
struct RunQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn run_url(
    cx: &Cx,
    slug: &str,
    scenario_id: &str,
    run_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(
        index,
        Slug(slug.to_owned()),
        ScenarioId(scenario_id.to_owned()),
        RunId(run_id.to_owned())
    )
    .query(Flash { error, message })
    .resolve(cx)
}

/// One line of the timeline.
struct Line {
    at: u64,
    step: String,
    outcome: String,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_AUTOMATIONS_READ)
        .await?;
    let query = query_params::<RunQuery>(cx)?;
    let scenario = require_scenario(&access, cx).await?;
    let id = path_param::<RunId>(cx).to_owned();
    let run = automations(&access)?
        .load_run(&id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|run| run.scenario_id == scenario.id)
        .ok_or_else(not_found)?;
    let can_manage = access.can(cx, PERMISSION_AUTOMATIONS_MANAGE).await?;
    let nav = TenantNav::of(cx, &access).await?;
    let tenant = access.tenant;
    let definition = scenario
        .version_definition(run.scenario_version)
        .map_err(topcoat::Error::from_anyhow)?;
    let lines: Vec<Line> = run
        .log
        .iter()
        .map(|record| {
            let step = definition
                .as_ref()
                .and_then(|definition| definition.find(&record.step));
            let kind = step.map_or("", |step| step.kind.as_str());
            Line {
                at: record.at,
                step: step
                    .map_or("A removed block", |step| step_label(&step.kind))
                    .to_owned(),
                outcome: outcome_label(&record.outcome, kind),
            }
        })
        .collect();
    let subject = catalog::trigger_kind(&run.trigger).map(|kind| kind.subject);
    let subject_href =
        subject.map(|subject| subject_link("", &tenant.slug, subject, &run.subject_id));
    let title = format!("Run · {} · Automations · {}", scenario.name, tenant.name);
    let back = href!(
        super::index,
        Slug(tenant.slug.clone()),
        ScenarioId(scenario.id.clone())
    )
    .resolve(cx);
    let cancel = href!(
        cancel::cancel,
        Slug(tenant.slug.clone()),
        ScenarioId(scenario.id.clone()),
        RunId(run.id.clone())
    )
    .resolve(cx);
    let under_way = !run.status.is_done();
    let status = run.status;
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "automations"),
            page_header(
                title: &scenario.name,
                subtitle: Some("One run, step by step."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to the runs"</a>
                if can_manage && under_way {
                    <form method="post" action=(cancel)>
                        button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Cancel the run")
                    </form>
                }
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            section_card(
                facts(
                    fact(term: "Status", status_badge(tone: run_status_tone(status), attrs: attributes! { data-run-status=(status.as_str()) }, (run_status_label(status))))
                    fact(term: "Started by", (trigger_label(&run.trigger)))
                    if let (Some(subject), Some(href)) = (subject, &subject_href) {
                        fact(term: "About", <a class=(LINK) href=(href)>(subject_kind(subject))</a>)
                    }
                    fact(term: "Version", (run.scenario_version))
                    fact(term: "Started", <time datetime=(run.started_at)>(format_time(run.started_at))</time>)
                    if let Some(at) = run.ended_at {
                        fact(term: "Ended", <time datetime=(at)>(format_time(at))</time>)
                    }
                    if let Some(reason) = &run.reason {
                        fact(term: "Why", (reason))
                    }
                )
            )
            section_card(
                title: Some("Steps"),
                if lines.is_empty() {
                    <p class="text-sm text-muted-foreground">"No step reached yet."</p>
                }
                <ol class="flex flex-col gap-3" data-run-log="">
                    for line in &lines {
                        <li class="flex flex-wrap items-baseline gap-x-3 gap-y-1 border-l-2 border-border pl-3">
                            <time class="text-xs text-muted-foreground" datetime=(line.at)>(format_time(line.at))</time>
                            <span class="text-sm font-medium">(&line.step)</span>
                            <span class="text-sm text-muted-foreground" data-run-outcome="">(&line.outcome)</span>
                        </li>
                    }
                </ol>
                if status == RunStatus::Finished {
                    <p class="mt-3 text-sm text-muted-foreground">"Every step reached was done."</p>
                }
            )
        )
    })
}
