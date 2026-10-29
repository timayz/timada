//! A shopper's browser, as far as the server can tell: one connection pool
//! shared by every shopper (like a proxy in front of them), but each with
//! its own cookies, kept by hand so a cookie set on a redirect (the cart's,
//! on adding to it) goes with the request that follows. Redirects are
//! followed here too, each hop timed under its own route.

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use regex::Regex;
use reqwest::Method;
use reqwest::header::{CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE};

use crate::metrics::Metrics;

const MAX_REDIRECTS: usize = 5;

pub struct Http {
    client: reqwest::Client,
    base: String,
    metrics: Arc<Metrics>,
}

impl Http {
    pub fn new(
        base: &str,
        insecure: bool,
        timeout: Duration,
        metrics: Arc<Metrics>,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("timada-loadtest")
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .danger_accept_invalid_certs(insecure)
            .pool_max_idle_per_host(1024)
            .build()?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
            metrics,
        })
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }
}

/// What a page answered, after its redirects.
pub struct Page {
    /// The path (and query) the last hop was for.
    pub path: String,
    pub body: String,
}

pub struct Visitor<'a> {
    http: &'a Http,
    cookies: BTreeMap<String, String>,
}

impl<'a> Visitor<'a> {
    pub fn new(http: &'a Http) -> Self {
        Self {
            http,
            cookies: BTreeMap::new(),
        }
    }

    pub async fn get(&mut self, path: &str) -> Result<Page> {
        self.send(Method::GET, path, None).await
    }

    pub async fn post(&mut self, path: &str, form: &[(&str, String)]) -> Result<Page> {
        let body = form
            .iter()
            .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        self.send(Method::POST, path, Some(body)).await
    }

    async fn send(&mut self, method: Method, path: &str, form: Option<String>) -> Result<Page> {
        let mut method = method;
        let mut path = path.to_owned();
        let mut form = form;
        for _ in 0..=MAX_REDIRECTS {
            let route = format!("{method} {}", route_of(&path));
            let mut request = self
                .http
                .client
                .request(method.clone(), format!("{}{path}", self.http.base));
            if !self.cookies.is_empty() {
                let header = self
                    .cookies
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                request = request.header(COOKIE, header);
            }
            if let Some(body) = form.take() {
                request = request
                    .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(body);
            }
            let started = Instant::now();
            let response = match request.send().await {
                Ok(response) => response,
                Err(err) => {
                    let kind = if err.is_timeout() {
                        "timeout"
                    } else {
                        "transport"
                    };
                    self.http
                        .metrics
                        .request(&route, started.elapsed(), Some(kind.into()));
                    return Err(err).with_context(|| route.clone());
                }
            };
            let status = response.status();
            self.keep_cookies(&response);
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let body = response.text().await;
            let took = started.elapsed();
            let body = match body {
                Ok(body) => body,
                Err(err) => {
                    self.http.metrics.request(&route, took, Some("body".into()));
                    return Err(err).with_context(|| route.clone());
                }
            };
            let failure = (status.is_server_error() || status.is_client_error())
                .then(|| status.as_u16().to_string());
            self.http.metrics.request(&route, took, failure.clone());
            if failure.is_some() {
                bail!("{route} answered {status}");
            }
            match (status.is_redirection(), location) {
                (true, Some(location)) => {
                    path = self.local(&location)?;
                    method = Method::GET;
                }
                _ => return Ok(Page { path, body }),
            }
        }
        bail!("too many redirects from {path}")
    }

    /// A `Location` as a path on this site.
    fn local(&self, location: &str) -> Result<String> {
        if location.starts_with('/') {
            return Ok(location.to_owned());
        }
        location
            .strip_prefix(&self.http.base)
            .map(str::to_owned)
            .with_context(|| format!("redirected off the site, to {location}"))
    }

    fn keep_cookies(&mut self, response: &reqwest::Response) {
        for header in response.headers().get_all(SET_COOKIE) {
            let Ok(header) = header.to_str() else {
                continue;
            };
            let mut parts = header.split(';');
            let Some((name, value)) = parts.next().and_then(|pair| pair.split_once('=')) else {
                continue;
            };
            let gone = value.is_empty()
                || parts.any(|attribute| {
                    let attribute = attribute.trim().to_ascii_lowercase();
                    attribute == "max-age=0" || attribute.starts_with("max-age=-")
                });
            if gone {
                self.cookies.remove(name.trim());
            } else {
                self.cookies
                    .insert(name.trim().to_owned(), value.trim().to_owned());
            }
        }
    }
}

/// The route a path is reported under: its fixed segments, the variable
/// ones named, the query left out.
pub fn route_of(path: &str) -> String {
    static ROUTES: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
        [
            (r"^/p/[^/]+$", "/p/{slug}"),
            (r"^/c/[^/]+$", "/c/{slug}"),
            (r"^/orders/[^/]+$", "/orders/{id}"),
            (r"^/media/.+$", "/media/{file}"),
        ]
        .into_iter()
        .map(|(pattern, route)| (Regex::new(pattern).unwrap(), route))
        .collect()
    });
    let path = path.split(['?', '#']).next().unwrap_or(path);
    ROUTES
        .iter()
        .find(|(pattern, _)| pattern.is_match(path))
        .map_or_else(|| path.to_owned(), |(_, route)| (*route).to_owned())
}

/// `application/x-www-form-urlencoded`, as a browser sends a form.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_name_their_variable_segments() {
        assert_eq!(route_of("/p/arbre-a-chat?error=x"), "/p/{slug}");
        assert_eq!(route_of("/c/jeux-pour-chat#cart"), "/c/{slug}");
        assert_eq!(route_of("/orders/abc"), "/orders/{id}");
        assert_eq!(route_of("/checkout/complete?x=1"), "/checkout/complete");
        assert_eq!(route_of("/"), "/");
    }

    #[test]
    fn a_form_value_is_encoded_as_a_browser_does() {
        assert_eq!(encode("1 rue de l'Été"), "1+rue+de+l%27%C3%89t%C3%A9");
        assert_eq!(encode("option:01AB"), "option%3A01AB");
    }
}
