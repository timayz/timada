//! Shared page furniture: the document shell ([`chrome`]), the page-level
//! helpers ([`page`]), the browser preferences ([`theme`]), the icons
//! ([`icons`]), the wording of errors and permissions, and the dates.

pub mod chrome;
pub mod icons;
pub mod page;
pub mod theme;
pub mod translations;

pub use chrome::{Shell, auth_document, document};
pub use page::*;

use topcoat::{
    Result,
    context::Cx,
    icon::icon,
    view::{View, attributes, component, view},
};

use crate::components::alert::{AlertVariant, alert, alert_description};

/// An amount that may be a loss, with its currency: `-3.20 €`.
pub fn signed_money(cents: i64, currency: &str) -> String {
    let money = timada_core::format_money(cents.unsigned_abs(), currency);
    if cents < 0 {
        format!("-{money}")
    } else {
        money
    }
}

/// A notice above a page's content: `error` codes from a redirect, or a
/// confirmation message.
#[component]
pub async fn notice(
    cx: &Cx,
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
        "signup_closed" => {
            "Accounts are created by invitation here. Ask the person who runs this platform for one."
        }
        "invitation_other_address" => {
            "This invitation is for another e-mail address: sign in at the provider with the address the invitation was sent to, one it has verified."
        }
        "invalid_credentials" => "The e-mail address or the password is wrong.",
        "invalid_code" => "That code is not right. Check the latest e-mail and try again.",
        "code_expired" => "That code has expired. Please start again.",
        "too_many_attempts" => "Too many wrong codes. Please start again to get a new one.",
        "too_many_logins" => "Too many sign-in attempts. Wait a few minutes and try again.",
        "code_recently_sent" => {
            "A code was sent a moment ago. Wait a minute before asking for another."
        }
        "invalid_email" => "Please enter a valid e-mail address.",
        "weak_password" => "The password must be between 8 and 128 characters.",
        "password_mismatch" => "The two passwords do not match.",
        "wrong_password" => "The current password is wrong.",
        "email_set" => "This account already has an e-mail address.",
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
        "product_slug_taken" => "Another product or a category already uses this slug.",
        "slug_reserved" => {
            "This slug is the address of one of the shop's own pages. Choose another one."
        }
        "invalid_description" => "A description is at most 4000 characters.",
        "invalid_seo" => {
            "The search engines' title is at most 70 characters, the description at most 160."
        }
        "invalid_brand" => "A brand is at most 70 characters.",
        "invalid_gtin" => {
            "A GTIN is the 8, 12, 13 or 14 digits under a barcode. Check it for a mistyped digit."
        }
        "invalid_mpn" => "A part number is at most 70 characters.",
        "identifier_conflict" => {
            "A product said to have no identifier carries no GTIN and no part number, on itself or on a variant."
        }
        "invalid_feed_label" => "A label is at most 100 characters.",
        "no_product_ticked" => "Tick the products first.",
        "invalid_google_category" => "A Google product category is at most 750 characters.",
        "invalid_audience" => "Choose one of the listed age groups and genders.",
        "invalid_option_kind" => "Choose one of the listed kinds.",
        "option_kind_taken" => "Another option of this product is already of this kind.",
        "compare_at_not_above" => "What it is reduced from has to be more than what it costs now.",
        "no_earlier_price" => {
            "It was on sale at nothing else in the 30 days before its current price, so there is no reduction to announce: lower its price first."
        }
        "compare_at_above_earlier" => {
            "A reduction is measured from the lowest it was on sale at in the 30 days before its current price, no more."
        }
        "invalid_amount" => "Enter the price as a number with at most two decimals, like 12.50.",
        "cost_on_variants" => {
            "This product is sold through its combinations: say what each of them costs."
        }
        "invalid_fee" => "A payment fee is between 0 and 100 % of the amount paid.",
        "invalid_margin_tiers" => {
            "A high margin starts above a medium one, both between 0 and 100 % of the price."
        }
        "invalid_currency" => "Choose one of the listed currencies.",
        "product_not_found" => "This product does not exist.",
        "product_archived" => "This product is archived and cannot change any more.",
        "product_not_priced" => "Set a price before publishing.",
        "product_ordered" => {
            "This product has been ordered, so it stays on record: archive it instead."
        }
        "invalid_category_name" => "A category name is 1 to 80 characters.",
        "invalid_category_slug" => {
            "A category slug is 2 to 64 lowercase letters, digits and dashes."
        }
        "invalid_category_description" => "A category description is at most 2000 characters.",
        "category_slug_taken" => "Another category or a product already uses this slug.",
        "category_not_found" => "This category does not exist.",
        "category_archived" => "This category is archived and cannot change any more.",
        "category_cycle" => "A category cannot be moved under one of its own sub-categories.",
        "category_too_deep" => "Categories nest at most three levels deep.",
        "category_parent_draft" => "The category above it is a draft: publish that one first.",
        "category_has_published_children" => {
            "Keep back its sub-categories first: they are shown to shoppers."
        }
        "feed_label_taken" => {
            "This label already says something else: the margin's tier and what is kept out of ads each need their own."
        }
        "category_invalid_questions" => {
            "A category answers at most 20 questions: each on one line of at most 200 characters, followed by its answer of at most 1,500."
        }
        "too_many_pinned_products" => "A category shows at most 8 products first.",
        "too_many_listing_filters" => "Listings are narrowed by at most 8 options or facts.",
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
        "invalid_return_days" => "A return window is 1 to 365 days.",
        "invalid_export_address" => {
            "Give the e-mail address to tell, or the https address to post the file to."
        }
        "invalid_export_schedule" => "Pick how often the export goes out.",
        "export_address_not_public" => "The file can only be posted to an address on the internet.",
        "export_address_unavailable" => {
            "Posting to an address is not available on this installation: its operator has not set the key the addresses are kept under."
        }
        "too_many_shipping_methods" => "A shop offers at most twenty shipping methods.",
        "shipping_method_not_found" => "This shipping method does not exist.",
        "category_has_children" => "Archive or move its sub-categories first.",
        "too_many_categories" => "A product belongs to at most ten categories.",
        "too_many_images" => "A product carries at most twenty pictures.",
        "unsupported_image" => "Upload a JPEG, PNG, WebP or GIF picture.",
        "image_too_large" => "A picture is at most 8 MiB.",
        "image_too_big" => "A picture is at most 25 million pixels (5000 × 5000).",
        "storage_full" => {
            "The shop's pictures take all the space it has. Remove some first, or ask the platform for more."
        }
        "invalid_media_quota" => "A picture quota is a number of megabytes above zero, or blank.",
        "invalid_node" => {
            "A node is named by its number: its place among the peers of the configuration."
        }
        "invalid_node_address" => {
            "A node's address is a host or an IP address and a port, as in timada-3.timada-headless:7000."
        }
        "members_not_changed" => {
            "The cluster's members were not changed: another change is in progress, or this node does not reach most of the others. Try again in a moment; the node's log says why."
        }
        "remove_this_node" => {
            "Remove this node from the page of another node: this one must go on answering."
        }
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
        "option_value_not_sold" => "No variant would be on sale at this value.",
        "option_hidden_at_value" => {
            "This option is hidden from shoppers at a value you dropped. Show it first."
        }
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
        "page_invalid_title" => "A title is 1 to 120 characters.",
        "page_invalid_body" => "The content is at most 100,000 characters.",
        "page_invalid_seo" => {
            "The search engines' title is at most 70 characters, the description at most 160."
        }
        "page_invalid_slug" => {
            "An address is 2 to 64 lowercase letters, digits or dashes, with no dash at either end."
        }
        "page_slug_taken" => "Another page has this address.",
        "page_too_many_images" => "A page or an article carries at most 50 pictures.",
        "page_not_found" => "This page does not exist.",
        "page_image_not_found" => "This picture is no longer there.",
        "article_invalid_summary" => "A summary is at most 300 characters.",
        "article_invalid_date" => "An article is dated today or before.",
        "article_slug_taken" => "Another article has this address.",
        "article_not_found" => "This article does not exist.",
        "announcement_invalid" => "An announcement is 1 to 200 characters.",
        "announcement_invalid_link" => {
            "An announcement links at most once, to a page of the shop (/…) or an https:// address."
        }
        "announcement_invalid_window" => "Give the dates as day and time, the end after the start.",
        "too_many_announcements" => "The bar holds at most 10 announcements.",
        "announcement_not_found" => "This announcement does not exist.",
        "redirect_invalid" => {
            "A redirect goes from an old path (/…, without a ?, accents written as %C3%A9) to a path of the storefront (/…), each at most 300 characters."
        }
        "redirect_exists" => "This old path already redirects. Delete that redirect first.",
        "redirect_chain" => {
            "A redirect cannot lead to another redirect: send the old path straight to the page."
        }
        "too_many_redirects" => "A shop keeps at most 500 redirects.",
        "redirect_not_found" => "This redirect does not exist.",
        "google_tag_invalid_id" => {
            "An Analytics ID reads G-XXXXXXXXXX (or GT-…) and a Google Ads ID AW-123456789."
        }
        "google_tag_invalid_label" => {
            "A conversion label holds letters, digits, dashes and underscores only."
        }
        "google_ads_id_missing" => {
            "Conversion labels and enhanced conversions need the Google Ads conversion ID."
        }
        "google_disabled" => {
            "Connecting a Google account is not available on this installation: its operator has to set it up first."
        }
        "google_no_access_chosen" => "Tick what the shop may read from Google.",
        "google_connect_expired" => {
            "Connecting took too long, or was started in another browser. Start again."
        }
        "google_connect_refused" => {
            "Google did not connect the account: it was refused there, or Google gave nothing to keep. Start again."
        }
        "google_nothing_granted" => {
            "The account granted none of the access asked for: tick the boxes on Google's page when it asks."
        }
        "google_unknown_access" => "This kind of access is not one a shop can turn on.",
        "google_unavailable" => "Google did not answer. Try again in a moment.",
        "google_refused" => "Google refused the request. Try again, or connect again.",
        "google_not_connected" => "The shop is not connected to a Google account.",
        "google_connect_again" => {
            "Google no longer honours this connection: connect the account again."
        }
        "google_no_search_console" => {
            "The connection does not cover Search Console: connect again and tick it."
        }
        "google_invalid_site" | "google_unknown_site" => {
            "Choose one of the sites the account reads in Search Console."
        }
        "google_no_merchant_center" => {
            "The connection does not cover Merchant Center: connect again and tick it."
        }
        "google_invalid_merchant_account" | "google_unknown_merchant_account" => {
            "Choose one of the Merchant Center accounts the Google account reaches."
        }
        "campaign_not_ready" => {
            "The shop is not ready for this campaign: the page says what is missing."
        }
        "campaign_waits" => {
            "This campaign is prepared already and waits for an answer, under Proposals."
        }
        "google_invalid_campaign" => "No such campaign.",
        "google_invalid_campaign_name" => "A campaign's name is one to 200 characters.",
        "google_invalid_campaign_amount" => {
            "An amount reads 0.30, in the account's currency, above zero."
        }
        "google_bid_over_budget" => "The bid is more than the campaign may spend in a day.",
        "google_invalid_countries" => {
            "Countries read FR, BE: the two letters of each, among the ones Google Ads is told by here."
        }
        "google_invalid_adjustment" => {
            "An adjustment is a whole percentage between -90 and 900, or nothing."
        }
        "google_invalid_start" => "A campaign starts paused or live.",
        "google_invalid_ad_groups" => {
            "An ad group is a line made of the id of a category the shop advertises and a name, no two the same, a hundred at most."
        }
        "google_invalid_keywords" => {
            "A campaign on the shop's name takes one to ten searches, a line each, of 80 characters at most."
        }
        "google_invalid_headlines" => {
            "An ad takes three to fifteen headlines, a line each, of 30 characters at most."
        }
        "google_invalid_descriptions" => {
            "An ad takes two to four descriptions, a line each, of 90 characters at most."
        }
        "google_campaign_exists" => "The campaign was created already: change it at Google Ads.",
        "google_no_campaign" => "The campaign was not created yet.",
        "google_campaign_live" => "The campaign runs already.",
        "google_invalid_tuning" => {
            "A threshold is a whole number within its range; a step or a return reads 0.02."
        }
        "google_campaign_settling" => {
            "The campaign does not run yet, or is in its first three weeks: a round comes after."
        }
        "google_invalid_tuned" => "No such ad group, device or search in the campaign.",
        "launch_unknown_step" => "No such step to confirm.",
        "launch_invalid_path_target" => "A number of products is between 1 and 100,000.",
        "launch_invalid_target" => {
            "A target is an amount above zero, in a currency of three letters."
        }
        "launch_invalid_target_day" => "A target's day is a day yet to come.",
        "launch_invalid_threshold" => {
            "A threshold is a whole number, or an amount like 0.70, within its range; the bar to go on is never under the bar to stop."
        }
        "launch_unknown_checkpoint" => "No such checkpoint.",
        "launch_not_reached" => "The checkpoint's day has not come.",
        "launch_invalid_decision" => "A decision is to go on, to fix or to stop.",
        "launch_note_too_long" => "A note is at most 1,000 characters.",
        "google_merchant_just_read" => {
            "Merchant Center was read a moment ago. Wait a few minutes before asking again."
        }
        "google_merchant_nothing_to_read" => {
            "There is nothing to read Merchant Center with: connect the Google account and choose the Merchant Center account."
        }
        "google_no_ads" => "The connection does not cover Google Ads: connect again and tick it.",
        "google_invalid_ads_account" | "google_unknown_ads_account" => {
            "Choose one of the Google Ads accounts the Google account reaches."
        }
        "proposal_unknown_kind" => "This is not a kind of change a proposal can carry.",
        "proposal_invalid_subject" => "A proposal names the product or the category it is about.",
        "proposal_invalid_source" => "A proposal says what prepared it.",
        "proposal_invalid_changes" => {
            "A proposal changes one to eight fields of what it is about, each once."
        }
        "proposal_value_too_long" => "A value is at most 20,000 characters.",
        "proposal_nothing_to_change" => {
            "Nothing would change: every field already holds that value."
        }
        "proposal_reason_too_long" => "A reason is at most 1,000 characters.",
        "proposal_invalid_validity" => "A proposal is valid for ninety days at most.",
        "proposal_not_found" => "This proposal does not exist.",
        "proposal_not_waiting" => "This proposal was answered already.",
        "proposal_expired" => {
            "This proposal waited past its date: it can be refused, no longer approved."
        }
        "proposal_unknown_field" => "The proposal does not change this field.",
        "proposal_no_applier" => "Nothing applies this kind of change yet.",
        "proposal_subject_missing" => "What the proposal is about is no longer there.",
        "proposal_out_of_date" => {
            "What this proposal is about changed since it was prepared: it ended as out of date and nothing was applied."
        }
        "proposal_none_selected" => "Tick the proposals to answer first.",
        "writer_off" => "Text drafts are not set up on this platform.",
        "writer_on_its_way" => {
            "A draft is on its way for it already: it will wait under Proposals."
        }
        "writer_unknown_kind" => "Texts are drafted for a product or a category.",
        "writer_invalid_subject" => "Name the product or the category to draft the texts of.",
        "writer_invalid_keyword" => "A search to write for is at most 120 characters.",
        "writer_brief_too_long" => {
            "The tone, and what never to claim, are at most 1,000 characters each."
        }
        "meta_pixel_invalid_id" => "A Meta pixel ID is 5 to 20 digits.",
        "meta_pixel_missing" => {
            "A Conversions API token needs the ID of the pixel it was made for."
        }
        "language_invalid" => "Choose the languages from the list.",
        "invalid_word" => "A word and its translation are at most 400 characters.",
        "too_many_languages" => "A storefront is translated into at most 10 languages.",
        "newsletter_popup_invalid" => {
            "A pop-up shown needs what it offers, in at most 80 characters; the sentence under it is at most 300, and it opens within 600 seconds."
        }
        "meta_token_invalid" => {
            "This does not look like a Conversions API token: paste the access token as Events Manager generated it."
        }
        "meta_token_unavailable" => {
            "A token cannot be kept on this installation: its operator has not set the key the tokens are kept under."
        }
        "campaign_sent" => "This campaign was sent already: it can no longer be changed.",
        "campaign_invalid_schedule" => {
            "This schedule does not hold together: pick at least one day (every week), a day 1 to 28 (every month), an end time after the start time, and dates from today on."
        }
        "campaign_not_scheduled" => "This campaign has no schedule running.",
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
            "A scenario that calls webhooks or posts to Discord is published by members who may manage the API keys: it hands the shop's events to another system."
        }
        "automation_webhook_address" => "A webhook's address is a web address (https://…).",
        "automation_discord_address" => {
            "That is not a Discord webhook address: copy it from the channel's settings, under Integrations (https://discord.com/api/webhooks/…)."
        }
        "automation_discord_invalid" => "A Discord message does not read: see what its step says.",
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
        "money_owed" => {
            "Payments are still held or owed: the keys stay until they are settled. Take the payments or cancel those orders first."
        }
        "payments_not_stopped" => "Payments are not stopped: stop them before forgetting the keys.",
        "payments_stopped_recently" => {
            "The keys can be forgotten only eight days after payments were stopped: a card may hold money that long."
        }
        "account_in_use" => {
            "Payments are still held or owed under the current Stripe account: keep its keys until they are settled. A new secret key of the same account can be saved."
        }
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
            "Stripe could not give the money back yet: the order is not cancelled. Try again in a moment."
        }
        "not_cancellable" => "A fulfilled order cannot be cancelled.",
        "capture_pending" => "The payment is being taken: try again in a moment.",
        "cancellation_pending" => "The order is being cancelled.",
        "payment_not_captured" => "The payment is only held on the shopper's card: take it first.",
        "capture_failed" => {
            "Stripe would not take the payment: see why on the order. You may try again, or cancel the order."
        }
        "hold_expired" => {
            "The hold on the shopper's card had expired: nothing was taken, and the order is cancelled."
        }
        "hold_expired_supplier_ordered" => {
            "The hold on the shopper's card had expired: nothing was taken. AliExpress may have the order, so it stays open: settle it with the shopper, then cancel it."
        }
        "order_fulfilled" => "This order has left already.",
        "not_kept_open" => {
            "Only an order whose card hold lapsed while AliExpress may have it is settled outside timada: take the payment instead."
        }
        "hold_expiring" => {
            "The hold on the shopper's card ends too soon to buy the goods: take the payment first."
        }
        "supplier_ordered" => {
            "AliExpress may already have this order: tick that you cancel it there too."
        }
        "supplier_asking" => {
            "The order is being placed on AliExpress, or AliExpress took it: there is nothing to rule out."
        }
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
        "invalid_delivery_days" => "A delivery time is 1 to 365 days, the shortest first.",
        "invalid_stock" => "A stock is a whole number from 0 to 1,000,000.",
        "out_of_stock" => "There is not enough of it left.",
        "invalid_aliexpress_market" => {
            "Give a two-letter country (FR), a currency and a two-letter language (fr)."
        }
        "invalid_pricing_rule" => {
            "A price rule multiplies the cost by 1 to 100 (2.5) and adds at most 10,000 (1.00)."
        }
        "invalid_price_tolerance" => "A price tolerance is a percentage from 0 to 20 (1.5).",
        "invalid_phone" => "A phone number has 6 to 32 characters: digits, spaces, +, -, ( and ).",
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
        "aliexpress_list_catching_up" => {
            "AliExpress's list of orders may not show an order it just took: look there again in half an hour."
        }
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
        "invalid_study_name" => "A study's name is 1 to 80 characters.",
        "invalid_seed" => "A seed keyword is 1 to 80 characters.",
        "study_without_seed" => "A study starts from one seed keyword at least, one a line.",
        "too_many_seeds" => "A study holds at most 20 seed keywords.",
        "invalid_study_place" => "Pick the country and the language from the list.",
        "study_not_found" => "This study does not exist.",
        "unknown_seed" => "The study has no such seed keyword.",
        "keywords_fetching" => "These keywords are being fetched: look again in a moment.",
        "nothing_to_fetch" => "Every seed keyword has its keywords, or is being fetched.",
        "invalid_study_currency" => "Pick the sale's currency from the list.",
        "invalid_study_amount" => "Enter each amount like 60 or 59.90.",
        "invalid_study_rate" => "A rate is between 0 and 100 %.",
        "invalid_study_word" => "A brand or an excluded word is at most 80 characters.",
        "too_many_study_words" => "A study lists at most 200 brands and 200 excluded words.",
        "invalid_supplier_products" => "Enter the number of products as a whole number.",
        "invalid_keyword" => "This keyword cannot be dropped or brought back.",
        "too_many_hand_set_keywords" => {
            "A study holds at most 2,000 keywords dropped by hand, and as many brought back: exclude a word instead."
        }
        "study_changed_too_often" => "This study was changed too many times: start another.",
        "no_main_keyword" => "The study has no main keyword yet: fetch its keywords first.",
        "too_many_checked_keywords" => "The first results of at most 50 keywords are looked at.",
        "nothing_to_check" => {
            "Every main keyword has its first results, or they are being searched."
        }
        "invalid_domain" => "A site is named by its domain, like example.com.",
        "too_many_marks" => "At most 2,000 sites are marked: no more can be.",
        "unknown_parent_category" => {
            "A category goes under one of the tree's parent categories, and a child holds none."
        }
        "tree_category_has_children" => {
            "This category has children: move them or take them out first."
        }
        "too_many_tree_categories" => "A tree holds at most 200 categories.",
        "too_many_parked_keywords" => "At most 500 keywords are parked.",
        "keyword_not_in_tree" => "This keyword is not in the tree.",
        "keyword_not_kept" => "Only a keyword the study keeps makes a category.",
        "too_many_niche_words" => "A niche is named by at most 10 words.",
        "tree_held_back" => {
            "Nothing is created while a category of the tree has something in its way."
        }
        "shop_not_manageable" => "Pick a shop whose catalog you may manage.",
        "invalid_study_shop" => "Name the shop to create, or pick one of yours.",
        "study_has_another_shop" => {
            "This study's categories are created in the shop it is linked to."
        }
        "categories_being_created" => "The categories are being created: look again in a moment.",
        "nothing_to_create" => "Every category that can be created is.",
        "keywords_disabled" => {
            "Keyword research is not set up on this platform: the operator has to give it a keyword source."
        }
        "keyword_places_unavailable" => {
            "The keyword source could not say which countries it covers. Try again in a moment."
        }
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
        "write_not_taken" => {
            "This change did not go through, and nothing was saved. Please try again."
        }
        _ => "Something went wrong. Please try again.",
    }
}

/// A delivery time as the back office shows it: `8–15 days`, `3 days`.
pub fn delivery_days_label(days: timada_catalog::DeliveryDays) -> String {
    if days.min == days.max {
        format!(
            "{} {}",
            days.max,
            if days.max == 1 { "day" } else { "days" }
        )
    } else {
        format!("{}–{} days", days.min, days.max)
    }
}

/// What a permission of [`timada_tenancy::PERMISSIONS`] lets a role do.
pub fn describe_permission(permission: &str) -> &'static str {
    match permission {
        "tenant:manage" => "Rename the tenant",
        "members:read" => "See members and pending invitations",
        "members:manage" => {
            "Invite and remove members, with a role no wider than their own (includes seeing them)"
        }
        "roles:manage" => "See the roles and what each allows",
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
        "proposals:prepare" => {
            "Prepare a change to the shop for someone else to approve, and see the ones that wait"
        }
        "proposals:approve" => {
            "Approve, edit or refuse the changes that wait; a change to the catalog also needs the permission to manage products"
        }
        "ads:manage" => {
            "Prepare the shop's campaigns, approve them and set them running at Google Ads"
        }
        _ => "Unknown permission",
    }
}

pub fn describe_api_key_scope(scope: &str) -> &'static str {
    match scope {
        "store:read" => "Read the events and run subscriptions over them",
        "store:checkout" => "Fill carts and place orders, nothing else",
        _ => "Unknown scope",
    }
}

/// Unix seconds as `YYYY-MM-DD HH:MM:SS UTC`, a day as `YYYY-MM-DD`, and
/// back ([`timada_core::civil`]).
pub use timada_core::civil::{format_date, format_time, parse_date};

/// A row of a sessions list: when it started, from where, on what.
#[component]
pub async fn session_row(
    cx: &Cx,
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
    cx: &Cx,
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
