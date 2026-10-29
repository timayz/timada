//! Shared page furniture: the document shell ([`chrome`]), the page-level
//! helpers ([`page`]), the browser preferences ([`theme`]), the icons
//! ([`icons`]), the wording of errors and permissions, and the dates.

pub mod chrome;
pub mod icons;
pub mod page;
pub mod theme;

pub use chrome::{Shell, auth_document, document};
pub use page::*;

use topcoat::{
    Result,
    icon::icon,
    view::{View, attributes, component, view},
};

use crate::components::alert::{AlertVariant, alert, alert_description};

/// A notice above a page's content: `error` codes from a redirect, or a
/// confirmation message.
#[component]
pub async fn notice(
    #[default] error: Option<&str>,
    #[default] message: Option<&str>,
) -> Result<impl View> {
    Ok(view! {
        if let Some(error) = error {
            alert(
                variant: AlertVariant::Destructive,
                attrs: attributes! { role="alert" class="mb-4" },
                icon(data: icons::ERROR)
                alert_description((describe_error(error)))
            )
        }
        if let Some(message) = message {
            alert(
                variant: AlertVariant::Success,
                attrs: attributes! { role="status" class="mb-4" },
                icon(data: icons::CHECK)
                alert_description((message))
            )
        }
        // `auth.js` fills this one in and takes `hidden` off it.
        <p role="alert" data-auth-error="" class="mb-4 hidden rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive-text"></p>
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
        "invalid_time_zone" => "Pick a time zone from the list.",
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
        "invalid_discount_code" => "A code is 3 to 32 letters, digits, dashes and underscores.",
        "discount_code_taken" => "Another live discount already uses this code.",
        "invalid_percent" => "Enter a percentage above 0 and at most 100, like 10 or 12.5.",
        "invalid_discount_amount" => {
            "Enter the amount off as a number with at most two decimals, like 5.00."
        }
        "invalid_value" => "Give either a percentage or an amount off.",
        "invalid_window" => "The last day must not come before the first day.",
        "invalid_date" => "Enter the days as dates, like 2026-12-01.",
        "invalid_max_uses" => "A limited code serves 1 to 10000 orders.",
        "discount_not_found" => "This discount does not exist.",
        "discount_archived" => "This discount is archived.",
        "offer_not_found" => "This offer does not exist any more.",
        "offer_product_not_found" => "This product does not exist any more.",
        "offer_variant" => "Choose one of the product's variants for the offer.",
        "invalid_offer_price" => {
            "Enter the offer price as a number above 0 and below the product's price, like 12.00."
        }
        "offer_currency" => "The offer is priced in the product's currency.",
        "invalid_offer_title" => "An offer's title is at most 80 characters.",
        "invalid_min_subtotal" => {
            "Enter the minimum as a positive amount with at most two decimals, like 50.00."
        }
        "too_many_offers" => "A shop has at most 20 offers: archive one first.",
        "invalid_shipping_name" => "A shipping method's name is 1 to 80 characters.",
        "invalid_shipping_countries" => {
            "List the countries as two-letter codes separated by commas, like FR, BE."
        }
        "too_many_shipping_methods" => "A shop offers at most twenty shipping methods.",
        "shipping_method_not_found" => "This shipping method does not exist.",
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
        "too_many_invitations" => "Too many invitations have been sent recently. Try again later.",
        "invalid_api_key_label" => "A key label is 1 to 40 characters.",
        "invalid_api_key_scopes" => "Choose at least one scope for the key.",
        "api_key_invalid" => "This API key does not exist or is already revoked.",
        "order_not_found" => "This order does not exist.",
        "newsletter_not_found" => "This subscriber is no longer on the list.",
        "campaign_not_found" => "This campaign does not exist.",
        "page_invalid_title" => "A page's title is 1 to 120 characters.",
        "page_invalid_body" => "A page is at most 100,000 characters.",
        "page_invalid_seo" => {
            "The search engines' title is at most 70 characters, the description at most 160."
        }
        "page_invalid_slug" => {
            "An address is 2 to 64 lowercase letters, digits or dashes, with no dash at either end."
        }
        "page_slug_taken" => "Another page has this address.",
        "page_too_many_images" => "A page carries at most 50 pictures.",
        "page_not_found" => "This page does not exist.",
        "page_image_not_found" => "This picture is no longer on the page.",
        "campaign_sent" => "This campaign was sent already: it can no longer be changed.",
        "scenario_invalid_name" => "A scenario's name is 1 to 80 characters.",
        "scenario_not_found" => "There is no such scenario.",
        "scenario_changed" => {
            "Someone saved this scenario since you opened it: reload it to see their version."
        }
        "scenario_archived" => "This scenario was deleted.",
        "scenario_not_published" => "This scenario was never published.",
        "scenario_invalid" => "The scenario is not ready to publish: see what needs setting up.",
        "scenario_too_large" => "The scenario is too large to save.",
        "run_not_found" => "There is no such run.",
        "automation_invalid_email" => "That does not look like an e-mail address.",
        "automation_webhook_permission" => {
            "A scenario that calls webhooks is published by members who may manage the API keys: it hands the shop's events to another system."
        }
        "automation_webhook_address" => "A webhook's address is a web address (https://…).",
        "automation_discount_permission" => {
            "A scenario that makes discount codes is published by members who may manage the discounts."
        }
        "automation_invalid_link" => {
            "This link does not stop any mail: it may have been cut short."
        }
        "automation_mail_permission" => {
            "A scenario that mails customers is published by members who may manage the newsletter."
        }
        "automation_mail_invalid" => {
            "A mail of the scenario does not read, or does not link to the page that stops it: open it to see why."
        }
        "automation_secret_required" => {
            "Mails to customers need the operator's secret (secrets.key), which signs their opt-out links."
        }
        "automation_storefront_required" => {
            "Say where the shop's storefront lives (on the API keys page) first: every automated mail links to its opt-out page."
        }
        "run_changed" => "The run moved on meanwhile: look again before cancelling it.",
        "campaign_invalid_subject" => "A subject is 1 to 200 characters.",
        "campaign_invalid_body" => "The body is empty or too long.",
        "order_cancelled" => "A cancelled order cannot be fulfilled.",
        "invalid_publishable_key" => "A publishable key starts with pk_.",
        "invalid_secret_key" => "A secret key starts with sk_ (or rk_ for a restricted key).",
        "invalid_webhook_secret" => "A webhook signing secret starts with whsec_.",
        "stripe_refused_key" => "Stripe did not accept this secret key.",
        "payments_disabled" => "Payments are not enabled on this platform.",
        "mail_servers_disabled" => "Shop mail servers are not enabled on this platform.",
        "invalid_mail_server" => {
            "Give the server's host name, a port from 1 to 65535 and how the connection is secured, and a name of at most 80 characters."
        }
        "too_many_mail_servers" => "A shop keeps at most ten mail servers: delete one first.",
        "mail_server_not_found" => "This mail server is no longer set up.",
        "mail_server_not_public" => {
            "This server is not reachable from the internet: give your provider's public server name."
        }
        "mail_server_failed" => {
            "The test mail did not go out: check the server, port, security, user name and password. Nothing was saved."
        }
        "mail_server_silent" => {
            "The server took the connection but never answered: check that the security matches the port (STARTTLS usually on 587, TLS on 465). Nothing was saved."
        }
        "unknown_mail" => "This shop sends no such mail.",
        "invalid_mail_languages" => "Pick the languages from the list.",
        "unknown_language" => "This shop does not send mails in that language.",
        "invalid_mail_branding" => {
            "An accent colour is written #rrggbb, and the footer is at most 500 characters."
        }
        "invalid_mail_template" => "The mail was not saved: fix the template and try again.",
        "mail_test_failed" => "The test mail did not go out: check the shop's mail server.",
        "mail_server_required" => {
            "Marketing goes out through the shop's own mail server only, never the platform's: add one on the Mail page and pick it for marketing."
        }
        "payments_unavailable" => "This tenant has no payments set up.",
        "refund_failed" => {
            "Stripe could not refund the payment: the order is not cancelled. Try again."
        }
        "not_cancellable" => "A fulfilled order cannot be cancelled.",
        "invalid_storefront_url" => {
            "A storefront address starts with https:// and is at most 200 characters."
        }
        "invalid_tracking" => "A tracking number is at most 200 characters.",
        "invalid_reason" => "Say why the order is cancelled, in at most 500 characters.",
        "invalid_siret" => "A SIRET is 14 digits; check it against your registration.",
        "invalid_vat_number" => {
            "A French VAT number is FR, a two-digit key and the SIREN (the SIRET's first nine digits)."
        }
        "invalid_tax_category" => "Choose one of the tax categories.",
        "invalid_stock" => "A stock is a whole number from 0 to 1,000,000.",
        "out_of_stock" => "There is not enough of it left.",
        "invalid_aliexpress_market" => {
            "Give a two-letter country (FR), a currency and a two-letter language (fr)."
        }
        "invalid_pricing_rule" => {
            "A price rule multiplies the cost by 1 to 100 (2.5) and adds at most 10,000 (1.00)."
        }
        "aliexpress_not_configured" => "Set what the shop buys on AliExpress first.",
        "aliexpress_not_connected" => "Connect the company's AliExpress account first.",
        "aliexpress_no_company" => "Link the shop to its company first, on its Invoicing page.",
        "aliexpress_disabled" => {
            "AliExpress is not set up on this platform: the operator has to give it an app."
        }
        "aliexpress_connect_expired" => {
            "The connection took too long or was started elsewhere. Try again."
        }
        "aliexpress_connect_refused" => "AliExpress did not connect the account. Try again.",
        "invalid_aliexpress_product" => "Give an AliExpress product number or its address.",
        "aliexpress_already_linked" => "This product is already imported.",
        "aliexpress_importing" => "This product is being imported.",
        "aliexpress_not_found" => "This product was not imported from AliExpress.",
        "aliexpress_combination_not_found" => {
            "AliExpress no longer offers this combination: read the product again."
        }
        "aliexpress_combinations_raced" => "The product was read again meanwhile. Try once more.",
        "aliexpress_already_forwarded" => "This order is already placed on AliExpress.",
        "aliexpress_forwarding" => {
            "This order is being placed on AliExpress. If it stays so, look at the account's orders on AliExpress, then place it again."
        }
        "aliexpress_nothing_to_forward" => "No item of this order comes from AliExpress.",
        "aliexpress_sync_failed" => "AliExpress did not answer. The product is read again later.",
        "secrets_disabled" => {
            "No secret can be kept on this platform: the operator has to set secrets.key first."
        }
        "oss_without_vat_number" => "The One-Stop Shop needs a VAT number.",
        "company_not_found" => "This company does not exist.",
        "siren_taken" => {
            "A company with this SIREN is already registered: ask one of its owners to add you."
        }
        "siren_mismatch" => "A company's SIRET keeps its SIREN (the first nine digits).",
        "not_company_owner" => "Only an owner of the company may do this.",
        "last_company_owner" => "A company keeps at least one owner.",
        "series_code_taken" => {
            "Another tenant of the company numbers its invoices under this code: choose another."
        }
        "invalid_series_code" => "A series code is up to 10 letters and digits.",
        "unknown_user" => "No registered user has this e-mail address.",
        "invalid_legal_identity" => {
            "Fill in the legal name, form, address and e-mail, each within its length."
        }
        "invalid_share_capital" => "Enter the share capital in euros, like 1000 or 1000.50.",
        "invalid_review_reason" => "A reason is at most 300 characters.",
        "invalid_review_reply" => "Write a reply of at most 1000 characters.",
        "review_not_found" => "This review does not exist.",
        "review_rejected" => "This review was rejected and cannot change any more.",
        "review_photo_not_found" => "This photo is no longer on the review.",
        "review_request_delay_too_long" => "Enter a number of days from 0 to 60.",
        "invoicing_not_configured" => {
            "Shoppers cannot pay until the legal identity on the Invoicing page is filled in."
        }
        "writes_paused" => {
            "Changes are temporarily not accepted while a node is away. Please try again in a few minutes."
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
        "discounts:read" => "See the discount codes",
        "discounts:manage" => {
            "Create, deactivate and archive discount codes (includes seeing them)"
        }
        "reviews:read" => "See the reviews shoppers wrote",
        "reviews:manage" => {
            "Publish and reject reviews, take photos off them (includes seeing them)"
        }
        "customers:read" => "See the shop's customer accounts",
        "customers:manage" => "Delete shopper accounts (includes seeing them)",
        "mail:manage" => "Set up the mail servers the shop's mails go out through",
        "shipping:manage" => "Set up the ways the shop ships and what they cost",
        "support:read" => "See the messages shoppers sent through the contact form",
        "support:manage" => "Mark messages handled and reopen them (includes seeing them)",
        "aliexpress:manage" => {
            "Connect the AliExpress app, import products from it and keep them in step"
        }
        "newsletter:read" => "See who subscribed to the shop's newsletter",
        "newsletter:manage" => {
            "Unsubscribe and erase newsletter subscribers (includes seeing them)"
        }
        "automations:read" => "See the automation scenarios",
        "automations:manage" => "Draw, save and delete automation scenarios (includes seeing them)",
        "pages:read" => "See the shop's pages (legal notice, terms, about), drafts included",
        "pages:manage" => "Write, publish and delete the shop's pages (includes seeing them)",
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

/// A day as `YYYY-MM-DD` (UTC), the way a date input gives it.
pub fn format_date(secs: u64) -> String {
    format_time(secs)[..10].to_owned()
}

/// Midnight UTC of a `YYYY-MM-DD` day, as unix seconds; `None` for anything
/// else (or a day before 1970).
pub fn parse_date(value: &str) -> Option<u64> {
    let mut parts = value.trim().splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days-from-civil (Howard Hinnant's algorithm), the inverse of the one
    // in `format_time`.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = u64::try_from(days).ok()? * 86_400;
    // A day the month does not have (`2026-02-30`) rolls over: refuse it.
    (format_date(secs) == value.trim()).then_some(secs)
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
        <li class="py-2.5 text-sm">
            <time datetime=(started_at)>(format_time(started_at))</time>
            if let Some(ip) = ip { " · " (ip) }
            if current { " · " <strong>"this browser"</strong> }
            if let Some(ua) = user_agent { <span class="block truncate text-muted-foreground">(ua)</span> }
        </li>
    })
}

/// A row of a sign-ins list: when, how, from where, on what.
#[component]
pub async fn sign_in_row(
    at: u64,
    method: String,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> Result<impl View> {
    Ok(view! {
        <li class="py-2.5 text-sm">
            <time datetime=(at)>(format_time(at))</time>
            " · " (method)
            if let Some(ip) = ip { " · " (ip) }
            if let Some(ua) = user_agent { <span class="block truncate text-muted-foreground">(ua)</span> }
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

    #[test]
    fn reads_days_back() {
        assert_eq!(super::parse_date("1970-01-01"), Some(0));
        assert_eq!(super::parse_date("2023-11-14"), Some(1_699_920_000));
        assert_eq!(super::parse_date(" 2024-02-29 "), Some(1_709_164_800));
        for bad in ["", "2026-02-30", "2026-13-01", "nope", "1969-12-31"] {
            assert_eq!(super::parse_date(bad), None, "{bad}");
        }
        assert_eq!(super::format_date(1_709_164_800), "2024-02-29");
    }
}
