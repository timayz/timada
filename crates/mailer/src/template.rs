//! The e-mails themselves: plain text, in French like the storefront.

use timada_core::format::{date, money};
use timada_order::OrderDetailsView;

use crate::config::MailerConfig;

/// `(subject, body)` of an e-mail.
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
    lines.push(format!("  Total TTC — {}", money(&order.total)));
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
