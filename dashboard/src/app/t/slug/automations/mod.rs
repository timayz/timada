//! `/t/{slug}/automations`: the scenarios a shop's team draws, for members
//! with `automations:read`. A scenario is a trigger (something that happens
//! in the shop) followed by steps, drawn on its own page
//! ([`scenario_id`]); members with `automations:manage` start one from
//! [`new`]. The list comes from the shared read model (a moment behind) and
//! leaves the deleted scenarios out unless asked for.

pub mod labels;
pub mod new;
pub mod scenario_id;

use timada_automations::{
    Command, ScenarioFilter, ScenarioStatus, count_scenarios, scenarios_of_tenant,
};
use timada_tenancy::{PERMISSION_AUTOMATIONS_MANAGE, PERMISSION_AUTOMATIONS_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use self::labels::{status_label, status_tone, trigger_label};
use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::ReadPool;
use crate::ui::{
    LINK, Shell, Tone, document, empty_row, filter_bar, format_time, icons, notice, page_header,
    pager, section_card, status_badge,
};

const PAGE_SIZE: u32 = 50;
/// The `status` of the list showing every scenario not deleted.
const CURRENT: &str = "current";

#[topcoat::router::query_params(error = bad_request)]
struct AutomationsQuery {
    /// `current` (the default: every status but deleted), or a status.
    status: Option<String>,
    /// 1-based.
    page: Option<u32>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct AutomationsLink<'a> {
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<u32>,
}

/// The automations page, with a flash error code or message.
pub fn automations_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The automations gateway in the tenant's streams (403 while suspended).
pub fn automations(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command::new(access.executor()?.clone(), access.tenant_id()))
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_AUTOMATIONS_READ)
        .await?;
    let query = query_params::<AutomationsQuery>(cx)?;
    let status = query.status.as_deref().and_then(ScenarioStatus::parse);
    let page_number = query.page.unwrap_or(1).max(1);
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_AUTOMATIONS_MANAGE).await?;
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<ReadPool>(cx);
    let filter = ScenarioFilter { status };
    let total = count_scenarios(pool, &tenant.id, &filter)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let rows = scenarios_of_tenant(
        pool,
        &tenant.id,
        &filter,
        PAGE_SIZE,
        (page_number - 1) * PAGE_SIZE,
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    let pages = total.div_ceil(u64::from(PAGE_SIZE)) as u32;
    let status_value = status.map_or(CURRENT, ScenarioStatus::as_str);
    let slug = tenant.slug.clone();
    let page_link = {
        let slug = slug.clone();
        move |page: u32| {
            href!(index, Slug(slug.clone()))
                .query(AutomationsLink {
                    status: status_value,
                    page: Some(page),
                })
                .resolve(cx)
        }
    };
    let title = format!("Automations · {}", tenant.name);
    let new_url = href!(new::new, Slug(slug.clone())).resolve(cx);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "automations"),
            page_header(
                title: "Automations",
                subtitle: Some("Scenarios that act on their own when something happens in the shop."),
                if can_manage {
                    <a class=(button_variants(ButtonVariant::Primary, ButtonSize::Sm)) href=(&new_url)>"New scenario"</a>
                }
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                description: Some("Each scenario starts when something happens in the shop (an order placed, a review submitted, a message arriving) and goes through its steps: waiting, checking a condition, telling the team. A scenario runs only once published; until then it is a draft."),
                filter_bar(
                    <label class="sr-only" for="automations-status">"Status"</label>
                    select(
                        attrs: attributes! { id="automations-status" name="status" class="w-56" },
                        <option value=(CURRENT) selected=(status.is_none())>"Every scenario"</option>
                        for option in ScenarioStatus::ALL {
                            <option value=(option.as_str()) selected=(status == Some(option))>(status_label(option))</option>
                        }
                    )
                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Filter")
                )
                <p class="mb-4 text-sm text-muted-foreground" data-scenario-count=(total)>(total) " scenario(s)"</p>
                table(
                    table_header(
                        table_row(
                            table_head("Name")
                            table_head("Trigger")
                            table_head("Steps")
                            table_head("Status")
                            table_head("Changed")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 5, text: "No scenario here.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-scenario=(&row.name) data-scenario-status=(&row.status) },
                                table_cell(
                                    <a class=(LINK) href=(href!(scenario_id::index, Slug(slug.clone()), scenario_id::ScenarioId(row.id.clone())).resolve(cx))>(&row.name)</a>
                                )
                                table_cell(
                                    if let Some(trigger) = &row.trigger {
                                        (trigger_label(trigger))
                                    } else {
                                        <span class="text-muted-foreground">"Not chosen yet"</span>
                                    }
                                )
                                table_cell((row.steps))
                                table_cell(
                                    <div class="flex flex-wrap items-center gap-2">
                                        status_badge(tone: status_tone(row.status()), (status_label(row.status())))
                                        if row.version > 0 && row.unpublished {
                                            status_badge(tone: Tone::Warning, "Unpublished changes")
                                        }
                                    </div>
                                )
                                table_cell(<time datetime=(row.updated_at)>(format_time(row.updated_at as u64))</time>)
                            )
                        }
                    )
                )
                if rows.is_empty() && status.is_none() && can_manage {
                    <p class="mt-4 flex items-center gap-2 text-sm text-muted-foreground">
                        icon(data: icons::AUTOMATIONS)
                        "Start with " <a class=(LINK) href=(&new_url)>"a new scenario"</a> ": name it, then drag its blocks into place."
                    </p>
                }
                pager(
                    page: i64::from(page_number),
                    pages: i64::from(pages),
                    previous: (page_number > 1).then(|| page_link(page_number - 1)),
                    next: (page_number < pages).then(|| page_link(page_number + 1)),
                )
            )
        )
    })
}
