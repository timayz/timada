//! `/{mount}/emails/{message_id}`: one e-mail as it was written, its delivery
//! state, and a retry for the ones the relay refused too many times.

use timada_mailer::{OutboxStatus, load_outbox_message, outbox_attachments, retry};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::RouterErrorExt, error::see_other, href, page, path_param, path_param as param,
    },
    view::{View, view},
};

use super::outbox_status_badge;
use crate::{
    auth::Section,
    components::{
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
    },
    config::AdminServices,
    ui::{date, detail_grid, detail_main, detail_side, fact, facts, page_header},
};

path_param!(pub message_id: String, error = not_found);

#[page]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let id = param::<MessageId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    let message = load_outbox_message(&services.db, &id)
        .await?
        .ok_or_not_found()?;
    let status = message.status();
    let attachments: Vec<String> = outbox_attachments(&services.db, &id)
        .await?
        .into_iter()
        .map(|file| format!("{} ({} Ko)", file.file_name, (file.size + 1_023) / 1_024))
        .collect();
    let sent = message
        .sent_at
        .map_or_else(|| "—".to_owned(), |at| date(at.max(0) as u64));
    // A failed e-mail waits before its next attempt.
    let waiting = (status == OutboxStatus::Pending && message.attempts > 0)
        .then(|| timada_core::time::now_unix_secs().ok())
        .flatten()
        .map(|now| (message.next_attempt_at - now as i64).max(0))
        .map(|secs| match secs {
            0 => "au prochain passage".to_owned(),
            s if s < 120 => format!("dans {s} s"),
            s => format!("dans {} min", s / 60),
        });

    Ok(view! {
        page_header(
            parent: Section::Emails,
            title: &message.subject,
            outbox_status_badge(status: status)
        )
        <p class="-mt-4 mb-6 font-mono text-xs text-muted-foreground">(id.clone()) " · " (message.kind.clone())</p>

        detail_grid(
            detail_main(
                card(
                    card_header(card_title("Message"))
                    card_content(
                        <pre class="whitespace-pre-wrap font-sans text-sm">(message.body.clone())</pre>
                    )
                )
            )
            detail_side(
                card(
                    card_header(card_title("Envoi"))
                    card_content(
                        facts(
                            fact(term: "De", (message.sender.clone()))
                            fact(term: "À", (message.recipient.clone()))
                            fact(term: "Créé le", (date(message.created_at.max(0) as u64)))
                            fact(term: "Envoyé le", (sent))
                            fact(term: "Essais", class: "tabular-nums", (message.attempts.to_string()))
                            if let Some(waiting) = &waiting {
                                fact(term: "Prochain essai", (waiting.clone()))
                            }
                            if message.html_body.is_some() {
                                fact(term: "Format", "Texte et HTML")
                            }
                            if !attachments.is_empty() {
                                <div>
                                    <dt class="text-muted-foreground">"Pièces jointes"</dt>
                                    for file in &attachments { <dd>(file.clone())</dd> }
                                </div>
                            }
                            if let Some(error) = &message.last_error {
                                fact(term: "Dernière erreur", class: "text-destructive", (error.clone()))
                            }
                        )
                    )
                )
                if status == OutboxStatus::Failed {
                    card(
                        card_header(card_title("Actions"))
                        card_content(
                            <form method="post" action=(href!(retry_delivery, MessageId(id.clone())))>
                                button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" class="w-full" }, "Réessayer l'envoi")
                            </form>
                        )
                    )
                }
            )
        )
    })
}

/// Gives a failed e-mail a fresh set of attempts; the delivery worker picks
/// it up on its next pass.
#[page(POST "./retry")]
pub async fn retry_delivery(cx: &Cx) -> Result<impl View> {
    let id = param::<MessageId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    retry(&services.db, &id).await?;
    Err::<(), _>(see_other(href!(show, MessageId(id)).resolve(cx)).into())
}
