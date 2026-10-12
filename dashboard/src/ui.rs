//! Shared page furniture.

use topcoat::{
    Result,
    asset::asset,
    tailwind,
    view::{Child, View, component, view},
};

/// The HTML document every page renders into.
#[component]
pub async fn document(title: &str, #[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>(title)</title>
                <link rel="stylesheet" href=(tailwind::stylesheet!())>
                <script type="module" src=(asset!("./app/auth/auth.js"))></script>
                topcoat::dev::script()
            </head>
            <body class="min-h-screen bg-neutral-50 text-neutral-900">
                <main class="mx-auto max-w-3xl p-8">
                    (child)
                </main>
            </body>
        </html>
    })
}

/// A notice above a page's content: `error` codes from a redirect, or a
/// confirmation message.
#[component]
pub async fn notice(
    #[default] error: Option<&str>,
    #[default] message: Option<&str>,
) -> Result<impl View> {
    Ok(view! {
        if let Some(error) = error {
            <p role="alert" class="mb-4 rounded border border-red-300 bg-red-50 p-3 text-sm text-red-800">
                (describe_error(error))
            </p>
        }
        if let Some(message) = message {
            <p role="status" class="mb-4 rounded border border-green-300 bg-green-50 p-3 text-sm text-green-800">
                (message)
            </p>
        }
        <p role="alert" data-auth-error="" class="mb-4 hidden rounded border border-red-300 bg-red-50 p-3 text-sm text-red-800"></p>
    })
}

/// Error codes travel in the query string; the wording lives here.
pub fn describe_error(code: &str) -> &'static str {
    match code {
        "not_registered" => "This account is not registered here yet.",
        "already_registered" => "This account is already registered. Sign in instead.",
        "already_linked" => "This account is already linked to another user.",
        "session_changed" => {
            "Your session changed while this step was in progress. Please try again."
        }
        "signed_in" => "You are already signed in. Sign out first to create another account.",
        "sign_in_required" => "Please sign in first.",
        "last_method" => "You cannot remove your only way to sign in.",
        "too_many_passkeys" => "This account has reached its passkey limit.",
        "expired" => "This sign-in attempt expired. Please start again.",
        "provider" => "The sign-in provider returned an error.",
        "unknown_provider" => "Unknown sign-in provider.",
        "not_allowed" => "You are not allowed to do that.",
        "invalid_credentials" => "The e-mail address or the password is wrong.",
        "invalid_code" => "That code is not right. Check the latest e-mail and try again.",
        "code_expired" => "That code has expired. Please start again.",
        "too_many_attempts" => "Too many wrong codes. Please start again to get a new one.",
        "code_recently_sent" => {
            "A code was sent a moment ago. Wait a minute before asking for another."
        }
        "invalid_email" => "Please enter a valid e-mail address.",
        "weak_password" => "The password must be between 8 and 128 characters.",
        "password_mismatch" => "The two passwords do not match.",
        "wrong_password" => "The current password is wrong.",
        "email_set" => "This account already has an e-mail address.",
        "mail_failed" => "The e-mail could not be sent. Please try again in a moment.",
        _ => "Something went wrong. Please try again.",
    }
}

/// Unix seconds as an ISO-8601 UTC timestamp (no calendar crate needed).
pub fn format_time(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// A row of a sessions list: when it started, from where, on what.
#[component]
pub async fn session_row(
    started_at: u64,
    ip: Option<&str>,
    user_agent: Option<&str>,
    #[default] current: bool,
) -> Result<impl View> {
    Ok(view! {
        <li class="py-2 text-sm">
            <time datetime=(started_at)>(format_time(started_at))</time>
            if let Some(ip) = ip { " · " (ip) }
            if current { " · " <strong>"this browser"</strong> }
            if let Some(ua) = user_agent { <span class="block truncate text-neutral-500">(ua)</span> }
        </li>
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_unix_time() {
        assert_eq!(super::format_time(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(super::format_time(1_700_000_000), "2023-11-14 22:13:20 UTC");
    }
}
