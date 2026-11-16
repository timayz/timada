//! What a visitor allowed: the answer to the shop's cookie banner, and what
//! Google's Consent Mode v2 is told of it.
//!
//! The library keeps no cookie: the host stores [`Consent::cookie_value`]
//! under [`CONSENT_COOKIE`] for [`CONSENT_MAX_AGE_DAYS`] and reads it back
//! with [`Consent::parse`]. Every tag then asks the same answer the same
//! questions: [`ConsentMode::loads`] before it writes a third party's script
//! at all, [`Consent::analytics`] and [`Consent::ads`] before it keeps
//! anything of its own, and [`consent_script`] goes first in the `<head>` so
//! Google's tags start from the four signals.
//!
//! No answer is a refusal: nothing is granted until the visitor says so.

use std::fmt::Write as _;

/// The first-party cookie a host keeps the answer in.
pub const CONSENT_COOKIE: &str = "consent";

/// How long the answer is kept before the banner asks again: six months, the
/// time the CNIL recommends.
pub const CONSENT_MAX_AGE_DAYS: i64 = 180;

/// What the visitor allowed, by purpose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Consent {
    /// Measuring visits: `analytics_storage`.
    pub analytics: bool,
    /// Advertising, its measure and its personalisation: `ad_storage`,
    /// `ad_user_data` and `ad_personalization`.
    pub ads: bool,
}

impl Consent {
    /// Everything accepted.
    pub const ALL: Consent = Consent {
        analytics: true,
        ads: true,
    };

    /// Everything refused.
    pub const NONE: Consent = Consent {
        analytics: false,
        ads: false,
    };

    /// The answer a cookie holds, or none for a value this never wrote: the
    /// banner then asks again.
    pub fn parse(value: &str) -> Option<Consent> {
        let mut consent = Consent::NONE;
        if value == "none" {
            return Some(consent);
        }
        for purpose in value.split('.') {
            match purpose {
                "analytics" if !consent.analytics => consent.analytics = true,
                "ads" if !consent.ads => consent.ads = true,
                _ => return None,
            }
        }
        Some(consent)
    }

    /// What the cookie holds: the purposes allowed (`analytics.ads`), or
    /// `none`. Readable on purpose, by the visitor and by a script of the
    /// shop's own.
    pub fn cookie_value(&self) -> &'static str {
        match (self.analytics, self.ads) {
            (true, true) => "analytics.ads",
            (true, false) => "analytics",
            (false, true) => "ads",
            (false, false) => "none",
        }
    }

    /// Consent Mode v2's four signals, each granted or not.
    pub fn signals(&self) -> [(&'static str, bool); 4] {
        [
            ("ad_storage", self.ads),
            ("ad_user_data", self.ads),
            ("ad_personalization", self.ads),
            ("analytics_storage", self.analytics),
        ]
    }
}

/// What a shop's tags do before a visitor answers, and after a refusal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConsentMode {
    /// Nothing is loaded and nothing is sent until the visitor allows a
    /// purpose.
    #[default]
    Basic,
    /// The tags load with every signal denied and send pings without
    /// cookies, which Google models conversions from.
    Advanced,
}

impl ConsentMode {
    /// Whether a third party's script may be written into the page: always
    /// in advanced mode (the signals hold it back), and in basic mode once
    /// the visitor allowed a purpose.
    pub fn loads(self, answer: Option<Consent>) -> bool {
        match self {
            ConsentMode::Advanced => true,
            ConsentMode::Basic => answer.is_some_and(|consent| consent.analytics || consent.ads),
        }
    }
}

fn signal(granted: bool) -> &'static str {
    if granted { "granted" } else { "denied" }
}

/// The script that goes before any tag in the `<head>`: it sets the four
/// signals to denied, then to the visitor's answer when there is one, as
/// Consent Mode v2 wants on every page.
///
/// The default is denied for every visitor, not only in the EEA: the page
/// does not know where its reader is, and nothing a visitor did not allow
/// is the rule the shop's own country sets anyway.
///
/// In advanced mode the tags run before an answer, so two more settings
/// follow: click ids travel in the links instead of a cookie
/// (`url_passthrough`), and the ad requests leave their identifiers out
/// while `ad_storage` is denied (`ads_data_redaction`).
pub fn consent_script(mode: ConsentMode, answer: Option<Consent>) -> String {
    let mut script = String::from(
        "window.dataLayer=window.dataLayer||[];function gtag(){dataLayer.push(arguments);}",
    );
    let object = |consent: Consent| {
        consent
            .signals()
            .iter()
            .map(|(name, granted)| format!("{name}:'{}'", signal(*granted)))
            .collect::<Vec<_>>()
            .join(",")
    };
    let _ = write!(
        script,
        "gtag('consent','default',{{{}}});",
        object(Consent::NONE)
    );
    if mode == ConsentMode::Advanced {
        let redact = !answer.is_some_and(|consent| consent.ads);
        let _ = write!(
            script,
            "gtag('set','url_passthrough',true);gtag('set','ads_data_redaction',{redact});"
        );
    }
    if let Some(consent) = answer {
        let _ = write!(script, "gtag('consent','update',{{{}}});", object(consent));
    }
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_survives_its_cookie() {
        for consent in [
            Consent::ALL,
            Consent::NONE,
            Consent {
                analytics: true,
                ads: false,
            },
            Consent {
                analytics: false,
                ads: true,
            },
        ] {
            assert_eq!(Consent::parse(consent.cookie_value()), Some(consent));
        }
    }

    #[test]
    fn a_value_never_written_is_no_answer() {
        for value in ["", "all", "analytics.analytics", "analytics.none", "ads.x"] {
            assert_eq!(Consent::parse(value), None, "{value:?}");
        }
    }

    #[test]
    fn the_ad_signals_follow_one_purpose_and_the_measure_the_other() {
        let consent = Consent {
            analytics: true,
            ads: false,
        };
        assert_eq!(
            consent.signals(),
            [
                ("ad_storage", false),
                ("ad_user_data", false),
                ("ad_personalization", false),
                ("analytics_storage", true),
            ]
        );
    }

    #[test]
    fn basic_mode_loads_nothing_before_a_purpose_is_allowed() {
        assert!(!ConsentMode::Basic.loads(None));
        assert!(!ConsentMode::Basic.loads(Some(Consent::NONE)));
        assert!(ConsentMode::Basic.loads(Some(Consent {
            analytics: true,
            ads: false,
        })));
        assert!(ConsentMode::Advanced.loads(None));
        assert!(ConsentMode::Advanced.loads(Some(Consent::NONE)));
    }

    #[test]
    fn the_script_denies_first_then_tells_the_answer() {
        let denied = "gtag('consent','default',{ad_storage:'denied',ad_user_data:'denied',ad_personalization:'denied',analytics_storage:'denied'});";
        let unanswered = consent_script(ConsentMode::Basic, None);
        assert!(unanswered.ends_with(denied), "{unanswered}");
        assert!(!unanswered.contains("update"));

        let answered = consent_script(
            ConsentMode::Basic,
            Some(Consent {
                analytics: true,
                ads: false,
            }),
        );
        let update = "gtag('consent','update',{ad_storage:'denied',ad_user_data:'denied',ad_personalization:'denied',analytics_storage:'granted'});";
        assert!(answered.ends_with(update), "{answered}");
        assert!(answered.find(denied) < answered.find(update));
    }

    #[test]
    fn advanced_mode_redacts_until_ads_are_allowed() {
        let refused = consent_script(ConsentMode::Advanced, Some(Consent::NONE));
        assert!(refused.contains("gtag('set','url_passthrough',true);"));
        assert!(refused.contains("gtag('set','ads_data_redaction',true);"));
        let allowed = consent_script(ConsentMode::Advanced, Some(Consent::ALL));
        assert!(allowed.contains("gtag('set','ads_data_redaction',false);"));
        assert!(!consent_script(ConsentMode::Basic, None).contains("url_passthrough"));
    }
}
