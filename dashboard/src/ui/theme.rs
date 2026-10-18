//! The two preferences a browser keeps for the back office, as plain
//! cookies: the colour scheme and whether the rail is folded. Neither is
//! sensitive, so they are not `__Host-` cookies and the rail one is written
//! by the browser itself.

use topcoat::{
    context::Cx,
    cookie::{Cookie, Cookies, SameSite, cookies, time::Duration},
    icon::IconData,
};

use super::icons;

pub const THEME_COOKIE: &str = "timada_theme";
pub const RAIL_COOKIE: &str = "timada_rail";

/// The colour scheme a visitor asked for; absent, the page follows the
/// operating system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Light,
    Dark,
    System,
}

impl Scheme {
    pub const ALL: [Scheme; 3] = [Scheme::Light, Scheme::Dark, Scheme::System];

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::System => "System",
        }
    }

    pub fn icon(self) -> IconData {
        match self {
            Self::Light => icons::SUN,
            Self::Dark => icons::MOON,
            Self::System => icons::MONITOR,
        }
    }

    /// The class on `<html>`: none for `System`, which leaves the choice to
    /// the `prefers-color-scheme` block of the stylesheet.
    pub fn html_class(self) -> Option<&'static str> {
        match self {
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
            Self::System => None,
        }
    }
}

pub fn scheme(cx: &Cx) -> Scheme {
    cookies(cx)
        .get(THEME_COOKIE)
        .and_then(|cookie| Scheme::parse(cookie.value()))
        .unwrap_or(Scheme::System)
}

/// Whether the rail shows its labels; folded only when the browser said so.
pub fn rail_open(cx: &Cx) -> bool {
    cookies(cx)
        .get(RAIL_COOKIE)
        .is_none_or(|cookie| cookie.value() != "closed")
}

/// A preference cookie: a year, the whole site, sent on navigation.
pub fn preference(name: &'static str, value: &str) -> Cookie<'static> {
    Cookie::build((name, value.to_owned()))
        .path("/")
        .same_site(SameSite::Lax)
        .max_age(Duration::days(365))
        .build()
}

/// Where a preference form sends the visitor back: the page it was on, as
/// long as that is a path of this site.
pub fn return_path(next: Option<&str>) -> &str {
    match next {
        Some(path) if path.starts_with('/') && !path.starts_with("//") => path,
        _ => "/",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_path_stays_on_site() {
        assert_eq!(
            return_path(Some("/t/shop/orders?page=2")),
            "/t/shop/orders?page=2"
        );
        assert_eq!(return_path(Some("//evil.example")), "/");
        assert_eq!(return_path(Some("https://evil.example/")), "/");
        assert_eq!(return_path(None), "/");
    }
}
