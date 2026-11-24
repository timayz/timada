//! `/t/{slug}/proposals/new?kind=…&subject=…`, for members with
//! `proposals:prepare` who may see the subject: a change to a product or a
//! category written by hand, for someone else to approve. The form opens on
//! what the subject holds; the fields left as they are stay out of the
//! proposal. A refused one comes back with what was typed.

use std::collections::HashMap;

use serde::Deserialize;
use timada_proposals::{
    FIELD_BODY, FIELD_DESCRIPTION, FIELD_PRICE, KIND_NEW_CATEGORY, MAX_REASON_LEN, SOURCE_PERSON,
};
use timada_tenancy::PERMISSION_PROPOSALS_PREPARE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{forbidden, not_found, see_other},
        href, page, query_params,
    },
    view::{View, attributes, component, view},
};

use super::proposals_url;
use crate::app::t::slug::{TenantAccess, TenantNav, require_tenant, slug};
use crate::auth::now;
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::textarea::textarea;
use crate::proposals::{
    Catalog, Draft, Subject, field_label as label_of, fields_of, kind_label, permissions_of,
    subject,
};
use crate::ui::{Shell, document, notice, page_header, section_card};

/// What the fields of the values are named after: followed by the field.
const VALUE_PREFIX: &str = "value:";

#[topcoat::router::query_params(error = bad_request)]
struct NewQuery {
    kind: Option<String>,
    /// The product or the category; for a category to create, the one
    /// above it (none for the top level).
    subject: Option<String>,
}

#[derive(serde::Serialize)]
struct NewLink<'a> {
    kind: &'a str,
    subject: &'a str,
}

/// The form for a change to the subject `subject_id` of `kind`.
pub fn new_url(cx: &Cx, slug: &str, kind: &str, subject_id: &str) -> String {
    href!(new, slug: slug.to_owned())
        .query(NewLink {
            kind,
            subject: subject_id,
        })
        .resolve(cx)
}

#[derive(Deserialize)]
pub struct NewForm {
    kind: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    reason: String,
    /// The values, among anything else posted.
    #[serde(flatten)]
    fields: HashMap<String, String>,
}

/// One field of the form.
struct Input {
    field: &'static str,
    label: String,
    /// What the subject holds.
    held: String,
    /// What the form shows.
    value: String,
    long: bool,
}

/// What both handlers need: the member may prepare a change of `kind`, and
/// its subject is there.
async fn open(cx: &Cx, kind: &str, subject_id: &str) -> Result<(TenantAccess, Subject)> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PROPOSALS_PREPARE)
        .await?;
    let Some((read, _)) = permissions_of(kind) else {
        return Err(not_found().into());
    };
    if !access.can(cx, read).await? {
        return Err(forbidden().into());
    }
    let catalog = Catalog::new(access.executor()?.clone(), access.tenant_id());
    let held = subject(&catalog, kind, subject_id, None)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(not_found)?;
    Ok((access, held))
}

/// The fields of the form: those of the kind the subject holds, with what
/// was typed where something was. A product sold through its variants has
/// no price of its own to change here.
fn inputs(kind: &str, held: &Subject, typed: &HashMap<String, String>) -> Vec<Input> {
    fields_of(kind)
        .iter()
        .filter(|field| **field != FIELD_PRICE || !held.value(FIELD_PRICE).unwrap_or("").is_empty())
        .map(|field| {
            let now = held.value(field).unwrap_or_default().to_owned();
            Input {
                field,
                label: label_of(field),
                value: typed
                    .get(&format!("{VALUE_PREFIX}{field}"))
                    .cloned()
                    .unwrap_or_else(|| now.clone()),
                held: now,
                long: matches!(*field, FIELD_DESCRIPTION | FIELD_BODY),
            }
        })
        .collect()
}

/// What the page is headed with.
fn heading(kind: &str, held: &Subject) -> String {
    if kind == KIND_NEW_CATEGORY {
        format!("Suggest a category under {}", held.name)
    } else {
        format!("Suggest a change to {}", held.name)
    }
}

#[page]
pub async fn new(cx: &Cx) -> Result<impl View> {
    let query = query_params::<NewQuery>(cx)?;
    let kind = query.kind.clone().unwrap_or_default();
    let subject_id = query.subject.clone().unwrap_or_default();
    let (access, held) = open(cx, &kind, &subject_id).await?;
    let nav = TenantNav::of(cx, &access).await?;
    let tenant = access.tenant;
    let heading = heading(&kind, &held);
    let title = format!("{heading} · Proposals · {}", tenant.name);
    let back = proposals_url(cx, &tenant.slug, None, None);
    let inputs = inputs(&kind, &held, &HashMap::new());
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "proposals"),
            page_header(
                title: &heading,
                subtitle: Some("Nothing changes until a member who may approve says yes."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Proposals"</a>
            )
            new_form(slug: &tenant.slug, kind: &kind, subject_id: &subject_id, inputs: &inputs, reason: "")
        )
    })
}

#[page(POST)]
pub async fn prepare(cx: &Cx, Form(form): Form<NewForm>) -> Result<impl View> {
    let (access, held) = open(cx, &form.kind, &form.subject).await?;
    let inputs = inputs(&form.kind, &held, &form.fields);
    let prepared = crate::proposals::prepare(
        access.executor()?,
        access.tenant_id(),
        &access.user_id,
        Draft {
            kind: form.kind.clone(),
            subject_id: form.subject.clone(),
            values: inputs
                .iter()
                .map(|input| (input.field.to_owned(), input.value.clone()))
                .collect(),
            reason: form.reason.clone(),
            source: SOURCE_PERSON.to_owned(),
            valid_until: None,
        },
        now(),
    )
    .await;
    let error = match prepared {
        Ok(_) => {
            return Err(see_other(proposals_url(
                cx,
                &access.tenant.slug,
                None,
                Some("Prepared: it waits for an answer."),
            ))
            .into());
        }
        Err(refusal) => match refusal.code() {
            Some(code) => code,
            None => return Err(topcoat::Error::from_anyhow(refusal.into())),
        },
    };
    let nav = TenantNav::of(cx, &access).await?;
    let tenant = access.tenant;
    let heading = heading(&form.kind, &held);
    let title = format!("{heading} · Proposals · {}", tenant.name);
    let back = proposals_url(cx, &tenant.slug, None, None);
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "proposals"),
            page_header(
                title: &heading,
                subtitle: Some("Nothing changes until a member who may approve says yes."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Proposals"</a>
            )
            notice(error: Some(error))
            new_form(slug: &tenant.slug, kind: &form.kind, subject_id: &form.subject, inputs: &inputs, reason: &form.reason)
        )
    })
}

#[component]
async fn new_form(
    cx: &Cx,
    slug: &str,
    kind: &str,
    subject_id: &str,
    inputs: &[Input],
    reason: &str,
) -> Result<impl View> {
    let about = kind_label(kind);
    Ok(view! {
        section_card(
            title: Some(about),
            description: Some("Change the fields you want changed and leave the others as they are."),
            <form method="post" action=(href!(prepare, slug: slug.to_owned())) class="flex max-w-3xl flex-col gap-4" data-proposal-form="">
                <input type="hidden" name="kind" value=(kind)>
                <input type="hidden" name="subject" value=(subject_id)>
                for input in inputs {
                    field(
                        field_label(attrs: attributes! { for=(format!("proposal-{}", input.field)) }, (&input.label))
                        if input.long {
                            textarea(
                                attrs: attributes! { id=(format!("proposal-{}", input.field)) name=(format!("{VALUE_PREFIX}{}", input.field)) rows="8" },
                                (&input.value)
                            )
                        } else {
                            input(attrs: attributes! { id=(format!("proposal-{}", input.field)) name=(format!("{VALUE_PREFIX}{}", input.field)) type="text" value=(&input.value) })
                        }
                        if input.value != input.held {
                            <p class="text-sm text-muted-foreground">"Now: " (if input.held.is_empty() { "(empty)" } else { input.held.as_str() })</p>
                        }
                    )
                }
                field(
                    field_label(attrs: attributes! { for="proposal-reason" }, "Why (optional)")
                    field_description(attrs: attributes! { id="proposal-reason-hint" }, "What whoever approves should know.")
                    textarea(
                        attrs: attributes! { id="proposal-reason" name="reason" rows="3" maxlength=(MAX_REASON_LEN) aria-describedby="proposal-reason-hint" },
                        (reason)
                    )
                )
                <div>button(attrs: attributes! { type="submit" }, "Send for approval")</div>
            </form>
        )
    })
}
