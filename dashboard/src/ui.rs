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
        "invalid_slug" => "A slug is 2 to 32 lowercase letters, digits and dashes.",
        "slug_taken" => "This slug is already taken. Choose another one.",
        "invalid_name" => "The name must be between 1 and 80 characters.",
        "tenant_not_found" => "This tenant does not exist.",
        "tenant_suspended" => "This tenant is suspended.",
        "not_suspended" => "This tenant is not suspended.",
        "invalid_role" => "A role name is 2 to 32 lowercase letters, digits and dashes.",
        "reserved_role" => "This role name is reserved.",
        "invalid_role_label" => "A role label is 1 to 40 characters.",
        "role_exists" => "This role already exists.",
        "unknown_role" => "This role does not exist.",
        "role_in_use" => "Move its members to another role before removing it.",
        "invalid_product_name" => "A product name is 1 to 120 characters.",
        "invalid_product_slug" => "A product slug is 2 to 64 lowercase letters, digits and dashes.",
        "product_slug_taken" => "Another product already uses this slug.",
        "invalid_description" => "A description is at most 4000 characters.",
        "invalid_amount" => "Enter the price as a number with at most two decimals, like 12.50.",
        "invalid_currency" => "Choose one of the listed currencies.",
        "product_not_found" => "This product does not exist.",
        "product_archived" => "This product is archived and cannot change any more.",
        "product_not_priced" => "Set a price before publishing.",
        "invalid_category_name" => "A category name is 1 to 80 characters.",
        "invalid_category_slug" => {
            "A category slug is 2 to 64 lowercase letters, digits and dashes."
        }
        "invalid_category_description" => "A category description is at most 2000 characters.",
        "category_slug_taken" => "Another category already uses this slug.",
        "category_not_found" => "This category does not exist.",
        "category_archived" => "This category is archived and cannot change any more.",
        "category_cycle" => "A category cannot be moved under one of its own sub-categories.",
        "category_too_deep" => "Categories nest at most three levels deep.",
        "category_has_children" => "Archive or move its sub-categories first.",
        "too_many_categories" => "A product belongs to at most ten categories.",
        "too_many_images" => "A product carries at most twenty pictures.",
        "unsupported_image" => "Upload a JPEG, PNG, WebP or GIF picture.",
        "image_too_large" => "A picture is at most 8 MiB.",
        "no_image" => "Choose a picture to upload.",
        "invalid_alt" => "The text describing a picture is at most 200 characters.",
        "image_not_found" => "This picture is no longer on the product.",
        "invalid_image_order" => "The gallery changed while you were reordering it. Try again.",
        "too_many_options" => "A product varies in at most three ways.",
        "invalid_option_name" => "An option's name is 1 to 40 characters.",
        "option_exists" => "This option is already there.",
        "invalid_option_values" => "An option takes 1 to 20 values, each at most 40 characters.",
        "option_not_found" => "This option is no longer on the product.",
        "option_in_use" => {
            "The options cannot change while the product has variants. Remove them first."
        }
        "option_value_in_use" => "A variant still uses one of the values you dropped.",
        "too_many_variants" => "A product sells at most a hundred variants.",
        "invalid_selection" => "Choose a value for every option.",
        "variant_exists" => "This combination is already sold as another variant.",
        "variant_not_found" => "This variant is no longer on the product.",
        "currency_mismatch" => "Every price of a product is in the same currency.",
        "invalid_sku" => "A code is 1 to 64 of letters, digits, dot, dash and underscore.",
        "sku_taken" => "Another variant already uses this code.",
        "invalid_attribute" => {
            "A fact needs a name of at most 40 characters and a value of at most 400."
        }
        "too_many_attributes" => "A product's table holds at most thirty facts.",
        "invalid_permission" => "One of the permissions is not known.",
        "already_member" => "This user is already a member.",
        "not_member" => "This user is not a member.",
        "last_owner" => "A tenant keeps at least one owner.",
        "invitation_invalid" => "This invitation is no longer valid.",
        "invalid_api_key_label" => "A key label is 1 to 40 characters.",
        "invalid_api_key_scopes" => "Choose at least one scope for the key.",
        "api_key_invalid" => "This API key does not exist or is already revoked.",
        "order_not_found" => "This order does not exist.",
        "order_cancelled" => "A cancelled order cannot be fulfilled.",
        "invalid_publishable_key" => "A publishable key starts with pk_.",
        "invalid_secret_key" => "A secret key starts with sk_ (or rk_ for a restricted key).",
        "invalid_webhook_secret" => "A webhook signing secret starts with whsec_.",
        "stripe_refused_key" => "Stripe did not accept this secret key.",
        "payments_disabled" => "Payments are not enabled on this platform.",
        "payments_unavailable" => "This tenant has no payments set up.",
        "refund_failed" => {
            "Stripe could not refund the payment: the order is not cancelled. Try again."
        }
        "not_cancellable" => "A fulfilled order cannot be cancelled.",
        "invalid_tracking" => "A tracking number is at most 200 characters.",
        "invalid_reason" => "Say why the order is cancelled, in at most 500 characters.",
        "invalid_siret" => "A SIRET is 14 digits; check it against your registration.",
        "invalid_vat_number" => {
            "A French VAT number is FR, a two-digit key and the SIREN (the SIRET's first nine digits)."
        }
        "invalid_tax_category" => "Choose one of the tax categories.",
        "oss_without_vat_number" => "The One-Stop Shop needs a VAT number.",
        "invalid_legal_identity" => {
            "Fill in the legal name, form, address and e-mail, each within its length."
        }
        "invalid_share_capital" => "Enter the share capital in euros, like 1000 or 1000.50.",
        "invoicing_not_configured" => {
            "Shoppers cannot pay until the legal identity on the Invoicing page is filled in."
        }
        _ => "Something went wrong. Please try again.",
    }
}

/// What a permission of [`timada_tenancy::PERMISSIONS`] lets a role do.
pub fn describe_permission(permission: &str) -> &'static str {
    match permission {
        "tenant:manage" => "Rename the tenant",
        "members:read" => "See members and pending invitations",
        "members:manage" => {
            "Invite, change roles and remove members (owners excepted; includes seeing them)"
        }
        "roles:manage" => "Define, edit and remove roles",
        "api-keys:manage" => "Create and revoke the store API keys",
        "products:read" => "See the products and categories",
        "products:manage" => "Create, edit, publish and archive products; organise the categories",
        "orders:read" => "See the orders and the carts shoppers left",
        "orders:manage" => {
            "Mark orders fulfilled, and cancel and refund them (includes seeing them)"
        }
        "payments:manage" => "Set up how shoppers pay (the payment provider's keys)",
        _ => "Unknown permission",
    }
}

pub fn describe_api_key_scope(scope: &str) -> &'static str {
    match scope {
        "store:read" => "Read the events and run subscriptions over them",
        "store:write" => "Write events (includes reading)",
        "store:checkout" => "Fill carts and place orders, nothing else",
        _ => "Unknown scope",
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
