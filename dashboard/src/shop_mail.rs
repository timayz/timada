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
//!
//! A server's host has to be on the internet, never this machine or a
//! private network ([`ShopMail::check_host`]). The check looks the name up,
//! and the transport then dials the addresses it found, in turn, rather than
//! the name (TLS still verifies the name), so a name that answers a private
//! address later (DNS rebinding) gets nowhere; a transport is dialled for
//! [`RESOLVE_TTL`] before its name is looked up and checked again.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lettre::{
    Address, AsyncSmtpTransport, AsyncTransport as _, Tokio1Executor,
    address::Envelope,
    transport::smtp::{
        Error as SmtpError,
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
};
use timada_tenancy::{
    MAIL_SECURITY_NONE, MAIL_SECURITY_STARTTLS, MAIL_SECURITY_TLS, MailServerSettings, TenantState,
};
use topcoat::{
    context::{Cx, app_context},
    mail::{Mail, Mailbox, Receipt, SendError, Transport, TransportFuture, send},
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

/// How long a transport is dialled before its server's name is looked up and
/// checked again: a name that moved to a private address is caught within
/// this, one that moved between public addresses is followed.
const RESOLVE_TTL: Duration = Duration::from_secs(5 * 60);

/// A shop's mail server, opened: the password in clear.
pub struct MailServer {
    pub host: String,
    pub port: u16,
    pub security: String,
    pub username: Option<String>,
    pub password: Option<String>,
    /// The addresses `host` was checked at ([`ShopMail::check_host`]), in the
    /// order the name answered them: the transport dials them in turn rather
    /// than looking the name up again, so the address dialled is one checked.
    /// Empty dials the name (private hosts allowed).
    pub addresses: Vec<SocketAddr>,
}

impl std::fmt::Debug for MailServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailServer")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("username", &self.username)
            .field("addresses", &self.addresses)
            .finish_non_exhaustive()
    }
}

impl MailServer {
    /// A transport to this server, whose sends give up after
    /// [`SEND_TIMEOUT`] with [`NoAnswer`]. Its connections are pooled, so it
    /// is worth keeping.
    pub fn transport(&self) -> anyhow::Result<TimedTransport> {
        let (client, fallbacks) = match self.addresses.split_first() {
            None => (self.smtp(&self.host)?, Vec::new()),
            Some((first, rest)) => (
                self.smtp(&first.ip().to_string())?,
                rest.iter()
                    .map(|address| self.smtp(&address.ip().to_string()))
                    .collect::<anyhow::Result<_>>()?,
            ),
        };
        Ok(TimedTransport {
            client,
            fallbacks,
            limit: SEND_TIMEOUT,
        })
    }

    /// The SMTP client dialling `dialled` (an address of the host, or its
    /// name): TLS, when asked for, is for `host` whatever is dialled, so the
    /// server has to hold a certificate for the name the merchant gave.
    fn smtp(&self, dialled: &str) -> anyhow::Result<AsyncSmtpTransport<Tokio1Executor>> {
        let tls = match self.security.as_str() {
            MAIL_SECURITY_TLS => Tls::Wrapper(TlsParameters::new(self.host.clone())?),
            MAIL_SECURITY_STARTTLS => Tls::Required(TlsParameters::new(self.host.clone())?),
            MAIL_SECURITY_NONE => Tls::None,
            other => anyhow::bail!("unknown mail server security {other:?}"),
        };
        let builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(dialled.to_owned())
            .port(self.port)
            .tls(tls)
            .timeout(Some(TIMEOUT));
        let builder = match (&self.username, &self.password) {
            (Some(username), Some(password)) => {
                builder.credentials(Credentials::new(username.clone(), password.clone()))
            }
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

/// A transport to a shop's server whose sends give up after `limit`: one
/// SMTP client per address the server's host was checked at (or one dialling
/// its name), and a mail goes through the first that can be reached.
pub struct TimedTransport {
    /// Dials the first address the host answered, or its name.
    client: AsyncSmtpTransport<Tokio1Executor>,
    /// Dial the host's other addresses, in turn, when the first cannot
    /// be reached.
    fallbacks: Vec<AsyncSmtpTransport<Tokio1Executor>>,
    limit: Duration,
}

impl TimedTransport {
    /// Sends the message through the first address that can be reached, as
    /// a client dialling the name would try the name's addresses in turn: an
    /// address that cannot be reached (down, or of a family this node has no
    /// route for) is passed over for the next. Only a failure to connect
    /// moves on; a server's answer, or a connection lost mid-way, is the
    /// send's result, so a mail is never handed to a second address.
    async fn send_raw(&self, envelope: &Envelope, message: &[u8]) -> Result<(), SmtpError> {
        let mut err = match self.client.send_raw(envelope, message).await {
            Ok(_) => return Ok(()),
            Err(err) => err,
        };
        for fallback in &self.fallbacks {
            if !is_unreachable(&err) {
                break;
            }
            err = match fallback.send_raw(envelope, message).await {
                Ok(_) => return Ok(()),
                Err(err) => err,
            };
        }
        Err(err)
    }
}

/// Whether `err` is a failure to reach the server at all (the connection
/// refused, no route to it, nothing taking it in time), rather than the
/// server's answer or a connection lost once made: what sends a mail to the
/// host's next address. The client dials one address at a time, so the
/// error of a failed connection is the one address's.
fn is_unreachable(err: &SmtpError) -> bool {
    use std::io::ErrorKind::{
        AddrNotAvailable, ConnectionRefused, HostUnreachable, NetworkUnreachable, TimedOut,
    };
    let mut source = std::error::Error::source(err);
    while let Some(err) = source {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            return matches!(
                io.kind(),
                ConnectionRefused
                    | NetworkUnreachable
                    | HostUnreachable
                    | AddrNotAvailable
                    | TimedOut
            );
        }
        source = err.source();
    }
    false
}

impl Transport for TimedTransport {
    fn send<'a>(&'a self, cx: &'a Cx, mail: Mail) -> TransportFuture<'a> {
        Box::pin(async move {
            let envelope = envelope_of(&mail).map_err(topcoat::Error::from_anyhow)?;
            let message = mail.formatted(cx)?;
            let receipt = Receipt::new(message_id_of(&message));
            match tokio::time::timeout(self.limit, self.send_raw(&envelope, &message)).await {
                Ok(Ok(())) => Ok(receipt),
                Ok(Err(err)) => Err(SendError::delivery(err).into()),
                Err(_) => Err(NoAnswer.into()),
            }
        })
    }
}

/// The envelope of `mail`: its sender and every recipient, `Bcc` included.
fn envelope_of(mail: &Mail) -> anyhow::Result<Envelope> {
    let address = |mailbox: &Mailbox| mailbox.address().parse::<Address>();
    let from = mail.from().map(address).transpose()?;
    let to = mail
        .to()
        .iter()
        .chain(mail.cc())
        .chain(mail.bcc())
        .map(address)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Envelope::new(from, to)?)
}

/// The `Message-ID` header of a formatted message, for its receipt.
fn message_id_of(message: &[u8]) -> String {
    message
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .take_while(|line| !line.is_empty())
        .find_map(|line| {
            let (name, value) = std::str::from_utf8(line).ok()?.split_once(':')?;
            name.eq_ignore_ascii_case("Message-ID")
                .then(|| value.trim().to_owned())
        })
        .unwrap_or_default()
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

/// `(tenant id, server id) → the transport kept for that server`.
type Transports = HashMap<(String, String), Cached>;

/// A transport kept for a shop's server.
struct Cached {
    /// The settings it was built from.
    settings: MailServerSettings,
    /// The addresses its host was checked at and it dials; empty dials the
    /// name.
    addresses: Vec<SocketAddr>,
    /// Until when it is dialled before its host is looked up and checked
    /// again: [`RESOLVE_TTL`] after the last check.
    checked_until: Instant,
    transport: Arc<dyn Transport>,
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

    /// Seals a password of one of `tenant_id`'s servers: it opens for that
    /// tenant alone.
    pub fn seal(&self, tenant_id: &str, plain: &str) -> anyhow::Result<Vec<u8>> {
        self.sealer()?.seal_for(tenant_id, plain)
    }

    /// A password [`ShopMail::seal`] sealed for `tenant_id`, in clear.
    pub fn unseal(&self, tenant_id: &str, sealed: &[u8]) -> anyhow::Result<String> {
        self.sealer()?.open_for(tenant_id, sealed)
    }

    /// One of `tenant_id`'s servers, opened: its password in clear, dialling
    /// `addresses` (what [`ShopMail::check_host`] gave for its host).
    pub fn open(
        &self,
        tenant_id: &str,
        settings: &MailServerSettings,
        addresses: Vec<SocketAddr>,
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
            addresses,
        })
    }

    /// Refuses a host on this machine or a private network, unless the
    /// operator allowed it (`mail.allow_private_hosts`): a merchant must not
    /// make this server talk to internal services. Gives the addresses the
    /// host was checked at, for the transport to dial in turn; empty when
    /// the check is off (the transport dials the name).
    pub async fn check_host(&self, host: &str, port: u16) -> anyhow::Result<Vec<SocketAddr>> {
        if self.0.allow_private_hosts {
            return Ok(Vec::new());
        }
        crate::net::public_addresses(host, port).await
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
    /// while its settings stay the same, its host looked up and checked
    /// again every [`RESOLVE_TTL`].
    async fn sender_through(
        &self,
        tenant: &TenantState,
        settings: &MailServerSettings,
    ) -> anyhow::Result<ShopSender> {
        let name = settings.from_name.as_deref().unwrap_or(&tenant.name);
        let from = Mailbox::named(name, &settings.from_address)?;
        let key = (tenant.id.clone(), settings.id.clone());
        let fresh = {
            let transports = self.0.transports.lock().expect("transports lock");
            transports
                .get(&key)
                .filter(|cached| {
                    cached.settings == *settings && Instant::now() < cached.checked_until
                })
                .map(|cached| cached.transport.clone())
        };
        let transport = match fresh {
            Some(transport) => transport,
            None => self.resolved_transport(tenant, settings, key).await?,
        };
        Ok(ShopSender {
            from,
            transport: Some(transport),
        })
    }

    /// Looks `settings`' host up and checks it, then the transport dialling
    /// the addresses found: the one kept, when it dials those addresses
    /// already (its pooled connections with them), else a new one. A host
    /// that now answers a private address is refused and the transport kept
    /// for it dropped.
    async fn resolved_transport(
        &self,
        tenant: &TenantState,
        settings: &MailServerSettings,
        key: (String, String),
    ) -> anyhow::Result<Arc<dyn Transport>> {
        let addresses = match self.check_host(&settings.host, settings.port).await {
            Ok(addresses) => addresses,
            Err(err) => {
                self.0
                    .transports
                    .lock()
                    .expect("transports lock")
                    .remove(&key);
                return Err(err);
            }
        };
        let mut transports = self.0.transports.lock().expect("transports lock");
        if let Some(cached) = transports.get_mut(&key)
            && cached.settings == *settings
            && same_addresses(&cached.addresses, &addresses)
        {
            cached.checked_until = Instant::now() + RESOLVE_TTL;
            return Ok(cached.transport.clone());
        }
        let transport: Arc<dyn Transport> = Arc::new(
            self.open(&tenant.id, settings, addresses.clone())?
                .transport()?,
        );
        // The transports of the tenant's servers that are gone.
        transports.retain(|(tenant_id, server_id), _| {
            *tenant_id != tenant.id || tenant.mail_server(server_id).is_some()
        });
        transports.insert(
            key,
            Cached {
                settings: settings.clone(),
                addresses,
                checked_until: Instant::now() + RESOLVE_TTL,
                transport: transport.clone(),
            },
        );
        Ok(transport)
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
            Cached {
                settings,
                addresses: Vec::new(),
                checked_until: Instant::now() + RESOLVE_TTL,
                transport,
            },
        );
    }

    /// Makes the transport kept for one of `tenant`'s servers look `by`
    /// older, as if its host had been checked that long ago at `addresses`.
    /// `by` is at most [`RESOLVE_TTL`], which lands no earlier than the
    /// check itself (never before the clock began).
    #[cfg(test)]
    fn age_transport(
        &self,
        tenant: &TenantState,
        server_id: &str,
        by: Duration,
        addresses: Vec<SocketAddr>,
    ) {
        let mut transports = self.0.transports.lock().expect("transports lock");
        let cached = transports
            .get_mut(&(tenant.id.clone(), server_id.to_owned()))
            .expect("a transport kept for the server");
        cached.checked_until = cached
            .checked_until
            .checked_sub(by)
            .expect("no older than the window");
        cached.addresses = addresses;
    }

    /// Whether a transport is kept for one of `tenant`'s servers.
    #[cfg(test)]
    fn keeps_transport_for(&self, tenant: &TenantState, server_id: &str) -> bool {
        self.0
            .transports
            .lock()
            .expect("transports lock")
            .contains_key(&(tenant.id.clone(), server_id.to_owned()))
    }
}

/// Whether two lookups of a host answered the same addresses: a name that
/// rotates the order it answers them in (round-robin DNS) keeps its
/// transport, and the pooled connections with it.
fn same_addresses(kept: &[SocketAddr], found: &[SocketAddr]) -> bool {
    let mut kept = kept.to_vec();
    let mut found = found.to_vec();
    kept.sort_unstable();
    found.sort_unstable();
    kept == found
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
    use topcoat::{
        context::CxTestBuilder,
        mail::{MemoryTransport, mail},
    };

    const KEY: &str = "a-secrets-key-of-thirty-two-chars!!";

    /// A public address, as a name a merchant gave may have answered.
    const ELSEWHERE: &str = "93.184.215.14:1025";

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

    async fn hello(cx: &Cx) -> Mail {
        mail! {
            from: "Shop <shop@acme.test>".parse::<Mailbox>().unwrap(),
            to: Mailbox::new("buyer@acme.test").unwrap(),
            subject: "Hello",
            html: { cx => <p>"Hello"</p> },
        }
        .unwrap()
    }

    fn transport_of(sender: ShopSender) -> Arc<dyn Transport> {
        sender.transport.expect("the shop's own server")
    }

    /// A mail server on this machine that takes one mail: the address it
    /// listens on, and the mail's wire form once it has it.
    async fn fake_smtp() -> (SocketAddr, tokio::sync::oneshot::Receiver<String>) {
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sent, received) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut lines = BufReader::new(read).lines();
            write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
            let mut sent = Some(sent);
            let mut data = String::new();
            let mut in_data = false;
            while let Ok(Some(line)) = lines.next_line().await {
                if in_data {
                    if line == "." {
                        in_data = false;
                        if let Some(sent) = sent.take() {
                            let _ = sent.send(std::mem::take(&mut data));
                        }
                        write.write_all(b"250 OK\r\n").await.unwrap();
                    } else {
                        data.push_str(&line);
                        data.push_str("\r\n");
                    }
                    continue;
                }
                let verb = line.split(' ').next().unwrap_or("").to_ascii_uppercase();
                let reply: &[u8] = match verb.as_str() {
                    "EHLO" | "HELO" => b"250 fake\r\n",
                    "DATA" => {
                        in_data = true;
                        b"354 go ahead\r\n"
                    }
                    "QUIT" => b"221 bye\r\n",
                    _ => b"250 OK\r\n",
                };
                write.write_all(reply).await.unwrap();
                if verb == "QUIT" {
                    break;
                }
            }
        });
        (address, received)
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
        let server = mail
            .open("t1", &tenant.mail_servers[0], Vec::new())
            .unwrap();
        assert!(
            mail.open("t2", &tenant.mail_servers[0], Vec::new())
                .is_err(),
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
        // A public host passes, and the check gives the addresses it was
        // checked at for the transport to dial.
        assert_eq!(
            mail.check_host("93.184.215.14", 25).await.unwrap(),
            vec!["93.184.215.14:25".parse::<SocketAddr>().unwrap()]
        );
        // With private hosts allowed, nothing is checked and the name is
        // dialled.
        let allowed = ShopMail::new(Some(Sealer::new("mail", KEY)), true);
        assert!(
            allowed
                .check_host("127.0.0.1", 25)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn the_transport_dials_the_address_the_check_gave() {
        let (address, received) = fake_smtp().await;
        // A name nothing answers for: the mail reaches the server only if
        // the transport dials the checked address rather than the name.
        let server = |dialled| MailServer {
            host: "mail.invalid".into(),
            port: address.port(),
            security: MAIL_SECURITY_NONE.into(),
            username: None,
            password: None,
            addresses: dialled,
        };
        let cx = CxTestBuilder::new().build();
        let receipt = server(vec![address])
            .transport()
            .unwrap()
            .send(&cx, hello(&cx).await)
            .await
            .unwrap();
        let wire = tokio::time::timeout(Duration::from_secs(5), received)
            .await
            .expect("the mail arrives")
            .unwrap();
        assert!(wire.contains("Subject: Hello"), "{wire}");
        assert!(wire.contains("To: buyer@acme.test"), "{wire}");
        assert!(!receipt.message_id().is_empty());
        assert!(wire.contains(receipt.message_id()), "{wire}");

        // Dialled by name, the name is what counts, and this one answers
        // nothing.
        let by_name = server(Vec::new()).transport().unwrap();
        assert!(by_name.send(&cx, hello(&cx).await).await.is_err());
    }

    #[tokio::test]
    async fn an_address_that_cannot_be_reached_is_passed_over_for_the_next() {
        // A port nothing listens on any more: the connection is refused.
        let dead = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap()
        };
        let (alive, received) = fake_smtp().await;
        let server = |dialled| MailServer {
            host: "mail.invalid".into(),
            port: alive.port(),
            security: MAIL_SECURITY_NONE.into(),
            username: None,
            password: None,
            addresses: dialled,
        };
        let cx = CxTestBuilder::new().build();
        // The name answered the dead address first, as a host with an AAAA
        // record this node has no route for, or a server down, does.
        server(vec![dead, alive])
            .transport()
            .unwrap()
            .send(&cx, hello(&cx).await)
            .await
            .unwrap();
        let wire = tokio::time::timeout(Duration::from_secs(5), received)
            .await
            .expect("the mail arrives through the next address")
            .unwrap();
        assert!(wire.contains("Subject: Hello"), "{wire}");

        // Every address dead: the send fails with the connection error.
        let err = server(vec![dead, dead])
            .transport()
            .unwrap()
            .send(&cx, hello(&cx).await)
            .await
            .unwrap_err();
        assert!(!err.is::<NoAnswer>(), "{err}");

        // What moved on: the connection refused.
        let refused = server(vec![dead]).smtp(&dead.ip().to_string()).unwrap();
        let envelope = envelope_of(&hello(&cx).await).unwrap();
        let err = refused.send_raw(&envelope, b"").await.unwrap_err();
        assert!(is_unreachable(&err), "{err}");
    }

    #[tokio::test]
    async fn tls_is_for_the_name_whatever_address_is_dialled() {
        use tokio::io::AsyncReadExt as _;

        // Takes one connection and gives back the server name the client's
        // TLS hello asks for.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sent, received) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut hello = Vec::new();
            // The TLS record header says how long the hello is.
            while hello.len() < 5 || hello.len() < 5 + record_length(&hello) {
                if socket.read_buf(&mut hello).await.unwrap() == 0 {
                    break;
                }
            }
            let _ = sent.send(server_name_of(&hello));
        });
        let server = MailServer {
            host: "mail.example".into(),
            port: address.port(),
            security: MAIL_SECURITY_TLS.into(),
            username: None,
            password: None,
            addresses: vec![address],
        };
        let cx = CxTestBuilder::new().build();
        // A fake that says nothing back: the send fails, the hello was sent.
        let err = server
            .transport()
            .unwrap()
            .send(&cx, hello(&cx).await)
            .await
            .unwrap_err();
        assert!(!err.is::<NoAnswer>(), "{err}");
        let name = tokio::time::timeout(Duration::from_secs(5), received)
            .await
            .expect("the hello arrives")
            .unwrap();
        assert_eq!(name.as_deref(), Some("mail.example"));
    }

    /// The length a TLS record header announces.
    fn record_length(record: &[u8]) -> usize {
        usize::from(u16::from_be_bytes([record[3], record[4]]))
    }

    /// The server name a TLS client hello asks for (its SNI extension).
    fn server_name_of(record: &[u8]) -> Option<String> {
        let number = |at: usize, bytes: usize| -> Option<usize> {
            record.get(at..at + bytes).map(|bytes| {
                bytes
                    .iter()
                    .fold(0, |n, byte| (n << 8) | usize::from(*byte))
            })
        };
        // Record header (type 22, version, length), handshake header (type 1,
        // length), then the hello: version, random, session id, cipher
        // suites, compression methods, extensions.
        assert_eq!(record.first(), Some(&22), "a handshake record");
        assert_eq!(record.get(5), Some(&1), "a client hello");
        let mut at = 5 + 4 + 2 + 32;
        at += 1 + number(at, 1)?;
        at += 2 + number(at, 2)?;
        at += 1 + number(at, 1)?;
        let end = at + 2 + number(at, 2)?;
        at += 2;
        while at + 4 <= end {
            let (kind, length) = (number(at, 2)?, number(at + 2, 2)?);
            at += 4;
            // server_name: list length, name type (0: host name), name
            // length, the name.
            if kind == 0 {
                let length = number(at + 3, 2)?;
                let name = record.get(at + 5..at + 5 + length)?;
                return String::from_utf8(name.to_vec()).ok();
            }
            at += length;
        }
        None
    }

    #[tokio::test]
    async fn a_kept_transport_is_looked_up_again_after_the_window() {
        let mail = ShopMail::new(Some(Sealer::new("mail", KEY)), true);
        let tenant = shop(vec![settings(&mail, "a", "orders@acme.test")]);
        let first = transport_of(mail.sender_of(&tenant, &platform()).await.unwrap());

        // Within the window the kept transport is dialled, no lookup.
        mail.age_transport(&tenant, "a", RESOLVE_TTL / 2, Vec::new());
        let again = transport_of(mail.sender_of(&tenant, &platform()).await.unwrap());
        assert!(Arc::ptr_eq(&first, &again));

        // Past it the host is looked up again: the same addresses keep the
        // transport, and its pooled connections, for another window.
        mail.age_transport(&tenant, "a", RESOLVE_TTL, Vec::new());
        let again = transport_of(mail.sender_of(&tenant, &platform()).await.unwrap());
        assert!(Arc::ptr_eq(&first, &again));
        mail.age_transport(&tenant, "a", RESOLVE_TTL / 2, Vec::new());
        let again = transport_of(mail.marketing_sender_of(&tenant).await.unwrap());
        assert!(Arc::ptr_eq(&first, &again));

        // A host that moved to another address gets a new transport, which
        // dials the new one.
        mail.age_transport(&tenant, "a", RESOLVE_TTL, vec![ELSEWHERE.parse().unwrap()]);
        let moved = transport_of(mail.sender_of(&tenant, &platform()).await.unwrap());
        assert!(!Arc::ptr_eq(&first, &moved));

        // The same addresses in another order (round-robin DNS) are the same
        // host: the transport is kept.
        let one: SocketAddr = ELSEWHERE.parse().unwrap();
        let two: SocketAddr = "93.184.215.15:1025".parse().unwrap();
        assert!(same_addresses(&[one, two], &[two, one]));
        assert!(!same_addresses(&[one, two], &[one]));
        assert!(!same_addresses(&[one], &[two]));
    }

    #[tokio::test]
    async fn a_host_that_turns_private_is_refused_when_looked_up_again() {
        let mail = ShopMail::new(Some(Sealer::new("mail", KEY)), false);
        let tenant = shop(vec![settings(&mail, "a", "orders@acme.test")]);
        // As if the host (localhost here) had answered a public address when
        // the transport was built.
        mail.stand_in_for(&tenant, "a", Arc::new(MemoryTransport::new()));
        mail.age_transport(
            &tenant,
            "a",
            Duration::ZERO,
            vec![ELSEWHERE.parse().unwrap()],
        );
        assert!(
            mail.sender_of(&tenant, &platform())
                .await
                .unwrap()
                .is_own_server()
        );

        // Past the window the name is looked up again and now answers a
        // private address: refused, and the transport to it dropped.
        mail.age_transport(&tenant, "a", RESOLVE_TTL, vec![ELSEWHERE.parse().unwrap()]);
        let err = mail.sender_of(&tenant, &platform()).await.err().unwrap();
        assert!(err.to_string().contains("not a public address"), "{err}");
        assert!(!mail.keeps_transport_for(&tenant, "a"));
        let err = mail.marketing_sender_of(&tenant).await.err().unwrap();
        assert!(err.to_string().contains("not a public address"), "{err}");
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
            addresses: Vec::new(),
        };
        assert_eq!(server.transport().unwrap().limit, SEND_TIMEOUT);
        let transport = TimedTransport {
            client: server.smtp(&server.host).unwrap(),
            fallbacks: Vec::new(),
            limit: Duration::from_millis(300),
        };
        let cx = CxTestBuilder::new().build();
        let err = tokio::time::timeout(
            Duration::from_secs(5),
            transport.send(&cx, hello(&cx).await),
        )
        .await
        .expect("given up within its limit")
        .unwrap_err();
        assert!(err.is::<NoAnswer>(), "{err}");
    }

    #[test]
    fn the_receipt_carries_the_message_id() {
        let wire = b"Date: Mon, 1 Jan 2026 00:00:00 +0000\r\nmessage-id: <abc@acme.test>\r\n\r\nMessage-ID: <in-the-body@acme.test>\r\n";
        assert_eq!(message_id_of(wire), "<abc@acme.test>");
        assert_eq!(message_id_of(b"Subject: Hello\r\n\r\nHello\r\n"), "");
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
