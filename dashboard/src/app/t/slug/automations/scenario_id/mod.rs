//! `/t/{slug}/automations/{scenario_id}`: one scenario, drawn. Members with
//! `automations:manage` edit a working copy kept in the page ([`editor`]):
//! every button posts it back here and the page comes back with the change
//! made, until "Save draft" writes it. `editor.js` adds dragging on top and
//! sends the same buttons without leaving the page. Members who may only
//! read see the flow as it was last saved.

pub mod archive;
mod canvas;
mod editor;
mod inspector;
pub mod pause;
pub mod resume;
pub mod runs;

use timada_automations::{ScenarioState, ScenarioStatus};
use timada_tenancy::{PERMISSION_AUTOMATIONS_MANAGE, PERMISSION_AUTOMATIONS_READ};
use topcoat::{
    Result,
    asset::asset,
    context::Cx,
    router::{
        content::Form,
        error::{not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{View, attributes, component, view},
};

use self::canvas::{Canvas, editor_view};
use self::editor::{Commit, Editor, EditorForm};
use super::labels::status_label;
use super::{automations, automations_url};
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::dropdown_menu::{
    dropdown_menu, dropdown_menu_content, dropdown_menu_trigger,
};
use crate::components::floating::FloatingAlign;
use crate::ui::{Shell, document, notice, page_header};

module_param!(pub scenario_id);

#[topcoat::router::query_params(error = bad_request)]
struct ScenarioQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn scenario_url(
    cx: &Cx,
    slug: &str,
    scenario_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(
        index,
        Slug(slug.to_owned()),
        ScenarioId(scenario_id.to_owned())
    )
    .query(Flash { error, message })
    .resolve(cx)
}

/// The scenario the URL names, or 404.
pub(crate) async fn require_scenario(access: &TenantAccess, cx: &Cx) -> Result<ScenarioState> {
    let id = path_param::<ScenarioId>(cx);
    automations(access)?
        .load_scenario(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| not_found().into())
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_AUTOMATIONS_READ)
        .await?;
    let query = query_params::<ScenarioQuery>(cx)?;
    let scenario = require_scenario(&access, cx).await?;
    let manage = access.can(cx, PERMISSION_AUTOMATIONS_MANAGE).await?
        && scenario.status != ScenarioStatus::Archived;
    let editor = Editor::of(&scenario).map_err(topcoat::Error::from_anyhow)?;
    let nav = TenantNav::of(cx, &access).await?;
    let pages = Pages::of(cx, &access, &scenario);
    Ok(view! {
        scenario_page(
            access: &access,
            nav: &nav,
            scenario: &scenario,
            editor: &editor,
            manage: manage,
            pages: &pages,
            error: query.error.as_deref(),
            message: query.message.as_deref(),
        )
    })
}

/// Runs a button of the editor on the copy it posted. "Save draft" writes
/// the copy and opens the saved scenario again; any other button, or a save
/// refused, shows the page with the copy as it now is.
#[page(POST)]
pub async fn edit(cx: &Cx, Form(form): Form<EditorForm>) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_AUTOMATIONS_MANAGE)
        .await?;
    let scenario = require_scenario(&access, cx).await?;
    let slug = access.tenant.slug.clone();
    if scenario.status == ScenarioStatus::Archived {
        return Err(see_other(automations_url(
            cx,
            &slug,
            Some(timada_automations::SCENARIO_ARCHIVED),
            None,
        ))
        .into());
    }
    let op = form.op();
    let mut editor = form
        .into_editor(&scenario)
        .map_err(topcoat::Error::from_anyhow)?;
    let mut error = None;
    let commit = if editor.refusal.is_none() {
        editor.run(op)
    } else {
        None
    };
    if let Some(commit) = commit {
        let automations = automations(&access)?;
        let written = match commit {
            Commit::Save => automations
                .save_scenario(
                    &access.user_id,
                    &scenario.id,
                    editor.base,
                    &editor.name,
                    &editor.definition,
                )
                .await
                .map(|changed| {
                    if changed {
                        "Draft saved.".to_owned()
                    } else {
                        "Nothing changed since the last save.".to_owned()
                    }
                }),
            Commit::Publish => automations
                .publish_scenario(
                    &access.user_id,
                    &scenario.id,
                    editor.base,
                    &editor.name,
                    &editor.definition,
                )
                .await
                .map(|version| format!("Published as version {version}: it runs from now on.")),
        };
        match written {
            Ok(message) => {
                return Err(
                    see_other(scenario_url(cx, &slug, &scenario.id, None, Some(&message))).into(),
                );
            }
            Err(err) if err.is_user_facing() => error = err.code(),
            Err(err) => return Err(topcoat::Error::from_anyhow(err.into())),
        }
    }
    let nav = TenantNav::of(cx, &access).await?;
    let pages = Pages::of(cx, &access, &scenario);
    Ok(view! {
        scenario_page(
            access: &access,
            nav: &nav,
            scenario: &scenario,
            editor: &editor,
            manage: true,
            pages: &pages,
            error: error,
            message: None,
        )
    })
}

/// The addresses the page links and posts to.
struct Pages {
    edit: String,
    archive: String,
    pause: String,
    resume: String,
    runs: String,
    back: String,
}

impl Pages {
    fn of(cx: &Cx, access: &TenantAccess, scenario: &ScenarioState) -> Self {
        let slug = &access.tenant.slug;
        let id = || ScenarioId(scenario.id.clone());
        Self {
            edit: href!(edit, Slug(slug.clone()), id()).resolve(cx),
            archive: href!(archive::archive, Slug(slug.clone()), id()).resolve(cx),
            pause: href!(pause::pause, Slug(slug.clone()), id()).resolve(cx),
            resume: href!(resume::resume, Slug(slug.clone()), id()).resolve(cx),
            runs: href!(runs::index, Slug(slug.clone()), id()).resolve(cx),
            back: automations_url(cx, slug, None, None),
        }
    }
}

#[component]
async fn scenario_page(
    access: &TenantAccess,
    nav: &TenantNav,
    scenario: &ScenarioState,
    editor: &Editor,
    manage: bool,
    pages: &Pages,
    error: Option<&str>,
    message: Option<&str>,
) -> Result<impl View> {
    let tenant = &access.tenant;
    let title = format!("{} · Automations · {}", scenario.name, tenant.name);
    let problems = editor.definition.check();
    let canvas = Canvas {
        editor,
        scenario_id: &scenario.id,
        status: scenario.status,
        version: scenario.version,
        problems,
        manage,
    };
    let subtitle = match (scenario.status, scenario.version) {
        (ScenarioStatus::Draft, _) => "A draft: nothing runs from it yet.".to_owned(),
        (ScenarioStatus::Archived, _) => "Deleted: nothing runs from it.".to_owned(),
        (status, version) => format!("{}, version {version}.", status_label(status)),
    };
    let archived = scenario.status == ScenarioStatus::Archived;
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(tenant, nav, "automations"),
            page_header(
                title: &scenario.name,
                subtitle: Some(subtitle.as_str()),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(&pages.back)>"Back to Automations"</a>
                if scenario.version > 0 {
                    <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(&pages.runs)>"Runs"</a>
                }
                if manage && scenario.status == ScenarioStatus::Active {
                    <form method="post" action=(&pages.pause)>
                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Pause")
                    </form>
                }
                if manage && scenario.status == ScenarioStatus::Paused {
                    <form method="post" action=(&pages.resume)>
                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Resume")
                    </form>
                }
                if manage {
                    dropdown_menu(
                        dropdown_menu_trigger(
                            target: "delete-scenario",
                            attrs: attributes! { class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) },
                            "Delete…"
                        )
                        dropdown_menu_content(
                            id: "delete-scenario",
                            align: FloatingAlign::End,
                            <form method="post" action=(&pages.archive) class="flex max-w-xs flex-col gap-2 p-2">
                                <p class="text-sm">"Delete this scenario? Nothing will run from it any more."</p>
                                button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Delete scenario")
                            </form>
                        )
                    )
                }
            )
            notice(error: error, message: message)
            <p id="editor-live" class="sr-only" aria-live="polite"></p>
            if manage {
                editor_view(canvas: &canvas, action: &pages.edit)
                <script type="module" src=(asset!("./editor.js"))></script>
            } else {
                if archived {
                    <p class="mb-4 text-sm text-muted-foreground">"This scenario was deleted; it is shown as it was."</p>
                }
                canvas::read_only_view(canvas: &canvas)
            }
        )
    })
}
