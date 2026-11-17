//! The languages the shop is read in, and the addresses of each.
//!
//! The pages are routed at their French addresses, the ones this
//! storefront is written under (`/boutique`, `/mentions-legales`). The
//! shop's main language is read at an address without a prefix, and each
//! of its other languages under its own (`/en/…`); a language that is not
//! French also names the pages its own way (`/en/shop`, `/en/legal-notice`).
//! [`Localized`] turns the address a visitor asked for into the French one
//! the router knows, and the addresses in the answer back into the
//! visitor's language: every link, form and redirect of a page, the
//! merchant's own links in a page or an announcement among them.
//!
//! Which languages a shop is read in is the merchant's choice
//! ([`shop_languages`]); this storefront serves those it has words for
//! ([`SPOKEN`]) and leaves the others out.

use crate::app::words::t;
use timada_storefront::{MoneyFormat, ReadIn, absolute_url, shop_languages};
use topcoat::{
    context::{Cx, context, memoize, try_context},
    l10n::{Locale, locale, try_locale},
    router::{
        Body, HeaderValue, Layer, LayerFuture, Method, Next, Path,
        error::{RedirectError, SeeOther, redirect_permanent, rewrite},
        header::{CONTENT_LENGTH, CONTENT_TYPE, LOCATION},
        request::{method, original_uri, uri},
        response::{IntoResponse, Response},
        to_bytes,
    },
};

/// The language the pages are routed in, and the one a shop that set no
/// language is read in.
pub const BASE: &str = "fr";

/// The languages this storefront has words for.
pub const SPOKEN: &[&str] = &["fr", "en"];

/// What English calls the pages whose French address is a French word:
/// the first part of the path, French then English.
const ENGLISH_ROUTES: &[(&str, &str)] = &[
    ("boutique", "shop"),
    ("conditions-generales-de-vente", "terms-of-sale"),
    ("indisponible", "unavailable"),
    ("mentions-legales", "legal-notice"),
    ("politique-de-confidentialite", "privacy-policy"),
    ("politique-de-livraison", "shipping-policy"),
    ("politique-de-remboursement", "refund-policy"),
    ("qui-sommes-nous", "about-us"),
    ("support-client", "support"),
];

/// The first parts of a path that are the same address in every language:
/// what search engines and the library's own callers read. The sitemap's
/// index is one of them; the files it lists are a language's each.
const UNLOCALIZED: &[&str] = &["robots.txt", "sitemap.xml", "store", "favicon.ico"];

/// The attributes of a page that hold an address of the shop.
const ADDRESS_ATTRIBUTES: &[&str] = &[
    "href",
    "action",
    "formaction",
    "data-search",
    "data-start-path",
    "data-places-path",
    "data-return-path",
];

/// The pages a language names its own way, French then that language.
fn routes(language: &str) -> &'static [(&'static str, &'static str)] {
    match language {
        "en" => ENGLISH_ROUTES,
        _ => &[],
    }
}

/// The languages the shop is read in here: the one at the addresses
/// without a prefix, and the ones under theirs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Languages {
    pub main: &'static str,
    pub others: Vec<&'static str>,
}

impl Default for Languages {
    fn default() -> Self {
        Languages {
            main: BASE,
            others: Vec::new(),
        }
    }
}

impl Languages {
    /// What the merchant chose, kept to the languages this storefront has
    /// words for. A main language it has none for leaves the shop in
    /// French.
    pub fn of(main: &str, others: &[String]) -> Self {
        let spoken = |code: &str| SPOKEN.iter().copied().find(|known| *known == code);
        let main = spoken(main).unwrap_or(BASE);
        Languages {
            main,
            others: others
                .iter()
                .filter_map(|code| spoken(code))
                .filter(|code| *code != main)
                .collect(),
        }
    }

    /// Every language, the main one first.
    pub fn all(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.main).chain(self.others.iter().copied())
    }

    /// Whether every address is its French one: nothing to turn.
    fn plain(&self) -> bool {
        self.main == BASE && self.others.is_empty()
    }

    /// The language an address a visitor asked for is in, and the French
    /// path the router knows it by. A query or a fragment stays on.
    pub fn read(&self, visible: &str) -> (&'static str, String) {
        let (path, rest) = split(visible);
        let mut segments = path.trim_start_matches('/').splitn(2, '/');
        let first = segments.next().unwrap_or_default();
        let (language, path) = match self.others.iter().find(|code| **code == first) {
            Some(language) => (
                *language,
                format!("/{}", segments.next().unwrap_or_default()),
            ),
            None => (self.main, path.to_owned()),
        };
        (
            language,
            format!("{}{rest}", renamed(language, &path, false)),
        )
    }

    /// The address `language` reads the French path `internal` at. An
    /// address already in that language, or one that is the same in every
    /// language, comes back as it is.
    pub fn write(&self, language: &str, internal: &str) -> String {
        let (path, rest) = split(internal);
        let first = path
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or_default();
        if !path.starts_with('/') || path.starts_with("//") || UNLOCALIZED.contains(&first) {
            return internal.to_owned();
        }
        if language == self.main {
            return format!("{}{rest}", renamed(language, path, true));
        }
        if first == language {
            return internal.to_owned();
        }
        let path = renamed(language, path, true);
        let path = if path == "/" { String::new() } else { path };
        format!("/{language}{path}{rest}")
    }
}

/// A path apart from its query and its fragment.
fn split(address: &str) -> (&str, &str) {
    let end = address.find(['?', '#']).unwrap_or(address.len());
    address.split_at(end)
}

/// `path` with its first part named as `language` names it (`to_language`)
/// or as French does.
fn renamed(language: &str, path: &str, to_language: bool) -> String {
    let trimmed = path.trim_start_matches('/');
    let (first, rest) = match trimmed.split_once('/') {
        Some((first, rest)) => (first, Some(rest)),
        None => (trimmed, None),
    };
    let name = routes(language)
        .iter()
        .find(|(french, theirs)| first == if to_language { *french } else { *theirs })
        .map(|(french, theirs)| if to_language { *theirs } else { *french });
    match (name, rest) {
        (Some(name), Some(rest)) => format!("/{name}/{rest}"),
        (Some(name), None) => format!("/{name}"),
        (None, _) => path.to_owned(),
    }
}

/// The languages the shop is read in here, read once per request. A copy
/// that cannot say leaves the shop in French, as one that was never told.
#[memoize]
pub async fn languages(cx: &Cx) -> Languages {
    match shop_languages(cx).await {
        Ok(Some(set)) => Languages::of(&set.main, &set.others),
        Ok(None) => Languages::default(),
        Err(err) => {
            tracing::warn!(error = %err, "cannot read the shop's languages: answering in French");
            Languages::default()
        }
    }
}

/// What a language calls itself, for the link that switches to it.
pub fn native_name(language: &str) -> &str {
    match language {
        "fr" => "Français",
        "en" => "English",
        other => other,
    }
}

/// The language a visitor reads this page in, as its address says, and
/// the shop's other ones.
pub struct Speaking<'a> {
    languages: &'a Languages,
    /// The visitor's.
    pub language: &'static str,
    /// The French path of the address they asked for, without its query.
    pub internal: String,
}

/// What the address of this request says of the visitor's language.
pub async fn speaking(cx: &Cx) -> Speaking<'_> {
    let languages = languages(cx).await;
    let (language, internal) = languages.read(original_uri(cx).path());
    Speaking {
        languages,
        language,
        internal,
    }
}

impl Speaking<'_> {
    /// The address the visitor's language reads the French path
    /// `internal` at. The links of a page need none of this, [`Localized`]
    /// turns them; an address written elsewhere (in data for a search
    /// engine, in an answer a script reads) does.
    pub fn path(&self, internal: &str) -> String {
        self.languages.write(self.language, internal)
    }

    /// [`path`](Self::path), as a whole address: what a search engine or
    /// a shared link is given.
    pub fn absolute(&self, cx: &Cx, internal: &str) -> String {
        absolute_url(cx, &self.path(internal))
    }

    /// The whole address of the French path `internal` in each language
    /// of the shop, the main one first: none for a shop read in one
    /// language. `elsewhere` gives the French path of the languages that
    /// read the page at another one (a product goes by another slug in
    /// each).
    pub fn alternates(
        &self,
        cx: &Cx,
        internal: &str,
        elsewhere: &[(&'static str, String)],
    ) -> Vec<(&'static str, String)> {
        if self.languages.others.is_empty() {
            return Vec::new();
        }
        self.languages
            .all()
            .map(|language| {
                let path = elsewhere
                    .iter()
                    .find(|(theirs, _)| *theirs == language)
                    .map_or(internal, |(_, path)| path.as_str());
                (
                    language,
                    absolute_url(cx, &self.languages.write(language, path)),
                )
            })
            .collect()
    }

    /// The whole address of the French path `internal` in every language
    /// of the shop, the main one first: one for a shop read in one.
    pub fn in_each(&self, cx: &Cx, internal: &str) -> Vec<String> {
        self.languages
            .all()
            .map(|language| absolute_url(cx, &self.languages.write(language, internal)))
            .collect()
    }

    /// The French path each language reads a page at, when it is `path`
    /// of the language's own slug: `canonical`'s query stays on.
    pub fn elsewhere(
        &self,
        canonical: &str,
        path: impl Fn(&str) -> String,
    ) -> Vec<(&'static str, String)> {
        let query = split(canonical).1;
        self.languages
            .all()
            .map(|language| (language, format!("{}{query}", path(language))))
            .collect()
    }
}

/// The catalogue (`l10n/{locale}`) the pages take their sentences from for
/// a visitor reading `language`: its own, as every language of [`SPOKEN`]
/// has one.
fn wording(language: &str) -> Locale {
    language.parse().unwrap_or(locale!("fr"))
}

/// How an amount is written for a visitor reading `language`, by the pages
/// and by the library's totals alike (cart, checkout, orders, shipping,
/// offers), with the catalogue's word for shipping that costs nothing.
/// `cx` already carries the language's catalogue.
fn amounts(cx: &Cx, language: &str) -> MoneyFormat {
    let language = language.to_owned();
    MoneyFormat::new(move |cents, currency| super::write_money(&language, cents, currency))
        .free(t!(checkout_shipping_free).resolve(cx))
}

/// The language a shopper checks out in, which the order keeps: its mails
/// go out in it, and its lines are named in it. In a shop read in several
/// languages it is the one of the address, what the shopper read the
/// goods in; a shop read in one keeps going by the browser's.
pub async fn checkout_language(cx: &Cx) -> Option<String> {
    if languages(cx).await.others.is_empty() {
        timada_storefront::request_language(cx)
    } else {
        Some(speaking(cx).await.language.to_owned())
    }
}

/// The code of the language the pages word this request in.
pub fn spoken(cx: &Cx) -> String {
    try_locale(cx).map_or_else(|| BASE.to_owned(), |locale| locale.to_string())
}

/// Wraps every request; register it on the router with no path.
pub struct Localized;

impl Layer for Localized {
    fn path(&self) -> Option<&Path> {
        None
    }

    fn handle<'a>(&'a self, cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        Box::pin(async move {
            let languages = languages(cx).await;
            let asked = original_uri(cx);
            let visible = asked
                .path_and_query()
                .map_or(asked.path(), |whole| whole.as_str());
            let (language, internal) = languages.read(visible);
            // What the pages word their own sentences in, from here on.
            let worded;
            let cx = if try_locale(cx).is_some() {
                cx
            } else {
                worded = cx.with(wording(language));
                &worded
            };
            // And how they, and the library, write an amount.
            let priced;
            let cx = if try_context::<MoneyFormat>(cx).is_some() {
                cx
            } else {
                priced = cx.with(amounts(cx, language));
                &priced
            };
            if languages.plain() {
                return next.run(cx, body).await;
            }
            // The first time through, the address is still the visitor's.
            if uri(cx) == asked {
                let proper = languages.write(language, &internal);
                if proper != visible && matches!(*method(cx), Method::GET | Method::HEAD) {
                    // One address a page and a language: `/en/boutique`
                    // is read at `/en/shop`.
                    return Err(redirect_permanent(proper).into());
                }
                if internal != visible {
                    let sent = rewrite(internal, body);
                    // What the library's readers word the catalogue in:
                    // the main language needs no saying.
                    let sent = sent
                        .with(wording(language))
                        .with(context::<MoneyFormat>(cx).clone());
                    return Err(if language == languages.main {
                        sent.into()
                    } else {
                        sent.with(ReadIn::new(language)).into()
                    });
                }
            }
            let answer = next.run(cx, body).await;
            if language == BASE && languages.main == BASE {
                return answer;
            }
            let mut response = match answer {
                Ok(response) => response,
                Err(err) => redirection(cx, err)?,
            };
            if let Some(to) = response
                .headers()
                .get(LOCATION)
                .and_then(|to| to.to_str().ok())
                .map(|to| languages.write(language, to))
                && let Ok(to) = HeaderValue::from_str(&to)
            {
                response.headers_mut().insert(LOCATION, to);
            }
            let html = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|kind| kind.to_str().ok())
                .is_some_and(|kind| kind.starts_with("text/html"));
            if !html {
                return Ok(response);
            }
            let (mut parts, body) = response.into_parts();
            let bytes = to_bytes(body, usize::MAX).await?;
            let page = match std::str::from_utf8(&bytes) {
                Ok(page) => with_addresses(page, |to| languages.write(language, to)),
                Err(_) => return Ok(Response::from_parts(parts, Body::from(bytes))),
            };
            parts.headers.remove(CONTENT_LENGTH);
            Ok(Response::from_parts(parts, Body::from(page)))
        })
    }
}

/// The answer a redirect raised as an error stands for; any other error
/// comes back as it was.
fn redirection(cx: &Cx, err: topcoat::Error) -> topcoat::Result<Response> {
    let err = match err.downcast_cloned::<RedirectError>() {
        Ok(redirect) => return redirect.into_response(cx),
        Err(err) => err,
    };
    match err.downcast_cloned::<SeeOther>() {
        Ok(redirect) => redirect.into_response(cx),
        Err(err) => Err(err),
    }
}

/// `page` with every address of the shop it holds turned by `write`: the
/// values of [`ADDRESS_ATTRIBUTES`] that are a path of this site.
fn with_addresses(page: &str, write: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(page.len() + page.len() / 50);
    let mut rest = page;
    while let Some(at) = rest.find("=\"/") {
        let (before, after) = rest.split_at(at);
        let value = &after[2..];
        let end = value.find('"').unwrap_or(value.len());
        // A link that names its language (`hreflang`) goes to another
        // one's address on purpose.
        let tag = before.rfind('<').map_or(before, |open| &before[open..]);
        let named = !tag.contains(" hreflang=")
            && ADDRESS_ATTRIBUTES.iter().any(|name| {
                before
                    .strip_suffix(name)
                    .is_some_and(|ahead| ahead.ends_with(|c: char| c.is_ascii_whitespace()))
            });
        out.push_str(before);
        out.push_str("=\"");
        if named {
            out.push_str(&write(&value[..end]));
        } else {
            out.push_str(&value[..end]);
        }
        rest = &value[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_spoken_has_its_sentences() {
        let worded = crate::app::words::catalogue::t::locales();
        for language in SPOKEN {
            assert!(worded.contains(&wording(language)), "{language}");
        }
        assert_eq!(worded.len(), SPOKEN.len());
    }

    fn french_and_english() -> Languages {
        Languages::of("fr", &["en".to_owned(), "de".to_owned(), "fr".to_owned()])
    }

    #[test]
    fn a_shop_is_read_in_the_languages_the_storefront_has_words_for() {
        assert_eq!(
            french_and_english(),
            Languages {
                main: "fr",
                others: vec!["en"],
            }
        );
        // A main language without words here leaves the shop in French.
        let german = Languages::of("de", &["en".to_owned()]);
        assert_eq!((german.main, german.others), ("fr", vec!["en"]));
        assert!(Languages::default().plain());
        assert!(!french_and_english().plain());
    }

    #[test]
    fn an_address_says_its_language_and_its_french_path() {
        let shop = french_and_english();
        let read = |visible: &str| {
            let (language, internal) = shop.read(visible);
            (language, internal)
        };
        assert_eq!(read("/"), ("fr", "/".to_owned()));
        assert_eq!(
            read("/boutique?page=2"),
            ("fr", "/boutique?page=2".to_owned())
        );
        assert_eq!(read("/en"), ("en", "/".to_owned()));
        assert_eq!(read("/en/"), ("en", "/".to_owned()));
        assert_eq!(
            read("/en/shop?page=2"),
            ("en", "/boutique?page=2".to_owned())
        );
        assert_eq!(
            read("/en/legal-notice"),
            ("en", "/mentions-legales".to_owned())
        );
        assert_eq!(read("/en/p/cat-tree"), ("en", "/p/cat-tree".to_owned()));
        // English names mean nothing in French, nor a language the shop
        // is not read in.
        assert_eq!(read("/shop"), ("fr", "/shop".to_owned()));
        assert_eq!(read("/de/p/x"), ("fr", "/de/p/x".to_owned()));
        // Only the first part of a path is a page's name.
        assert_eq!(read("/en/p/shop"), ("en", "/p/shop".to_owned()));
    }

    #[test]
    fn a_french_path_is_written_in_each_language() {
        let shop = french_and_english();
        assert_eq!(shop.write("fr", "/boutique"), "/boutique");
        assert_eq!(shop.write("en", "/"), "/en");
        assert_eq!(shop.write("en", "/?q=1"), "/en?q=1");
        assert_eq!(
            shop.write("en", "/boutique?page=2#top"),
            "/en/shop?page=2#top"
        );
        assert_eq!(shop.write("en", "/support-client"), "/en/support");
        assert_eq!(shop.write("en", "/p/arbre-a-chat"), "/en/p/arbre-a-chat");
        // Already written, it stays; so does what has one address.
        assert_eq!(shop.write("en", "/en/shop"), "/en/shop");
        assert_eq!(shop.write("en", "/en"), "/en");
        assert_eq!(shop.write("en", "/robots.txt"), "/robots.txt");
        assert_eq!(shop.write("en", "/sitemap.xml"), "/sitemap.xml");
        assert_eq!(
            shop.write("en", "/sitemap-products.xml?part=1"),
            "/en/sitemap-products.xml?part=1"
        );
        assert_eq!(shop.write("en", "/store/status"), "/store/status");
        assert_eq!(shop.write("en", "//cdn.example/x"), "//cdn.example/x");
        assert_eq!(
            shop.write("en", "https://example.com/boutique"),
            "https://example.com/boutique"
        );
        // What a visitor asked for reads back to the same address.
        for visible in ["/en", "/en/shop", "/en/p/x?v=1", "/boutique", "/"] {
            let (language, internal) = shop.read(visible);
            assert_eq!(shop.write(language, &internal), visible);
        }
    }

    #[test]
    fn a_shop_whose_main_language_is_english_names_its_pages_in_english() {
        let shop = Languages::of("en", &["fr".to_owned()]);
        assert_eq!(
            shop.read("/legal-notice"),
            ("en", "/mentions-legales".to_owned())
        );
        assert_eq!(
            shop.read("/fr/mentions-legales"),
            ("fr", "/mentions-legales".to_owned())
        );
        assert_eq!(shop.write("en", "/mentions-legales"), "/legal-notice");
        assert_eq!(
            shop.write("fr", "/mentions-legales"),
            "/fr/mentions-legales"
        );
        assert_eq!(shop.write("fr", "/"), "/fr");
        // The French name is not the page's address in English.
        let (language, internal) = shop.read("/mentions-legales");
        assert_eq!(shop.write(language, &internal), "/legal-notice");
    }

    #[test]
    fn the_addresses_of_a_page_follow_the_language() {
        let shop = french_and_english();
        let page = r#"<a class="x" href="/boutique?page=2">A</a><form method="post" action="/cart/add"><button formaction="/cart/remove"></button></form><img src="/media/a.webp"><a href="https://else.example/boutique">B</a><div data-href="/boutique" data-search="/search"></div><a href="/en/shop">C</a><input value="/boutique"><a hreflang="fr" href="/boutique">Français</a>"#;
        assert_eq!(
            with_addresses(page, |to| shop.write("en", to)),
            r#"<a class="x" href="/en/shop?page=2">A</a><form method="post" action="/en/cart/add"><button formaction="/en/cart/remove"></button></form><img src="/media/a.webp"><a href="https://else.example/boutique">B</a><div data-href="/boutique" data-search="/en/search"></div><a href="/en/shop">C</a><input value="/boutique"><a hreflang="fr" href="/boutique">Français</a>"#
        );
        assert_eq!(
            with_addresses("no address", |to| to.to_owned()),
            "no address"
        );
    }
}
