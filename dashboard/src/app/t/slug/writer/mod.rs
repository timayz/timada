//! `/t/{slug}/writer`: first drafts of the shop's texts, written by a model
//! for a person to correct, for members who may prepare proposals and see
//! the catalog. A draft is asked for here for the products of a category,
//! or from a product's or a category's own page; it is written off the
//! page, and what comes back waits under Proposals. Nothing a draft says
//! is published until a member approves it.
//!
//! The page also holds how the shop wants to be written for (its tone,
//! what never to claim), which members with `products:manage` set, what
//! the day's drafts cost against the shop's cap, and what became of each
//! draft: the proposal it waits as, what to check in it, or why it failed.

pub mod batch;
pub mod brief;
pub mod draft;

use timada_catalog::categories_of_tenant;
use timada_core::page::{Cursor, PageFrom};
use timada_proposals::{KIND_CATEGORY, KIND_PRODUCT};
use timada_tenancy::{
    PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ, PERMISSION_PROPOSALS_PREPARE,
};
use timada_writer::{
    DraftStatus, MAX_BRIEF_LEN, MAX_KEYWORD_LEN, RequestDraft, Spend, WriterError,
    drafts_of_tenant, open_draft_of, spend_day,
};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::proposals::{names, proposal_id, subject_url};
use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::app::auth::Flash;
use crate::auth::now;
use crate::components::alert::{alert, alert_description};
use crate::components::button::button;
use crate::components::field::{field, field_description, field_label};
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::components::textarea::textarea;
use crate::db::Db;
use crate::proposals::{kind_label, subject};
use crate::ui::{
    LINK, Shell, Tone, cursor_pager, document, empty_row, fact, facts, format_time, notice,
    page_header, section_card, status_badge,
};
use crate::writer::{Spending, Writers, format_usd};

const PAGE_SIZE: i64 = 50;

#[topcoat::router::query_params(error = bad_request)]
struct WriterQuery {
    /// The cursor of the row the page starts after (older drafts).
    after: Option<String>,
    /// The cursor of the row the page ends before (newer drafts).
    before: Option<String>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct WriterLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<&'a str>,
}

/// The page, with a flash error code or message.
pub fn writer_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

/// The tenant, for a member who may ask for drafts: one who prepares
/// proposals and sees the catalog. 403 for anyone else.
pub async fn require_drafter(cx: &Cx) -> Result<TenantAccess> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PROPOSALS_PREPARE)
        .await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_READ)
        .await?;
    Ok(access)
}

/// The writer's gateway in the tenant's streams (403 while suspended).
pub fn gateway(access: &TenantAccess) -> Result<timada_writer::Command<crate::cluster::Executor>> {
    Ok(timada_writer::Command::new(
        access.executor()?.clone(),
        access.tenant_id(),
    ))
}

/// How asking for one draft went.
pub enum Asked {
    Requested,
    /// One is on its way for the same subject already.
    OnItsWay,
    /// The subject is not there.
    Missing,
}

/// Asks, for the member, for the texts of the `kind` at `subject_id` to be
/// drafted for `keyword` (empty for its own name).
pub async fn ask(
    cx: &Cx,
    access: &TenantAccess,
    kind: &str,
    subject_id: &str,
    keyword: &str,
) -> std::result::Result<Asked, WriterError> {
    let store = access.executor().map_err(|_| WriterError::InvalidSubject)?;
    if kind != KIND_PRODUCT && kind != KIND_CATEGORY {
        return Err(WriterError::UnknownKind);
    }
    let Some(held) = subject(store, access.tenant_id(), kind, subject_id, None).await? else {
        return Ok(Asked::Missing);
    };
    if open_draft_of(context::<Db>(cx), access.tenant_id(), subject_id, now())
        .await?
        .is_some()
    {
        return Ok(Asked::OnItsWay);
    }
    timada_writer::Command::new(store.clone(), access.tenant_id())
        .request_draft(
            &access.user_id,
            RequestDraft {
                kind: kind.to_owned(),
                subject_id: subject_id.to_owned(),
                subject_name: held.name,
                keyword: keyword.to_owned(),
            },
        )
        .await?;
    Ok(Asked::Requested)
}

/// The word for where a draft stands, and its colour.
fn status_of(status: DraftStatus, open: bool) -> (&'static str, Tone) {
    match status {
        DraftStatus::Drafted => ("Drafted", Tone::Success),
        DraftStatus::Failed => ("Not drafted", Tone::Danger),
        _ if open => ("Being drafted", Tone::Info),
        _ => ("Lost", Tone::Neutral),
    }
}

/// A row of the list.
struct Row {
    id: String,
    subject: String,
    subject_href: Option<String>,
    kind: &'static str,
    keyword: String,
    status: &'static str,
    status_key: &'static str,
    tone: Tone,
    proposal_href: Option<String>,
    to_check: Vec<String>,
    reason: String,
    cost: String,
    by: String,
    at: i64,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_drafter(cx).await?;
    let query = query_params::<WriterQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_PRODUCTS_MANAGE).await?;
    let enabled = context::<Writers>(cx).enabled();
    let caps = context::<Spending>(cx).caps;
    let db = context::<Db>(cx);
    let brief = gateway(&access)?
        .load_brief()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let today = Spend(access.executor()?.clone())
        .load_spend_day(&spend_day(now()))
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .charged_to(access.tenant_id());
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let categories: Vec<(String, String)> = categories_of_tenant(db, &tenant.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .into_iter()
        .filter(|row| !row.archived)
        .map(|row| (row.id, row.name))
        .collect();
    let after = query.after.as_deref().and_then(Cursor::parse);
    let before = query.before.as_deref().and_then(Cursor::parse);
    let listed = drafts_of_tenant(
        db,
        &tenant.id,
        PageFrom::of(after.as_ref(), before.as_ref()),
        PAGE_SIZE,
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    let names = names(
        cx,
        listed
            .rows
            .iter()
            .map(|row| row.requested_by.clone())
            .collect(),
    )
    .await?;
    let at = now();
    let rows: Vec<Row> = listed
        .rows
        .iter()
        .map(|row| {
            let status = row.status();
            let open = row.is_open(at);
            let (label, tone) = status_of(status, open);
            Row {
                id: row.id.clone(),
                subject: row.subject_name.clone(),
                subject_href: subject_url(cx, &slug, &row.kind, &row.subject_id),
                kind: kind_label(&row.kind),
                keyword: row.keyword.clone(),
                status: label,
                status_key: match status {
                    DraftStatus::Drafted | DraftStatus::Failed => status.as_str(),
                    _ if open => "open",
                    _ => "lost",
                },
                tone,
                proposal_href: (!row.proposal_id.is_empty()).then(|| {
                    href!(proposal_id::index, slug: slug.clone(), proposal_id::proposal_id: row.proposal_id.clone())
                        .resolve(cx)
                }),
                to_check: row
                    .to_check
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect(),
                reason: row.reason.clone(),
                cost: if row.cost_usd_micros > 0 {
                    format_usd(row.cost_usd_micros as u64)
                } else {
                    String::new()
                },
                by: super::proposals::name_of(&names, &row.requested_by),
                at: row.requested_at,
            }
        })
        .collect();
    let page_link = |after: Option<&str>, before: Option<&str>| {
        href!(index, slug: slug.clone())
            .query(WriterLink { after, before })
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
    let title = format!("Text drafts · {}", tenant.name);
    let spent = format!(
        "{} of {}",
        format_usd(today.usd_micros),
        format_usd(caps.shop_usd_micros)
    );
    let calls = match today.calls {
        1 => "1 draft".to_owned(),
        n => format!("{n} drafts"),
    };

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "writer"),
            page_header(
                title: "Text drafts",
                subtitle: Some("First drafts of names, descriptions and what search engines show, written by a model from what the shop knows of a product. Each waits under Proposals: nothing is published until someone approves it."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            if !enabled {
                alert(
                    attrs: attributes! { role="status" class="mb-6" data-writer-off="" },
                    alert_description("Text drafts are not set up on this platform: nothing can be drafted.")
                )
            }

            section_card(
                title: Some("Today"),
                facts(
                    fact(term: "Spent", attrs: attributes! { data-writer-spent=(today.usd_micros) }, (&spent))
                    fact(term: "Drafted", attrs: attributes! { data-writer-calls=(today.calls) }, (&calls))
                )
                <p class="mt-3 text-sm text-muted-foreground">"The platform pays for each draft and caps what a shop's drafts may cost in a day (UTC)."</p>
            )

            section_card(
                title: Some("How the shop is written for"),
                description: Some("Given to the writer with every draft."),
                <form method="post" action=(href!(brief::brief, slug: slug.clone())) class="flex max-w-3xl flex-col gap-4" data-writer-brief="">
                    <fieldset class="contents" disabled=(!can_manage)>
                        field(
                            field_label(attrs: attributes! { for="writer-tone" }, "Tone")
                            field_description(attrs: attributes! { id="writer-tone-hint" }, "The shop's voice: who it speaks to, how formal, what it sounds like.")
                            textarea(
                                attrs: attributes! { id="writer-tone" name="tone" rows="4" maxlength=(MAX_BRIEF_LEN) aria-describedby="writer-tone-hint" },
                                (&brief.tone)
                            )
                        )
                        field(
                            field_label(attrs: attributes! { for="writer-never" }, "Never claim")
                            field_description(attrs: attributes! { id="writer-never-hint" }, "What no text may say: a promise the shop cannot keep, a word it does not use.")
                            textarea(
                                attrs: attributes! { id="writer-never" name="never" rows="4" maxlength=(MAX_BRIEF_LEN) aria-describedby="writer-never-hint" },
                                (&brief.never)
                            )
                        )
                        if can_manage {
                            <div>button(attrs: attributes! { type="submit" }, "Save")</div>
                        }
                    </fieldset>
                </form>
            )

            if enabled {
                section_card(
                    title: Some("Draft a category's products"),
                    description: Some("One draft per product, fifty at a time, written for the product's own name. A product drafted in the last day, or whose draft is on its way, is passed over."),
                    if categories.is_empty() {
                        <p class="text-sm text-muted-foreground">"The shop has no category yet. A product's own page drafts its texts too."</p>
                    } else {
                        <form method="post" action=(href!(batch::batch, slug: slug.clone())) class="flex max-w-xl flex-col gap-4" data-writer-batch="">
                            field(
                                field_label(attrs: attributes! { for="writer-category" }, "Category")
                                select(
                                    attrs: attributes! { id="writer-category" name="category" required="" },
                                    for (id, name) in &categories {
                                        <option value=(id)>(name)</option>
                                    }
                                )
                            )
                            <label class="flex items-center gap-2 text-sm">
                                <input type="checkbox" name="with_category" value="1">
                                "And the category's own texts"
                            </label>
                            <div>button(attrs: attributes! { type="submit" }, "Draft them")</div>
                        </form>
                    }
                )
            }

            section_card(
                title: Some("Drafts"),
                table(
                    table_header(
                        table_row(
                            table_head("About")
                            table_head("Written for")
                            table_head("Outcome")
                            table_head("Cost")
                            table_head("Asked")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 5, text: "No draft was asked for yet.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-draft=(&row.id) data-draft-status=(row.status_key) },
                                table_cell(
                                    attrs: attributes! { class="whitespace-normal align-top" },
                                    if let Some(href) = &row.subject_href {
                                        <a class=(LINK) href=(href)>(&row.subject)</a>
                                    } else {
                                        (&row.subject)
                                    }
                                    <span class="block text-sm text-muted-foreground">(row.kind)</span>
                                )
                                table_cell(attrs: attributes! { class="whitespace-normal align-top text-sm" }, (if row.keyword.is_empty() { "Its own name" } else { row.keyword.as_str() }))
                                table_cell(
                                    attrs: attributes! { class="whitespace-normal align-top" },
                                    status_badge(tone: row.tone, (row.status))
                                    if let Some(href) = &row.proposal_href {
                                        <a class=(format!("{LINK} mt-1 block text-sm")) href=(href)>"Read the proposal"</a>
                                    }
                                    if !row.to_check.is_empty() {
                                        <p class="mt-1 text-sm text-muted-foreground" data-draft-to-check="">"To check: " (row.to_check.join(", "))</p>
                                    }
                                    if !row.reason.is_empty() {
                                        <p class="mt-1 text-sm text-muted-foreground" data-draft-reason="">(&row.reason)</p>
                                    }
                                )
                                table_cell(attrs: attributes! { class="align-top text-sm" }, (&row.cost))
                                table_cell(
                                    attrs: attributes! { class="align-top text-sm" },
                                    <span class="block">(&row.by)</span>
                                    <time class="text-muted-foreground" datetime=(row.at)>(format_time(row.at as u64))</time>
                                )
                            )
                        }
                    )
                )
                cursor_pager(summary: "Latest asked first", newer: newer, older: older)
            )
        )
    })
}

/// A field for the search a draft is written for, and the button that asks
/// for it: what a product's and a category's page show to members who may
/// prepare proposals, while a writer is set.
#[topcoat::view::component]
pub async fn draft_form(cx: &Cx, slug: &str, kind: &str, subject_id: &str) -> Result<impl View> {
    Ok(view! {
        <form method="post" action=(href!(draft::draft, slug: slug.to_owned())) class="flex flex-wrap items-end gap-2" data-draft-form="">
            <input type="hidden" name="kind" value=(kind)>
            <input type="hidden" name="subject" value=(subject_id)>
            <label class="flex flex-col gap-1 text-sm">
                <span class="text-muted-foreground">"Search to write for (optional)"</span>
                <input class="h-9 w-64 rounded-lg border border-border bg-transparent px-3 text-sm" type="text" name="keyword" maxlength=(MAX_KEYWORD_LEN)>
            </label>
            button(variant: crate::components::button::ButtonVariant::Outline, size: crate::components::button::ButtonSize::Sm, attrs: attributes! { type="submit" }, "Draft the texts")
        </form>
    })
}
