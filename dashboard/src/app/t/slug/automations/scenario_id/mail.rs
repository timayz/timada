//! The mails a scenario sends customers, checked and previewed as the
//! merchant writes them: templates over the shop's variables (and the
//! order's, for a scenario about orders or reviews), which must link to
//! `{{ unsubscribe_url }}`, the page that stops the shop's automated mails.

use timada_automations::{
    Definition, Step,
    catalog::{self, EMAIL, Subject},
};
use timada_tenancy::{
    MailTemplateError, RenderedMail, ShopMailKind, TenantState, links_to, render, render_text,
    validate, validate_text,
};
use topcoat::context::Cx;

use crate::mail::{automation_kind, shop_vars};

/// The kind a scenario's mails are written as: about an order when its
/// trigger is about one (or a review of one).
pub fn kind_of(definition: &Definition) -> ShopMailKind {
    automation_kind(matches!(
        definition.subject(),
        Some(Subject::Order | Subject::Review)
    ))
}

/// Whether a scenario mails customers.
pub fn sends_mail(definition: &Definition) -> bool {
    definition.walk().iter().any(|step| step.kind == EMAIL)
}

/// A mail step rendered with made-up variables, or why it does not read.
pub fn preview(
    cx: &Cx,
    tenant: &TenantState,
    definition: &Definition,
    step: &Step,
) -> Result<RenderedMail, MailTemplateError> {
    let settings = catalog::parse_email(&step.settings)
        .map_err(|_| MailTemplateError("Write a subject and a body.".into()))?;
    let kind = kind_of(definition);
    validate(kind, &settings.subject, &settings.html)?;
    if let Some(text) = &settings.text {
        validate_text(kind, text)?;
    }
    // The rendered mail must carry the link, not merely mention the variable.
    let vars = kind.sample(shop_vars(cx, tenant));
    let rendered = render(
        &settings.subject,
        &settings.html,
        settings.text.as_deref(),
        &vars,
    )?;
    let unsubscribe = render_text("{{ unsubscribe_url }}", &vars)?;
    if !links_to(&rendered.html, &unsubscribe) {
        return Err(MailTemplateError(
            "Link to {{ unsubscribe_url }}: every automated mail lets its reader stop them.".into(),
        ));
    }
    Ok(rendered)
}

/// Whether every mail of the scenario reads.
pub fn mails_read(cx: &Cx, tenant: &TenantState, definition: &Definition) -> bool {
    definition
        .walk()
        .iter()
        .filter(|step| step.kind == EMAIL)
        .all(|step| preview(cx, tenant, definition, step).is_ok())
}
