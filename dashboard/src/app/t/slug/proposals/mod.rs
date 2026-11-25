//! `/t/{slug}/proposals`: the changes prepared for the shop that wait for a
//! person, for members with `proposals:prepare` or `proposals:approve`.
//! Those with `proposals:approve` say yes or no, to one after reading it
//! (and editing what it would write) or to several at once from the list;
//! a change to the catalog also asks for `products:manage`, as typing it
//! would. The list is grouped by what prepared the changes, and a second
//! tab keeps what was answered, and by whom.
//!
//! The lists come from the shared read model (a moment behind); a member
//! about to answer reads the proposal from its stream. Approving records
//! the yes: the change itself is made a moment later
//! ([`crate::proposals::apply`]).

pub mod answer;
pub mod new;
pub mod proposal_id;

use std::collections::HashMap;

use timada_core::page::{Cursor, PageFrom};
use timada_proposals::aggregator::Change;
use timada_proposals::{
    KIND_CATEGORY, KIND_PRODUCT, ProposalRow, ProposalState, ProposalStatus, answered_proposals,
    waiting_proposals, waiting_sources,
};
use timada_tenancy::{PERMISSION_PROPOSALS_APPROVE, PERMISSION_PROPOSALS_PREPARE};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{error::forbidden, href, page, query_params},
    view::{View, attributes, component, view},
};

use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::app::auth::Flash;
use crate::auth::now;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::checkbox::checkbox;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::proposals::{Proposals, Refusal, field_label, kind_label, permissions_of, source_label};
use crate::ui::{
    LINK, PageTab, Shell, Tone, cursor_pager, document, empty_row, format_time, notice,
    page_header, page_tabs, section_card, status_badge,
};

const PAGE_SIZE: i64 = 50;
/// The `view` of the tab of what was answered.
const HISTORY: &str = "history";
/// How much of a value a list shows.
const EXCERPT_LEN: usize = 160;

/// What the checkbox of a row is named after: followed by the proposal's
/// id.
pub const PICK_PREFIX: &str = "pick:";
/// What the hidden field of a row is named after, so a button can answer
/// every row shown: followed by the proposal's id.
pub const SHOWN_PREFIX: &str = "shown:";

#[topcoat::router::query_params(error = bad_request)]
struct ProposalsQuery {
    /// `history` for what was answered; what waits otherwise.
    view: Option<String>,
    /// The cursor of the row the page starts after (older ones).
    after: Option<String>,
    /// The cursor of the row the page ends before (newer ones).
    before: Option<String>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct ProposalsLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    view: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<&'a str>,
}

/// The list of what waits, with a flash error code or message.
pub fn proposals_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

/// The proposals gateway in the tenant's streams (403 while suspended).
pub fn gateway(access: &TenantAccess) -> Result<Proposals> {
    Ok(Proposals::new(
        access.executor()?.clone(),
        access.tenant_id(),
    ))
}

/// What a member may do with the shop's proposals.
pub struct Rights {
    pub prepare: bool,
    pub approve: bool,
}

/// The tenant, for a member who prepares proposals or answers them: 403
/// for anyone else.
pub async fn require_reader(cx: &Cx) -> Result<(TenantAccess, Rights)> {
    let access = require_tenant(cx).await?;
    let rights = Rights {
        prepare: access.can(cx, PERMISSION_PROPOSALS_PREPARE).await?,
        approve: access.can(cx, PERMISSION_PROPOSALS_APPROVE).await?,
    };
    if !rights.prepare && !rights.approve {
        return Err(forbidden().into());
    }
    Ok((access, rights))
}

/// The word for where a proposal stands, and its colour.
pub fn status_of(status: ProposalStatus, expired: bool) -> (&'static str, Tone) {
    match status {
        ProposalStatus::Waiting if expired => ("Expired", Tone::Neutral),
        ProposalStatus::Waiting => ("Waiting", Tone::Warning),
        ProposalStatus::Approved => ("Approved", Tone::Info),
        ProposalStatus::Applied => ("Applied", Tone::Success),
        ProposalStatus::Refused => ("Refused", Tone::Neutral),
        ProposalStatus::OutOfDate => ("Out of date", Tone::Danger),
        ProposalStatus::Failed => ("Not applied", Tone::Danger),
    }
}

/// What ended a proposal, in a sentence; empty while it waits.
pub fn outcome_of(status: ProposalStatus, note: &str) -> String {
    match status {
        ProposalStatus::OutOfDate => match note.split_once(": ") {
            Some((field, found)) => format!(
                "{} was changed meanwhile, to: {}",
                field_label(field),
                if found.is_empty() { "(empty)" } else { found }
            ),
            None => "Its subject was changed meanwhile.".to_owned(),
        },
        _ => note.to_owned(),
    }
}

/// The names of the members `ids` name, by id. An id that is no member's
/// (a rule, an agent) is shown as it is.
pub async fn names(cx: &Cx, ids: Vec<String>) -> Result<HashMap<String, String>> {
    let users = timada_identity::users_by_ids(context::<Db>(cx), &ids)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(users
        .into_iter()
        .map(|user| (user.id, user.label))
        .collect())
}

/// `id` as a name of `names`, or as it is.
pub fn name_of(names: &HashMap<String, String>, id: &str) -> String {
    names.get(id).cloned().unwrap_or_else(|| id.to_owned())
}

/// Where a proposal's subject is edited, when it has a page.
pub fn subject_url(cx: &Cx, slug: &str, kind: &str, subject_id: &str) -> Option<String> {
    use super::{categories::category_id, products::product_id};
    if subject_id.is_empty() {
        return None;
    }
    match kind {
        timada_proposals::KIND_ADS_CAMPAIGN => {
            Some(href!(super::ads::campaigns::index, slug: slug.to_owned()).resolve(cx))
        }
        KIND_PRODUCT => Some(
            href!(product_id::index, slug: slug.to_owned(), product_id::product_id: subject_id.to_owned())
                .resolve(cx),
        ),
        KIND_CATEGORY | timada_proposals::KIND_NEW_CATEGORY => Some(
            href!(category_id::index, slug: slug.to_owned(), category_id::category_id: subject_id.to_owned())
                .resolve(cx),
        ),
        _ => None,
    }
}

/// How answering one proposal went.
pub enum Answered {
    Done,
    /// Its subject changed since it was prepared: ended, not applied.
    OutOfDate,
    /// Left as it was, with the code of why.
    Left(&'static str),
}

/// Approves `proposal` for the member, with the values they changed. A
/// change the member may not make themselves is left alone.
pub async fn approve_one(
    cx: &Cx,
    access: &TenantAccess,
    proposal: &ProposalState,
    edits: Vec<(String, String)>,
) -> Result<Answered> {
    let Some((_, manage)) = permissions_of(&proposal.kind) else {
        return Ok(Answered::Left("proposal_no_applier"));
    };
    if !access.can(cx, manage).await? {
        return Ok(Answered::Left("not_allowed"));
    }
    let approved = crate::proposals::approve(
        access.executor()?,
        access.tenant_id(),
        &access.user_id,
        proposal,
        edits,
        now(),
    )
    .await;
    match approved {
        Ok(()) => Ok(Answered::Done),
        Err(Refusal::OutOfDate) => Ok(Answered::OutOfDate),
        Err(refusal) => match refusal.code() {
            Some(code) => Ok(Answered::Left(code)),
            None => Err(topcoat::Error::from_anyhow(refusal.into())),
        },
    }
}

/// Refuses the proposal `proposal_id` for the member.
pub async fn refuse_one(
    access: &TenantAccess,
    proposal_id: &str,
    reason: &str,
) -> Result<Answered> {
    match gateway(access)?
        .refuse(&access.user_id, proposal_id, reason)
        .await
    {
        Ok(()) => Ok(Answered::Done),
        Err(err) => match err.code() {
            Some(code) => Ok(Answered::Left(code)),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}

/// The start of a value, for a list.
fn excerpt(value: &str) -> String {
    if value.chars().count() <= EXCERPT_LEN {
        return value.to_owned();
    }
    let cut: String = value.chars().take(EXCERPT_LEN).collect();
    format!("{}…", cut.trim_end())
}

/// One field of a proposal, as a list shows it.
pub struct ChangeLine {
    pub field: String,
    pub label: String,
    pub before: String,
    pub after: String,
}

impl ChangeLine {
    pub fn of(change: &Change, whole: bool) -> Self {
        let show = |value: &str| {
            if whole {
                value.to_owned()
            } else {
                excerpt(value)
            }
        };
        Self {
            field: change.field.clone(),
            label: field_label(&change.field),
            before: show(&change.before),
            after: show(&change.after),
        }
    }
}

/// What a proposal would change: each field, what it holds and what it
/// would take.
#[component]
pub async fn change_list(cx: &Cx, changes: &[ChangeLine]) -> Result<impl View> {
    Ok(view! {
        <dl class="grid gap-2 text-sm">
            for change in changes {
                <div data-proposal-change=(&change.field)>
                    <dt class="font-medium">(&change.label)</dt>
                    <dd class="whitespace-pre-line">
                        if change.before.is_empty() {
                            <span class="text-muted-foreground">"(empty)"</span>
                        } else {
                            <del class="text-muted-foreground" data-proposal-before="">(&change.before)</del>
                        }
                        <span aria-hidden="true">" → "</span>
                        <span class="sr-only">" becomes "</span>
                        if change.after.is_empty() {
                            <span class="text-muted-foreground">"(empty)"</span>
                        } else {
                            <ins class="font-medium no-underline" data-proposal-after="">(&change.after)</ins>
                        }
                    </dd>
                </div>
            }
        </dl>
    })
}

/// A row of either list.
struct Row {
    id: String,
    href: String,
    kind: &'static str,
    subject: String,
    changes: Vec<ChangeLine>,
    reason: String,
    prepared_by: String,
    at: i64,
    valid_until: i64,
    status: &'static str,
    status_key: &'static str,
    tone: Tone,
    expired: bool,
    /// Who answered and what came of it, for the history.
    outcome: String,
}

/// The rows of what waits that one thing prepared.
struct Group {
    source: String,
    label: String,
    /// How many of its proposals wait, on every page.
    waiting: u64,
    rows: Vec<Row>,
}

fn row_of(cx: &Cx, slug: &str, row: &ProposalRow, names: &HashMap<String, String>) -> Row {
    let status = row.status();
    let expired = row.is_expired(now());
    let (label, tone) = status_of(status, expired);
    let outcome = outcome_of(status, &row.note);
    let outcome = match (row.answered_by.is_empty(), outcome.is_empty()) {
        (true, _) => outcome,
        (false, true) => format!("By {}", name_of(names, &row.answered_by)),
        (false, false) => format!("By {}. {outcome}", name_of(names, &row.answered_by)),
    };
    Row {
        id: row.id.clone(),
        href: href!(proposal_id::index, slug: slug.to_owned(), proposal_id::proposal_id: row.id.clone())
            .resolve(cx),
        kind: kind_label(&row.kind),
        subject: row.subject_name.clone(),
        changes: row
            .changes
            .0
            .iter()
            .map(|change| ChangeLine::of(change, false))
            .collect(),
        reason: excerpt(&row.reason),
        prepared_by: name_of(names, &row.prepared_by),
        at: row.listed_at,
        valid_until: row.valid_until,
        status: label,
        status_key: if expired { "expired" } else { status.as_str() },
        tone,
        expired,
        outcome,
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let (access, rights) = require_reader(cx).await?;
    let query = query_params::<ProposalsQuery>(cx)?;
    let history = query.view.as_deref() == Some(HISTORY);
    let nav = TenantNav::of(cx, &access).await?;
    let TenantAccess { tenant, .. } = access;
    let db = context::<Db>(cx);
    let after = query.after.as_deref().and_then(Cursor::parse);
    let before = query.before.as_deref().and_then(Cursor::parse);
    let from = PageFrom::of(after.as_ref(), before.as_ref());
    let listed = if history {
        answered_proposals(db, &tenant.id, from, PAGE_SIZE).await
    } else {
        waiting_proposals(db, &tenant.id, from, PAGE_SIZE).await
    }
    .map_err(topcoat::Error::from_anyhow)?;
    let sources = waiting_sources(db, &tenant.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let waiting: u64 = sources.iter().map(|(_, count)| count).sum();
    let names = names(
        cx,
        listed
            .rows
            .iter()
            .flat_map(|row| [row.prepared_by.clone(), row.answered_by.clone()])
            .filter(|id| !id.is_empty())
            .collect(),
    )
    .await?;
    let slug = tenant.slug.clone();
    let rows: Vec<(String, Row)> = listed
        .rows
        .iter()
        .map(|row| (row.source.clone(), row_of(cx, &slug, row, &names)))
        .collect();
    // What waits, by what prepared it, in the order the sources sort in.
    let mut groups: Vec<Group> = Vec::new();
    let mut answered: Vec<Row> = Vec::new();
    for (source, row) in rows {
        if history {
            answered.push(row);
            continue;
        }
        match groups.iter_mut().find(|group| group.source == source) {
            Some(group) => group.rows.push(row),
            None => groups.push(Group {
                label: source_label(&source),
                waiting: sources
                    .iter()
                    .find(|(name, _)| name == &source)
                    .map_or(0, |(_, count)| *count),
                source,
                rows: vec![row],
            }),
        }
    }
    groups.sort_by(|a, b| a.source.cmp(&b.source));
    let view_param = history.then_some(HISTORY);
    let page_link = |after: Option<&str>, before: Option<&str>| {
        href!(index, slug: slug.clone())
            .query(ProposalsLink {
                view: view_param,
                after,
                before,
            })
            .resolve(cx)
    };
    let newer = listed
        .newer
        .as_ref()
        .map(|cursor| page_link(None, Some(&cursor.param())));
    let older = listed
        .older
        .as_ref()
        .map(|cursor| page_link(Some(&cursor.param()), None));
    let tabs = [
        PageTab {
            label: "Waiting",
            href: href!(index, slug: slug.clone()).resolve(cx),
            count: Some(waiting as usize),
            current: !history,
        },
        PageTab {
            label: "Answered",
            href: href!(index, slug: slug.clone())
                .query(ProposalsLink {
                    view: Some(HISTORY),
                    after: None,
                    before: None,
                })
                .resolve(cx),
            count: None,
            current: history,
        },
    ];
    let title = format!("Proposals · {}", tenant.name);
    let summary = if history {
        "Latest answered first".to_owned()
    } else {
        "Latest prepared first".to_owned()
    };
    let answer_url = href!(answer::answer, slug: slug.clone()).resolve(cx);
    let can_approve = rights.approve;

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "proposals"),
            page_header(
                title: "Proposals",
                subtitle: Some("Changes prepared for the shop by a rule, a writer, an agent or a member. Nothing is applied until someone approves it."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            page_tabs(label: "Proposals", tabs: &tabs)

            if !history {
                if groups.is_empty() {
                    section_card(
                        <p class="text-sm text-muted-foreground" data-proposals-empty="">"Nothing waits for an answer."</p>
                    )
                }
                for group in &groups {
                    section_card(
                        title: Some(&group.label),
                        attrs: attributes! { data-proposal-group=(&group.source) data-proposal-waiting=(group.waiting) },
                        <form method="post" action=(&answer_url)>
                            table(
                                table_header(
                                    table_row(
                                        if can_approve {
                                            table_head(<span class="sr-only">"Select"</span>)
                                        }
                                        table_head("About")
                                        table_head("Change")
                                        table_head("Why")
                                        table_head("Prepared")
                                        table_head("Status")
                                    )
                                )
                                table_body(
                                    for row in &group.rows {
                                        table_row(
                                            attrs: attributes! { data-proposal=(&row.id) data-proposal-status=(row.status_key) },
                                            if can_approve {
                                                table_cell(
                                                    attrs: attributes! { class="align-top" },
                                                    <input type="hidden" name=(format!("{SHOWN_PREFIX}{}", row.id)) value="1">
                                                    checkbox(attrs: attributes! {
                                                        name=(format!("{PICK_PREFIX}{}", row.id))
                                                        aria-label=(format!("Select the change to {}", row.subject))
                                                    })
                                                )
                                            }
                                            table_cell(
                                                attrs: attributes! { class="whitespace-normal align-top" },
                                                <a class=(LINK) href=(&row.href)>(&row.subject)</a>
                                                <span class="block text-sm text-muted-foreground">(row.kind)</span>
                                            )
                                            table_cell(attrs: attributes! { class="whitespace-normal align-top" }, change_list(changes: &row.changes))
                                            table_cell(attrs: attributes! { class="whitespace-normal align-top text-sm" }, (&row.reason))
                                            table_cell(
                                                attrs: attributes! { class="align-top text-sm" },
                                                <span class="block">(&row.prepared_by)</span>
                                                <time class="text-muted-foreground" datetime=(row.at)>(format_time(row.at as u64))</time>
                                            )
                                            table_cell(
                                                attrs: attributes! { class="align-top" },
                                                status_badge(tone: row.tone, (row.status))
                                                if !row.expired {
                                                    <span class="mt-1 block text-sm text-muted-foreground">"Until " <time datetime=(row.valid_until)>(format_time(row.valid_until as u64))</time></span>
                                                }
                                            )
                                        )
                                    }
                                )
                            )
                            if can_approve {
                                <div class="mt-4 flex flex-wrap gap-2 px-6">
                                    button(size: ButtonSize::Sm, attrs: attributes! { type="submit" name="verdict" value="approve" }, "Approve the selected")
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" name="verdict" value="refuse" }, "Refuse the selected")
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" name="verdict" value="approve_all" }, "Approve all shown")
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" name="verdict" value="refuse_all" }, "Refuse all shown")
                                </div>
                            }
                        </form>
                    )
                }
            } else {
                section_card(
                    table(
                        table_header(
                            table_row(
                                table_head("About")
                                table_head("Change")
                                table_head("Prepared by")
                                table_head("Outcome")
                                table_head("At")
                            )
                        )
                        table_body(
                            if answered.is_empty() {
                                empty_row(colspan: 5, text: "No proposal was answered yet.")
                            }
                            for row in &answered {
                                table_row(
                                    attrs: attributes! { data-proposal=(&row.id) data-proposal-status=(row.status_key) },
                                    table_cell(
                                        attrs: attributes! { class="whitespace-normal align-top" },
                                        <a class=(LINK) href=(&row.href)>(&row.subject)</a>
                                        <span class="block text-sm text-muted-foreground">(row.kind)</span>
                                    )
                                    table_cell(attrs: attributes! { class="whitespace-normal align-top" }, change_list(changes: &row.changes))
                                    table_cell(attrs: attributes! { class="align-top text-sm" }, (&row.prepared_by))
                                    table_cell(
                                        attrs: attributes! { class="whitespace-normal align-top" },
                                        status_badge(tone: row.tone, (row.status))
                                        <span class="mt-1 block text-sm text-muted-foreground" data-proposal-outcome="">(&row.outcome)</span>
                                    )
                                    table_cell(attrs: attributes! { class="align-top text-sm" }, <time datetime=(row.at)>(format_time(row.at as u64))</time>)
                                )
                            }
                        )
                    )
                )
            }
            cursor_pager(summary: &summary, newer: newer, older: older)
            if rights.prepare {
                <p class="text-sm text-muted-foreground">"To prepare a change by hand, open a product or a category and choose “Suggest a change”."</p>
            }
        )
    })
}

/// What answering several proposals came to, in a sentence.
pub fn tally_message(approve: bool, done: usize, out_of_date: usize, left: usize) -> String {
    let mut parts = Vec::new();
    if done > 0 {
        parts.push(if approve {
            format!("{done} approved: the changes are being made.")
        } else {
            format!("{done} refused.")
        });
    }
    if out_of_date > 0 {
        parts.push(format!(
            "{out_of_date} out of date: what they were about changed meanwhile, so nothing was applied."
        ));
    }
    if left > 0 {
        parts.push(format!(
            "{left} left as they were: answered already, past their date, or not yours to approve."
        ));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tally_says_what_happened_to_each() {
        assert_eq!(
            tally_message(true, 2, 0, 0),
            "2 approved: the changes are being made."
        );
        assert_eq!(tally_message(false, 1, 0, 0), "1 refused.");
        let mixed = tally_message(true, 1, 1, 1);
        assert!(mixed.contains("1 approved"), "{mixed}");
        assert!(mixed.contains("1 out of date"), "{mixed}");
        assert!(mixed.contains("1 left as they were"), "{mixed}");
    }

    #[test]
    fn an_out_of_date_note_reads_as_a_sentence() {
        assert_eq!(
            outcome_of(ProposalStatus::OutOfDate, "name: Gants cuir"),
            "Name was changed meanwhile, to: Gants cuir"
        );
        assert_eq!(outcome_of(ProposalStatus::Refused, "Not now."), "Not now.");
        assert_eq!(excerpt(&"x".repeat(200)).chars().count(), EXCERPT_LEN + 1);
    }
}
