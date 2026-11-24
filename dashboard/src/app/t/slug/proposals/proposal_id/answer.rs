//! Answers one proposal: approved, with the values the member changed, or
//! refused, with why.

use std::collections::HashMap;

use serde::Deserialize;
use timada_tenancy::PERMISSION_PROPOSALS_APPROVE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{AFTER_PREFIX, proposal_url, require_proposal};
use crate::app::t::slug::proposals::{Answered, approve_one, refuse_one};
use crate::app::t::slug::require_tenant;

#[derive(Deserialize)]
pub struct AnswerForm {
    /// `approve` or `refuse`.
    verdict: String,
    #[serde(default)]
    reason: String,
    /// The values, among anything else posted.
    #[serde(flatten)]
    fields: HashMap<String, String>,
}

/// A text as it compares whatever line ends the browser posted it with.
fn lines(value: &str) -> String {
    value.replace("\r\n", "\n")
}

#[route(POST)]
pub async fn answer(cx: &Cx, Form(form): Form<AnswerForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PROPOSALS_APPROVE)
        .await?;
    let proposal = require_proposal(cx, &access).await?;
    let slug = access.tenant.slug.clone();
    let (answered, message) = if form.verdict == "approve" {
        // What the member changed: a value posted as it was shown is none.
        let edits = proposal
            .changes
            .iter()
            .filter_map(|change| {
                let posted = form
                    .fields
                    .get(&format!("{AFTER_PREFIX}{}", change.field))?;
                (lines(posted).trim() != lines(&change.after).trim())
                    .then(|| (change.field.clone(), posted.clone()))
            })
            .collect();
        (
            approve_one(cx, &access, &proposal, edits).await?,
            "Approved: the change is being made.",
        )
    } else {
        (
            refuse_one(&access, &proposal.id, &form.reason).await?,
            "Refused.",
        )
    };
    let (error, message) = match answered {
        Answered::Done => (None, Some(message)),
        Answered::OutOfDate => (Some("proposal_out_of_date"), None),
        Answered::Left(code) => (Some(code), None),
    };
    Ok(see_other(proposal_url(
        cx,
        &slug,
        &proposal.id,
        error,
        message,
    )))
}
