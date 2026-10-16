use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{
    MailBrandingCleared, MailBrandingSet, MailServerConfigured, MailServerRemoved,
    MailTemplateReset, MailTemplateSaved,
};
use crate::error::TenancyError;
use crate::mail_template::{ShopMailKind, validate};
use crate::value_object::{MAIL_SECURITIES, PERMISSION_MAIL_MANAGE};

/// The merchant's own mail server, its password sealed by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigureMailServer {
    pub host: String,
    pub port: u16,
    pub security: String,
    pub username: Option<String>,
    pub password_sealed: Option<Vec<u8>>,
    pub from_address: String,
    pub from_name: Option<String>,
}

/// The look all of a shop's mails share. Blank parts are unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetMailBranding {
    /// A picture already in the tenant's media, `<id>.<ext>`.
    pub logo_file: Option<String>,
    pub logo_size: u64,
    pub accent: Option<String>,
    pub footer: Option<String>,
}

/// The longest footer a shop's mails may carry, in characters.
pub const MAX_MAIL_FOOTER_LEN: usize = 500;

/// `#rrggbb`, lowercased; `None` when it is not one.
pub fn parse_accent(accent: &str) -> Option<String> {
    let hex = accent.trim().strip_prefix('#')?;
    (hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_lowercase()))
}

/// `<id>.<ext>`: letters and digits on each side, so it names one file in
/// the tenant's media and nothing above it.
fn is_media_file(file: &str) -> bool {
    file.split_once('.').is_some_and(|(id, ext)| {
        (1..=64).contains(&id.len())
            && (1..=5).contains(&ext.len())
            && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
            && ext.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

/// Trimmed, `None` when blank.
fn filled(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Sets (or replaces) the server the shop's mails go out through. Needs
    /// `mail:manage`.
    pub async fn configure_mail_server(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        cmd: ConfigureMailServer,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let host = cmd.host.trim().to_owned();
        let from_address = cmd.from_address.trim().to_owned();
        if host.is_empty()
            || host.len() > 253
            || host.contains(|c: char| c.is_whitespace() || c == '/')
            || cmd.port == 0
            || !MAIL_SECURITIES.contains(&cmd.security.as_str())
            || from_address.len() > 254
            || !from_address.contains('@')
        {
            return Err(TenancyError::InvalidMailServer);
        }
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_MAIL_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&MailServerConfigured {
                host,
                port: cmd.port,
                security: cmd.security,
                username: filled(cmd.username),
                password_sealed: cmd.password_sealed,
                from_address,
                from_name: filled(cmd.from_name),
                configured_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// The shop's mails go out through the platform's server again; nothing
    /// set up is a no-op. Needs `mail:manage`.
    pub async fn remove_mail_server(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_MAIL_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if tenant.mail_server.is_none() {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&MailServerRemoved {
                removed_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Words one of the shop's mails the merchant's way. The template is
    /// checked first ([`validate`]): a refused one says why. Needs
    /// `mail:manage`.
    pub async fn save_mail_template(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        mail: &str,
        subject: &str,
        html: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let kind = ShopMailKind::from_key(mail).ok_or(TenancyError::UnknownMail)?;
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_MAIL_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        let subject = subject.trim();
        validate(kind, subject, html)?;
        if tenant
            .mail_template(kind)
            .is_some_and(|saved| saved.subject == subject && saved.html == html)
        {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&MailTemplateSaved {
                mail: kind.key().to_owned(),
                subject: subject.to_owned(),
                html: html.to_owned(),
                saved_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Sends one of the shop's mails in the built-in wording again; a mail
    /// already in it is a no-op. Needs `mail:manage`.
    pub async fn reset_mail_template(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        mail: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let kind = ShopMailKind::from_key(mail).ok_or(TenancyError::UnknownMail)?;
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_MAIL_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if tenant.mail_template(kind).is_none() {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&MailTemplateReset {
                mail: kind.key().to_owned(),
                reset_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Sets the look all the shop's mails share: a logo, an accent colour
    /// and a footer, each optional. Needs `mail:manage`.
    pub async fn set_mail_branding(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        cmd: SetMailBranding,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let accent = match filled(cmd.accent) {
            Some(accent) => Some(parse_accent(&accent).ok_or(TenancyError::InvalidMailBranding)?),
            None => None,
        };
        let footer = filled(cmd.footer).map(|footer| footer.replace("\r\n", "\n"));
        if footer
            .as_ref()
            .is_some_and(|footer| footer.chars().count() > MAX_MAIL_FOOTER_LEN)
            || cmd
                .logo_file
                .as_deref()
                .is_some_and(|file| !is_media_file(file))
        {
            return Err(TenancyError::InvalidMailBranding);
        }
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_MAIL_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        let logo_size = if cmd.logo_file.is_some() {
            cmd.logo_size
        } else {
            0
        };
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&MailBrandingSet {
                logo_file: cmd.logo_file,
                logo_size,
                accent,
                footer,
                set_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// The shop's mails go back to the plain look; none set is a no-op.
    /// Needs `mail:manage`.
    pub async fn clear_mail_branding(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_MAIL_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if tenant.mail_branding.is_none() {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&MailBrandingCleared {
                cleared_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
