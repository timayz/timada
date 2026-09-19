//! `/{mount}/emails/{message_id}`: one e-mail as it was written, its delivery
//! state, and a retry for the ones the relay refused too many times.

use timada_mailer::{OutboxStatus, load_outbox_message, retry};
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
    components::{
        button::{ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
    },
    config::AdminServices,
    ui::{date, page_header},
};

path_param!(pub message_id: String, error = not_found);

#[page]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let id = param::<MessageId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    let message = load_outbox_message(&services.db, &id)
        .await
        .map_err(anyhow::Error::from)?
        .ok_or_not_found()?;
    let status = message.status();
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
            title: &message.subject,
            outbox_status_badge(status: status)
        )
        <p class="-mt-4 mb-6 font-mono text-xs text-muted-foreground">(id.clone()) " · " (message.kind.clone())</p>

        <div class="grid gap-6 lg:grid-cols-3">
            <div class="lg:col-span-2">
                card(
                    card_header(card_title("Message"))
                    card_content(
                        <pre class="whitespace-pre-wrap font-sans text-sm">(message.body.clone())</pre>
                    )
                )
            </div>
            <div class="flex flex-col gap-6">
                card(
                    card_header(card_title("Envoi"))
                    card_content(
                        <dl class="flex flex-col gap-2 text-sm">
                            <div><dt class="text-muted-foreground">"De"</dt><dd>(message.sender.clone())</dd></div>
                            <div><dt class="text-muted-foreground">"À"</dt><dd>(message.recipient.clone())</dd></div>
                            <div><dt class="text-muted-foreground">"Créé le"</dt><dd>(date(message.created_at.max(0) as u64))</dd></div>
                            <div><dt class="text-muted-foreground">"Envoyé le"</dt><dd>(sent)</dd></div>
                            <div><dt class="text-muted-foreground">"Essais"</dt><dd class="tabular-nums">(message.attempts.to_string())</dd></div>
                            if let Some(waiting) = &waiting {
                                <div><dt class="text-muted-foreground">"Prochain essai"</dt><dd>(waiting.clone())</dd></div>
                            }
                            if message.html_body.is_some() {
                                <div><dt class="text-muted-foreground">"Format"</dt><dd>"Texte et HTML"</dd></div>
                            }
                            if let Some(error) = &message.last_error {
                                <div><dt class="text-muted-foreground">"Dernière erreur"</dt><dd class="text-destructive">(error.clone())</dd></div>
                            }
                        </dl>
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
            </div>
        </div>
    })
}

/// Gives a failed e-mail a fresh set of attempts; the delivery worker picks
/// it up on its next pass.
#[page(POST "./retry")]
pub async fn retry_delivery(cx: &Cx) -> Result<impl View> {
    let id = param::<MessageId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    retry(&services.db, &id)
        .await
        .map_err(anyhow::Error::from)?;
    Err::<(), _>(see_other(href!(show, MessageId(id)).resolve(cx)).into())
}
