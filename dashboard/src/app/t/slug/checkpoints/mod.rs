//! `/t/{slug}/checkpoints`: what the shop set out to reach and the dates
//! its numbers are read at, for members with `orders:read`: the target (a
//! monthly profit after ads, by a day) and every change of it, the four
//! checkpoints counted from the first campaign's start, each with its
//! numbers and what they say once its day came, and what was decided.
//! A member with `tenant:manage` sets the target, records a decision and
//! changes the thresholds. A "stop" is advice: nothing is switched off
//! from here (`crate::checkpoints`).

pub mod decide;
pub mod target;
pub mod thresholds;

use timada_core::civil::format_date;
use timada_core::format_money;
use timada_launch::{
    ADVICES, CHECKPOINTS, D21_GO, D21_STOP, D21_STOP_CLICKS, D45_WITHIN, D90_CATEGORIES,
    D90_POSITION, D120_GO, D120_STOP, FIX, GO_ON, MAX_NOTE_LEN, STOP, THRESHOLDS,
};
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
use crate::auth::now;
use crate::checkpoints::{
    Launches, advice_label, checkpoint_label, checkpoint_reads, due_at, standing,
};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_description, field_label as label};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::textarea::textarea;
use crate::db::Db;
use crate::ui::{
    Shell, Tone, document, fact, facts, format_time, notice, page_header, section_card,
    status_badge,
};

#[topcoat::router::query_params(error = bad_request)]
struct CheckpointsQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The page, with a flash error code or message.
pub fn checkpoints_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

/// The launches gateway in the tenant's streams (403 while suspended).
pub fn launches(access: &TenantAccess) -> Result<Launches> {
    Ok(Launches::new(
        access.executor()?.clone(),
        access.tenant_id(),
    ))
}

/// The colour of an advice.
pub fn advice_tone(advice: &str) -> Tone {
    match advice {
        GO_ON => Tone::Success,
        FIX => Tone::Warning,
        STOP => Tone::Danger,
        _ => Tone::Neutral,
    }
}

/// A threshold's name and what its field says under it.
fn threshold(name: &str) -> (&'static str, &'static str) {
    match name {
        D21_GO => (
            "Day 21: return to go on",
            "The first campaign's return on ad spend at or above which it is on course.",
        ),
        D21_STOP => (
            "Day 21: return to stop",
            "Under it, with the clicks below, the niche does not pay for its clicks.",
        ),
        D21_STOP_CLICKS => (
            "Day 21: clicks that make it a verdict",
            "A poor return on fewer clicks says to fix, not to stop.",
        ),
        D45_WITHIN => (
            "Day 45: within reach, in %",
            "How far under its break-even the return may be and still be closed by tuning.",
        ),
        D90_CATEGORIES => (
            "Day 90: categories placed",
            "How many categories have to stand in the first positions.",
        ),
        D90_POSITION => (
            "Day 90: last position that counts",
            "20 is the end of the second page of results.",
        ),
        D120_GO => (
            "Day 120: margin to go on, in %",
            "What is left of the revenue once goods, costs and ads are paid.",
        ),
        D120_STOP => (
            "Day 120: margin to stop, in %",
            "Under it and not improving.",
        ),
        _ => ("Threshold", ""),
    }
}

struct Threshold {
    name: &'static str,
    label: &'static str,
    hint: &'static str,
    value: String,
}

/// A target as the page shows it.
struct Shown {
    amount: String,
    by_day: String,
    set_by: String,
    set_at: u64,
}

/// A checkpoint as the page shows it.
struct Line {
    checkpoint: u32,
    label: String,
    reads: &'static str,
    /// `waiting` for the first campaign, `ahead`, `due` (its day came, the
    /// daily read has not read it yet) or `reached`.
    key: &'static str,
    /// Unix seconds its day comes at; 0 while nothing counts yet.
    at: u64,
    reached_at: u64,
    advice: String,
    why: String,
    figures: Vec<(String, String)>,
    decision: String,
    note: String,
    decided: String,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let query = query_params::<CheckpointsQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_TENANT_MANAGE).await?;
    let failed = topcoat::Error::from_anyhow;
    let now = now();
    let launch = launches(&access)?.load_launch().await.map_err(failed)?;
    let standing = standing(
        access.executor()?,
        context::<Db>(cx),
        access.tenant_id(),
        now,
    )
    .await
    .map_err(failed)?;
    let mut ids: Vec<String> = Vec::new();
    if let Some(launch) = &launch {
        ids.extend(launch.targets.iter().map(|target| target.set_by.clone()));
        ids.extend(
            launch
                .reached
                .iter()
                .filter_map(|reached| Some(reached.decision.as_ref()?.decided_by.clone())),
        );
    }
    ids.sort();
    ids.dedup();
    let names = names(cx, ids).await?;
    let mut targets: Vec<Shown> = launch
        .iter()
        .flat_map(|launch| &launch.targets)
        .map(|target| Shown {
            amount: format_money(target.monthly_profit_cents, &target.currency),
            by_day: target.by_day.clone(),
            set_by: name_of(&names, &target.set_by),
            set_at: target.set_at,
        })
        .collect();
    let target = targets.pop();
    // The earlier ones, the latest first.
    targets.reverse();
    let held = launch.as_ref().and_then(|launch| launch.target());
    let progress = held.and_then(|held| {
        let percent = standing.progress_percent(held.monthly_profit_cents)?;
        let month = standing.month_cents?;
        let sign = if month < 0 { "-" } else { "" };
        Some((
            percent,
            format!(
                "{sign}{}",
                format_money(month.unsigned_abs(), &standing.currency)
            ),
        ))
    });
    let form_amount = held
        .map(|held| timada_core::format_amount(held.monthly_profit_cents))
        .unwrap_or_default();
    let form_currency = held
        .map(|held| held.currency.clone())
        .or((!standing.currency.is_empty()).then(|| standing.currency.clone()))
        .unwrap_or_else(|| "EUR".to_owned());
    let form_day = held.map(|held| held.by_day.clone()).unwrap_or_default();
    let lines: Vec<Line> = CHECKPOINTS
        .into_iter()
        .map(|checkpoint| {
            let at = standing
                .started_at
                .map_or(0, |started| due_at(started, checkpoint));
            let reached = launch
                .as_ref()
                .and_then(|launch| launch.reached(checkpoint))
                .filter(|reached| Some(reached.started_at) == standing.started_at);
            let decision = reached.and_then(|reached| reached.decision.as_ref());
            Line {
                checkpoint,
                label: checkpoint_label(checkpoint),
                reads: checkpoint_reads(checkpoint),
                key: match (reached, standing.started_at) {
                    (Some(_), _) => "reached",
                    (None, None) => "waiting",
                    (None, Some(_)) if now >= at => "due",
                    (None, Some(_)) => "ahead",
                },
                at,
                reached_at: reached.map_or(0, |reached| reached.reached_at),
                advice: reached
                    .map(|reached| reached.advice.clone())
                    .unwrap_or_default(),
                why: reached
                    .map(|reached| reached.why.clone())
                    .unwrap_or_default(),
                figures: reached
                    .map(|reached| reached.figures.clone())
                    .unwrap_or_default(),
                decision: decision
                    .map(|decision| decision.decision.clone())
                    .unwrap_or_default(),
                note: decision
                    .map(|decision| decision.note.clone())
                    .unwrap_or_default(),
                decided: decision
                    .map(|decision| {
                        format!(
                            "{} · {}",
                            name_of(&names, &decision.decided_by),
                            format_time(decision.decided_at)
                        )
                    })
                    .unwrap_or_default(),
            }
        })
        .collect();
    let set = launch
        .as_ref()
        .map(|launch| launch.thresholds())
        .unwrap_or_default();
    let thresholds: Vec<Threshold> = THRESHOLDS
        .iter()
        .map(|(name, _)| {
            let (label, hint) = threshold(name);
            Threshold {
                name,
                label,
                hint,
                value: set.value(name),
            }
        })
        .collect();
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Checkpoints · {}", tenant.name);
    let target_url = href!(target::target, slug: slug.clone()).resolve(cx);
    let decide_url = href!(decide::decide, slug: slug.clone()).resolve(cx);
    let thresholds_url = href!(thresholds::thresholds, slug: slug.clone()).resolve(cx);
    let today = format_date(now);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "checkpoints"),
            page_header(
                title: "Checkpoints",
                subtitle: Some("A new shop loses money for weeks by design. A target fixed beforehand, and dates to read the numbers at, keep « is this working » from being answered by mood."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some("Target"),
                description: Some("What the shop is to bring a month once its ads are paid, and by when. Set it before the first campaign. A change is kept beside the one before, not written over it."),
                attrs: attributes! { data-target=(if target.is_some() { "set" } else { "none" }) },
                if let Some(target) = &target {
                    facts(
                        fact(term: "Profit after ads, a month", attrs: attributes! { data-target-amount="" }, (&target.amount))
                        fact(term: "By", attrs: attributes! { data-target-day="" }, (&target.by_day))
                        fact(term: "Set by", (&target.set_by) " · " <time datetime=(target.set_at)>(format_time(target.set_at))</time>)
                        if let Some((percent, month)) = &progress {
                            fact(
                                term: "Last thirty days",
                                attrs: attributes! { data-target-progress=(percent) },
                                (month) " after ads: " (percent) " % of the target"
                            )
                        } else {
                            fact(term: "Last thirty days", "Not known yet: it takes a Google Ads account, sales, and what the goods cost.")
                        }
                    )
                    if !targets.is_empty() {
                        <h3 class="mt-4 text-sm font-medium">"Before"</h3>
                        <ul class="mt-2 flex flex-col gap-1 text-sm text-muted-foreground" data-target-history="">
                            for earlier in &targets {
                                <li>(&earlier.amount) " by " (&earlier.by_day) " · " (&earlier.set_by) " · " <time datetime=(earlier.set_at)>(format_time(earlier.set_at))</time></li>
                            }
                        </ul>
                    }
                }
                if can_manage {
                    <form method="post" action=(&target_url) class="mt-6 flex max-w-xl flex-col gap-4" data-target-form="">
                        field(
                            label(attrs: attributes! { for="target-amount" }, "Profit after ads, a month")
                            input(attrs: attributes! { id="target-amount" name="amount" type="text" inputmode="decimal" required="" value=(&form_amount) class="w-40" })
                        )
                        field(
                            label(attrs: attributes! { for="target-currency" }, "Currency")
                            input(attrs: attributes! { id="target-currency" name="currency" type="text" maxlength="3" required="" value=(&form_currency) class="w-24" })
                        )
                        field(
                            label(attrs: attributes! { for="target-day" }, "By")
                            input(attrs: attributes! { id="target-day" name="by_day" type="date" required="" min=(&today) value=(&form_day) class="w-48" })
                        )
                        <div>button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, (if target.is_some() { "Change the target" } else { "Set the target" }))</div>
                    </form>
                }
            )

            section_card(
                title: Some("Checkpoints"),
                description: Some("Counted in days from the moment the first campaign went live. Each is read from numbers timada holds and says to go on, to fix or to stop. That is advice: you record what you decide, and nothing is switched off by itself."),
                <ol class="flex flex-col gap-6">
                    for line in &lines {
                        <li class="border-l-2 pl-4" data-checkpoint=(line.checkpoint) data-checkpoint-state=(line.key)>
                            <h3 class="font-medium">
                                (&line.label) " "
                                if line.key == "reached" {
                                    status_badge(tone: advice_tone(&line.advice), attrs: attributes! { data-checkpoint-advice=(&line.advice) }, (advice_label(&line.advice)))
                                }
                            </h3>
                            <p class="text-sm text-muted-foreground">(line.reads)</p>
                            if line.key == "waiting" {
                                <p class="mt-1 text-sm">"Counted once the first campaign runs."</p>
                            } else if line.key == "ahead" {
                                <p class="mt-1 text-sm">"On " <time datetime=(line.at) data-checkpoint-on="">(format_date(line.at))</time> "."</p>
                            } else if line.key == "due" {
                                <p class="mt-1 text-sm">"Its day came on " (format_date(line.at)) ": it is read with the next daily read of Google Ads."</p>
                            } else {
                                <p class="mt-1 text-sm" data-checkpoint-why="">(&line.why)</p>
                                <ul class="mt-2 flex flex-col gap-1 text-sm text-muted-foreground" data-checkpoint-figures="">
                                    for (name, value) in &line.figures {
                                        <li>(name) ": " (value)</li>
                                    }
                                    <li>"Read " <time datetime=(line.reached_at)>(format_time(line.reached_at))</time></li>
                                </ul>
                                if !line.decision.is_empty() {
                                    <p class="mt-3 text-sm" data-checkpoint-decision=(&line.decision)>
                                        "Decided: " <strong>(advice_label(&line.decision))</strong>
                                        if !line.note.is_empty() { ". " (&line.note) }
                                        <span class="block text-muted-foreground">(&line.decided)</span>
                                    </p>
                                }
                                if can_manage {
                                    <form method="post" action=(&decide_url) class="mt-3 flex max-w-xl flex-col gap-3" data-checkpoint-decide="">
                                        <input type="hidden" name="checkpoint" value=(line.checkpoint) />
                                        field(
                                            label(attrs: attributes! { for=(format!("decision-{}", line.checkpoint)) }, "What you decide")
                                            select(
                                                attrs: attributes! { id=(format!("decision-{}", line.checkpoint)) name="decision" class="w-64" },
                                                for advice in ADVICES {
                                                    <option value=(advice) selected=(if line.decision.is_empty() { line.advice == advice } else { line.decision == advice })>(advice_label(advice))</option>
                                                }
                                            )
                                        )
                                        field(
                                            label(attrs: attributes! { for=(format!("note-{}", line.checkpoint)) }, "Why")
                                            textarea(attrs: attributes! { id=(format!("note-{}", line.checkpoint)) name="note" rows="2" maxlength=(MAX_NOTE_LEN) }, (&line.note))
                                        )
                                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Record the decision")</div>
                                    </form>
                                }
                            }
                        </li>
                    }
                </ol>
            )

            section_card(
                title: Some("Thresholds"),
                description: Some("What each checkpoint is judged on. They come from one case study: change them once your own numbers say otherwise. What was read already stays as it was read."),
                <form method="post" action=(&thresholds_url) data-checkpoint-thresholds="">
                    <fieldset disabled=(!can_manage) class="flex max-w-3xl flex-col gap-4">
                        for threshold in &thresholds {
                            field(
                                label(attrs: attributes! { for=(format!("threshold-{}", threshold.name)) }, (threshold.label))
                                field_description(attrs: attributes! { id=(format!("threshold-{}-hint", threshold.name)) }, (threshold.hint))
                                input(attrs: attributes! { id=(format!("threshold-{}", threshold.name)) name=(threshold.name) type="text" inputmode="decimal" value=(&threshold.value) aria-describedby=(format!("threshold-{}-hint", threshold.name)) class="w-40" })
                            )
                        }
                        if can_manage {
                            <div>button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, "Save the thresholds")</div>
                        }
                    </fieldset>
                </form>
            )
        )
    })
}
