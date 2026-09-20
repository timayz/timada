//! The built-in e-mails: plain text, in French like the demo storefront. They
//! are the default methods of [`crate::Templates`]; a host overrides the ones
//! it wants in its own words, language or HTML.

use timada_core::format::{date, money, vat_rate};
use timada_order::OrderDetailsView;
use timada_returns::ReturnView;

use crate::config::MailerConfig;

/// `(subject, body)` of a built-in e-mail.
pub(crate) type Content = (String, String);

fn signed(config: &MailerConfig, greeting_name: &str, paragraphs: &[String]) -> String {
    let mut body = format!("Bonjour {greeting_name},\n\n");
    body.push_str(&paragraphs.join("\n\n"));
    body.push_str(&format!("\n\nÀ bientôt,\n{}\n", config.shop_name));
    body
}

fn order_link(config: &MailerConfig, order: &OrderDetailsView) -> String {
    config.url(&format!("/account/orders/{}", order.id))
}

fn order_lines(order: &OrderDetailsView) -> String {
    let mut lines: Vec<String> = order
        .lines
        .iter()
        .map(|l| format!("  {} × {} — {}", l.quantity, l.name, money(&l.unit_price)))
        .collect();
    lines.push(format!("  Livraison — {}", money(&order.shipping_fee)));
    if order.handling_fee.is_positive() {
        lines.push(format!(
            "  Frais de dossier — {}",
            money(&order.handling_fee)
        ));
    }
    if let Some(discount) = &order.discount {
        lines.push(format!(
            "  Remise ({}) — − {}",
            discount.code,
            money(&discount.amount)
        ));
    }
    match &order.tax {
        Some(_) if order.reverse_charge.is_some() => {
            lines.push(format!("  Total HT — {}", money(&order.total)));
            lines.push(
                "  Vente hors TVA : livraison intracommunautaire, TVA autoliquidée par votre entreprise."
                    .to_owned(),
            );
        }
        Some(tax) if tax.treatment.exemption_mention().is_some() => {
            lines.push(format!("  Total HT — {}", money(&order.total)));
            lines.push(
                "  Vente hors TVA française (livraison hors du territoire fiscal).".to_owned(),
            );
        }
        Some(tax) => {
            lines.push(format!("  Total TTC — {}", money(&order.total)));
            for line in tax.vat_lines.iter().filter(|l| l.rate_bp > 0) {
                lines.push(format!(
                    "  dont TVA {} — {}",
                    vat_rate(line.rate_bp),
                    money(&line.vat)
                ));
            }
        }
        None => lines.push(format!("  Total TTC — {}", money(&order.total))),
    }
    lines.join("\n")
}

pub(crate) fn order_confirmation(
    config: &MailerConfig,
    first_name: &str,
    order: &OrderDetailsView,
) -> Content {
    let number = order.display_number();
    (
        format!("Confirmation de votre commande {number}"),
        signed(
            config,
            first_name,
            &[
                format!(
                    "Nous avons bien enregistré votre commande {number} du {}.",
                    date(order.placed_at)
                ),
                order_lines(order),
                format!("Suivre votre commande : {}", order_link(config, order)),
            ],
        ),
    )
}

pub(crate) fn order_shipped(
    config: &MailerConfig,
    first_name: &str,
    order: &OrderDetailsView,
) -> Content {
    let number = order.display_number();
    let carrier = order.carrier.as_deref().unwrap_or("notre transporteur");
    let tracking = match &order.tracking_number {
        Some(tracking) => format!("Numéro de suivi : {tracking}."),
        None => "Le numéro de suivi vous sera communiqué prochainement.".to_owned(),
    };
    (
        format!("Votre commande {number} a été expédiée"),
        signed(
            config,
            first_name,
            &[
                format!("Votre commande {number} vient d'être remise à {carrier}."),
                tracking,
                format!("Suivre votre commande : {}", order_link(config, order)),
            ],
        ),
    )
}

pub(crate) fn order_cancelled(
    config: &MailerConfig,
    first_name: &str,
    order: &OrderDetailsView,
    reason: &str,
) -> Content {
    let number = order.display_number();
    (
        format!("Votre commande {number} a été annulée"),
        signed(
            config,
            first_name,
            &[
                format!(
                    "Votre commande {number} a été annulée. Motif : {}.",
                    timada_order::cancellation_reason_label(reason)
                ),
                "Si un paiement avait été encaissé, il vous est remboursé ; vous recevrez un \
                 e-mail de confirmation du remboursement."
                    .to_owned(),
                format!("Détail de la commande : {}", order_link(config, order)),
            ],
        ),
    )
}

pub(crate) fn refund(
    config: &MailerConfig,
    first_name: &str,
    order: &OrderDetailsView,
    amount: &timada_core::Money,
) -> Content {
    let number = order.display_number();
    (
        format!(
            "Remboursement de {} sur votre commande {number}",
            money(amount)
        ),
        signed(
            config,
            first_name,
            &[
                format!(
                    "Nous vous avons remboursé {} sur votre commande {number}. Le montant \
                     apparaîtra sur votre moyen de paiement sous quelques jours.",
                    money(amount)
                ),
                format!(
                    "L'avoir correspondant est disponible sur la page de la commande : {}",
                    order_link(config, order)
                ),
            ],
        ),
    )
}

pub(crate) fn back_in_stock(
    config: &MailerConfig,
    product_id: &str,
    product_name: &str,
) -> Content {
    (
        format!("{product_name} est de nouveau disponible"),
        signed(
            config,
            "",
            &[
                format!(
                    "Vous nous aviez demandé de vous prévenir : {product_name} est de nouveau \
                     en stock."
                ),
                format!(
                    "Voir le produit : {}",
                    config.url(&format!("/p/{product_id}"))
                ),
                "Les quantités sont limitées ; cette alerte ne réserve pas le produit.".to_owned(),
            ],
        )
        .replacen("Bonjour ,", "Bonjour,", 1),
    )
}

pub(crate) fn question_answered(
    config: &MailerConfig,
    first_name: &str,
    product_id: &str,
    product_name: &str,
    question: &str,
    answer: &str,
) -> Content {
    (
        format!("Une réponse à votre question sur {product_name}"),
        signed(
            config,
            first_name,
            &[
                format!("Vous aviez demandé : « {question} »"),
                format!("Réponse : « {answer} »"),
                format!(
                    "Voir toutes les questions : {}",
                    config.url(&format!("/p/{product_id}#questions"))
                ),
            ],
        ),
    )
}

fn return_link(config: &MailerConfig, request: &ReturnView) -> String {
    config.url(&format!("/account/returns/{}", request.id))
}

fn return_lines(request: &ReturnView) -> String {
    request
        .lines
        .iter()
        .map(|l| format!("  {} × {}", l.quantity, l.name))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn return_approved(
    config: &MailerConfig,
    first_name: &str,
    request: &ReturnView,
) -> Content {
    let number = &request.rma_number;
    (
        format!("Votre retour {number} est accepté"),
        signed(
            config,
            first_name,
            &[
                format!("Votre demande de retour {number} est acceptée pour :"),
                return_lines(request),
                format!(
                    "Inscrivez le numéro {number} sur le colis et envoyez-le à :\n{}",
                    config.returns_address
                ),
                format!(
                    "Le bon de retour et le suivi de votre demande : {}",
                    return_link(config, request)
                ),
            ],
        ),
    )
}

pub(crate) fn return_refused(
    config: &MailerConfig,
    first_name: &str,
    request: &ReturnView,
) -> Content {
    let number = &request.rma_number;
    let reason = request.refused_reason.as_deref().unwrap_or("non précisé");
    (
        format!("Votre demande de retour {number} n'a pas été acceptée"),
        signed(
            config,
            first_name,
            &[
                format!("Nous ne pouvons pas accepter votre demande de retour {number}."),
                format!("Motif : {reason}"),
                format!(
                    "Le détail de votre demande : {}",
                    return_link(config, request)
                ),
            ],
        ),
    )
}

/// The parcel was handled. The card refund has its own e-mail; this one says
/// what was taken back and carries the store-credit code, if any.
pub(crate) fn return_completed(
    config: &MailerConfig,
    first_name: &str,
    request: &ReturnView,
) -> Content {
    let number = &request.rma_number;
    let accepted: u32 = request.received.iter().map(|l| l.accepted).sum();
    let mut paragraphs = vec![format!(
        "Nous avons bien reçu votre colis pour le retour {number} : {accepted} article(s) repris \
         sur {} demandé(s).",
        request.units()
    )];
    if request.money.is_positive() {
        paragraphs.push(format!(
            "Le remboursement de {} sur votre moyen de paiement est en cours ; un e-mail vous \
             le confirmera.",
            money(&request.money)
        ));
    }
    if let Some(code) = request
        .voucher_code
        .as_deref()
        .filter(|_| request.credit.is_positive())
    {
        paragraphs.push(format!(
            "{} vous sont crédités sous forme d'avoir : saisissez le code {code} dans votre \
             panier lors d'une prochaine commande.",
            money(&request.credit)
        ));
    }
    if !request.money.is_positive() && !request.credit.is_positive() {
        paragraphs
            .push("Aucun article n'ayant pu être repris, aucun remboursement n'est dû.".to_owned());
    }
    paragraphs.push(format!(
        "Le détail de votre retour : {}",
        return_link(config, request)
    ));
    (
        format!("Votre retour {number} est traité"),
        signed(config, first_name, &paragraphs),
    )
}

pub(crate) fn welcome(config: &MailerConfig, first_name: &str) -> Content {
    (
        format!("Bienvenue chez {}", config.shop_name),
        signed(
            config,
            first_name,
            &[
                format!(
                    "Votre compte {} est créé. Vous y retrouverez vos commandes, vos factures, \
                     vos retours et vos paniers sauvegardés.",
                    config.shop_name
                ),
                format!("Votre compte : {}", config.url("/account")),
            ],
        ),
    )
}

pub(crate) fn review_published(
    config: &MailerConfig,
    first_name: &str,
    product_id: &str,
    product_name: &str,
) -> Content {
    (
        format!("Votre avis sur {product_name} est en ligne"),
        signed(
            config,
            first_name,
            &[
                format!("Merci ! Votre avis sur {product_name} est maintenant visible de tous."),
                format!("Le voir : {}", config.url(&format!("/p/{product_id}#avis"))),
            ],
        ),
    )
}

pub(crate) fn review_rejected(
    config: &MailerConfig,
    first_name: &str,
    product_name: &str,
    reason: &str,
) -> Content {
    (
        format!("Votre avis sur {product_name} n'a pas été publié"),
        signed(
            config,
            first_name,
            &[
                format!("Nous n'avons pas pu publier votre avis sur {product_name}."),
                format!("Motif : {reason}"),
            ],
        ),
    )
}

pub(crate) fn question_refused(
    config: &MailerConfig,
    first_name: &str,
    product_name: &str,
    question: &str,
    reason: &str,
) -> Content {
    (
        format!("Votre question sur {product_name} n'a pas été publiée"),
        signed(
            config,
            first_name,
            &[
                format!("Vous aviez demandé : « {question} »"),
                format!("Nous n'avons pas pu la publier. Motif : {reason}"),
            ],
        ),
    )
}

#[cfg(feature = "invoice-pdf")]
pub(crate) fn credit_note_issued(
    config: &MailerConfig,
    first_name: &str,
    credit_note: &timada_invoice::CreditNoteDocument,
) -> Content {
    let heading = if credit_note.amounts_include_vat {
        "Montant TTC"
    } else {
        "Montant HT"
    };
    (
        format!("Votre avoir {}", credit_note.number),
        signed(
            config,
            first_name,
            &[
                format!(
                    "Vous trouverez en pièce jointe (PDF) l'avoir {} : il documente le \
                     remboursement fait sur votre commande {} et vient en déduction de la \
                     facture {}.",
                    credit_note.number, credit_note.order_label, credit_note.invoice_number
                ),
                format!("  Motif — {}", credit_note.reason),
                format!("  {heading} — {}", money(&credit_note.amount)),
                format!(
                    "Il reste disponible dans votre compte : {}",
                    config.url(&format!("/account/orders/{}", credit_note.order_id))
                ),
            ],
        ),
    )
}

#[cfg(feature = "invoice-pdf")]
pub(crate) fn invoice_issued(
    config: &MailerConfig,
    first_name: &str,
    invoice: &timada_invoice::InvoiceDocument,
) -> Content {
    let heading = if invoice.amounts_include_vat {
        "Total TTC"
    } else {
        "Total HT"
    };
    (
        format!("Votre facture {}", invoice.number),
        signed(
            config,
            first_name,
            &[
                format!(
                    "Votre commande {} est réglée : vous trouverez sa facture {} en pièce jointe (PDF).",
                    invoice.order_label, invoice.number
                ),
                format!("  {heading} — {}", money(&invoice.total)),
                format!(
                    "Elle reste disponible dans votre compte : {}",
                    config.url(&format!("/account/orders/{}", invoice.order_id))
                ),
            ],
        ),
    )
}
