//! What this server may be made to talk to on a merchant's say-so: a host
//! a merchant names (a mail server, a picture's address, a webhook) has to
//! be on the internet, never this machine or a private network.
//!
//! Two checks, because a name can answer differently twice (DNS rebinding):
//! [`check_public_url`] / [`check_public_host`] look the name up before a
//! request and refuse an address that is not public, and [`PublicResolver`]
//! is the resolver the HTTP clients then connect through, so the address
//! actually dialled is checked too. A literal IP address never reaches a
//! resolver; the first check covers it. The mail transports take the other
//! way round: [`public_addresses`] gives the addresses a name was checked
//! at, and the transport dials those, in turn, rather than the name
//! (`shop_mail.rs`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use anyhow::Context as _;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Refuses a host that resolves to an address that is not public.
pub async fn check_public_host(host: &str, port: u16) -> anyhow::Result<()> {
    public_addresses(host, port).await.map(drop)
}

/// The addresses `host` answers at `port`, in the order it answers them,
/// when every one of them is public; refused otherwise (never empty). What a
/// client dials, in turn, to reach the host as checked, rather than looking
/// the name up again.
pub async fn public_addresses(host: &str, port: u16) -> anyhow::Result<Vec<SocketAddr>> {
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .with_context(|| format!("cannot resolve {host}"))?
        .collect();
    anyhow::ensure!(!addresses.is_empty(), "{host} has no address");
    if let Some(address) = addresses.iter().find(|address| !is_public(address.ip())) {
        anyhow::bail!(
            "{host} resolves to {}, which is not a public address",
            address.ip()
        );
    }
    Ok(addresses)
}

/// Refuses an address whose host is not public (see [`check_public_host`]).
pub async fn check_public_url(url: &str) -> anyhow::Result<()> {
    let parsed = reqwest::Url::parse(url).with_context(|| format!("not an address: {url}"))?;
    anyhow::ensure!(
        matches!(parsed.scheme(), "http" | "https"),
        "not a web address: {url}"
    );
    let host = parsed.host_str().context("an address without a host")?;
    let port = parsed.port_or_known_default().unwrap_or(443);
    check_public_host(host.trim_start_matches('[').trim_end_matches(']'), port).await
}

/// Whether `address` is reachable from the internet: none of the special
/// ranges IANA keeps off it (loopback, private, link-local, shared (CGNAT),
/// unspecified, broadcast, documentation, benchmarking, multicast, the
/// protocol-assignment and reserved blocks), and for IPv6 none of the
/// transition and special blocks either (IPv4-mapped, NAT64, 6to4, Teredo,
/// ORCHID, discard, unique-local, link-local); an address that embeds an
/// IPv4 one is public only when that one is.
pub fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(v4: Ipv4Addr) -> bool {
    let [a, b, c, _] = v4.octets();
    !(v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_multicast()
        // "This network".
        || a == 0
        // Shared address space (CGNAT), 100.64.0.0/10.
        || (a == 100 && (64..128).contains(&b))
        // IETF protocol assignments, 192.0.0.0/24.
        || (a == 192 && b == 0 && c == 0)
        // 6to4 relay anycast, 192.88.99.0/24 (deprecated).
        || (a == 192 && b == 88 && c == 99)
        // Benchmarking, 198.18.0.0/15.
        || (a == 198 && (b == 18 || b == 19))
        // Reserved, 240.0.0.0/4.
        || a >= 240)
}

fn is_public_v6(v6: Ipv6Addr) -> bool {
    if let Some(v4) = v6.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let segments = v6.segments();
    let [first, second, third, ..] = segments;
    // An IPv4 address carried inside: public only if that one is.
    let embedded = |hi: u16, lo: u16| Ipv4Addr::from(((hi as u32) << 16) | lo as u32);
    // IPv4-compatible (::a.b.c.d, deprecated): everything but the last two
    // segments zero, and not :: or ::1 themselves.
    if segments[..6] == [0; 6] && !v6.is_unspecified() && !v6.is_loopback() {
        return is_public_v4(embedded(segments[6], segments[7]));
    }
    // NAT64 well-known prefix, 64:ff9b::/96.
    if first == 0x64 && second == 0xff9b && segments[2..6] == [0; 4] {
        return is_public_v4(embedded(segments[6], segments[7]));
    }
    // 6to4, 2002::/16: the IPv4 address follows the prefix.
    if first == 0x2002 {
        return is_public_v4(embedded(second, third));
    }
    !(v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        // Unique local, fc00::/7.
        || (first & 0xfe00) == 0xfc00
        // Link-local, fe80::/10.
        || (first & 0xffc0) == 0xfe80
        // Discard-only, 100::/64.
        || (first == 0x100 && segments[1..4] == [0; 3])
        // Local-use NAT64, 64:ff9b:1::/48.
        || (first == 0x64 && second == 0xff9b && third == 1)
        // Teredo, 2001::/32: the client's address is in there, obfuscated;
        // nothing to check it against, so refused.
        || (first == 0x2001 && second == 0)
        // Benchmarking, 2001:2::/48.
        || (first == 0x2001 && second == 2 && third == 0)
        // ORCHID and ORCHIDv2, 2001:10::/28 and 2001:20::/28.
        || (first == 0x2001 && (second & 0xfff0) == 0x10)
        || (first == 0x2001 && (second & 0xfff0) == 0x20)
        // Documentation, 2001:db8::/32.
        || (first == 0x2001 && second == 0xdb8))
}

/// The resolver an HTTP client that reaches hosts merchants name connects
/// through: every address a name answers with must be public, or the name
/// fails to resolve. With it the address dialled is the one checked, so a
/// name that answered a public address to [`check_public_url`] and a private
/// one a moment later (DNS rebinding) gets nowhere.
#[derive(Debug, Default, Clone, Copy)]
pub struct PublicResolver;

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses: Vec<_> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if let Some(address) = addresses.iter().find(|address| !is_public(address.ip())) {
                return Err(format!(
                    "{host} resolves to {}, which is not a public address",
                    address.ip()
                )
                .into());
            }
            let addresses: Addrs = Box::new(addresses.into_iter());
            Ok(addresses)
        })
    }
}

/// A client builder for requests to hosts merchants name: it follows no
/// redirect (the next hop would escape the check) and, unless
/// `allow_private_hosts` (development and tests), connects through
/// [`PublicResolver`]. The caller still runs [`check_public_url`] first,
/// which covers literal IP addresses.
pub fn public_client(allow_private_hosts: bool) -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder()
        .user_agent("timada")
        .redirect(reqwest::redirect::Policy::none());
    if allow_private_hosts {
        builder
    } else {
        builder.dns_resolver(Arc::new(PublicResolver))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_addresses_and_the_ranges_kept_off_the_internet() {
        for public in [
            "93.184.215.14",
            "8.8.8.8",
            "1.1.1.1",
            "198.17.255.255",
            "198.20.0.1",
            "192.0.1.1",
            "223.255.255.254",
            "2606:2800:21f:cb07:6820:80da:af6b:8b2c",
            "2001:4860:4860::8888",
            "2001:1::1",
            "2001:30::1",
            // 6to4 and NAT64 around public IPv4 addresses.
            "2002:5db8:d70e::1",
            "64:ff9b::5db8:d70e",
            "::ffff:93.184.215.14",
        ] {
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
            "0.1.2.3",
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.1",
            "203.0.113.1",
            "240.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fd00::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::127.0.0.1",
            "::10.1.2.3",
            "100::1",
            "64:ff9b::7f00:1",
            "64:ff9b::a00:1",
            "64:ff9b:1::1",
            "2002:7f00:1::1",
            "2002:c0a8:101::1",
            "2001::1",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2001:2::1",
            "2001:10::1",
            "2001:1f::1",
            "2001:20::1",
            "2001:db8::1",
            "ff02::1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
    }

    #[tokio::test]
    async fn the_public_addresses_of_a_host_are_the_ones_checked() {
        assert_eq!(
            public_addresses("93.184.215.14", 587).await.unwrap(),
            vec!["93.184.215.14:587".parse::<SocketAddr>().unwrap()]
        );
        let err = public_addresses("localhost", 25).await.unwrap_err();
        assert!(err.to_string().contains("not a public address"), "{err}");
        assert!(public_addresses("127.0.0.1", 25).await.is_err());
    }

    #[tokio::test]
    async fn the_resolver_refuses_a_name_that_answers_a_private_address() {
        let err = match PublicResolver.resolve("localhost".parse().unwrap()).await {
            Err(err) => err,
            Ok(_) => panic!("localhost resolved"),
        };
        assert!(err.to_string().contains("not a public address"), "{err}");
    }

    #[tokio::test]
    async fn a_client_for_merchant_hosts_cannot_reach_this_machine() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = public_client(false).build().unwrap();
        let err = client
            .get(format!("http://localhost:{port}/"))
            .send()
            .await
            .unwrap_err();
        assert!(err.is_connect() || err.is_request(), "{err}");
        // The literal address never reaches a resolver: the URL check is
        // what refuses it.
        assert!(
            check_public_url(&format!("http://127.0.0.1:{port}/"))
                .await
                .is_err()
        );
        // With private hosts allowed (tests, development), it connects.
        let allowed = public_client(true).build().unwrap();
        let (accepted, _) = tokio::join!(
            async {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::io::AsyncWriteExt::write_all(
                    &mut socket,
                    b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n",
                )
                .await
                .unwrap();
            },
            async {
                let response = allowed
                    .get(format!("http://localhost:{port}/"))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 204);
            }
        );
        let () = accepted;
    }
}
