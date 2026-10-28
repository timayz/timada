//! The editor's markup: the palette of blocks, the flow (the trigger, then
//! its lane of steps, a condition's lanes side by side under it, each lane
//! down to its end) and the toolbar. Every place between two steps is a
//! slot (`data-drop-slot`), but none follows a condition: its "+" menu adds
//! a block there, and while a block is picked up it offers "Move here". `editor.js` adds dragging on top: it only ever presses these
//! same buttons, so the page works the same without it.

use timada_automations::{
    Definition, Lane, MAX_DEPTH, Problem, ProblemAt, ScenarioStatus, Slot, Step,
    catalog::{self, Subject},
};
use topcoat::{
    Result,
    icon::icon,
    view::{View, ViewExt, attributes, class, component, view},
};

use super::super::labels::{
    branch_label, problem_label, status_label, status_tone, step_description, step_icon,
    step_label, step_summary, trigger_label,
};
use super::editor::{Editor, Moving, TRIGGER};
use super::inspector::inspector;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::dropdown_menu::{dropdown_menu, dropdown_menu_content};
use crate::components::field::{field, field_label};
use crate::components::floating::FloatingAlign;
use crate::components::input::input;
use crate::ui::{Tone, icons, status_badge};

/// Everything the flow is drawn from.
pub struct Canvas<'a> {
    pub editor: &'a Editor,
    pub scenario_id: &'a str,
    pub status: ScenarioStatus,
    /// The version running; 0 while never published.
    pub version: u32,
    pub problems: Vec<Problem>,
    /// Whether the member may edit: otherwise the flow is drawn read-only.
    pub manage: bool,
}

impl Canvas<'_> {
    fn definition(&self) -> &Definition {
        &self.editor.definition
    }

    fn subject(&self) -> Option<Subject> {
        self.editor.definition.subject()
    }

    fn editable(&self) -> bool {
        self.manage && self.editor.definition.is_editable()
    }

    fn step_problems(&self, id: &str) -> usize {
        self.problems
            .iter()
            .filter(|problem| problem.at == ProblemAt::Step(id.to_owned()))
            .count()
    }

    fn trigger_problem(&self) -> Option<&Problem> {
        self.problems
            .iter()
            .find(|problem| problem.at == ProblemAt::Trigger)
    }

    fn focused(&self, target: &str) -> bool {
        self.editor.focus.as_deref() == Some(target)
    }

    fn selected(&self, target: &str) -> bool {
        self.editor.selected.as_deref() == Some(target)
    }

    /// Whether a new block of `kind` may go into `lane`.
    fn fits(&self, kind: &str, at: &Slot) -> bool {
        let branches = catalog::step_kind(kind).map_or(0, |kind| kind.branches);
        branches == 0
            || self
                .definition()
                .condition_depth(at)
                .is_some_and(|depth| depth <= MAX_DEPTH)
    }
}

/// A palette block or a menu item.
const BLOCK: &str = "flex w-full items-start gap-3 rounded-lg border border-border bg-card p-3 text-left text-sm shadow-xs transition hover:border-primary/60 hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring";
const MENU_ITEM: &str = "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm whitespace-nowrap outline-none hover:bg-foreground/5 focus-visible:bg-foreground/5 [&_svg]:size-4 [&_svg]:shrink-0";
/// The card of a step or of the trigger.
const CARD: &str = "relative w-60 rounded-xl border border-border bg-card text-card-foreground shadow-xs data-[selected]:border-primary data-[selected]:ring-2 data-[selected]:ring-primary/30";
/// A small icon button on a card.
const CARD_ACTION: &str = "inline-flex size-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring [&_svg]:size-4";

/// The whole editor: the form holding the working copy.
#[component]
pub async fn editor_view(canvas: &Canvas<'_>, action: &str) -> Result<impl View> {
    let editor = canvas.editor;
    let json = editor.definition.to_json();
    let dirty = editor.is_dirty();
    let selected = editor.selected.clone().unwrap_or_default();
    let moving = editor
        .moving
        .as_ref()
        .map(Moving::encode)
        .unwrap_or_default();
    let announce = editor.announce.clone().unwrap_or_default();
    let to_set_up = canvas.problems.len();
    let putting = matches!(editor.moving, Some(Moving::Kind(_)));
    Ok(view! {
        <form
            method="post"
            action=(action)
            autocomplete="off"
            // A draft may be unfinished, and every button sends the whole
            // form: what is missing is said next to it, never blocked.
            novalidate=""
            class="group/editor flex flex-col gap-4 aria-busy:opacity-70"
            data-scenario-editor=""
            data-scenario=(canvas.scenario_id)
            data-base=(editor.base)
            data-dirty=(dirty)
            data-announce=(&announce)
        >
            // Enter in a field applies the settings: the first submit button
            // is the one a browser presses then.
            <button type="submit" name="op" value="apply" class="sr-only" tabindex="-1" aria-hidden="true">"Apply"</button>
            <input type="hidden" name="definition" value=(&json)>
            <input type="hidden" name="base" value=(editor.base)>
            <input type="hidden" name="selected" value=(&selected)>
            <input type="hidden" name="moving" value=(&moving)>
            <template data-drag-ghost="">
                <div class="pointer-events-none fixed top-0 left-0 z-50 flex items-center gap-2 rounded-lg border border-primary bg-card px-3 py-2 text-sm font-medium shadow-lg"></div>
            </template>
            <div data-restore="" hidden="" class="flex flex-wrap items-center gap-3 rounded-lg border border-border bg-muted p-3 text-sm">
                <span class="flex-1">"This browser kept changes to this scenario that were not saved."</span>
                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="button" data-restore-yes="" }, "Restore them")
                button(variant: ButtonVariant::Ghost, size: ButtonSize::Sm, attrs: attributes! { type="button" data-restore-no="" }, "Discard them")
            </div>

            <div class="flex flex-wrap items-end gap-3 rounded-xl border border-border bg-card p-4">
                field(
                    attrs: attributes! { class="min-w-56 flex-1" },
                    field_label(attrs: attributes! { for="scenario-name" }, "Name")
                    input(attrs: attributes! {
                        id="scenario-name" name="name" value=(&editor.name) required="" maxlength="80"
                    })
                )
                <div class="flex flex-wrap items-center gap-2 pb-1.5">
                    status_badge(tone: status_tone(canvas.status), attrs: attributes! { data-scenario-state=(canvas.status.as_str()) }, (status_label(canvas.status)))
                    if canvas.version > 0 {
                        <span class="text-sm text-muted-foreground">"Version " (canvas.version) " runs"</span>
                    }
                    if dirty {
                        status_badge(tone: Tone::Warning, attrs: attributes! { data-unsaved="" }, "Unsaved changes")
                    }
                </div>
                <p class="pb-2 text-sm text-muted-foreground" data-to-set-up=(to_set_up)>
                    if to_set_up == 0 {
                        "Ready to publish."
                    } else {
                        (to_set_up) " thing(s) to set up before it can be published."
                    }
                </p>
                button(attrs: attributes! { type="submit" name="op" value="save" }, "Save draft")
            </div>

            if let Some(refusal) = editor.refusal {
                alert(
                    variant: AlertVariant::Destructive,
                    attrs: attributes! { role="alert" },
                    icon(data: icons::ERROR)
                    alert_description((refusal))
                )
            }
            if let Some(label) = editor.moving_label() {
                <div class="flex flex-wrap items-center gap-3 rounded-lg border border-primary/40 bg-primary/5 p-3 text-sm" data-moving="">
                    <p class="flex-1" tabindex="-1" autofocus=(canvas.focused("moving"))>
                        if putting {
                            "Choose where to put " <strong>(label)</strong> ": press “Put here” between two steps."
                        } else {
                            "Choose where to move " <strong>(label)</strong> ": press “Move here” between two steps."
                        }
                    </p>
                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" name="op" value="cancel" }, "Cancel")
                </div>
            }

            <div class="grid items-start gap-4 lg:grid-cols-[13rem_minmax(0,1fr)] xl:grid-cols-[13rem_minmax(0,1fr)_19rem]">
                palette(canvas: canvas)
                <section aria-labelledby="flow-heading" class="min-w-0 rounded-xl border border-border bg-muted/40">
                    <h2 id="flow-heading" class="sr-only" tabindex="-1" autofocus=(canvas.focused("canvas"))>"Flow"</h2>
                    <div class="overflow-x-auto p-4" data-canvas="">
                        <div class="mx-auto flex w-max min-w-full flex-col items-center">
                            flow(canvas: canvas)
                        </div>
                    </div>
                </section>
                inspector(canvas: canvas)
            </div>
        </form>
    })
}

/// The blocks a step can be made of. Each is dragged onto a slot, or
/// pressed to pick it up and then put down with a slot's "Put here".
#[component]
async fn palette(canvas: &Canvas<'_>) -> Result<impl View> {
    let editable = canvas.editable();
    Ok(view! {
        <aside aria-labelledby="palette-heading" class="flex flex-col gap-3 rounded-xl border border-border bg-card p-4 lg:sticky lg:top-20">
            <h2 id="palette-heading" class="text-sm font-semibold">"Blocks"</h2>
            <p class="text-sm text-muted-foreground">
                "Drag a block between two steps, or press it and then choose where it goes."
            </p>
            <ul class="flex flex-col gap-2">
                for kind in catalog::STEPS {
                    <li>
                        <button
                            type="submit"
                            name="op"
                            value=(format!("place:{}", kind.key))
                            class=(format!("{BLOCK} cursor-grab touch-none select-none"))
                            data-block-kind=(kind.key)
                            data-drag-label=(step_label(kind.key))
                            disabled=(!editable)
                        >
                            <span class="mt-0.5 rounded-md bg-muted p-1.5 text-foreground">icon(data: step_icon(kind.key))</span>
                            <span class="flex min-w-0 flex-col">
                                <span class="font-medium">(step_label(kind.key))</span>
                                <span class="text-muted-foreground">(step_description(kind.key))</span>
                            </span>
                        </button>
                    </li>
                }
            </ul>
        </aside>
    })
}

/// The trigger's card, then its lane.
#[component]
async fn flow(canvas: &Canvas<'_>) -> Result<impl View> {
    let definition = canvas.definition();
    let kind = definition
        .trigger
        .as_ref()
        .map(|trigger| trigger.kind.clone());
    let problem = canvas.trigger_problem().map(problem_label);
    Ok(view! {
        <div
            class=(CARD)
            data-trigger=""
            data-trigger-kind=(kind.as_deref())
            data-selected=(canvas.selected(TRIGGER))
        >
            <div class="flex items-start gap-3 p-3">
                <span class="mt-0.5 rounded-md bg-primary/10 p-1.5 text-primary">icon(data: icons::TRIGGER)</span>
                if canvas.manage {
                    <button
                        type="submit"
                        name="op"
                        value="select:trigger"
                        class="flex min-w-0 flex-1 flex-col text-left focus-visible:outline-2 focus-visible:outline-ring"
                        aria-pressed=(if canvas.selected(TRIGGER) { "true" } else { "false" })
                        autofocus=(canvas.focused(TRIGGER))
                    >
                        <span class="text-xs font-medium tracking-wide text-muted-foreground uppercase">"Trigger"</span>
                        <span class="text-sm font-medium">
                            (kind.as_deref().map_or("Choose what starts it", trigger_label))
                        </span>
                    </button>
                } else {
                    <div class="flex min-w-0 flex-1 flex-col">
                        <span class="text-xs font-medium tracking-wide text-muted-foreground uppercase">"Trigger"</span>
                        <span class="text-sm font-medium">(kind.as_deref().map_or("Not chosen yet", trigger_label))</span>
                    </div>
                }
            </div>
            if let Some(problem) = problem {
                <p class="border-t border-border px-3 py-2 text-sm text-warning-text" data-needs-setup="">(problem)</p>
            }
        </div>
        flow_lane(canvas: canvas, steps: &definition.steps, lane: &Lane::Root)
    })
}

/// A lane: a slot, then each step followed by a slot, then where the run
/// ends. A condition ends its lane instead: each of its lanes ends on its
/// own. Recursive through the steps' own lanes.
#[component]
async fn flow_lane(canvas: &Canvas<'_>, steps: &[Step], lane: &Lane) -> Result<impl View> {
    let slots: Vec<Slot> = (0..=steps.len())
        .map(|index| Slot::new(lane.clone(), index))
        .collect();
    let ends = !steps.last().is_some_and(Step::ends_lane);
    Ok(view! {
        drop_slot(canvas: canvas, at: &slots[0])
        for (index, step) in steps.iter().enumerate() {
            step_block(canvas: canvas, step: step)
            if !step.ends_lane() {
                drop_slot(canvas: canvas, at: &slots[index + 1])
            }
        }
        if ends {
            <div class="mt-1 rounded-full border border-dashed border-border px-3 py-1 text-xs text-muted-foreground" aria-hidden="true" data-lane-end="">"End"</div>
        }
    }
    .boxed())
}

/// A place between two steps.
#[component]
async fn drop_slot(canvas: &Canvas<'_>, at: &Slot) -> Result<impl View> {
    let name = at.to_string();
    let menu = format!("add-{name}");
    let picking = canvas.editor.moving.is_some();
    let takes = canvas.editor.takes(at);
    let kinds: Vec<&'static str> = catalog::STEPS
        .iter()
        .map(|kind| kind.key)
        .filter(|kind| canvas.fits(kind, at))
        .collect();
    let putting = matches!(canvas.editor.moving, Some(Moving::Kind(_)));
    let fits = match &canvas.editor.moving {
        Some(Moving::Kind(kind)) => canvas.fits(kind, at),
        _ => true,
    };
    Ok(view! {
        <div
            class="group/slot relative flex min-h-10 w-full flex-col items-center justify-center py-1 group-data-[dragging]/editor:min-h-14"
            data-drop-slot=(&name)
        >
            <span aria-hidden="true" class="absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-border group-data-[drop-target]/slot:w-0.5 group-data-[drop-target]/slot:bg-primary"></span>
            if canvas.editable() {
                if picking {
                    if takes && fits {
                        <button
                            type="submit"
                            name="op"
                            value=(format!("drop:{name}"))
                            class=(class!("relative", button_variants(ButtonVariant::Outline, ButtonSize::Sm)))
                        >
                            if putting { "Put here" } else { "Move here" }
                        </button>
                    }
                } else {
                    dropdown_menu(
                        <button
                            type="button"
                            popovertarget=(&menu)
                            aria-label="Add a step here"
                            class="relative inline-flex size-7 cursor-pointer items-center justify-center rounded-full border border-border bg-card text-muted-foreground shadow-xs hover:border-primary hover:text-primary focus-visible:outline-2 focus-visible:outline-ring group-data-[dragging]/editor:opacity-0 [&_svg]:size-4"
                        >
                            icon(data: icons::PLUS)
                        </button>
                        dropdown_menu_content(
                            id: &menu,
                            align: FloatingAlign::Center,
                            for kind in &kinds {
                                <button type="submit" name="op" value=(format!("add:{name}:{kind}")) class=(MENU_ITEM)>
                                    icon(data: step_icon(kind))
                                    (step_label(kind))
                                </button>
                            }
                        )
                    )
                }
            }
            <span aria-hidden="true" class="pointer-events-none absolute inset-x-6 inset-y-1 hidden rounded-lg border-2 border-dashed border-primary/40 group-data-[dragging]/editor:block group-data-[drop-target]/slot:border-solid group-data-[drop-target]/slot:border-primary group-data-[drop-target]/slot:bg-primary/10"></span>
        </div>
    })
}

/// A step's card, then, for a condition, its lanes side by side. The whole
/// block is what a drag of its handle moves.
#[component]
async fn step_block(canvas: &Canvas<'_>, step: &Step) -> Result<impl View> {
    let subject = canvas.subject();
    let problems = canvas.step_problems(&step.id);
    let label = step_label(&step.kind);
    let summary = step_summary(step, subject);
    let editable = canvas.editable();
    let picking = canvas.editor.moving.is_some();
    let lanes: Vec<(usize, &[Step], Lane)> = step
        .branches
        .iter()
        .enumerate()
        .map(|(branch, steps)| {
            let at = Lane::Branch {
                step: step.id.clone(),
                branch,
            };
            (branch, steps.as_slice(), at)
        })
        .collect();
    Ok(view! {
        <div class="flex flex-col items-center" data-step=(&step.id) data-step-kind=(&step.kind) data-drag-label=(label)>
            <div class=(CARD) data-selected=(canvas.selected(&step.id))>
                <div class="flex items-start gap-2 p-3">
                    if editable {
                        <span
                            data-drag-handle=""
                            hidden=""
                            aria-hidden="true"
                            title="Drag to move"
                            class="mt-1 cursor-grab touch-none text-muted-foreground [&_svg]:size-4"
                        >
                            icon(data: icons::GRIP)
                        </span>
                    }
                    <span class="mt-0.5 rounded-md bg-muted p-1.5 text-foreground">icon(data: step_icon(&step.kind))</span>
                    if canvas.manage {
                        <button
                            type="submit"
                            name="op"
                            value=(format!("select:{}", step.id))
                            class="flex min-w-0 flex-1 flex-col text-left focus-visible:outline-2 focus-visible:outline-ring"
                            aria-pressed=(if canvas.selected(&step.id) { "true" } else { "false" })
                            autofocus=(canvas.focused(&step.id))
                        >
                            <span class="text-sm font-medium">(label)</span>
                            <span class="truncate text-sm text-muted-foreground" data-step-summary="">(&summary)</span>
                        </button>
                    } else {
                        <div class="flex min-w-0 flex-1 flex-col">
                            <span class="text-sm font-medium">(label)</span>
                            <span class="truncate text-sm text-muted-foreground" data-step-summary="">(&summary)</span>
                        </div>
                    }
                </div>
                if problems > 0 {
                    <p class="border-t border-border px-3 py-1.5 text-xs text-warning-text" data-needs-setup="">"Needs setting up"</p>
                }
                if editable && !picking {
                    <div class="flex items-center justify-end gap-0.5 border-t border-border px-2 py-1">
                        <button type="submit" name="op" value=(format!("pick:{}", step.id)) class=(CARD_ACTION) aria-label=(format!("Move {label}")) title="Move">
                            icon(data: icons::MOVE)
                        </button>
                        <button type="submit" name="op" value=(format!("up:{}", step.id)) class=(CARD_ACTION) aria-label=(format!("Move {label} up")) title="Move up">
                            icon(data: icons::ARROW_UP)
                        </button>
                        <button type="submit" name="op" value=(format!("down:{}", step.id)) class=(CARD_ACTION) aria-label=(format!("Move {label} down")) title="Move down">
                            icon(data: icons::ARROW_DOWN)
                        </button>
                        <button type="submit" name="op" value=(format!("remove:{}", step.id)) class=(format!("{CARD_ACTION} hover:text-destructive")) aria-label=(format!("Remove {label}")) title="Remove">
                            icon(data: icons::TRASH)
                        </button>
                    </div>
                }
            </div>
            if !lanes.is_empty() {
                <div class="flex items-start justify-center gap-4 border-t border-border pt-2">
                    for (branch, steps, branch_lane) in &lanes {
                        <div class="flex min-w-60 flex-col items-center" data-branch=(branch_lane.to_string())>
                            <span class="rounded-full border border-border bg-card px-2.5 py-0.5 text-xs font-medium">(branch_label(&step.kind, *branch))</span>
                            flow_lane(canvas: canvas, steps: steps, lane: branch_lane)
                        </div>
                    }
                </div>
            }
        </div>
    })
}

/// The flow as it was last saved, for members who may only read.
#[component]
pub async fn read_only_view(canvas: &Canvas<'_>) -> Result<impl View> {
    Ok(view! {
        <section aria-labelledby="flow-heading" class="rounded-xl border border-border bg-muted/40" data-scenario-flow="">
            <h2 id="flow-heading" class="sr-only">"Flow"</h2>
            <div class="overflow-x-auto p-4">
                <div class="mx-auto flex w-max min-w-full flex-col items-center">
                    flow(canvas: canvas)
                </div>
            </div>
        </section>
    })
}
