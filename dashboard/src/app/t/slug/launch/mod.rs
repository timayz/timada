//! `/t/{slug}/launch`: the shop's launch path, for members with
//! `orders:read`: every step between a shop with a name and a shop that is
//! advertised, in its order, done, to do or not checked, with where it is
//! put right. The steps the first campaign waits for are marked. A member
//! with `tenant:manage` says which of the steps timada cannot check hold,
//! and sets how large the catalog is planned (`crate::launch_path`).

pub mod confirm;
pub mod targets;

use timada_tenancy::{PERMISSION_ORDERS_READ, PERMISSION_TENANT_MANAGE};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::proposals::{name_of, names};
use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::app::auth::Flash;
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_label as label};
use crate::components::input::input;
use crate::db::Db;
use crate::launch_path::{Fix, State, path};
use crate::ui::chrome::section_url;
use crate::ui::{
    LINK, Shell, Tone, document, format_time, notice, page_header, section_card, status_badge,
};

#[topcoat::router::query_params(error = bad_request)]
struct LaunchQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The page, with a flash error code or message.
pub fn launch_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

/// A step as the page shows it.
struct Line {
    key: &'static str,
    label: &'static str,
    state: State,
    tone: Tone,
    detail: String,
    /// Where it is put right: what the link says, its address, and whether
    /// it leaves timada.
    fix: Option<(&'static str, String, bool)>,
    before_ads: bool,
    confirms: bool,
    /// Who said it holds, and when.
    word: Option<(String, u64)>,
}

/// The steps of one part of the path.
struct Part {
    name: &'static str,
    lines: Vec<Line>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let query = query_params::<LaunchQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_TENANT_MANAGE).await?;
    let failed = topcoat::Error::from_anyhow;
    let steps = path(access.executor()?, context::<Db>(cx), &access.tenant)
        .await
        .map_err(failed)?;
    let (per_category, products) =
        timada_launch::Command::new(access.executor()?.clone(), access.tenant_id())
            .load_launch()
            .await
            .map_err(failed)?
            .map(|launch| launch.path_targets())
            .unwrap_or((
                timada_launch::DEFAULT_PRODUCTS_PER_CATEGORY,
                timada_launch::DEFAULT_PRODUCTS,
            ));
    let names = names(
        cx,
        steps
            .iter()
            .filter_map(|step| Some(step.confirmed.as_ref()?.0.clone()))
            .collect(),
    )
    .await?;
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let done = steps
        .iter()
        .filter(|step| step.state == State::Done)
        .count();
    let all = steps.len();
    let waits = steps
        .iter()
        .filter(|step| step.before_ads && step.state != State::Done)
        .count();
    let mut parts: Vec<Part> = Vec::new();
    for step in steps {
        let fix = step.fix.map(|(says, fix)| match fix {
            Fix::Section(key) => (says, section_url(cx, &slug, key), false),
            Fix::Study(Some(id)) => (
                says,
                href!(
                    crate::app::research::study_id::index,
                    crate::app::research::study_id::study_id: id
                )
                .resolve(cx),
                false,
            ),
            Fix::Study(None) => (says, href!(crate::app::research::index).resolve(cx), false),
            Fix::Campaigns => (
                says,
                href!(super::ads::campaigns::index, slug: slug.clone()).resolve(cx),
                false,
            ),
            Fix::Outside(address) => (says, address, true),
        });
        let line = Line {
            key: step.key,
            label: step.label,
            state: step.state,
            tone: match step.state {
                State::Done => Tone::Success,
                State::ToDo => Tone::Warning,
                State::NotChecked => Tone::Neutral,
            },
            detail: step.detail,
            fix,
            before_ads: step.before_ads,
            confirms: step.confirms,
            word: step.confirmed.map(|(by, at)| (name_of(&names, &by), at)),
        };
        match parts.last_mut() {
            Some(part) if part.name == step.group => part.lines.push(line),
            _ => parts.push(Part {
                name: step.group,
                lines: vec![line],
            }),
        }
    }
    let title = format!("Launch path · {}", tenant.name);
    let confirm_url = href!(confirm::confirm, slug: slug.clone()).resolve(cx);
    let targets_url = href!(targets::targets, slug: slug.clone()).resolve(cx);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "launch"),
            page_header(
                title: "Launch path",
                subtitle: Some("What to get right between a shop with a name and a shop that is advertised, in its order. Each step reads what timada already knows, and asks you only where it cannot."),
                status_badge(
                    tone: if done == all { Tone::Success } else { Tone::Neutral },
                    attrs: attributes! { data-launch-done=(done) data-launch-steps=(all) },
                    (done) " of " (all) " done"
                )
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if waits > 0 {
                <p class="mb-6 text-sm text-muted-foreground" data-launch-waits=(waits)>
                    (waits) " of the steps marked « before ads » do not hold: the first campaign is not prepared until they do."
                </p>
            }

            for part in &parts {
                section_card(
                    title: Some(part.name),
                    <ul class="flex flex-col gap-5">
                        for line in &part.lines {
                            <li class="flex flex-col gap-1" data-launch-step=(line.key) data-launch-state=(line.state.key())>
                                <div class="flex flex-wrap items-center gap-2">
                                    status_badge(tone: line.tone, (line.state.label()))
                                    <span class="font-medium">(line.label)</span>
                                    if line.before_ads {
                                        badge(variant: BadgeVariant::Outline, attrs: attributes! { data-launch-before-ads="" }, "Before ads")
                                    }
                                </div>
                                <p class="text-sm text-muted-foreground" data-launch-detail="">(&line.detail)</p>
                                if let Some((by, at)) = &line.word {
                                    <p class="text-sm text-muted-foreground" data-launch-word="">
                                        "Said by " (by) " · " <time datetime=(at)>(format_time(*at))</time>
                                    </p>
                                }
                                <div class="flex flex-wrap items-center gap-4">
                                    if let Some((says, address, outside)) = &line.fix {
                                        if *outside {
                                            <a class=(LINK) href=(address) rel="noopener noreferrer" target="_blank">(says)</a>
                                        } else {
                                            <a class=(LINK) href=(address)>(says)</a>
                                        }
                                    }
                                    if line.confirms && can_manage && line.state != State::ToDo {
                                        <form method="post" action=(&confirm_url)>
                                            <input type="hidden" name="step" value=(line.key) />
                                            if line.word.is_some() {
                                                button(variant: ButtonVariant::Ghost, size: ButtonSize::Sm, attrs: attributes! { type="submit" name="holds" value="no" }, "Take it back")
                                            } else {
                                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" name="holds" value="yes" }, "I checked: it holds")
                                            }
                                        </form>
                                    }
                                </div>
                            </li>
                        }
                    </ul>
                )
            }

            section_card(
                title: Some("How large the catalog is planned"),
                description: Some("What the path holds the categories and the catalog to."),
                <form method="post" action=(&targets_url) data-launch-targets="">
                    <fieldset disabled=(!can_manage) class="flex max-w-xl flex-col gap-4">
                        field(
                            label(attrs: attributes! { for="launch-per-category" }, "Products in every category at the bottom of the tree")
                            input(attrs: attributes! { id="launch-per-category" name="per_category" type="number" min="1" max="100000" required="" value=(per_category) class="w-32" })
                        )
                        field(
                            label(attrs: attributes! { for="launch-products" }, "Products in the shop")
                            input(attrs: attributes! { id="launch-products" name="products" type="number" min="1" max="100000" required="" value=(products) class="w-32" })
                        )
                        if can_manage {
                            <div>button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, "Save")</div>
                        }
                    </fieldset>
                </form>
            )
        )
    })
}
