//! The e-mails this application sends: a code that proves an address, and
//! an invitation to join a tenant.

use anyhow::Context as _;
use topcoat::{
    Result,
    context::{Cx, app_context},
    mail::{Mail, MailConfig, Mailbox, SmtpTransport, mail, send},
};

use crate::auth::cookies::EmailFlowKind;
use crate::config;

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

    #[tokio::test]
    async fn the_code_reaches_the_address() {
        let transport = MemoryTransport::new();
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(transport.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
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
    async fn the_invitation_carries_the_link() {
        let transport = MemoryTransport::new();
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(transport.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
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
