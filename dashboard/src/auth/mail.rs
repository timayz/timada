//! The e-mails this application sends: a code that proves an address, an
//! invitation to join a tenant, and, speaking for a shop, its shoppers'
//! sign-in codes, order confirmations and requests for a review.

use std::sync::Arc;

use anyhow::Context as _;
use topcoat::{
    Result,
    context::{Cx, app_context},
    mail::{Mail, MailConfig, Mailbox, SmtpTransport, TextBody, mail, send},
    view::Unescaped,
};

use timada_catalog::format_money;
use timada_orders::OrderState;
use timada_reviews::REVIEW_LINK_TTL_SECS;
use timada_shoppers::CODE_TTL_SECS;
use timada_tenancy::{
    AddressVars, DiscountVars, LineVars, MailVars, OrderConfirmationVars, OrderVars, RenderedMail,
    ReviewOrderVars, ReviewRequestVars, ShippingMethodVars, ShopMailKind, ShopVars,
    ShopperCodeVars, TenantState, render, render_text,
};

use crate::auth::cookies::EmailFlowKind;
use crate::config;
use crate::media::Media;
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

/// Sends one of a shop's mails to `to`, in the merchant's wording when the
/// shop has one and in the built-in one otherwise. The wording is the
/// shopper's `language`'s when the shop mails in it and worded the mail,
/// else the default language's ([`TenantState::mail_template_for`]). The
/// mail speaks for the shop, not for the platform: it comes from the
/// shop's own server and address when it has one, under the shop's name in
/// any case.
///
/// A merchant's template that fails to render (it was checked when saved,
/// but an order may hold what the sample did not) is logged and the
/// built-in wording sent instead, so the shopper still gets the mail. A
/// plain-text part that fails alone is logged too, and the text derived
/// from the HTML sent instead. The merchant's plain text never goes out
/// with the built-in HTML.
async fn send_shop_mail(
    cx: &Cx,
    shop: &TenantState,
    kind: ShopMailKind,
    to: &str,
    vars: &MailVars,
    language: Option<&str>,
) -> Result<()> {
    let rendered = match shop.mail_template_for(kind, language) {
        Some(template) => match render(&template.subject, &template.html, None, vars) {
            Ok(mut rendered) => {
                // The merchant's plain text goes only with the merchant's HTML.
                if let Some(text) = template.text.as_deref() {
                    match render_text(text, vars) {
                        Ok(text) => rendered.text = Some(text),
                        Err(err) => tracing::warn!(
                            tenant = %shop.id,
                            mail = kind.key(),
                            language = language.unwrap_or("-"),
                            error = %err,
                            "the shop's plain-text part failed, sending the text derived from the HTML"
                        ),
                    }
                }
                Ok(rendered)
            }
            Err(err) => {
                tracing::warn!(
                    tenant = %shop.id,
                    mail = kind.key(),
                    language = language.unwrap_or("-"),
                    error = %err,
                    "the shop's mail template failed, sending the built-in one"
                );
                render(kind.default_subject(), kind.default_html(), None, vars)
            }
        },
        None => render(kind.default_subject(), kind.default_html(), None, vars),
    }
    .map_err(|err| topcoat::Error::from_anyhow(anyhow::anyhow!("{}: {err}", kind.key())))?;
    send_rendered(cx, shop, to, rendered).await
}

/// Sends a rendered shop mail from the shop (see [`send_shop_mail`]).
pub async fn send_rendered(
    cx: &Cx,
    shop: &TenantState,
    to: &str,
    rendered: RenderedMail,
) -> Result<()> {
    let sender = shop_sender(cx, shop).await?;
    let from = sender.from.clone();
    // The template's markup was checked when it was saved (no scripts,
    // frames, forms or handlers) and every variable in it is escaped, so the
    // rendered HTML is safe to send as is.
    let html = Unescaped::new_unchecked(rendered.html);
    // The merchant's own plain text, or the one derived from the HTML.
    let text = rendered.text.map_or(TextBody::FromHtml, TextBody::Text);
    let mail = mail! {
        from: from,
        to: Mailbox::new(to)?,
        subject: &rendered.subject,
        html: {
            cx =>
            (html)
        },
        text: text,
    }?;
    sender.send(cx, mail).await?;
    Ok(())
}

/// The shop as its mails' templates see it: its name and branding, the
/// logo as an address a mail client can load.
pub fn shop_vars(cx: &Cx, shop: &TenantState) -> ShopVars {
    let branding = shop.mail_branding.as_ref();
    ShopVars {
        name: shop.name.clone(),
        logo_url: branding
            .and_then(|branding| branding.logo_file.as_deref())
            .map(|file| app_context::<Arc<Media>>(cx).url(&shop.id, file)),
        accent: branding.and_then(|branding| branding.accent.clone()),
        footer: branding.and_then(|branding| branding.footer.clone()),
    }
}

/// Mails a shop's sign-in code to one of its shoppers. `language` is the
/// shopper's, as the storefront reported it with the request.
pub async fn send_shopper_code(
    cx: &Cx,
    to: &str,
    shop: &TenantState,
    code: &str,
    language: Option<&str>,
) -> Result<()> {
    let vars = MailVars::ShopperCode(ShopperCodeVars {
        shop: shop_vars(cx, shop),
        code: code.to_owned(),
        valid_minutes: CODE_TTL_SECS / 60,
    });
    send_shop_mail(cx, shop, ShopMailKind::ShopperCode, to, &vars, language).await
}

fn address_vars(
    name: &str,
    line1: &str,
    line2: Option<&String>,
    postal_code: &str,
    city: &str,
    country: &str,
) -> AddressVars {
    AddressVars {
        name: name.to_owned(),
        line1: line1.to_owned(),
        line2: line2.cloned(),
        postal_code: postal_code.to_owned(),
        city: city.to_owned(),
        country: country.to_owned(),
    }
}

/// An order as the order confirmation's template sees it, its amounts
/// worded with the currency.
fn order_vars(order: &OrderState) -> OrderVars {
    let currency = &order.currency;
    let ship = &order.shipping;
    OrderVars {
        reference: order.reference.clone(),
        email: order.email.clone(),
        lines: order
            .lines
            .iter()
            .map(|line| LineVars {
                name: line.name.clone(),
                variant: line.variant_label.clone(),
                quantity: line.quantity,
                amount: format_money(line.amount_cents(), currency),
            })
            .collect(),
        subtotal: format_money(order.subtotal_cents(), currency),
        discount: order.discount.as_ref().map(|discount| DiscountVars {
            code: discount.code.clone(),
            amount: format_money(discount.amount_cents, currency),
        }),
        shipping_method: order
            .shipping_charge
            .as_ref()
            .map(|shipping| ShippingMethodVars {
                name: shipping.name.clone(),
                amount: format_money(shipping.amount_cents, currency),
                free: shipping.amount_cents == 0,
            }),
        total: format_money(order.total_cents, currency),
        shipping_address: address_vars(
            &ship.name,
            &ship.line1,
            ship.line2.as_ref(),
            &ship.postal_code,
            &ship.city,
            &ship.country,
        ),
        billing_address: order.billing.as_ref().map(|bill| {
            address_vars(
                &bill.name,
                &bill.line1,
                bill.line2.as_ref(),
                &bill.postal_code,
                &bill.city,
                &bill.country,
            )
        }),
        note: order.note.clone(),
    }
}

/// Mails a shop's confirmation of a paid order to its shopper. `link` is
/// the order's page on the shop's storefront, when the merchant said where
/// it lives.
pub async fn send_order_confirmation(
    cx: &Cx,
    shop: &TenantState,
    order: &OrderState,
    link: Option<&str>,
) -> Result<()> {
    let vars = MailVars::OrderConfirmation(Box::new(OrderConfirmationVars {
        shop: shop_vars(cx, shop),
        order: order_vars(order),
        link: link.map(str::to_owned),
    }));
    send_shop_mail(
        cx,
        shop,
        ShopMailKind::OrderConfirmation,
        &order.email,
        &vars,
        order.language.as_deref(),
    )
    .await
}

/// Asks the buyer of a fulfilled order, from the shop, what they think of
/// `products` (the names of the ones not reviewed yet). `link` opens the
/// order's review forms on the storefront without signing in.
pub async fn send_review_request(
    cx: &Cx,
    shop: &TenantState,
    order: &OrderState,
    products: &[String],
    link: &str,
) -> Result<()> {
    let vars = MailVars::ReviewRequest(ReviewRequestVars {
        shop: shop_vars(cx, shop),
        order: ReviewOrderVars {
            reference: order.reference.clone(),
        },
        products: products.to_vec(),
        link: link.to_owned(),
        valid_days: REVIEW_LINK_TTL_SECS / (24 * 60 * 60),
    });
    send_shop_mail(
        cx,
        shop,
        ShopMailKind::ReviewRequest,
        &order.email,
        &vars,
        order.language.as_deref(),
    )
    .await
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

        send_shopper_code(&cx, "ada@example.com", &acme(), "654321", None)
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
    async fn a_shop_writes_its_own_plain_text() {
        let transport = MemoryTransport::new();
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(transport.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
            .app_context(ShopMail::new(None, false))
            .build();
        let mut shop = acme();
        let mut template = worded("Order {{ order.reference }}", "<p>Thanks!</p>");
        template.text = Some("Plain thanks for {{ order.reference }} & {{ order.note }}".into());
        shop.mail_templates.insert(
            (ShopMailKind::OrderConfirmation.key().into(), "en".into()),
            template,
        );
        let mut broken = worded("Code", "<p>Your code: {{ code }}</p>");
        broken.text = Some(
            "{{ code }} {% for i in range(100000) %}{% for j in range(100000) %}.{% endfor %}{% endfor %}"
                .into(),
        );
        shop.mail_templates.insert(
            (ShopMailKind::ShopperCode.key().into(), "en".into()),
            broken,
        );
        let mut order = paid_order();
        order.note = Some("<b>fragile</b>".into());

        send_order_confirmation(&cx, &shop, &order, None)
            .await
            .unwrap();
        send_shopper_code(&cx, "ada@example.com", &shop, "654321", None)
            .await
            .unwrap();

        let sent = transport.sent();
        assert_eq!(
            sent[0].text(),
            &TextBody::Text("Plain thanks for K7Q4-MZP2 & <b>fragile</b>".into())
        );
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap()).unwrap();
        let text = &wire[wire.find("text/plain").unwrap()..wire.find("text/html").unwrap()];
        assert!(text.contains("Plain thanks for K7Q4-MZP2"), "{wire}");
        // A plain-text part that fails sends the text derived from the HTML.
        assert_eq!(sent[1].text(), &TextBody::FromHtml);
        let wire = String::from_utf8(sent[1].clone().formatted(&cx).unwrap()).unwrap();
        let text = &wire[wire.find("text/plain").unwrap()..wire.find("text/html").unwrap()];
        assert!(text.contains("Your code: 654321"), "{wire}");
    }

    fn worded(subject: &str, html: &str) -> timada_tenancy::MailTemplate {
        timada_tenancy::MailTemplate {
            subject: subject.into(),
            html: html.into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_shop_words_its_mails_its_own_way() {
        let transport = MemoryTransport::new();
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(transport.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
            .app_context(ShopMail::new(None, false))
            .build();
        let mut shop = acme();
        shop.mail_templates.insert(
            (ShopMailKind::OrderConfirmation.key().into(), "en".into()),
            worded(
                "Merci ! Commande {{ order.reference }}",
                "<p>Bonjour, voici votre commande chez {{ shop.name }} :</p>\
                 {% for line in order.lines %}<p>{{ line.name }} ({{ line.quantity }})</p>{% endfor %}\
                 <p>Note : {{ order.note }}</p>",
            ),
        );
        let mut order = paid_order();
        order.note = Some("<script>alert(1)</script>".into());

        send_order_confirmation(&cx, &shop, &order, None)
            .await
            .unwrap();

        let sent = transport.sent();
        assert_eq!(sent[0].subject(), "Merci ! Commande K7Q4-MZP2");
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap()).unwrap();
        assert!(wire.contains("Bonjour"), "{wire}");
        assert!(wire.contains("Mug (2)"), "{wire}");
        let html = &wire[wire.find("text/html").unwrap()..];
        assert!(!html.contains("<script>"), "{wire}");
        assert!(html.contains("&lt;script&gt;"), "{wire}");
    }

    #[tokio::test]
    async fn a_template_that_fails_sends_the_built_in_mail() {
        let transport = MemoryTransport::new();
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(transport.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
            .app_context(ShopMail::new(None, false))
            .build();
        let mut shop = acme();
        let mut template = worded("Order", "<p>{{ order.billing_address.city }}</p>");
        template.text = Some("Plain order {{ order.reference }}".into());
        shop.mail_templates.insert(
            (ShopMailKind::OrderConfirmation.key().into(), "en".into()),
            template,
        );

        send_order_confirmation(&cx, &shop, &paid_order(), None)
            .await
            .unwrap();

        let sent = transport.sent();
        assert_eq!(sent.len(), 1, "the mail is not lost");
        assert_eq!(sent[0].subject(), "Your Acme Tea order K7Q4-MZP2");
        // The merchant's plain text does not go with the built-in HTML.
        assert_eq!(sent[0].text(), &TextBody::FromHtml);
    }

    #[tokio::test]
    async fn a_shopper_gets_the_mail_in_their_language() {
        let transport = MemoryTransport::new();
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(transport.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
            .app_context(ShopMail::new(None, false))
            .build();
        let mut shop = acme();
        shop.mail_languages = Some(timada_tenancy::MailLanguages {
            default: "en".into(),
            others: vec!["fr".into()],
            ..Default::default()
        });
        let kind = ShopMailKind::OrderConfirmation;
        shop.mail_templates.insert(
            (kind.key().into(), "en".into()),
            worded("Thanks for {{ order.reference }}", "<p>Thanks</p>"),
        );
        shop.mail_templates.insert(
            (kind.key().into(), "fr".into()),
            worded("Merci pour {{ order.reference }}", "<p>Merci</p>"),
        );
        let in_language = |language: Option<&str>| {
            let mut order = paid_order();
            order.language = language.map(str::to_owned);
            order
        };

        // Theirs; the default's when theirs is not one of the shop's, or
        // unknown.
        send_order_confirmation(&cx, &shop, &in_language(Some("fr")), None)
            .await
            .unwrap();
        send_order_confirmation(&cx, &shop, &in_language(Some("de")), None)
            .await
            .unwrap();
        send_order_confirmation(&cx, &shop, &in_language(None), None)
            .await
            .unwrap();
        // Worded in French only: the built-in English for everyone else.
        shop.mail_templates
            .remove(&(kind.key().to_owned(), "en".to_owned()));
        send_order_confirmation(&cx, &shop, &in_language(Some("fr")), None)
            .await
            .unwrap();
        send_order_confirmation(&cx, &shop, &in_language(None), None)
            .await
            .unwrap();

        let subjects: Vec<_> = transport
            .sent()
            .iter()
            .map(|mail| mail.subject().to_owned())
            .collect();
        assert_eq!(
            subjects,
            [
                "Merci pour K7Q4-MZP2",
                "Thanks for K7Q4-MZP2",
                "Thanks for K7Q4-MZP2",
                "Merci pour K7Q4-MZP2",
                "Your Acme Tea order K7Q4-MZP2",
            ]
        );
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
