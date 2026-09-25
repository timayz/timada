//! `/{mount}/promotions`: promo codes and vouchers, plus creating either.

pub mod code_id;

use serde::Deserialize;
use timada_core::Money;
use timada_promotion::{
    CodeListRow, CreateDiscount, DISCOUNT, DiscountKind, IssueVoucher, ListCodes, VOUCHER,
    VoucherKind, count_codes, list_codes,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::see_other, href, page, query_params, query_params as query},
    view::{View, view},
};

use super::products::field;
use crate::{
    components::{
        button::{ButtonVariant, button, button_variants},
        card::{card, card_content},
        select::select,
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::{AdminConfig, AdminServices},
    ui::{date, empty_state, money, page_header, pagination},
};

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct PromotionsQuery {
    page: Option<u32>,
    kind: Option<String>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query::<PromotionsQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let kind = query.kind.clone().filter(|k| k == DISCOUNT || k == VOUCHER);
    let db = &app_context::<AdminServices>(cx).db;
    let rows = list_codes(
        db,
        &ListCodes {
            kind: kind.clone(),
            limit: PAGE_SIZE,
            offset: (page - 1) * PAGE_SIZE,
        },
    )
    .await?;
    let total = count_codes(db, kind.as_deref()).await?;

    Ok(view! {
        page_header(
            title: "Promotions",
            <form method="get" class="flex items-center gap-2 text-sm">
                <label for="kind" class="text-muted-foreground">"Type"</label>
                select(
                    attrs: topcoat::view::attributes! { id="kind" name="kind" },
                    <option value="" selected=(kind.is_none())>"Tous"</option>
                    <option value=(DISCOUNT) selected=(kind.as_deref() == Some(DISCOUNT))>"Codes promo"</option>
                    <option value=(VOUCHER) selected=(kind.as_deref() == Some(VOUCHER))>"Bons d'achat"</option>
                )
                <button type="submit" class="h-9 rounded-lg border border-border px-3">"Filtrer"</button>
            </form>
            <a href=(href!(new_discount)) class=(button_variants(ButtonVariant::Primary, Default::default()))>"Nouveau code promo"</a>
            <a href=(href!(new_voucher)) class=(button_variants(ButtonVariant::Outline, Default::default()))>"Nouveau bon d'achat"</a>
        )
        if rows.is_empty() {
            empty_state(message: "Aucun code.")
        } else {
            table(
                table_header(table_row(
                    table_head("Code") table_head("Type") table_head("Valeur") table_head("Créé le") table_head("État")
                ))
                table_body(
                    for row in &rows { code_row(row: row) }
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

/// `10 %`, `12,50 %` or the amount of a fixed code / a voucher.
pub fn code_value(percent_bp: Option<i64>, amount: Option<Money>) -> String {
    match (percent_bp, amount) {
        (Some(bp), _) if bp % 100 == 0 => format!("{} %", bp / 100),
        (Some(bp), _) => format!("{},{:02} %", bp / 100, bp % 100),
        (None, Some(amount)) => money(&amount),
        (None, None) => String::new(),
    }
}

#[topcoat::view::component]
async fn code_row(cx: &Cx, row: &CodeListRow) -> Result<impl View> {
    let link = href!(code_id::show, code_id::CodeId(row.id.clone())).resolve(cx);
    let amount = row
        .amount_minor
        .zip(row.currency.as_ref())
        .map(|(minor, currency)| Money::new(minor, currency));
    let value = code_value(row.percent_bp, amount);
    let kind = if row.kind == VOUCHER {
        "Bon d'achat"
    } else {
        "Code promo"
    };
    Ok(view! {
        table_row(
            table_cell(<a href=(link) class="font-mono text-xs underline-offset-4 hover:underline">(row.code.clone())</a>)
            table_cell((kind))
            table_cell(attrs: topcoat::view::attributes! { class="tabular-nums" }, (value))
            table_cell((date(row.created_at as u64)))
            table_cell(if row.active { "Actif" } else { "Inactif" })
        )
    })
}

/// An optional number typed in a text box: blank is `None`.
fn optional_number(raw: &str, label: &str) -> std::result::Result<Option<u32>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    raw.parse()
        .map(Some)
        .map_err(|_| format!("{label} : nombre entier attendu."))
}

/// `days` from now, as Unix seconds.
fn days_from_now(days: Option<u32>) -> anyhow::Result<Option<u64>> {
    let Some(days) = days else { return Ok(None) };
    Ok(Some(
        timada_core::time::now_unix_secs()? + u64::from(days) * 86_400,
    ))
}

#[derive(Debug, Deserialize)]
pub struct NewDiscountForm {
    code: String,
    kind: String,
    value: u32,
    /// Of a fixed amount; a percentage works in every currency.
    currency: Option<String>,
    max_redemptions: String,
    valid_days: String,
}

#[page("./new-discount")]
pub async fn new_discount() -> Result<impl View> {
    Ok(view! { new_discount_form(error: None) })
}

#[page(POST "./new-discount")]
pub async fn create_discount(cx: &Cx, Form(form): Form<NewDiscountForm>) -> Result<impl View> {
    let error = match save_discount(cx, form).await? {
        Ok(id) => {
            let show = href!(code_id::show, code_id::CodeId(id)).resolve(cx);
            return Err(see_other(show).into());
        }
        Err(error) => error,
    };
    Ok(view! { new_discount_form(error: Some(error)) })
}

/// The currency a form asks for — the base one when it says nothing — if the
/// shop sells in it. An amount belongs to its currency: it is never
/// converted, and is refused on a cart in another one.
fn sold_currency(cx: &Cx, asked: Option<&str>) -> Option<String> {
    let currencies = &app_context::<AdminConfig>(cx).currencies;
    match asked.map(str::trim).filter(|code| !code.is_empty()) {
        Some(code) => currencies.sells_in(code).then(|| code.to_owned()),
        None => Some(currencies.base().to_owned()),
    }
}

/// The shop's currencies as a select — nothing when it has only one.
#[topcoat::view::component]
async fn currency_select(cx: &Cx, hint: &str) -> Result<impl View> {
    let currencies = &app_context::<AdminConfig>(cx).currencies;
    let codes: Vec<String> = if currencies.others().is_empty() {
        Vec::new()
    } else {
        currencies.all().map(str::to_owned).collect()
    };
    Ok(view! {
        if !codes.is_empty() {
            <div class="flex flex-col gap-1.5">
                <label for="currency" class="text-sm font-medium">"Devise"</label>
                select(
                    attrs: topcoat::view::attributes! { id="currency" name="currency" },
                    for code in &codes { <option value=(code.clone())>(code.clone())</option> }
                )
                <p class="text-xs text-muted-foreground">(hint.to_owned())</p>
            </div>
        }
    })
}

async fn save_discount(
    cx: &Cx,
    form: NewDiscountForm,
) -> Result<std::result::Result<String, String>> {
    let kind = match form.kind.as_str() {
        "percent" => match u16::try_from(form.value) {
            Ok(bp) => DiscountKind::Percent { bp },
            Err(_) => return Ok(Err("Pourcentage trop élevé.".into())),
        },
        "fixed" => match sold_currency(cx, form.currency.as_deref()) {
            Some(currency) => DiscountKind::FixedAmount {
                amount: Money::new(i64::from(form.value), currency),
            },
            None => return Ok(Err("La boutique ne vend pas dans cette devise.".into())),
        },
        _ => return Ok(Err("Choisissez un type de remise.".into())),
    };
    let max_redemptions = match optional_number(&form.max_redemptions, "Utilisations maximum") {
        Ok(max) => max,
        Err(error) => return Ok(Err(error)),
    };
    let valid_days = match optional_number(&form.valid_days, "Durée de validité") {
        Ok(days) => days,
        Err(error) => return Ok(Err(error)),
    };

    let services = app_context::<AdminServices>(cx);
    let created = timada_promotion::Command {
        executor: &services.executor,
        db: services.db.clone(),
    }
    .create_discount(CreateDiscount {
        code: form.code,
        kind,
        max_redemptions,
        valid_until: days_from_now(valid_days)?,
    })
    .await;
    Ok(created.map_err(|err| err.to_string()))
}

#[topcoat::view::component]
async fn new_discount_form(cx: &Cx, error: Option<String>) -> Result<impl View> {
    Ok(view! {
        page_header(title: "Nouveau code promo")
        <div class="max-w-2xl">
            card(card_content(
                <form method="post" action=(href!(create_discount).resolve(cx)) class="grid gap-4 sm:grid-cols-2">
                    field(name: "code", label_text: "Code", attrs: topcoat::view::attributes! { required=(true) autocomplete="off" })
                    <div class="flex flex-col gap-1.5">
                        <label for="kind" class="text-sm font-medium">"Type de remise"</label>
                        select(
                            attrs: topcoat::view::attributes! { id="kind" name="kind" required=(true) },
                            <option value="percent">"Pourcentage (points de base, 1000 = 10 %)"</option>
                            <option value="fixed">"Montant fixe (centimes)"</option>
                        )
                    </div>
                    field(name: "value", label_text: "Valeur", attrs: topcoat::view::attributes! { type="number" min="1" required=(true) })
                    currency_select(hint: "D'un montant fixe : il ne vaut que sur un panier dans cette devise. Un pourcentage vaut partout.")
                    field(name: "max_redemptions", label_text: "Utilisations maximum (vide = illimité)", attrs: topcoat::view::attributes! { type="number" min="1" })
                    field(name: "valid_days", label_text: "Durée de validité en jours (vide = sans limite)", attrs: topcoat::view::attributes! { type="number" min="1" })
                    if let Some(error) = &error {
                        <p role="alert" class="text-sm text-destructive sm:col-span-2">(error.clone())</p>
                    }
                    <div class="sm:col-span-2">
                        button(attrs: topcoat::view::attributes! { type="submit" }, "Créer")
                    </div>
                </form>
            ))
        </div>
    })
}

#[derive(Debug, Deserialize)]
pub struct NewVoucherForm {
    code: String,
    value_cents: i64,
    currency: Option<String>,
    customer_id: String,
    valid_days: String,
}

#[page("./new-voucher")]
pub async fn new_voucher() -> Result<impl View> {
    Ok(view! { new_voucher_form(error: None) })
}

#[page(POST "./new-voucher")]
pub async fn create_voucher(cx: &Cx, Form(form): Form<NewVoucherForm>) -> Result<impl View> {
    let error = match save_voucher(cx, form).await? {
        Ok(id) => {
            let show = href!(code_id::show, code_id::CodeId(id)).resolve(cx);
            return Err(see_other(show).into());
        }
        Err(error) => error,
    };
    Ok(view! { new_voucher_form(error: Some(error)) })
}

async fn save_voucher(
    cx: &Cx,
    form: NewVoucherForm,
) -> Result<std::result::Result<String, String>> {
    let valid_days = match optional_number(&form.valid_days, "Durée de validité") {
        Ok(days) => days,
        Err(error) => return Ok(Err(error)),
    };
    let customer_id = Some(form.customer_id.trim().to_owned()).filter(|id| !id.is_empty());
    let Some(currency) = sold_currency(cx, form.currency.as_deref()) else {
        return Ok(Err("La boutique ne vend pas dans cette devise.".into()));
    };

    let services = app_context::<AdminServices>(cx);
    let issued = timada_promotion::Command {
        executor: &services.executor,
        db: services.db.clone(),
    }
    .issue_voucher(IssueVoucher {
        code: form.code,
        customer_id,
        value: Money::new(form.value_cents, currency),
        kind: VoucherKind::GiftVoucher,
        expires_at: days_from_now(valid_days)?,
    })
    .await;
    Ok(issued.map_err(|err| err.to_string()))
}

#[topcoat::view::component]
async fn new_voucher_form(cx: &Cx, error: Option<String>) -> Result<impl View> {
    Ok(view! {
        page_header(title: "Nouveau bon d'achat")
        <div class="max-w-2xl">
            card(card_content(
                <form method="post" action=(href!(create_voucher).resolve(cx)) class="grid gap-4 sm:grid-cols-2">
                    field(name: "code", label_text: "Code", attrs: topcoat::view::attributes! { required=(true) autocomplete="off" })
                    field(name: "value_cents", label_text: "Valeur (centimes)", attrs: topcoat::view::attributes! { type="number" min="1" required=(true) })
                    currency_select(hint: "Le bon vaut dans cette devise et ne se dépense que sur un panier qui y est.")
                    field(name: "customer_id", label_text: "Client (identifiant, vide = au porteur)", attrs: topcoat::view::attributes! {})
                    field(name: "valid_days", label_text: "Durée de validité en jours (vide = sans limite)", attrs: topcoat::view::attributes! { type="number" min="1" })
                    if let Some(error) = &error {
                        <p role="alert" class="text-sm text-destructive sm:col-span-2">(error.clone())</p>
                    }
                    <div class="sm:col-span-2">
                        button(attrs: topcoat::view::attributes! { type="submit" }, "Émettre")
                    </div>
                </form>
            ))
        </div>
    })
}
