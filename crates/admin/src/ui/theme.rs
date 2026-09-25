//! The colour scheme the operator works in.
//!
//! Three states, and no JavaScript to hold them: the operating system's
//! preference, or light, or dark. A choice is kept in a cookie, read on every
//! request, and turned by the shell into the class on `<html>` and the
//! document's declared `color-scheme`. Choosing "système" forgets the cookie
//! rather than recording a third value, so an operator who has never chosen is
//! indistinguishable from one who chose to follow their system — which is the
//! same thing.

use topcoat::{
    context::Cx,
    cookie::{Cookies, cookie, cookies, time::Duration},
    router::request,
};

use crate::ui::mount_path;

/// Named for the admin: the shop out front keeps its own skin.
const COOKIE: &str = "timada_admin_theme";

/// A year. The choice is a preference, not a session.
const REMEMBERED: Duration = Duration::days(365);

/// Which colour scheme the pages are rendered in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Scheme {
    /// Whatever the operating system asks for. The default, and what choosing
    /// "Système" returns to.
    #[default]
    System,
    Light,
    Dark,
}

impl Scheme {
    /// Every scheme, in the order the switch offers them.
    pub const ALL: [Self; 3] = [Self::Light, Self::Dark, Self::System];

    /// The scheme a stored or submitted value names.
    #[must_use]
    pub fn of_value(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    /// How the scheme is written in the cookie and in the switch's form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// The class the shell puts on `<html>`.
    ///
    /// `System` puts none: `styles.css` reads the absence of both `light` and
    /// `dark` as "ask the operating system".
    #[must_use]
    pub const fn html_class(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
        }
    }

    /// What the document declares to the browser, so that native widgets — a
    /// `<select>`'s popup, a scrollbar, a date picker — are drawn in the same
    /// scheme as the page, and so the first paint is the right colour before
    /// any stylesheet has arrived.
    #[must_use]
    pub const fn color_scheme(self) -> &'static str {
        match self {
            Self::System => "light dark",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// How the switch names it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "Système",
            Self::Light => "Clair",
            Self::Dark => "Sombre",
        }
    }
}

/// What the operator chose, or [`Scheme::System`] when they have not chosen.
#[must_use]
pub fn chosen(cx: &Cx) -> Scheme {
    cookies(cx)
        .get(COOKIE)
        .and_then(|cookie| Scheme::of_value(cookie.value()))
        .unwrap_or_default()
}

/// The page the switch was pressed on, to return to: its path and query, never
/// a host.
#[must_use]
pub fn here(cx: &Cx) -> String {
    let uri = request::uri(cx);
    match uri.query() {
        Some(query) => format!("{}?{}", uri.path(), query),
        None => uri.path().to_owned(),
    }
}

/// Records a choice. [`Scheme::System`] forgets it instead.
///
/// The cookie is scoped to the admin's own mount, because a colour scheme for
/// the back office says nothing about the shop. It is deliberately not
/// `__Host-`-prefixed: that prefix requires `Path=/`, and this is a
/// preference, not a credential.
pub fn remember(cx: &Cx, scheme: Scheme) {
    let path = mount_path(cx);
    match scheme {
        Scheme::System => cookies(cx).remove(cookie! { COOKIE = ""; Path = path }),
        _ => cookies(cx).add(cookie! {
            COOKIE = scheme.as_str();
            Path = path;
            HttpOnly;
            SameSite = Lax;
            MaxAge = REMEMBERED
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::Scheme;

    #[test]
    fn a_scheme_round_trips_through_the_cookie_value() {
        for scheme in Scheme::ALL {
            assert_eq!(Scheme::of_value(scheme.as_str()), Some(scheme));
        }
    }

    #[test]
    fn an_unknown_value_is_no_choice_at_all() {
        for value in ["", "DARK", "auto", "sombre"] {
            assert_eq!(Scheme::of_value(value), None);
        }
    }

    #[test]
    fn only_a_choice_puts_a_class_on_the_document() {
        assert_eq!(Scheme::System.html_class(), None);
        assert_eq!(Scheme::Light.html_class(), Some("light"));
        assert_eq!(Scheme::Dark.html_class(), Some("dark"));
    }

    #[test]
    fn the_document_declares_what_it_was_asked_for() {
        assert_eq!(Scheme::System.color_scheme(), "light dark");
        assert_eq!(Scheme::Light.color_scheme(), "light");
        assert_eq!(Scheme::Dark.color_scheme(), "dark");
    }
}
