//! Answers several waiting proposals at once, from the list: the ones
//! ticked, or every one a group shows.

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

use super::{
    Answered, PICK_PREFIX, SHOWN_PREFIX, approve_one, gateway, proposals_url, refuse_one,
    tally_message,
};
use crate::app::t::slug::require_tenant;

#[derive(Deserialize)]
pub struct AnswerForm {
    /// `approve`, `refuse`, or either with `_all` for every row shown.
    verdict: String,
    /// The rows, among anything else posted.
    #[serde(flatten)]
    fields: HashMap<String, String>,
}

#[route(POST)]
pub async fn answer(cx: &Cx, Form(form): Form<AnswerForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PROPOSALS_APPROVE)
        .await?;
    let slug = access.tenant.slug.clone();
    let (approve, prefix) = match form.verdict.as_str() {
        "approve" => (true, PICK_PREFIX),
        "refuse" => (false, PICK_PREFIX),
        "approve_all" => (true, SHOWN_PREFIX),
        "refuse_all" => (false, SHOWN_PREFIX),
        _ => {
            return Ok(see_other(proposals_url(
                cx,
                &slug,
                Some("not_allowed"),
                None,
            )));
        }
    };
    let mut ids: Vec<&str> = form
        .fields
        .keys()
        .filter_map(|field| field.strip_prefix(prefix))
        .collect();
    ids.sort_unstable();
    if ids.is_empty() {
        return Ok(see_other(proposals_url(
            cx,
            &slug,
            Some("proposal_none_selected"),
            None,
        )));
    }
    let proposals = gateway(&access)?;
    let (mut done, mut out_of_date, mut left) = (0, 0, 0);
    for id in ids {
        let answered = if approve {
            match proposals
                .load_proposal(id)
                .await
                .map_err(topcoat::Error::from_anyhow)?
            {
                Some(proposal) => approve_one(cx, &access, &proposal, Vec::new()).await?,
                None => Answered::Left("proposal_not_found"),
            }
        } else {
            refuse_one(&access, id, "").await?
        };
        match answered {
            Answered::Done => done += 1,
            Answered::OutOfDate => out_of_date += 1,
            Answered::Left(_) => left += 1,
        }
    }
    Ok(see_other(proposals_url(
        cx,
        &slug,
        None,
        Some(&tally_message(approve, done, out_of_date, left)),
    )))
}
