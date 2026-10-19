//! `/t/{slug}/aliexpress`: the shop's own AliExpress app, for members with
//! `aliexpress:manage`. The merchant enters the app's key and secret (kept
//! sealed), connects the AliExpress account the app acts for, then imports
//! products by their number or address: each becomes a draft whose price,
//! stock and availability are read again from AliExpress.

pub mod callback;
pub mod configure;
pub mod connect;
pub mod import;
pub mod products;
pub mod remove;

use std::sync::Arc;

use timada_aliexpress::{LinkRow, links_of_tenant};
use timada_catalog::{CURRENCIES, format_money};
use timada_tenancy::{PERMISSION_ALIEXPRESS_MANAGE, PricingRule, Rounding};
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::aliexpress::Aliexpress;
use crate::app::auth::Flash;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::config::Config;
use crate::db::ReadPool;
use crate::ui::{
    LINK, Shell, Tone, document, empty_row, fact, facts, format_time, icons, notice, page_header,
    section_card, status_badge,
};

/// How many imports the page lists.
const LISTED: i64 = 200;

#[topcoat::router::query_params(error = bad_request)]
struct AliexpressQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn aliexpress_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// Where AliExpress sends the merchant back once their account said yes:
/// the address to register as the app's callback.
pub fn callback_url(cx: &Cx, slug: &str) -> String {
    format!(
        "{}{}",
        app_context::<Config>(cx).server.url.trim_end_matches('/'),
        href!(callback::callback, Slug(slug.to_owned())).resolve(cx)
    )
}

/// A price rule as the forms send it: a multiplier (`2.5`), an amount added
/// (`1.00`) and a rounding. No multiplier is no rule.
#[derive(Debug, Default, serde::Deserialize)]
pub struct PricingForm {
    #[serde(default)]
    pub multiplier: String,
    #[serde(default)]
    pub plus: String,
    #[serde(default)]
    pub rounding: String,
}

impl PricingForm {
    /// The rule, `None` without a multiplier; `Err` with the error code of
    /// a rule that does not read.
    pub fn rule(&self) -> std::result::Result<Option<PricingRule>, &'static str> {
        let multiplier = self.multiplier.trim().replace(',', ".");
        if multiplier.is_empty() {
            return Ok(None);
        }
        let hundredths =
            timada_aliexpress::parse_amount(&multiplier).ok_or("invalid_pricing_rule")?;
        let plus = self.plus.trim().replace(',', ".");
        let plus_cents = if plus.is_empty() {
            0
        } else {
            timada_aliexpress::parse_amount(&plus).ok_or("invalid_pricing_rule")?
        };
        let rule = PricingRule {
            multiplier_bps: u32::try_from(hundredths.saturating_mul(100))
                .map_err(|_| "invalid_pricing_rule")?,
            plus_cents,
            rounding: Rounding::parse(self.rounding.trim()).unwrap_or_default(),
        };
        if !rule.is_valid() {
            return Err("invalid_pricing_rule");
        }
        Ok(Some(rule))
    }
}

/// `×2.50 + 1.00, to .99`, or `By hand` without a rule.
pub fn rule_label(rule: Option<PricingRule>, currency: &str) -> String {
    let Some(rule) = rule else {
        return "By hand (the cost is the starting price)".to_owned();
    };
    let mut label = format!(
        "Cost ×{}.{:02}",
        rule.multiplier_bps / 10_000,
        rule.multiplier_bps % 10_000 / 100
    );
    if rule.plus_cents > 0 {
        label.push_str(&format!(" + {}", format_money(rule.plus_cents, currency)));
    }
    label.push_str(match rule.rounding {
        Rounding::Cent => "",
        Rounding::Ninety9 => ", up to .99",
        Rounding::Whole => ", up to a whole amount",
    });
    label
}

/// What a rule's fields show in a form.
pub struct RuleFields {
    pub multiplier: String,
    pub plus: String,
    pub rounding: &'static str,
}

impl RuleFields {
    pub fn of(rule: Option<PricingRule>) -> Self {
        match rule {
            Some(rule) => Self {
                multiplier: format!(
                    "{}.{:02}",
                    rule.multiplier_bps / 10_000,
                    rule.multiplier_bps % 10_000 / 100
                ),
                plus: if rule.plus_cents == 0 {
                    String::new()
                } else {
                    format!("{}.{:02}", rule.plus_cents / 100, rule.plus_cents % 100)
                },
                rounding: rule.rounding.as_str(),
            },
            None => Self {
                multiplier: String::new(),
                plus: String::new(),
                rounding: Rounding::Cent.as_str(),
            },
        }
    }
}

/// How the list words an import's status.
fn status_of(row: &LinkRow) -> (&'static str, Tone) {
    match (row.status.as_str(), row.offline) {
        ("linked", true) => ("No longer sold", Tone::Warning),
        ("linked", false) => ("Linked", Tone::Success),
        ("importing", _) => ("Importing…", Tone::Info),
        ("failed", _) => ("Failed", Tone::Danger),
        _ => ("Unlinked", Tone::Neutral),
    }
}

/// The rounding choices of a rule form.
pub const ROUNDINGS: [(&str, &str); 3] = [
    ("cent", "To the cent"),
    ("99", "Up to .99"),
    ("whole", "Up to a whole amount"),
];

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let query = query_params::<AliexpressQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let pool = app_context::<ReadPool>(cx);
    let links = links_of_tenant(pool, &access.tenant.id, LISTED)
        .await
        .map_err(|err| topcoat::Error::from_anyhow(err.into()))?;
    let TenantAccess { tenant, .. } = access;
    let enabled = app_context::<Arc<Aliexpress>>(cx).enabled();
    let slug = tenant.slug.clone();
    let callback = callback_url(cx, &slug);
    let settings = tenant.aliexpress.clone();
    let state = match &settings {
        Some(settings) if settings.authorization.is_some() => "connected",
        Some(_) => "configured",
        None => "none",
    };
    let currency = settings
        .as_ref()
        .map(|settings| settings.currency.clone())
        .unwrap_or_else(|| CURRENCIES[0].to_owned());
    let rule = settings.as_ref().and_then(|settings| settings.pricing);
    let rule_text = rule_label(rule, &currency);
    let fields = RuleFields::of(rule);
    let app_key = settings
        .as_ref()
        .map(|settings| settings.app_key.clone())
        .unwrap_or_default();
    let country = settings
        .as_ref()
        .map(|settings| settings.ship_to_country.clone())
        .unwrap_or_else(|| "FR".to_owned());
    let language = settings
        .as_ref()
        .map(|settings| settings.language.clone())
        .unwrap_or_else(|| "fr".to_owned());
    let title = format!("AliExpress · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "aliexpress"),
            page_header(title: "AliExpress", subtitle: Some("Import products from AliExpress through the shop's own app, and keep their price and stock in step."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if !enabled {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" class="mb-6" },
                    icon(data: icons::WARNING)
                    alert_description("No secret can be kept on this platform: the operator has to set secrets.key first.")
                )
            }

            section_card(
                title: Some("Connection"),
                <div class="mb-4" data-aliexpress=(state)>
                    if let Some(settings) = &settings {
                        facts(
                            fact(term: "App key", <code data-aliexpress-app-key="">(&settings.app_key)</code>)
                            fact(term: "App secret", "Saved, never shown again")
                            fact(term: "Buys for", (&settings.ship_to_country) " · " (&settings.currency) " · " (&settings.language))
                            fact(term: "Price rule", attrs: attributes! { data-aliexpress-rule="" }, (&rule_text))
                            if let Some(authorization) = &settings.authorization {
                                fact(term: "Account", status_badge(tone: Tone::Success, "Connected") " " <span data-aliexpress-account="">(&authorization.account)</span>)
                                fact(term: "Access until", <time datetime=(authorization.expires_at)>(format_time(authorization.expires_at))</time>)
                            } else {
                                fact(term: "Account", status_badge(tone: Tone::Warning, "Not connected"))
                            }
                        )
                    } else {
                        <p class="text-sm text-muted-foreground">"Not set up: enter the app's key and secret below."</p>
                    }
                </div>
                if settings.is_some() {
                    <div class="flex flex-wrap gap-2">
                        <form method="post" action=(href!(connect::connect, Slug(slug.clone())))>
                            button(size: ButtonSize::Sm, attrs: attributes! { type="submit" },
                                (if state == "connected" { "Connect again" } else { "Connect the AliExpress account" }))
                        </form>
                        <form method="post" action=(href!(remove::remove, Slug(slug.clone())))>
                            button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Disconnect")
                        </form>
                    </div>
                }
            )

            if state == "connected" {
                section_card(
                    title: Some("Import a product"),
                    description: Some("It becomes a draft, priced by the rule, with AliExpress's pictures, options and stock. Check it, then publish it."),
                    <form method="post" action=(href!(import::import, Slug(slug.clone()))) class="flex max-w-xl flex-wrap items-end gap-2">
                        field(
                            attrs: attributes! { class="min-w-0 flex-1" },
                            field_label(attrs: attributes! { for="aliexpress-product" }, "AliExpress product")
                            input(attrs: attributes! {
                                id="aliexpress-product" name="product" required="" placeholder="https://www.aliexpress.com/item/1005006123456789.html"
                            })
                        )
                        button(attrs: attributes! { type="submit" }, "Import")
                    </form>
                )
            }

            section_card(
                title: Some("Imported products"),
                table(
                    attrs: attributes! { data-aliexpress-links="" },
                    table_header(
                        table_row(
                            table_head("AliExpress")
                            table_head("Product")
                            table_head("Status")
                            table_head("Last read")
                            table_head(<span class="sr-only">"Actions"</span>)
                        )
                    )
                    table_body(
                        if links.is_empty() {
                            empty_row(colspan: 5, text: "Nothing imported yet.")
                        }
                        for row in &links {
                            table_row(
                                attrs: attributes! { data-aliexpress-link=(&row.ae_product_id) data-link-status=(&row.status) data-link-offline=(row.offline) },
                                table_cell(
                                    <a class=(LINK) href=(format!("https://www.aliexpress.com/item/{}.html", row.ae_product_id)) rel="noopener noreferrer" target="_blank">(&row.ae_product_id)</a>
                                )
                                table_cell(
                                    if let Some(product_id) = &row.product_id {
                                        <a class=(LINK) href=(crate::app::t::slug::products::product_id::product_url(cx, &slug, product_id, None, None))>"Open"</a>
                                    } else { "—" }
                                )
                                table_cell(
                                    status_badge(tone: status_of(row).1, (status_of(row).0))
                                    if let Some(error) = &row.last_error {
                                        <p class="mt-1 text-xs text-muted-foreground" data-link-error="">(error)</p>
                                    }
                                    if row.status == "linked" && row.new_combinations > 0 {
                                        if let Some(product_id) = &row.product_id {
                                            <p class="mt-1 text-xs" data-link-new=(row.new_combinations)>
                                                <a class=(LINK) href=(crate::app::t::slug::products::product_id::product_url(cx, &slug, product_id, None, None))>
                                                    if row.new_combinations == 1 { "1 new combination" } else { (format!("{} new combinations", row.new_combinations)) }
                                                </a>
                                            </p>
                                        }
                                    }
                                )
                                table_cell(
                                    if let Some(at) = row.last_synced_at {
                                        <time datetime=(at)>(format_time(at as u64))</time>
                                    } else { "—" }
                                )
                                table_cell(
                                    attrs: attributes! { class="text-right" },
                                    if row.status == "linked" {
                                        <form method="post" action=(href!(products::link_id::sync::sync, Slug(slug.clone()), products::link_id::LinkId(row.id.clone())))>
                                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" aria-label=(format!("Read {} again", row.ae_product_id)) }, "Sync now")
                                        </form>
                                    }
                                )
                            )
                        }
                    )
                )
            )

            section_card(
                title: Some(if settings.is_some() { "Change the app" } else { "Set up the app" }),
                <ol class="mb-4 list-decimal space-y-1 pl-5 text-sm text-muted-foreground">
                    <li>"On AliExpress's open platform, create an app with access to the dropshipping API."</li>
                    <li>
                        "Register "
                        <code class="text-foreground" data-aliexpress-callback="">(&callback)</code>
                        " as its callback address, and copy its app key and secret here."
                    </li>
                </ol>
                <form method="post" action=(href!(configure::configure, Slug(slug.clone()))) class="flex max-w-xl flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="aliexpress-key" }, "App key")
                        input(attrs: attributes! { id="aliexpress-key" name="app_key" required="" class="font-mono" value=(&app_key) })
                    )
                    field(
                        field_label(attrs: attributes! { for="aliexpress-secret" }, "App secret")
                        input(attrs: attributes! {
                            id="aliexpress-secret" name="app_secret" type="password" autocomplete="off" class="font-mono"
                            placeholder=(if settings.is_some() { "Leave blank to keep the saved one" } else { "" })
                        })
                    )
                    <div class="flex flex-wrap gap-4">
                        field(
                            field_label(attrs: attributes! { for="aliexpress-country" }, "Ships to (country)")
                            input(attrs: attributes! { id="aliexpress-country" name="ship_to_country" required="" maxlength="2" class="w-24 uppercase" value=(&country) })
                        )
                        field(
                            field_label(attrs: attributes! { for="aliexpress-currency" }, "Bought in (currency)")
                            select(
                                attrs: attributes! { id="aliexpress-currency" name="currency" class="w-28" },
                                for code in CURRENCIES {
                                    <option value=(*code) selected=(currency == *code)>(*code)</option>
                                }
                            )
                        )
                        field(
                            field_label(attrs: attributes! { for="aliexpress-language" }, "Worded in (language)")
                            input(attrs: attributes! { id="aliexpress-language" name="language" required="" maxlength="2" class="w-24" value=(&language) })
                        )
                    </div>
                    <fieldset class="flex flex-wrap gap-4">
                        <legend class="mb-2 text-sm font-medium">"Price rule (blank: price by hand)"</legend>
                        field(
                            field_label(attrs: attributes! { for="aliexpress-multiplier" }, "Multiply the cost by")
                            input(attrs: attributes! { id="aliexpress-multiplier" name="multiplier" inputmode="decimal" placeholder="2.5" class="w-28" value=(&fields.multiplier) })
                        )
                        field(
                            field_label(attrs: attributes! { for="aliexpress-plus" }, "Then add")
                            input(attrs: attributes! { id="aliexpress-plus" name="plus" inputmode="decimal" placeholder="1.00" class="w-28" value=(&fields.plus) })
                        )
                        field(
                            field_label(attrs: attributes! { for="aliexpress-rounding" }, "Round")
                            select(
                                attrs: attributes! { id="aliexpress-rounding" name="rounding" class="w-48" },
                                for (value, label) in ROUNDINGS {
                                    <option value=(value) selected=(fields.rounding == value)>(label)</option>
                                }
                            )
                        )
                    </fieldset>
                    <div>
                        button(attrs: attributes! { type="submit" }, "Save")
                    </div>
                </form>
            )
        )
    })
}
