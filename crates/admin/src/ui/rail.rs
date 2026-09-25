//! How wide the navigation rail is.
//!
//! Two states, kept the way the colour scheme is kept: a cookie, a POST, and a
//! redirect back. Unlike the drawer, this one cannot be pure CSS — folding the
//! rail means the labels stop being shown, and from `md` up that is a decision
//! the server makes about what to render classes for, not a state a selector
//! can reach.
//!
//! The labels are still written out when the rail is folded, and hidden with a
//! class from `md` up. That keeps one copy of the navigation for the drawer
//! below `md`, which is full-screen and wants its labels, and it keeps every
//! entry's accessible name intact without a single `aria-label`.

use topcoat::{
    context::Cx,
    cookie::{Cookies, cookie, cookies, time::Duration},
};

use crate::ui::mount_path;

const COOKIE: &str = "timada_admin_rail";

/// A year, like the colour scheme: a preference, not a session.
const REMEMBERED: Duration = Duration::days(365);

/// Whether the rail shows its labels or only its icons.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Rail {
    /// Labels and headings, 16rem wide. The default.
    #[default]
    Open,
    /// Icons only, 4rem wide.
    Folded,
}

impl Rail {
    /// How it is written in the cookie and in the toggle's form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Folded => "folded",
        }
    }

    /// The state a stored or submitted value names.
    #[must_use]
    pub fn of_value(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "folded" => Some(Self::Folded),
            _ => None,
        }
    }

    /// The other one, which is what the toggle submits.
    #[must_use]
    pub const fn flipped(self) -> Self {
        match self {
            Self::Open => Self::Folded,
            Self::Folded => Self::Open,
        }
    }

    /// What the toggle is called, which is what it will do rather than where it
    /// is: a button named for its current state tells the operator nothing.
    #[must_use]
    pub const fn action_label(self) -> &'static str {
        match self {
            Self::Open => "Replier le menu",
            Self::Folded => "Déplier le menu",
        }
    }

    #[must_use]
    pub const fn is_folded(self) -> bool {
        matches!(self, Self::Folded)
    }
}

/// How the operator left the rail, open unless they folded it.
#[must_use]
pub fn chosen(cx: &Cx) -> Rail {
    cookies(cx)
        .get(COOKIE)
        .and_then(|cookie| Rail::of_value(cookie.value()))
        .unwrap_or_default()
}

/// Records the state. [`Rail::Open`] forgets the cookie, being the default.
pub fn remember(cx: &Cx, rail: Rail) {
    let path = mount_path(cx);
    match rail {
        Rail::Open => cookies(cx).remove(cookie! { COOKIE = ""; Path = path }),
        Rail::Folded => cookies(cx).add(cookie! {
            COOKIE = rail.as_str();
            Path = path;
            HttpOnly;
            SameSite = Lax;
            MaxAge = REMEMBERED
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::Rail;

    #[test]
    fn a_state_round_trips_through_the_cookie_value() {
        for rail in [Rail::Open, Rail::Folded] {
            assert_eq!(Rail::of_value(rail.as_str()), Some(rail));
        }
    }

    #[test]
    fn an_unknown_value_leaves_the_rail_open() {
        assert_eq!(Rail::of_value("wide"), None);
        assert_eq!(Rail::default(), Rail::Open);
    }

    #[test]
    fn the_toggle_submits_the_other_state_and_says_what_it_will_do() {
        assert_eq!(Rail::Open.flipped(), Rail::Folded);
        assert_eq!(Rail::Open.action_label(), "Replier le menu");
        assert_eq!(Rail::Folded.flipped(), Rail::Open);
        assert_eq!(Rail::Folded.action_label(), "Déplier le menu");
    }
}
