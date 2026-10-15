use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{MailServerConfigured, MailServerRemoved};
use crate::error::TenancyError;
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
}
