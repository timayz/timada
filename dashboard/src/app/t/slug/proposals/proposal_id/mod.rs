//! `/t/{slug}/proposals/{proposal_id}`: one proposal, read from its stream
//! (so an answer shows at once): what it would change, beside what the
//! shop holds, why, and who prepared it. A member with `proposals:approve`
//! (and the permission the change itself needs) edits what it would write,
//! then approves or refuses it.

pub mod answer;

use timada_proposals::{
    FIELD_BODY, FIELD_DESCRIPTION, MAX_NOTE_LEN, ProposalState, ProposalStatus,
};
use topcoat::{
    Result,
    context::Cx,
    router::{error::not_found, href, module_param, page, query_params},
    view::{View, attributes, view},
};

use super::{
    ChangeLine, change_list, gateway, name_of, names, outcome_of, proposals_url, require_reader,
    status_of, subject_url,
};
use crate::app::auth::Flash;
use crate::app::t::slug::{TenantAccess, TenantNav, slug};
use crate::auth::now;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::textarea::textarea;
use crate::proposals::{
    Catalog, Standing, field_label as label_of, kind_label, permissions_of, source_label, standing,
    subject,
};
use crate::ui::{
    LINK, Shell, document, fact, facts, format_time, icons, notice, page_header, section_card,
    status_badge,
};
use topcoat::icon::icon;

module_param!(pub proposal_id);

/// What the field of a value is named after: followed by the field.
pub const AFTER_PREFIX: &str = "after:";

#[topcoat::router::query_params(error = bad_request)]
struct ProposalQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn proposal_url(
    cx: &Cx,
    slug: &str,
    proposal_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(
        index,
        slug: slug.to_owned(),
        proposal_id: proposal_id.to_owned()
    )
    .query(Flash { error, message })
    .resolve(cx)
}

/// The proposal the URL names, or 404.
pub async fn require_proposal(cx: &Cx, access: &TenantAccess) -> Result<ProposalState> {
    gateway(access)?
        .load_proposal(proposal_id(cx))
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| not_found().into())
}

/// One field of the form that answers a proposal.
struct Edit {
    field: String,
    label: String,
    before: String,
    after: String,
    long: bool,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let (access, rights) = require_reader(cx).await?;
    let query = query_params::<ProposalQuery>(cx)?;
    let proposal = require_proposal(cx, &access).await?;
    let nav = TenantNav::of(cx, &access).await?;
    let expired = proposal.is_expired(now());
    let waiting = proposal.status == ProposalStatus::Waiting;
    // Whether this member may make the change itself: what approving asks
    // over the permission to approve.
    let may_change = match permissions_of(&proposal.kind) {
        Some((_, manage)) => access.can(cx, manage).await?,
        None => false,
    };
    let can_answer = waiting && rights.approve;
    let can_approve = can_answer && !expired && may_change;
    // What the subject holds now, for a member about to say yes.
    let stale = if can_approve {
        let catalog = Catalog::new(access.executor()?.clone(), access.tenant_id());
        let held = subject(
            &catalog,
            &proposal.kind,
            &proposal.subject_id,
            Some(&proposal.id),
        )
        .await
        .map_err(topcoat::Error::from_anyhow)?;
        match held {
            None => Some("What this proposal is about is no longer there.".to_owned()),
            Some(held) => match standing(&held, &proposal.changes) {
                Standing::Current => None,
                Standing::OutOfDate { field, found } => Some(format!(
                    "{} was changed since this was prepared, to: {}",
                    label_of(&field),
                    if found.is_empty() { "(empty)" } else { &found }
                )),
            },
        }
    } else {
        None
    };
    let names = names(
        cx,
        [proposal.prepared_by.clone(), proposal.answered_by.clone()]
            .into_iter()
            .filter(|id| !id.is_empty())
            .collect(),
    )
    .await?;
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let (status, tone) = status_of(proposal.status, expired);
    let status_key = if expired {
        "expired"
    } else {
        proposal.status.as_str()
    };
    let outcome = outcome_of(proposal.status, &proposal.note);
    let prepared_by = name_of(&names, &proposal.prepared_by);
    let answered_by = name_of(&names, &proposal.answered_by);
    let about = if proposal.subject_name.is_empty() {
        kind_label(&proposal.kind).to_owned()
    } else {
        proposal.subject_name.clone()
    };
    let heading = format!("Change to {about}");
    let title = format!("{heading} · Proposals · {}", tenant.name);
    let back = proposals_url(cx, &slug, None, None);
    let subject_link = subject_url(cx, &slug, &proposal.kind, &proposal.subject_id);
    let source = source_label(&proposal.source);
    let lines: Vec<ChangeLine> = proposal
        .changes
        .iter()
        .map(|change| ChangeLine::of(change, true))
        .collect();
    let edits: Vec<Edit> = proposal
        .changes
        .iter()
        .map(|change| Edit {
            label: label_of(&change.field),
            long: matches!(change.field.as_str(), FIELD_DESCRIPTION | FIELD_BODY),
            field: change.field.clone(),
            before: change.before.clone(),
            after: change.after.clone(),
        })
        .collect();
    let answer_url =
        href!(answer::answer, slug: slug.clone(), proposal_id: proposal.id.clone()).resolve(cx);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "proposals"),
            page_header(
                title: &heading,
                h1_attrs: attributes! { data-proposal-id=(&proposal.id) },
                status_badge(tone: tone, attrs: attributes! { data-proposal-status=(status_key) }, (status))
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Proposals"</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                facts(
                    fact(
                        term: "About",
                        if let Some(link) = &subject_link {
                            <a class=(LINK) href=(link)>(&about)</a>
                        } else {
                            (&about)
                        }
                        " · " (kind_label(&proposal.kind))
                    )
                    fact(term: "Prepared by", (&prepared_by) " · " (&source))
                    fact(term: "Prepared", <time datetime=(proposal.prepared_at)>(format_time(proposal.prepared_at))</time>)
                    fact(term: "Valid until", <time datetime=(proposal.valid_until)>(format_time(proposal.valid_until))</time>)
                    if !proposal.reason.is_empty() {
                        fact(term: "Why", attrs: attributes! { class="whitespace-pre-line" data-proposal-reason="" }, (&proposal.reason))
                    }
                    if !proposal.answered_by.is_empty() {
                        fact(
                            term: "Answered by",
                            attrs: attributes! { data-proposal-answered-by="" },
                            (&answered_by)
                            if proposal.edited { " · edited before approving" }
                        )
                    }
                    if !outcome.is_empty() {
                        fact(term: "Outcome", attrs: attributes! { data-proposal-outcome="" }, (&outcome))
                    }
                )
            )

            if let Some(stale) = &stale {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" class="mb-6" data-proposal-stale="" },
                    icon(data: icons::WARNING)
                    alert_description((stale) " Approving will end this proposal as out of date, without changing anything.")
                )
            }

            if can_approve {
                section_card(
                    title: Some("What it would change"),
                    description: Some("Edit a value to approve it your way. A field set back to what the shop holds is left out."),
                    <form method="post" action=(&answer_url) class="flex max-w-3xl flex-col gap-4" data-proposal-answer="">
                        for edit in &edits {
                            field(
                                attrs: attributes! { data-proposal-change=(&edit.field) },
                                field_label(attrs: attributes! { for=(format!("after-{}", edit.field)) }, (&edit.label))
                                <p class="whitespace-pre-line text-sm text-muted-foreground" data-proposal-before="">
                                    "Now: " (if edit.before.is_empty() { "(empty)" } else { edit.before.as_str() })
                                </p>
                                if edit.long {
                                    textarea(
                                        attrs: attributes! { id=(format!("after-{}", edit.field)) name=(format!("{AFTER_PREFIX}{}", edit.field)) rows="8" },
                                        (&edit.after)
                                    )
                                } else {
                                    input(attrs: attributes! { id=(format!("after-{}", edit.field)) name=(format!("{AFTER_PREFIX}{}", edit.field)) type="text" value=(&edit.after) })
                                }
                            )
                        }
                        <div class="flex flex-wrap gap-2">
                            button(attrs: attributes! { type="submit" name="verdict" value="approve" }, "Approve")
                        </div>
                    </form>
                )
            } else {
                section_card(
                    title: Some(if waiting { "What it would change" } else { "What it was about" }),
                    change_list(changes: &lines)
                    if can_answer && !expired && !may_change {
                        <p class="mt-4 text-sm text-muted-foreground" data-proposal-not-yours="">"Approving this change needs the permission to make it yourself."</p>
                    }
                )
            }

            if can_answer {
                section_card(
                    title: Some(if expired { "Dismiss" } else { "Refuse" }),
                    description: Some(if expired { "This proposal waited past its date: it can no longer be approved." } else { "Nothing changes; whoever prepared it reads why." }),
                    <form method="post" action=(&answer_url) class="flex max-w-3xl flex-col gap-4" data-proposal-refuse="">
                        field(
                            field_label(attrs: attributes! { for="proposal-refusal" }, "Why (optional)")
                            input(attrs: attributes! { id="proposal-refusal" name="reason" type="text" maxlength=(MAX_NOTE_LEN) })
                        )
                        <div>
                            button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" name="verdict" value="refuse" }, "Refuse")
                        </div>
                    </form>
                )
            }
        )
    })
}
