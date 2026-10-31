//! Where a shop's mails go out. A merchant sets up mail servers of its own
//! on `/t/{slug}/mail` and picks one for each kind of mail:
//!
//! - its mails to its shoppers (order confirmations, review requests,
//!   sign-in codes) go through the server picked for them, from its
//!   address; with none picked, through the platform's server under the
//!   shop's name ([`ShopMail::sender_of`]);
//! - its marketing (newsletter campaigns, automated mails) only ever goes
//!   through the server picked for it ([`ShopMail::marketing_sender_of`]),
//!   never the platform's, so a shop with none picked sends none.
//!
//! The servers' passwords are recorded sealed under `secrets.key` (see
//! [`Sealer`]); without that setting no shop can set a server up, and the
//! ones recorded are not used.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use timada_tenancy::{
    MAIL_SECURITY_NONE, MAIL_SECURITY_STARTTLS, MAIL_SECURITY_TLS, MailServerSettings, TenantState,
};
use topcoat::{
    context::{Cx, app_context},
    mail::{Mail, Mailbox, SmtpTransport, Transport, TransportFuture, send},
};

use crate::mail::MailSender;
use crate::seal::Sealer;

/// How long a shop's server has to take the connection.
const TIMEOUT: Duration = Duration::from_secs(10);

/// How long a whole send through a shop's server may take. The transport's
/// own timeout only bounds the connection: a server that takes it and then
/// says nothing (a port that speaks TLS from the first byte, asked for
/// STARTTLS) would otherwise hold the send, and whatever waits on it, for
/// ever.
const SEND_TIMEOUT: Duration = Duration::from_secs(20);

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
    /// A transport to this server, whose sends give up after
    /// [`SEND_TIMEOUT`] with [`NoAnswer`]. Its connections are pooled, so it
    /// is worth keeping.
    pub fn transport(&self) -> anyhow::Result<TimedTransport> {
        Ok(TimedTransport {
            inner: self.smtp()?,
            limit: SEND_TIMEOUT,
        })
    }

    fn smtp(&self) -> anyhow::Result<SmtpTransport> {
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

/// A shop's server took the connection and did not answer in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoAnswer;

impl std::fmt::Display for NoAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the mail server did not answer in time")
    }
}

impl std::error::Error for NoAnswer {}

/// A transport to a shop's server whose sends give up after `limit`.
pub struct TimedTransport {
    inner: SmtpTransport,
    limit: Duration,
}

impl Transport for TimedTransport {
    fn send<'a>(&'a self, cx: &'a Cx, mail: Mail) -> TransportFuture<'a> {
        Box::pin(async move {
            tokio::time::timeout(self.limit, self.inner.send(cx, mail))
                .await
                .unwrap_or_else(|_| Err(NoAnswer.into()))
        })
    }
}

/// A shop asked to send its marketing and picked no server for it, or shop
/// servers are off on this node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoMarketingServer;

impl std::fmt::Display for NoMarketingServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the shop picked no mail server for its marketing")
    }
}

impl std::error::Error for NoMarketingServer {}

/// Who a shop's mail comes from and the server it goes through.
pub struct ShopSender {
    pub from: Mailbox,
    /// `None`: the platform's server.
    transport: Option<Arc<dyn Transport>>,
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
/// and a transport per shop server in use.
#[derive(Clone)]
pub struct ShopMail(Arc<Inner>);

struct Inner {
    sealer: Option<Sealer>,
    allow_private_hosts: bool,
    transports: Mutex<Transports>,
}

/// `(tenant id, server id) → (the settings it was built from, the transport)`.
type Transports = HashMap<(String, String), (MailServerSettings, Arc<dyn Transport>)>;

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

    /// Seals a password of one of `tenant_id`'s servers: it opens for that
    /// tenant alone.
    pub fn seal(&self, tenant_id: &str, plain: &str) -> anyhow::Result<Vec<u8>> {
        self.sealer()?.seal_for(tenant_id, plain)
    }

    /// A password [`ShopMail::seal`] sealed for `tenant_id`, in clear.
    pub fn unseal(&self, tenant_id: &str, sealed: &[u8]) -> anyhow::Result<String> {
        self.sealer()?.open_for(tenant_id, sealed)
    }

    /// One of `tenant_id`'s servers, opened: its password in clear.
    pub fn open(
        &self,
        tenant_id: &str,
        settings: &MailServerSettings,
    ) -> anyhow::Result<MailServer> {
        let password = settings
            .password_sealed
            .as_deref()
            .map(|sealed| self.unseal(tenant_id, sealed))
            .transpose()?;
        Ok(MailServer {
            host: settings.host.clone(),
            port: settings.port,
            security: settings.security.clone(),
            username: settings.username.clone(),
            password,
        })
    }

    /// Refuses a host on this machine or a private network, unless the
    /// operator allowed it (`mail.allow_private_hosts`): a merchant must not
    /// make this server talk to internal services.
    pub async fn check_host(&self, host: &str, port: u16) -> anyhow::Result<()> {
        if self.0.allow_private_hosts {
            return Ok(());
        }
        crate::net::check_public_host(host, port).await?;
        Ok(())
    }

    /// Whether `tenant`'s marketing has a server to go out through here.
    pub fn has_marketing_server(&self, tenant: &TenantState) -> bool {
        self.enabled() && tenant.marketing_server().is_some()
    }

    /// Who `tenant`'s mails to its shoppers come from and through which
    /// server: the one it picked for them, else the platform's, whose
    /// address `platform` the shop borrows under its own name.
    pub async fn sender_of(
        &self,
        tenant: &TenantState,
        platform: &Mailbox,
    ) -> anyhow::Result<ShopSender> {
        match tenant.shop_mails_server().filter(|_| self.enabled()) {
            Some(settings) => self.sender_through(tenant, settings).await,
            None => Ok(ShopSender {
                from: Mailbox::named(&tenant.name, platform.address())?,
                transport: None,
            }),
        }
    }

    /// `tenant`'s sender for its marketing: the server it picked for it,
    /// and nothing else. [`NoMarketingServer`] when it picked none.
    pub async fn marketing_sender_of(&self, tenant: &TenantState) -> anyhow::Result<ShopSender> {
        let Some(settings) = tenant.marketing_server().filter(|_| self.enabled()) else {
            return Err(NoMarketingServer.into());
        };
        self.sender_through(tenant, settings).await
    }

    /// Mails from `settings`' address, through it. Its transport is kept
    /// while its settings stay the same.
    async fn sender_through(
        &self,
        tenant: &TenantState,
        settings: &MailServerSettings,
    ) -> anyhow::Result<ShopSender> {
        let name = settings.from_name.as_deref().unwrap_or(&tenant.name);
        let from = Mailbox::named(name, &settings.from_address)?;
        self.check_host(&settings.host, settings.port).await?;
        let key = (tenant.id.clone(), settings.id.clone());
        let cached = {
            let transports = self.0.transports.lock().expect("transports lock");
            transports
                .get(&key)
                .filter(|(built_from, _)| built_from == settings)
                .map(|(_, transport)| transport.clone())
        };
        let transport = match cached {
            Some(transport) => transport,
            None => {
                let transport: Arc<dyn Transport> =
                    Arc::new(self.open(&tenant.id, settings)?.transport()?);
                let mut transports = self.0.transports.lock().expect("transports lock");
                // The transports of the tenant's servers that are gone.
                transports.retain(|(tenant_id, server_id), _| {
                    *tenant_id != tenant.id || tenant.mail_server(server_id).is_some()
                });
                transports.insert(key, (settings.clone(), transport.clone()));
                transport
            }
        };
        Ok(ShopSender {
            from,
            transport: Some(transport),
        })
    }

    /// Stands `transport` in for one of `tenant`'s servers, so a test can
    /// read what the shop sent through it.
    #[cfg(test)]
    pub fn stand_in_for(
        &self,
        tenant: &TenantState,
        server_id: &str,
        transport: Arc<dyn Transport>,
    ) {
        let settings = tenant
            .mail_server(server_id)
            .cloned()
            .expect("one of the tenant's servers");
        self.0.transports.lock().expect("transports lock").insert(
            (tenant.id.clone(), server_id.to_owned()),
            (settings, transport),
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::is_public;

    const KEY: &str = "a-secrets-key-of-thirty-two-chars!!";

    /// A shop with `servers`, the first picked for its mails to shoppers
    /// and the last for its marketing.
    fn shop(servers: Vec<MailServerSettings>) -> TenantState {
        TenantState {
            id: "t1".into(),
            name: "Acme Tea".into(),
            shop_mails_server_id: servers.first().map(|server| server.id.clone()),
            marketing_server_id: servers.last().map(|server| server.id.clone()),
            mail_servers: servers,
            ..Default::default()
        }
    }

    fn settings(mail: &ShopMail, id: &str, from_address: &str) -> MailServerSettings {
        MailServerSettings {
            id: id.into(),
            host: "localhost".into(),
            port: 1025,
            security: MAIL_SECURITY_NONE.into(),
            username: Some("shop".into()),
            password_sealed: Some(mail.seal("t1", "s3cret").unwrap()),
            from_address: from_address.into(),
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
        let sender = mail
            .sender_of(&shop(Vec::new()), &platform())
            .await
            .unwrap();
        assert!(!sender.is_own_server());
        assert_eq!(sender.from.name(), Some("Acme Tea"));
        assert_eq!(sender.from.address(), "no-reply@example.com");
    }

    #[tokio::test]
    async fn marketing_never_borrows_the_platform_server() {
        let mail = ShopMail::new(Some(Sealer::new("mail", KEY)), true);
        assert!(!mail.has_marketing_server(&shop(Vec::new())));
        let err = mail
            .marketing_sender_of(&shop(Vec::new()))
            .await
            .err()
            .unwrap();
        assert!(err.is::<NoMarketingServer>(), "{err}");

        // A server picked for the shop's mails only sends no marketing.
        let mut tenant = shop(vec![settings(&mail, "a", "orders@acme.test")]);
        tenant.marketing_server_id = None;
        assert!(!mail.has_marketing_server(&tenant));
        let err = mail.marketing_sender_of(&tenant).await.err().unwrap();
        assert!(err.is::<NoMarketingServer>(), "{err}");

        tenant.marketing_server_id = Some("a".into());
        assert!(mail.has_marketing_server(&tenant));
        let sender = mail.marketing_sender_of(&tenant).await.unwrap();
        assert!(sender.is_own_server());
        assert_eq!(sender.from.address(), "orders@acme.test");

        // Shop servers off on this node: no marketing at all.
        let off = ShopMail::new(None, true);
        assert!(!off.has_marketing_server(&tenant));
        let err = off.marketing_sender_of(&tenant).await.err().unwrap();
        assert!(err.is::<NoMarketingServer>(), "{err}");
    }

    #[tokio::test]
    async fn a_shop_with_a_server_sends_from_its_own_address() {
        let mail = ShopMail::new(Some(Sealer::new("mail", KEY)), true);
        let tenant = shop(vec![settings(&mail, "a", "orders@acme.test")]);
        let sender = mail.sender_of(&tenant, &platform()).await.unwrap();
        assert!(sender.is_own_server());
        assert_eq!(sender.from.name(), Some("Acme Tea"));
        assert_eq!(sender.from.address(), "orders@acme.test");
        let server = mail.open("t1", &tenant.mail_servers[0]).unwrap();
        assert!(
            mail.open("t2", &tenant.mail_servers[0]).is_err(),
            "sealed for one shop, it opens for no other"
        );
        assert_eq!(server.password.as_deref(), Some("s3cret"));
        assert!(!format!("{server:?}").contains("s3cret"));

        // One server picked for both shares its transport.
        let again = mail.sender_of(&tenant, &platform()).await.unwrap();
        let marketing = mail.marketing_sender_of(&tenant).await.unwrap();
        assert!(Arc::ptr_eq(
            sender.transport.as_ref().unwrap(),
            again.transport.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            sender.transport.as_ref().unwrap(),
            marketing.transport.as_ref().unwrap()
        ));
        let mut named = settings(&mail, "a", "orders@acme.test");
        named.from_name = Some("Acme Orders".into());
        let renamed = mail
            .sender_of(&shop(vec![named]), &platform())
            .await
            .unwrap();
        assert_eq!(renamed.from.name(), Some("Acme Orders"));
        assert!(!Arc::ptr_eq(
            sender.transport.as_ref().unwrap(),
            renamed.transport.as_ref().unwrap()
        ));
    }

    #[tokio::test]
    async fn each_role_goes_through_its_own_server() {
        let mail = ShopMail::new(Some(Sealer::new("mail", KEY)), true);
        let tenant = shop(vec![
            settings(&mail, "a", "orders@acme.test"),
            settings(&mail, "b", "news@acme.test"),
        ]);
        let orders = mail.sender_of(&tenant, &platform()).await.unwrap();
        let news = mail.marketing_sender_of(&tenant).await.unwrap();
        assert_eq!(orders.from.address(), "orders@acme.test");
        assert_eq!(news.from.address(), "news@acme.test");
        assert!(!Arc::ptr_eq(
            orders.transport.as_ref().unwrap(),
            news.transport.as_ref().unwrap()
        ));

        // The shop's mails through the platform's server, by choice; the
        // marketing keeps its own.
        let mut platform_mails = tenant.clone();
        platform_mails.shop_mails_server_id = None;
        let sender = mail.sender_of(&platform_mails, &platform()).await.unwrap();
        assert!(!sender.is_own_server());
        assert_eq!(sender.from.address(), "no-reply@example.com");
        let news = mail.marketing_sender_of(&platform_mails).await.unwrap();
        assert_eq!(news.from.address(), "news@acme.test");
    }

    #[tokio::test]
    async fn a_pick_of_a_server_gone_reads_as_none() {
        let mail = ShopMail::new(Some(Sealer::new("mail", KEY)), true);
        let mut tenant = shop(vec![settings(&mail, "a", "orders@acme.test")]);
        tenant.shop_mails_server_id = Some("gone".into());
        tenant.marketing_server_id = Some("gone".into());
        let sender = mail.sender_of(&tenant, &platform()).await.unwrap();
        assert!(!sender.is_own_server());
        assert!(!mail.has_marketing_server(&tenant));
        let err = mail.marketing_sender_of(&tenant).await.err().unwrap();
        assert!(err.is::<NoMarketingServer>(), "{err}");
    }

    #[tokio::test]
    async fn without_the_secrets_key_every_shop_uses_the_platform_server() {
        let sealing = ShopMail::new(Some(Sealer::new("mail", KEY)), true);
        let tenant = shop(vec![settings(&sealing, "a", "orders@acme.test")]);
        let off = ShopMail::new(None, true);
        let sender = off.sender_of(&tenant, &platform()).await.unwrap();
        assert!(!sender.is_own_server());
        assert_eq!(sender.from.address(), "no-reply@example.com");
    }

    #[tokio::test]
    async fn a_private_host_is_refused() {
        let mail = ShopMail::new(Some(Sealer::new("mail", KEY)), false);
        let tenant = shop(vec![settings(&mail, "a", "orders@acme.test")]);
        let err = mail.sender_of(&tenant, &platform()).await.err().unwrap();
        assert!(err.to_string().contains("not a public address"), "{err}");
        let err = mail.marketing_sender_of(&tenant).await.err().unwrap();
        assert!(err.to_string().contains("not a public address"), "{err}");
        assert!(mail.check_host("127.0.0.1", 25).await.is_err());
        assert!(mail.check_host("10.1.2.3", 25).await.is_err());
        assert!(mail.check_host("93.184.215.14", 25).await.is_ok());
    }

    #[tokio::test]
    async fn a_server_that_takes_the_connection_and_never_answers_is_given_up() {
        // As a port that speaks TLS from the first byte does to a client
        // waiting for its greeting: the connection is taken, nothing is said.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                held.push(socket);
            }
        });
        let server = MailServer {
            host: "127.0.0.1".into(),
            port,
            security: MAIL_SECURITY_NONE.into(),
            username: None,
            password: None,
        };
        assert_eq!(server.transport().unwrap().limit, SEND_TIMEOUT);
        let transport = TimedTransport {
            inner: server.smtp().unwrap(),
            limit: Duration::from_millis(300),
        };
        let cx = topcoat::context::CxTestBuilder::new().build();
        let mail = topcoat::mail::mail! {
            from: "Shop <shop@acme.test>".parse::<Mailbox>().unwrap(),
            to: Mailbox::new("buyer@acme.test").unwrap(),
            subject: "Hello",
            html: { cx => <p>"Hello"</p> },
        }
        .unwrap();
        let err = tokio::time::timeout(Duration::from_secs(5), transport.send(&cx, mail))
            .await
            .expect("given up within its limit")
            .unwrap_err();
        assert!(err.is::<NoAnswer>(), "{err}");
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
