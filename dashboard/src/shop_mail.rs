//! Where a shop's mails to its shoppers go out: through the merchant's own
//! mail server, from its own address, once it set one up on
//! `/t/{slug}/mail`; through the platform's server under the shop's name
//! otherwise.
//!
//! The server's password is recorded sealed under `secrets.key` (see
//! [`Sealer`]); without that setting no shop can set a server up, and the
//! ones recorded are not used.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use timada_tenancy::{
    MAIL_SECURITY_NONE, MAIL_SECURITY_STARTTLS, MAIL_SECURITY_TLS, MailServerSettings, TenantState,
};
use topcoat::{
    context::{Cx, app_context},
    mail::{Mail, Mailbox, SmtpTransport, Transport, send},
};

use crate::auth::mail::MailSender;
use crate::seal::Sealer;

/// How long a shop's server has to answer before a send gives up.
const TIMEOUT: Duration = Duration::from_secs(10);

/// A shop's mail server, opened: the password in clear.
pub struct MailServer {
    pub host: String,
    pub port: u16,
    pub security: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl std::fmt::Debug for MailServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailServer")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

impl MailServer {
    /// A transport to this server. Its connections are pooled, so it is
    /// worth keeping.
    pub fn transport(&self) -> anyhow::Result<SmtpTransport> {
        let builder = match self.security.as_str() {
            MAIL_SECURITY_TLS => SmtpTransport::relay(&self.host)?,
            MAIL_SECURITY_STARTTLS => SmtpTransport::starttls(&self.host)?,
            MAIL_SECURITY_NONE => SmtpTransport::unencrypted(&self.host),
            other => anyhow::bail!("unknown mail server security {other:?}"),
        };
        let builder = builder.port(self.port).timeout(TIMEOUT);
        let builder = match (&self.username, &self.password) {
            (Some(username), Some(password)) => builder.credentials(username, password),
            _ => builder,
        };
        Ok(builder.build())
    }
}

/// Who a shop's mail comes from and the server it goes through.
pub struct ShopSender {
    pub from: Mailbox,
    /// `None`: the platform's server.
    transport: Option<Arc<SmtpTransport>>,
}

impl ShopSender {
    /// Whether the mail goes through the shop's own server.
    #[cfg(test)]
    pub fn is_own_server(&self) -> bool {
        self.transport.is_some()
    }

    pub async fn send(&self, cx: &Cx, mail: Mail) -> topcoat::Result<()> {
        match &self.transport {
            Some(transport) => transport.send(cx, mail).await?,
            None => send(cx, mail).await?,
        };
        Ok(())
    }
}

/// The shop mail service, cheap to clone: the key to the servers' passwords,
/// the platform's sender, and a transport per shop server in use.
#[derive(Clone)]
pub struct ShopMail(Arc<Inner>);

struct Inner {
    sealer: Option<Sealer>,
    allow_private_hosts: bool,
    /// `tenant id → (the settings it was built from, the transport)`.
    transports: Mutex<HashMap<String, (MailServerSettings, Arc<SmtpTransport>)>>,
}

impl ShopMail {
    /// `sealer`: `None` keeps every shop on the platform's server.
    pub fn new(sealer: Option<Sealer>, allow_private_hosts: bool) -> Self {
        Self(Arc::new(Inner {
            sealer,
            allow_private_hosts,
            transports: Mutex::default(),
        }))
    }

    /// Whether shops can set up a server of their own on this node.
    pub fn enabled(&self) -> bool {
        self.0.sealer.is_some()
    }

    fn sealer(&self) -> anyhow::Result<&Sealer> {
        self.0
            .sealer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("shop mail servers are off (no [secrets] section)"))
    }

    pub fn seal(&self, plain: &str) -> anyhow::Result<Vec<u8>> {
        self.sealer()?.seal(plain)
    }

    /// The tenant's server, opened; `None` when it set up none or shop
    /// servers are off here.
    pub fn server_of(&self, tenant: &TenantState) -> anyhow::Result<Option<MailServer>> {
        let (Some(settings), Some(sealer)) = (&tenant.mail_server, &self.0.sealer) else {
            return Ok(None);
        };
        let password = settings
            .password_sealed
            .as_deref()
            .map(|sealed| sealer.open(sealed))
            .transpose()?;
        Ok(Some(MailServer {
            host: settings.host.clone(),
            port: settings.port,
            security: settings.security.clone(),
            username: settings.username.clone(),
            password,
        }))
    }

    /// Refuses a host on this machine or a private network, unless the
    /// operator allowed it (`mail.allow_private_hosts`): a merchant must not
    /// make this server talk to internal services.
    pub async fn check_host(&self, host: &str, port: u16) -> anyhow::Result<()> {
        if self.0.allow_private_hosts {
            return Ok(());
        }
        let addresses: Vec<IpAddr> = tokio::net::lookup_host((host, port))
            .await
            .with_context(|| format!("cannot resolve {host}"))?
            .map(|address| address.ip())
            .collect();
        anyhow::ensure!(!addresses.is_empty(), "{host} has no address");
        if let Some(address) = addresses.iter().find(|address| !is_public(**address)) {
            anyhow::bail!("{host} resolves to {address}, which is not a public address");
        }
        Ok(())
    }

    /// Who `tenant`'s mails come from and through which server.
    /// `platform` is the platform's sender, whose address the shop borrows
    /// when it has no server of its own.
    pub async fn sender_of(
        &self,
        tenant: &TenantState,
        platform: &Mailbox,
    ) -> anyhow::Result<ShopSender> {
        let Some(settings) = tenant.mail_server.as_ref().filter(|_| self.enabled()) else {
            return Ok(ShopSender {
                from: Mailbox::named(&tenant.name, platform.address())?,
                transport: None,
            });
        };
        let name = settings.from_name.as_deref().unwrap_or(&tenant.name);
        let from = Mailbox::named(name, &settings.from_address)?;
        self.check_host(&settings.host, settings.port).await?;
        let cached = {
            let transports = self.0.transports.lock().expect("transports lock");
            transports
                .get(&tenant.id)
                .filter(|(built_from, _)| built_from == settings)
                .map(|(_, transport)| transport.clone())
        };
        let transport = match cached {
            Some(transport) => transport,
            None => {
                let server = self
                    .server_of(tenant)?
                    .context("the shop's mail server is gone")?;
                let transport = Arc::new(server.transport()?);
                self.0
                    .transports
                    .lock()
                    .expect("transports lock")
                    .insert(tenant.id.clone(), (settings.clone(), transport.clone()));
                transport
            }
        };
        Ok(ShopSender {
            from,
            transport: Some(transport),
        })
    }
}

/// `tenant`'s sender, from the context's [`ShopMail`] and [`MailSender`].
pub async fn shop_sender(cx: &Cx, tenant: &TenantState) -> topcoat::Result<ShopSender> {
    let platform = &app_context::<MailSender>(cx).0;
    let sender = app_context::<ShopMail>(cx)
        .sender_of(tenant, platform)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(sender)
}

/// Whether `address` is reachable from the internet: not loopback, private,
/// link-local, shared (CGNAT), unspecified, broadcast, documentation or
/// multicast.
fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || a == 0
                || (a == 100 && (64..128).contains(&b)))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "a-secrets-key-of-thirty-two-chars!!";

    fn shop(mail_server: Option<MailServerSettings>) -> TenantState {
        TenantState {
            id: "t1".into(),
            name: "Acme Tea".into(),
            mail_server,
            ..Default::default()
        }
    }

    fn settings(mail: &ShopMail) -> MailServerSettings {
        MailServerSettings {
            host: "localhost".into(),
            port: 1025,
            security: MAIL_SECURITY_NONE.into(),
            username: Some("shop".into()),
            password_sealed: Some(mail.seal("s3cret").unwrap()),
            from_address: "orders@acme.test".into(),
            from_name: None,
            ..Default::default()
        }
    }

    fn platform() -> Mailbox {
        "Timada <no-reply@example.com>".parse().unwrap()
    }

    #[tokio::test]
    async fn a_shop_without_a_server_borrows_the_platform_address() {
        let mail = ShopMail::new(Some(Sealer::new("mail", KEY)), false);
        let sender = mail.sender_of(&shop(None), &platform()).await.unwrap();
        assert!(!sender.is_own_server());
        assert_eq!(sender.from.name(), Some("Acme Tea"));
        assert_eq!(sender.from.address(), "no-reply@example.com");
    }

    #[tokio::test]
    async fn a_shop_with_a_server_sends_from_its_own_address() {
        let mail = ShopMail::new(Some(Sealer::new("mail", KEY)), true);
        let tenant = shop(Some(settings(&mail)));
        let sender = mail.sender_of(&tenant, &platform()).await.unwrap();
        assert!(sender.is_own_server());
        assert_eq!(sender.from.name(), Some("Acme Tea"));
        assert_eq!(sender.from.address(), "orders@acme.test");
        let server = mail.server_of(&tenant).unwrap().unwrap();
        assert_eq!(server.password.as_deref(), Some("s3cret"));
        assert!(!format!("{server:?}").contains("s3cret"));

        let again = mail.sender_of(&tenant, &platform()).await.unwrap();
        assert!(Arc::ptr_eq(
            sender.transport.as_ref().unwrap(),
            again.transport.as_ref().unwrap()
        ));
        let mut named = settings(&mail);
        named.from_name = Some("Acme Orders".into());
        let renamed = mail
            .sender_of(&shop(Some(named)), &platform())
            .await
            .unwrap();
        assert_eq!(renamed.from.name(), Some("Acme Orders"));
        assert!(!Arc::ptr_eq(
            sender.transport.as_ref().unwrap(),
            renamed.transport.as_ref().unwrap()
        ));
    }

    #[tokio::test]
    async fn without_the_secrets_key_every_shop_uses_the_platform_server() {
        let sealing = ShopMail::new(Some(Sealer::new("mail", KEY)), true);
        let tenant = shop(Some(settings(&sealing)));
        let off = ShopMail::new(None, true);
        let sender = off.sender_of(&tenant, &platform()).await.unwrap();
        assert!(!sender.is_own_server());
        assert_eq!(sender.from.address(), "no-reply@example.com");
    }

    #[tokio::test]
    async fn a_private_host_is_refused() {
        let mail = ShopMail::new(Some(Sealer::new("mail", KEY)), false);
        let tenant = shop(Some(settings(&mail)));
        let err = mail.sender_of(&tenant, &platform()).await.err().unwrap();
        assert!(err.to_string().contains("not a public address"), "{err}");
        assert!(mail.check_host("127.0.0.1", 25).await.is_err());
        assert!(mail.check_host("10.1.2.3", 25).await.is_err());
        assert!(mail.check_host("93.184.215.14", 25).await.is_ok());
    }

    #[test]
    fn public_addresses() {
        for public in ["93.184.215.14", "2606:2800:21f:cb07:6820:80da:af6b:8b2c"] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
        for private in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.5.4",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
    }
}
