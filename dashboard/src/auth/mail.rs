//! The e-mails this application sends: a code that proves an address, an
//! invitation to join a tenant, and, speaking for a shop, its shoppers'
//! sign-in codes, order confirmations and requests for a review.

use anyhow::Context as _;
use topcoat::{
    Result,
    context::{Cx, app_context},
    mail::{Mail, MailConfig, Mailbox, SmtpTransport, mail, send},
};

use timada_catalog::format_money;
use timada_orders::OrderState;
use timada_tenancy::TenantState;

use crate::auth::cookies::EmailFlowKind;
use crate::config;
use crate::shop_mail::shop_sender;

/// Who the codes come from (`mail.from`).
#[derive(Debug, Clone)]
pub struct MailSender(pub Mailbox);

/// What the router needs to send mail: the transport behind [`send`] and
/// the sender.
pub struct Mailer {
    pub transport: MailConfig,
    pub sender: MailSender,
}

/// From the configuration (already validated by `Config::load`).
pub fn build(config: &config::MailConfig) -> anyhow::Result<Mailer> {
    let transport = SmtpTransport::from_url(&config.smtp_url)
        .context("mail.smtp_url")?
        .build();
    let from: Mailbox = config.from.parse().context("mail.from")?;
    Ok(Mailer {
        transport: MailConfig::builder().transport(transport).build(),
        sender: MailSender(from),
    })
}

/// Mails `code` to `to`. The code is never logged; a delivery failure is.
pub async fn send_code(cx: &Cx, to: &str, kind: EmailFlowKind, code: &str) -> Result<()> {
    let mail = code_mail(cx, to, kind, code).await?;
    send(cx, mail).await?;
    Ok(())
}

/// Mails an invitation link. `link` is absolute (the recipient is not on
/// this site yet).
pub async fn send_invitation(
    cx: &Cx,
    to: &str,
    tenant_name: &str,
    inviter: &str,
    role_label: &str,
    link: &str,
) -> Result<()> {
    let from = app_context::<MailSender>(cx).0.clone();
    let subject = format!("You are invited to join {tenant_name} on Timada");
    let mail = mail! {
        from: from,
        to: Mailbox::new(to)?,
        subject: &subject,
        html: {
            cx =>
            <p><strong>(inviter)</strong> " invites you to join " <strong>(tenant_name)</strong> " as " <strong>(role_label)</strong> "."</p>
            <p><a href=(link)>"Accept the invitation"</a></p>
            <p>"Or open this address: " (link)</p>
            <p>"The link is valid for 7 days. If you do not know this tenant, you can ignore this message."</p>
        },
    }?;
    send(cx, mail).await?;
    Ok(())
}

/// Mails a shop's sign-in code to one of its shoppers. The mail speaks for
/// the shop, not for the platform: it comes from the shop's own server and
/// address when it has one, under the shop's name in any case.
pub async fn send_shopper_code(cx: &Cx, to: &str, shop: &TenantState, code: &str) -> Result<()> {
    let sender = shop_sender(cx, shop).await?;
    let shop_name = shop.name.as_str();
    let from = sender.from.clone();
    let subject = format!("Your sign-in code for {shop_name}");
    let mail = mail! {
        from: from,
        to: Mailbox::new(to)?,
        subject: &subject,
        html: {
            cx =>
            <p>"Enter this code to sign in to " <strong>(shop_name)</strong> ":"</p>
            <p style="font-size: 1.5rem; letter-spacing: 0.25rem;"><strong>(code)</strong></p>
            <p>"It is valid for 10 minutes."</p>
            <p>"If you did not ask for it, you can ignore this message."</p>
        },
    }?;
    sender.send(cx, mail).await?;
    Ok(())
}

/// Mails a shop's confirmation of a paid order to its shopper, from the
/// shop (see [`send_shopper_code`]). `link` is the order's page on the
/// shop's storefront, when the merchant said where it lives.
pub async fn send_order_confirmation(
    cx: &Cx,
    shop: &TenantState,
    order: &OrderState,
    link: Option<&str>,
) -> Result<()> {
    let sender = shop_sender(cx, shop).await?;
    let shop_name = shop.name.as_str();
    let from = sender.from.clone();
    let subject = format!("Your {shop_name} order {}", order.reference);
    let currency = &order.currency;
    let lines: Vec<(String, u32, String)> = order
        .lines
        .iter()
        .map(|line| {
            let name = match &line.variant_label {
                Some(variant) => format!("{} · {variant}", line.name),
                None => line.name.clone(),
            };
            (
                name,
                line.quantity,
                format_money(line.amount_cents(), currency),
            )
        })
        .collect();
    let discount = order.discount.as_ref().map(|discount| {
        (
            discount.code.clone(),
            format!("-{}", format_money(discount.amount_cents, currency)),
        )
    });
    let shipping = order.shipping_charge.as_ref().map(|shipping| {
        (
            shipping.name.clone(),
            match shipping.amount_cents {
                0 => "Free".to_owned(),
                cents => format_money(cents, currency),
            },
        )
    });
    let total = format_money(order.total_cents, currency);
    let ship = &order.shipping;
    let city = format!("{} {}", ship.postal_code, ship.city);
    let billing = order
        .billing
        .as_ref()
        .map(|bill| (bill, format!("{} {}", bill.postal_code, bill.city)));
    let mail = mail! {
        from: from,
        to: Mailbox::new(&order.email)?,
        subject: &subject,
        html: {
            cx =>
            <p>"Thank you for your order at " <strong>(shop_name)</strong> "."</p>
            <p>"Order " <strong>(&order.reference)</strong></p>
            <table cellpadding="4" style="border-collapse: collapse;">
                for (name, quantity, amount) in &lines {
                    <tr>
                        <td>(name)</td>
                        <td>"× " (quantity)</td>
                        <td style="text-align: right;">(amount)</td>
                    </tr>
                }
                if let Some((code, amount)) = &discount {
                    <tr>
                        <td colspan="2">"Discount " (code)</td>
                        <td style="text-align: right;">(amount)</td>
                    </tr>
                }
                if let Some((name, amount)) = &shipping {
                    <tr>
                        <td colspan="2">"Shipping: " (name)</td>
                        <td style="text-align: right;">(amount)</td>
                    </tr>
                }
                <tr>
                    <td colspan="2"><strong>"Total"</strong></td>
                    <td style="text-align: right;"><strong>(&total)</strong></td>
                </tr>
            </table>
            <p>
                "Shipping to:" <br>
                (&ship.name) <br>
                (&ship.line1) <br>
                if let Some(line2) = &ship.line2 {
                    (line2) <br>
                }
                (&city) <br>
                (&ship.country)
            </p>
            if let Some((bill, bill_city)) = &billing {
                <p>
                    "Billing to:" <br>
                    (&bill.name) <br>
                    (&bill.line1) <br>
                    if let Some(line2) = &bill.line2 {
                        (line2) <br>
                    }
                    (bill_city) <br>
                    (&bill.country)
                </p>
            }
            if let Some(note) = &order.note {
                <p>"Your note: " (note)</p>
            }
            if let Some(link) = link {
                <p><a href=(link)>"View your order"</a></p>
            }
        },
    }?;
    sender.send(cx, mail).await?;
    Ok(())
}

/// Asks the buyer of a fulfilled order, from the shop (see
/// [`send_shopper_code`]), what they think of `products` (the names of the
/// ones not reviewed yet). `link` opens the order's review forms on the
/// storefront without signing in.
pub async fn send_review_request(
    cx: &Cx,
    shop: &TenantState,
    order: &OrderState,
    products: &[String],
    link: &str,
) -> Result<()> {
    let sender = shop_sender(cx, shop).await?;
    let shop_name = shop.name.as_str();
    let from = sender.from.clone();
    let subject = format!("How was your {shop_name} order {}?", order.reference);
    let mail = mail! {
        from: from,
        to: Mailbox::new(&order.email)?,
        subject: &subject,
        html: {
            cx =>
            <p>"Thank you again for your order " <strong>(&order.reference)</strong>
                " at " <strong>(shop_name)</strong> "."</p>
            <p>"Now that it has reached you, how do you like it? A few words help other shoppers choose:"</p>
            <ul>
                for name in products {
                    <li>(name)</li>
                }
            </ul>
            <p><a href=(link)>"Review your purchase"</a></p>
            <p>"The link opens your order on the shop for 60 days."</p>
        },
    }?;
    sender.send(cx, mail).await?;
    Ok(())
}

fn subject(kind: EmailFlowKind) -> &'static str {
    match kind {
        EmailFlowKind::Register | EmailFlowKind::AddEmail => {
            "Confirm your e-mail address for Timada"
        }
        EmailFlowKind::SignIn => "Your Timada sign-in code",
        EmailFlowKind::ResetPassword => "Reset your Timada password",
    }
}

async fn code_mail(cx: &Cx, to: &str, kind: EmailFlowKind, code: &str) -> Result<Mail> {
    let from = app_context::<MailSender>(cx).0.clone();
    let intro = match kind {
        EmailFlowKind::Register => "Enter this code to finish creating your account:",
        EmailFlowKind::SignIn => "Enter this code to finish signing in:",
        EmailFlowKind::ResetPassword => "Enter this code to choose a new password:",
        EmailFlowKind::AddEmail => "Enter this code to add this address to your account:",
    };
    Ok(mail! {
        from: from,
        to: Mailbox::new(to)?,
        subject: subject(kind),
        html: {
            cx =>
            <p>(intro)</p>
            <p style="font-size: 1.5rem; letter-spacing: 0.25rem;"><strong>(code)</strong></p>
            <p>"It is valid for 10 minutes."</p>
            <p>"If you did not ask for it, you can ignore this message."</p>
        },
    }?)
}

#[cfg(test)]
mod tests {
    use topcoat::context::CxTestBuilder;
    use topcoat::mail::MemoryTransport;

    use super::*;
    use crate::shop_mail::ShopMail;

    fn acme() -> TenantState {
        TenantState {
            id: "t1".into(),
            name: "Acme Tea".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn the_code_reaches_the_address() {
        let transport = MemoryTransport::new();
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(transport.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
            .app_context(ShopMail::new(None, false))
            .build();

        send_code(&cx, "alice@example.com", EmailFlowKind::SignIn, "123456")
            .await
            .unwrap();

        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to(), [Mailbox::new("alice@example.com").unwrap()]);
        assert_eq!(sent[0].subject(), "Your Timada sign-in code");
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap()).unwrap();
        assert!(wire.contains("123456"), "{wire}");
        assert!(wire.contains("valid for 10 minutes"), "{wire}");
    }

    #[tokio::test]
    async fn the_shopper_code_comes_from_the_shop() {
        let transport = MemoryTransport::new();
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(transport.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
            .app_context(ShopMail::new(None, false))
            .build();

        send_shopper_code(&cx, "ada@example.com", &acme(), "654321")
            .await
            .unwrap();

        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].subject(), "Your sign-in code for Acme Tea");
        assert_eq!(sent[0].from().unwrap().name(), Some("Acme Tea"));
        assert_eq!(sent[0].from().unwrap().address(), "no-reply@example.com");
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap()).unwrap();
        assert!(wire.contains("654321"), "{wire}");
        assert!(!wire.contains("Timada"), "{wire}");
    }

    fn paid_order() -> OrderState {
        OrderState {
            id: "o1".into(),
            reference: "K7Q4-MZP2".into(),
            email: "ada@example.com".into(),
            shipping: timada_orders::ShippingAddress {
                name: "Ada Lovelace".into(),
                line1: "1 rue de la Paix".into(),
                line2: None,
                postal_code: "75002".into(),
                city: "Paris".into(),
                country: "FR".into(),
                phone: None,
            },
            note: Some("Leave it at the door".into()),
            lines: vec![timada_orders::OrderLine {
                name: "Mug".into(),
                variant_label: Some("Blue".into()),
                unit_amount_cents: 1250,
                quantity: 2,
                ..Default::default()
            }],
            currency: "EUR".into(),
            total_cents: 2250,
            discount: Some(timada_orders::OrderDiscount {
                code: "WELCOME".into(),
                subtotal_cents: 2500,
                amount_cents: 250,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn the_order_confirmation_comes_from_the_shop() {
        let transport = MemoryTransport::new();
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(transport.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
            .app_context(ShopMail::new(None, false))
            .build();

        let order = paid_order();
        send_order_confirmation(&cx, &acme(), &order, Some("https://shop.example/orders/o1"))
            .await
            .unwrap();
        send_order_confirmation(&cx, &acme(), &order, None)
            .await
            .unwrap();

        let sent = transport.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].subject(), "Your Acme Tea order K7Q4-MZP2");
        assert_eq!(sent[0].to(), [Mailbox::new("ada@example.com").unwrap()]);
        assert_eq!(sent[0].from().unwrap().name(), Some("Acme Tea"));
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap()).unwrap();
        for part in [
            "K7Q4-MZP2",
            "Mug",
            "Blue",
            "WELCOME",
            "Ada Lovelace",
            "Leave it at the door",
        ] {
            assert!(wire.contains(part), "{part}: {wire}");
        }
        assert!(wire.contains("https://shop.example/orders/o1"), "{wire}");
        assert!(!wire.contains("Timada"), "{wire}");
        let wire = String::from_utf8(sent[1].clone().formatted(&cx).unwrap()).unwrap();
        assert!(!wire.contains("View your order"), "{wire}");
    }

    #[tokio::test]
    async fn the_review_request_links_to_the_order() {
        let transport = MemoryTransport::new();
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(transport.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
            .app_context(ShopMail::new(None, false))
            .build();

        send_review_request(
            &cx,
            &acme(),
            &paid_order(),
            &["Mug".to_owned(), "Teapot".to_owned()],
            "https://shop.example/reviews/t0k3n",
        )
        .await
        .unwrap();

        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].subject(), "How was your Acme Tea order K7Q4-MZP2?");
        assert_eq!(sent[0].to(), [Mailbox::new("ada@example.com").unwrap()]);
        assert_eq!(sent[0].from().unwrap().name(), Some("Acme Tea"));
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap()).unwrap();
        for part in ["Mug", "Teapot", "https://shop.example/reviews/t0k3n"] {
            assert!(wire.contains(part), "{part}: {wire}");
        }
        assert!(!wire.contains("Timada"), "{wire}");
    }

    #[tokio::test]
    async fn the_invitation_carries_the_link() {
        let transport = MemoryTransport::new();
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(transport.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
            .app_context(ShopMail::new(None, false))
            .build();

        send_invitation(
            &cx,
            "bob@example.com",
            "Acme",
            "Ada",
            "Support",
            "https://timada.localhost/invitations/t0k3n",
        )
        .await
        .unwrap();

        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].subject(), "You are invited to join Acme on Timada");
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap()).unwrap();
        assert!(wire.contains("/invitations/t0k3n"), "{wire}");
        assert!(wire.contains("Ada"), "{wire}");
        assert!(wire.contains("Support"), "{wire}");
    }
}
