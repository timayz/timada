//! The editor's right column: the settings of the block pressed in the
//! flow, or the trigger picker. The settings are fields named
//! `settings.<key>` with the block's id in `settings_for`; every button of
//! the editor applies them before doing its own thing.

use timada_automations::{
    ProblemAt, Step,
    catalog::{self, CONDITION, EMAIL, NOTIFY, Op, Unit, WAIT},
};
use topcoat::{
    Result,
    view::{View, attributes, component, view},
};

use super::super::labels::{
    TRIGGER_GROUPS, field_label, op_label, problem_label, step_label, subject_label,
    trigger_description, trigger_label, value_hint,
};
use super::canvas::Canvas;
use super::editor::TRIGGER;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_description, field_label as label_of};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::textarea::textarea;

/// A choice of the trigger picker.
const CHOICE: &str = "flex w-full flex-col gap-0.5 rounded-lg border border-border bg-card p-3 text-left text-sm hover:border-primary/60 hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring data-[current]:border-primary data-[current]:ring-2 data-[current]:ring-primary/30";

#[component]
pub async fn inspector(canvas: &Canvas<'_>) -> Result<impl View> {
    let selected = canvas.editor.selected.as_deref();
    let step = selected
        .filter(|id| *id != TRIGGER)
        .and_then(|id| canvas.editor.definition.find(id));
    let heading = match (selected, step) {
        (Some(TRIGGER), _) => "Trigger",
        (_, Some(step)) => step_label(&step.kind),
        _ => "Settings",
    };
    let focus = canvas.editor.focus.as_deref() == Some("inspector");
    let editable = canvas.manage && canvas.editor.definition.is_editable();
    Ok(view! {
        <aside aria-labelledby="inspector-heading" class="flex flex-col gap-4 rounded-xl border border-border bg-card p-4 lg:col-span-2 xl:sticky xl:top-20 xl:col-span-1" data-inspector="">
            <h2 id="inspector-heading" class="text-sm font-semibold" tabindex="-1" autofocus=(focus)>(heading)</h2>
            if !editable {
                <p class="text-sm text-muted-foreground">"This scenario was drawn with a newer version of Timada: it is shown, not edited."</p>
            } else if selected == Some(TRIGGER) {
                trigger_picker(canvas: canvas)
            } else if let Some(step) = step {
                step_settings(canvas: canvas, step: step)
            } else {
                <p class="text-sm text-muted-foreground">"Press a block in the flow to set it up."</p>
                <ul class="flex list-disc flex-col gap-1 pl-5 text-sm text-muted-foreground">
                    <li>"Drag a block from the left onto a line between two steps, or use the + on a line."</li>
                    <li>"Drag a block by its handle to move it, or press Move, then “Move here”."</li>
                    <li>"An If / else splits the run: it goes down its Yes or its No lane, each to its own end."</li>
                </ul>
            }
        </aside>
    })
}

/// The triggers, by group; the one chosen is marked.
#[component]
async fn trigger_picker(canvas: &Canvas<'_>) -> Result<impl View> {
    let current = canvas
        .editor
        .definition
        .trigger
        .as_ref()
        .map(|trigger| trigger.kind.clone());
    let has_conditions = canvas
        .editor
        .definition
        .walk()
        .iter()
        .any(|step| step.kind == CONDITION);
    Ok(view! {
        <p class="text-sm text-muted-foreground">"What starts a run of this scenario."</p>
        if has_conditions {
            <p class="text-sm text-muted-foreground">"Another trigger may leave the conditions' rules to set up again: they look at what the trigger is about."</p>
        }
        for (group, kinds) in TRIGGER_GROUPS {
            <div class="flex flex-col gap-2" role="group" aria-label=(group)>
                <h3 class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">(group)</h3>
                for kind in kinds.iter() {
                    <button
                        type="submit"
                        name="op"
                        value=(format!("trigger:{kind}"))
                        class=(CHOICE)
                        data-current=(current.as_deref() == Some(*kind))
                        aria-pressed=(if current.as_deref() == Some(*kind) { "true" } else { "false" })
                    >
                        <span class="font-medium">(trigger_label(kind))</span>
                        <span class="text-muted-foreground">(trigger_description(kind))</span>
                    </button>
                }
            </div>
        }
    })
}

/// What the problems of one setting say.
fn issues_of(canvas: &Canvas<'_>, id: &str, key: &str) -> Vec<&'static str> {
    canvas
        .problems
        .iter()
        .filter(|problem| {
            problem.at == ProblemAt::Step(id.to_owned()) && problem.key.as_deref() == Some(key)
        })
        .map(problem_label)
        .collect()
}

/// The problems of one setting, under its field.
#[component]
async fn issue_list(texts: Vec<&'static str>) -> Result<impl View> {
    Ok(view! {
        for text in &texts {
            <p class="text-sm text-destructive" data-issue="">(text)</p>
        }
    })
}

#[component]
async fn step_settings(canvas: &Canvas<'_>, step: &Step) -> Result<impl View> {
    let known = catalog::step_kind(&step.kind).is_some();
    Ok(view! {
        if known {
            <input type="hidden" name="settings_for" value=(&step.id)>
        }
        if step.kind == WAIT {
            wait_settings(canvas: canvas, step: step)
        } else if step.kind == CONDITION {
            condition_settings(canvas: canvas, step: step)
        } else if step.kind == NOTIFY {
            notify_settings(canvas: canvas, step: step)
        } else if step.kind == EMAIL {
            email_settings(canvas: canvas, step: step)
        } else {
            <p class="text-sm text-muted-foreground">"This block was made with a newer version of Timada. It is kept as it is; remove it to publish the scenario."</p>
        }
        <div class="flex flex-wrap gap-2">
            if known {
                button(attrs: attributes! { type="submit" name="op" value="apply" }, "Apply")
            }
            button(variant: ButtonVariant::Ghost, attrs: attributes! { type="submit" name="op" value="deselect" }, "Close")
        </div>
    })
}

#[component]
async fn wait_settings(canvas: &Canvas<'_>, step: &Step) -> Result<impl View> {
    let amount = step.settings.get("amount").cloned().unwrap_or_default();
    let unit = step.settings.get("unit").cloned().unwrap_or_default();
    let mut texts = issues_of(canvas, &step.id, "amount");
    texts.extend(issues_of(canvas, &step.id, "unit"));
    Ok(view! {
        <div class="flex items-end gap-2">
            field(
                attrs: attributes! { class="w-28" },
                label_of(attrs: attributes! { for="wait-amount" }, "Wait for")
                input(attrs: attributes! {
                    id="wait-amount" name="settings.amount" type="number" min="1" inputmode="numeric"
                    value=(&amount) required=""
                })
            )
            field(
                attrs: attributes! { class="flex-1" },
                label_of(attrs: attributes! { for="wait-unit" class="sr-only" }, "Unit")
                select(
                    attrs: attributes! { id="wait-unit" name="settings.unit" },
                    for option in Unit::ALL {
                        <option value=(option.as_str()) selected=(unit == option.as_str())>(option.as_str())</option>
                    }
                )
            )
        </div>
        issue_list(texts: texts)
        <p class="text-sm text-muted-foreground">"At most 90 days. The run goes on with the next step once the time is up."</p>
    })
}

/// One rule of a condition as the form shows it.
struct RuleRow {
    index: usize,
    field: String,
    op: String,
    value: String,
    /// The subject's fields, and whether `field` is one of them.
    known_field: bool,
    /// The operators the chosen field takes.
    ops: Vec<Op>,
    takes_value: bool,
    hint: &'static str,
    issues: Vec<&'static str>,
}

#[component]
async fn condition_settings(canvas: &Canvas<'_>, step: &Step) -> Result<impl View> {
    let subject = canvas.editor.definition.subject();
    let setting = |key: &str| step.settings.get(key).cloned().unwrap_or_default();
    let all = setting("match") != "any";
    let rows: Vec<RuleRow> = catalog::rule_indices(&step.settings)
        .into_iter()
        .map(|index| {
            let chosen = setting(&format!("rules.{index}.field"));
            let op = setting(&format!("rules.{index}.op"));
            let known = subject.and_then(|subject| subject.field(&chosen));
            let ops = known
                .map(|known| known.ty.ops().to_vec())
                .unwrap_or_default();
            let takes_value = Op::parse(&op).is_none_or(Op::takes_value);
            let mut found = Vec::new();
            for part in ["field", "op", "value"] {
                found.extend(issues_of(
                    canvas,
                    &step.id,
                    &format!("rules.{index}.{part}"),
                ));
            }
            RuleRow {
                index,
                value: setting(&format!("rules.{index}.value")),
                known_field: known.is_some(),
                hint: known.map_or("", |known| value_hint(known.ty)),
                field: chosen,
                op,
                ops,
                takes_value,
                issues: found,
            }
        })
        .collect();
    let fields = subject.map(|subject| subject.fields()).unwrap_or_default();
    let about = subject.map_or("", subject_label);
    let rule_issues = issues_of(canvas, &step.id, "rules");
    Ok(view! {
        if subject.is_none() {
            <p class="text-sm text-muted-foreground">"Choose the trigger first: the rules look at what it is about."</p>
            for (key, value) in &step.settings {
                <input type="hidden" name=(format!("settings.{key}")) value=(value)>
            }
        } else {
            field(
                label_of(attrs: attributes! { for="condition-match" }, "Go down Yes when")
                select(
                    attrs: attributes! { id="condition-match" name="settings.match" },
                    <option value="all" selected=(all)>"all the rules hold"</option>
                    <option value="any" selected=(!all)>"any rule holds"</option>
                )
                field_description((format!("The rules look at {about} as it is when the run gets here.")))
            )
            for row in &rows {
                <fieldset class="flex flex-col gap-2 rounded-lg border border-border p-3" data-rule=(row.index)>
                    <legend class="px-1 text-sm font-medium">"Rule"</legend>
                    select(
                        attrs: attributes! {
                            name=(format!("settings.rules.{}.field", row.index))
                            aria-label="What the rule looks at" data-auto-apply=""
                        },
                        for choice in fields {
                            <option value=(choice.key) selected=(row.field == choice.key)>(field_label(choice.key))</option>
                        }
                        if !row.known_field {
                            <option value=(&row.field) selected="">"Not available for this trigger"</option>
                        }
                    )
                    select(
                        attrs: attributes! {
                            name=(format!("settings.rules.{}.op", row.index))
                            aria-label="How it compares" data-auto-apply=""
                        },
                        for op in &row.ops {
                            <option value=(op.as_str()) selected=(row.op == op.as_str())>(op_label(*op))</option>
                        }
                        if !row.ops.iter().any(|op| op.as_str() == row.op) {
                            <option value=(&row.op) selected="">"Choose"</option>
                        }
                    )
                    if row.takes_value {
                        input(attrs: attributes! {
                            name=(format!("settings.rules.{}.value", row.index))
                            value=(&row.value) placeholder=(row.hint) aria-label="Value"
                        })
                    } else {
                        <input type="hidden" name=(format!("settings.rules.{}.value", row.index)) value="">
                    }
                    issue_list(texts: row.issues.clone())
                    <div>
                        button(
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Sm,
                            attrs: attributes! { type="submit" name="op" value=(format!("rule-remove:{}", row.index)) },
                            "Remove this rule"
                        )
                    </div>
                </fieldset>
            }
            issue_list(texts: rule_issues)
            <div>
                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" name="op" value="rule-add" }, "Add a rule")
            </div>
        }
    })
}

#[component]
async fn notify_settings(canvas: &Canvas<'_>, step: &Step) -> Result<impl View> {
    let subject = step.settings.get("subject").cloned().unwrap_or_default();
    let note = step.settings.get("note").cloned().unwrap_or_default();
    let about = canvas
        .editor
        .definition
        .subject()
        .map_or("what started the run", subject_label);
    Ok(view! {
        field(
            label_of(attrs: attributes! { for="notify-subject" }, "Subject")
            input(attrs: attributes! {
                id="notify-subject" name="settings.subject" value=(&subject) maxlength="200" required=""
            })
            issue_list(texts: issues_of(canvas, &step.id, "subject"))
        )
        field(
            label_of(attrs: attributes! { for="notify-note" }, "Note")
            textarea(attrs: attributes! { id="notify-note" name="settings.note" rows="5" maxlength="2000" }, (&note))
            field_description((format!("Mailed to the members allowed to see {about}, with a link to it. Write what they should do.")))
            issue_list(texts: issues_of(canvas, &step.id, "note"))
        )
    })
}

#[component]
async fn email_settings(canvas: &Canvas<'_>, step: &Step) -> Result<impl View> {
    let setting = |key: &str| step.settings.get(key).cloned().unwrap_or_default();
    let subject = setting("subject");
    let html = setting("html");
    let text = setting("text");
    let about_orders = super::mail::kind_of(&canvas.editor.definition)
        == timada_tenancy::ShopMailKind::AutomationOrder;
    Ok(view! {
        <p class="text-sm text-muted-foreground">
            "Goes to the address of whoever the run is about, from the shop's own mail server, and at most once a day per scenario. Not to those who stopped the shop's automated mails, left its newsletter or erased their account."
        </p>
        field(
            label_of(attrs: attributes! { for="email-subject" }, "Subject")
            input(attrs: attributes! { id="email-subject" name="settings.subject" value=(&subject) maxlength="200" })
            issue_list(texts: issues_of(canvas, &step.id, "subject"))
        )
        field(
            label_of(attrs: attributes! { for="email-html" }, "Body (HTML)")
            textarea(
                attrs: attributes! { id="email-html" name="settings.html" rows="14" spellcheck="false" class="font-mono text-xs" },
                (&html)
            )
            field_description(
                if about_orders {
                    "Write {{ shop.name }}, {{ order.reference }}, {{ order.total }} or loop over {{ order.lines }}; {{ link }} is the order's page."
                } else {
                    "Write {{ shop.name }} and the shop's other variables."
                }
                " Link to {{ unsubscribe_url }}: every automated mail lets its reader stop them."
            )
            issue_list(texts: issues_of(canvas, &step.id, "html"))
        )
        field(
            label_of(attrs: attributes! { for="email-text" }, "Plain-text version")
            textarea(
                attrs: attributes! { id="email-text" name="settings.text" rows="5" spellcheck="false" class="font-mono text-xs" },
                (&text)
            )
            field_description("Leave it empty to derive it from the HTML.")
        )
        if let Some(preview) = &canvas.preview {
            if let Ok(rendered) = preview {
                <p class="text-sm"><span class="text-muted-foreground">"Subject: "</span><span data-preview-subject="">(&rendered.subject)</span></p>
                <iframe sandbox="" title="Preview of the mail" srcdoc=(&rendered.html)
                    class="h-80 w-full rounded-lg border border-border bg-white"></iframe>
            }
            if let Err(err) = preview {
                <p class="text-sm text-destructive" role="alert" data-template-error="">(err.to_string())</p>
            }
        }
    })
}
