//! What brought a shopper: the click ids (`gclid`, `gbraid`, `wbraid`,
//! `fbclid`) and campaign parameters (`utm_*`) an address carries, and the
//! site the visitor came from.
//!
//! The library reads an arrival off a request ([`arriving_click`]) and
//! folds it into what the browser already holds ([`remember_click`]): the
//! first arrival stays, the last one moves. The host keeps the
//! [`OrderSource`] in a first-party cookie for [`SOURCE_MAX_AGE_DAYS`] and
//! hands it to [`Checkout::source`](crate::Checkout), so the order keeps
//! it. Everything goes through [`source_within`] first: a click id is kept
//! only when the visitor allowed advertising, the rest only when they
//! allowed measuring, and nothing without an answer.

use topcoat_core::context::Cx;
use topcoat_router::request::{headers, original_uri};

pub use timada_orders::{Browser, Click, OrderSource};

use crate::consent::Consent;
use crate::context::store;

/// How long a host keeps what brought a visitor: ninety days, the longest
/// Google Ads counts a sale after a click.
pub const SOURCE_MAX_AGE_DAYS: i64 = 90;

/// The longest value kept of an arrival; a longer one is no click id or
/// campaign name, and the cookie has to hold two arrivals.
const MAX_VALUE_LEN: usize = 255;

fn kept(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.chars().count() <= MAX_VALUE_LEN
        && !value.chars().any(char::is_control))
    .then(|| value.to_owned())
}

/// `referrer` without its query and fragment when it is a page of another
/// site than `hosts` (the shop's own names): what a visitor came from.
pub fn external_referrer(referrer: &str, hosts: &[&str]) -> Option<String> {
    let url = reqwest::Url::parse(referrer.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_lowercase();
    if hosts.iter().any(|own| own.eq_ignore_ascii_case(&host)) {
        return None;
    }
    kept(&format!("{}://{}{}", url.scheme(), host, url.path()))
}

/// The arrival an address tells of, at `now` (Unix seconds): `query` is
/// its query string, `landing` its path, `referrer` the page it was
/// reached from, already known to be another site's. `None` when nothing
/// says where the visitor came from: a page reached from the shop itself,
/// or typed in.
pub fn click_of(query: &str, referrer: Option<&str>, landing: &str, now: u64) -> Option<Click> {
    let mut click = Click {
        referrer: referrer.and_then(kept),
        at: now,
        ..Click::default()
    };
    // The first of a parameter given twice stands.
    if let Ok(url) = reqwest::Url::parse(&format!("http://shop/?{query}")) {
        for (name, value) in url.query_pairs() {
            let slot = match name.as_ref() {
                "gclid" => &mut click.gclid,
                "gbraid" => &mut click.gbraid,
                "wbraid" => &mut click.wbraid,
                "fbclid" => &mut click.fbclid,
                "utm_source" => &mut click.utm_source,
                "utm_medium" => &mut click.utm_medium,
                "utm_campaign" => &mut click.utm_campaign,
                "utm_term" => &mut click.utm_term,
                "utm_content" => &mut click.utm_content,
                _ => continue,
            };
            if slot.is_none() {
                *slot = kept(&value);
            }
        }
    }
    if !click.tells() {
        return None;
    }
    click.landing = kept(landing);
    Some(click)
}

/// The shop's own host names, as a request shows them: the one it was
/// asked under and the one of
/// [`StoreConfig::public_url`](crate::StoreConfig::public_url).
fn own_hosts(cx: &Cx) -> Vec<String> {
    let host_of = |authority: &str| {
        authority
            .rsplit_once(':')
            .filter(|(_, port)| port.bytes().all(|b| b.is_ascii_digit()))
            .map_or(authority, |(host, _)| host)
            .to_owned()
    };
    let mut hosts = Vec::new();
    if let Some(host) = headers(cx).get("host").and_then(|host| host.to_str().ok()) {
        hosts.push(host_of(host));
    }
    if let Some(host) = store(cx)
        .public_url()
        .and_then(|url| reqwest::Url::parse(url).ok())
        .and_then(|url| url.host_str().map(str::to_owned))
    {
        hosts.push(host);
    }
    hosts
}

/// The page of another site this request was reached from (its `Referer`,
/// without the query), if any: what a form that answers later (the cookie
/// banner's) carries along.
pub fn request_referrer(cx: &Cx) -> Option<String> {
    let referrer = headers(cx).get("referer")?.to_str().ok()?;
    let hosts = own_hosts(cx);
    let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();
    external_referrer(referrer, &hosts)
}

/// The arrival this request tells of, if any: see [`click_of`].
pub fn arriving_click(cx: &Cx) -> Option<Click> {
    let uri = original_uri(cx);
    click_of(
        uri.query().unwrap_or_default(),
        request_referrer(cx).as_deref(),
        uri.path(),
        now(),
    )
}

/// The arrival a path on the shop (`/boutique?gclid=…`) and the page it was
/// reached from tell of: what the cookie banner's form carries, for the
/// visitor who allows it only once they have landed.
pub fn click_at(cx: &Cx, path_and_query: &str, referrer: Option<&str>) -> Option<Click> {
    let (path, query) = path_and_query
        .split_once('?')
        .unwrap_or((path_and_query, ""));
    let hosts = own_hosts(cx);
    let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();
    let referrer = referrer.and_then(|referrer| external_referrer(referrer, &hosts));
    click_of(query, referrer.as_deref(), path, now())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// What of an arrival the visitor allowed keeping: the click ids with
/// advertising, the campaign, the referrer and the landing page with
/// measuring. `None` when nothing that says where they came from is left.
pub fn click_within(mut click: Click, consent: Consent) -> Option<Click> {
    if !consent.ads {
        click.gclid = None;
        click.gbraid = None;
        click.wbraid = None;
        click.fbclid = None;
    }
    if !consent.analytics {
        click.utm_source = None;
        click.utm_medium = None;
        click.utm_campaign = None;
        click.utm_term = None;
        click.utm_content = None;
        click.referrer = None;
        click.landing = None;
    }
    click.tells().then_some(click)
}

/// What of a kept source the visitor allows now: an answer changed since
/// takes back what it no longer covers. No answer allows nothing.
pub fn source_within(source: OrderSource, consent: Option<Consent>) -> Option<OrderSource> {
    let consent = consent?;
    match (
        click_within(source.first, consent),
        click_within(source.last, consent),
    ) {
        (Some(first), Some(last)) => Some(OrderSource { first, last }),
        (Some(only), None) | (None, Some(only)) => Some(OrderSource {
            first: only.clone(),
            last: only,
        }),
        (None, None) => None,
    }
}

/// The source once `click` arrived: the first arrival the browser holds
/// stays, the last is this one. The last arrival seen again (its page
/// loaded once more, or come back to with its address whole) is the same
/// arrival, not a new one: nothing moves.
pub fn remember_click(source: Option<OrderSource>, click: Click) -> OrderSource {
    match source {
        Some(source)
            if (Click {
                at: click.at,
                ..source.last.clone()
            }) == click =>
        {
            source
        }
        Some(source) => OrderSource {
            first: source.first,
            last: click,
        },
        None => OrderSource {
            first: click.clone(),
            last: click,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADS_ONLY: Consent = Consent {
        analytics: false,
        ads: true,
    };
    const ANALYTICS_ONLY: Consent = Consent {
        analytics: true,
        ads: false,
    };

    #[test]
    fn an_address_tells_its_click_ids_and_its_campaign() {
        let click = click_of(
            "page=2&gclid=Cj0KCQ&utm_source=google&utm_medium=cpc&utm_campaign=Soldes+d%27hiver&utm_source=other",
            Some("https://www.google.com/"),
            "/c/tapis",
            1_800_000_000,
        )
        .unwrap();
        assert_eq!(click.gclid.as_deref(), Some("Cj0KCQ"));
        assert_eq!(click.utm_source.as_deref(), Some("google"));
        assert_eq!(click.utm_campaign.as_deref(), Some("Soldes d'hiver"));
        assert_eq!(click.referrer.as_deref(), Some("https://www.google.com/"));
        assert_eq!(click.landing.as_deref(), Some("/c/tapis"));
        assert_eq!(click.at, 1_800_000_000);
        assert_eq!(click.channel(), "google / cpc");
    }

    #[test]
    fn a_page_reached_from_nowhere_known_is_no_arrival() {
        assert_eq!(click_of("", None, "/", 1), None);
        assert_eq!(click_of("page=2&q=tapis", None, "/boutique", 1), None);
        // An empty or absurdly long value says nothing.
        assert_eq!(click_of("gclid=", None, "/", 1), None);
        let long = format!("gclid={}", "x".repeat(MAX_VALUE_LEN + 1));
        assert_eq!(click_of(&long, None, "/", 1), None);
        // Another site alone is an arrival.
        let click = click_of("", Some("https://blog.example/post"), "/", 1).unwrap();
        assert_eq!(click.channel(), "blog.example / referral");
    }

    #[test]
    fn a_referrer_is_another_sites_page_without_its_query() {
        let own = ["shop.example", "127.0.0.1"];
        assert_eq!(
            external_referrer("https://www.Google.com/search?q=tapis+chat#top", &own).as_deref(),
            Some("https://www.google.com/search")
        );
        assert_eq!(
            external_referrer("https://shop.example/boutique", &own),
            None
        );
        assert_eq!(external_referrer("http://127.0.0.1:3457/", &own), None);
        assert_eq!(
            external_referrer("android-app://com.google.android.gm/", &own),
            None
        );
        assert_eq!(external_referrer("nonsense", &own), None);
    }

    #[test]
    fn only_what_the_visitor_allowed_is_kept() {
        let click = click_of(
            "gclid=G&utm_source=lettre",
            Some("https://a.example/"),
            "/",
            1,
        )
        .unwrap();
        let ads = click_within(click.clone(), ADS_ONLY).unwrap();
        assert_eq!(ads.gclid.as_deref(), Some("G"));
        assert_eq!(
            (ads.utm_source, ads.referrer, ads.landing),
            (None, None, None)
        );
        let measured = click_within(click.clone(), ANALYTICS_ONLY).unwrap();
        assert_eq!(measured.gclid, None);
        assert_eq!(measured.utm_source.as_deref(), Some("lettre"));
        assert_eq!(click_within(click.clone(), Consent::NONE), None);
        assert_eq!(click_within(click.clone(), Consent::ALL), Some(click));
        // A campaign alone is nothing to keep for a visitor who allowed
        // advertising only.
        let campaign = click_of("utm_source=lettre", None, "/", 1).unwrap();
        assert_eq!(click_within(campaign, ADS_ONLY), None);
    }

    #[test]
    fn the_first_arrival_stays_and_the_last_moves() {
        let letter = click_of("utm_source=lettre", None, "/", 1).unwrap();
        let ad = click_of("gclid=G", None, "/p/tapis", 2).unwrap();
        let again = click_of("fbclid=F", None, "/", 3).unwrap();
        let source = remember_click(None, letter.clone());
        assert_eq!((&source.first, &source.last), (&letter, &letter));
        let source = remember_click(Some(source), ad);
        let source = remember_click(Some(source), again.clone());
        assert_eq!((&source.first, &source.last), (&letter, &again));
        // The same address loaded again later is no new arrival.
        let reloaded = click_of("fbclid=F", None, "/", 9).unwrap();
        assert_eq!(remember_click(Some(source.clone()), reloaded), source);
        let once = remember_click(None, letter.clone());
        let reloaded = click_of("utm_source=lettre", None, "/", 9).unwrap();
        assert_eq!(remember_click(Some(once.clone()), reloaded), once);
    }

    #[test]
    fn an_answer_taken_back_takes_the_source_with_it() {
        let letter = click_of("utm_source=lettre", None, "/", 1).unwrap();
        let ad = click_of("gclid=G", None, "/p/tapis", 2).unwrap();
        let source = remember_click(Some(remember_click(None, letter.clone())), ad);
        assert_eq!(source_within(source.clone(), None), None);
        assert_eq!(source_within(source.clone(), Some(Consent::NONE)), None);
        // What is left of one arrival stands for both.
        let measured = source_within(source.clone(), Some(ANALYTICS_ONLY)).unwrap();
        assert_eq!(measured.first, letter);
        assert_eq!(measured.last.utm_source.as_deref(), Some("lettre"));
        assert_eq!(
            source_within(source.clone(), Some(Consent::ALL)),
            Some(source)
        );
    }
}
