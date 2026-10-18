//! What this server may be made to talk to on a merchant's say-so: a host
//! a merchant names (a mail server, a picture's address) has to be on the
//! internet, never this machine or a private network.

use std::net::IpAddr;

use anyhow::Context as _;

/// Refuses a host that resolves to an address that is not public.
pub async fn check_public_host(host: &str, port: u16) -> anyhow::Result<()> {
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

/// Whether `address` is reachable from the internet: not loopback, private,
/// link-local, shared (CGNAT), unspecified, broadcast, documentation or
/// multicast.
pub fn is_public(address: IpAddr) -> bool {
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
