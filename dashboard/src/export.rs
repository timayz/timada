//! A shop's orders, customers and newsletter subscribers as CSV files, for
//! an accountant, a sourcing agent or another tool.
//!
//! Each file is written behind the response, a page of rows at a time read
//! off a key of the read model, so a shop of any size is exported without
//! being held in memory (as its feeds are, see `crate::feed`). The rows are
//! the read models', a moment behind the store.
//!
//! A cell that a shopper typed (a name, an address, a note) is written so a
//! spreadsheet takes it for text: one that starts like a formula gets an
//! apostrophe before it.

use bytes::Bytes;
use jiff::{Timestamp, civil::Date, tz::TimeZone};
use timada_core::format_amount;
use timada_core::page::Cursor;
use timada_invoicing::{OrderInvoice, invoice_of_order};
use timada_newsletter::subscribers_page;
use timada_orders::{
    ExportDestination, ExportEvery, ExportKind, OrderState, Period, orders_placed_over,
};
use timada_shoppers::shoppers_page;
use tokio::sync::mpsc;
use topcoat::{
    Result,
    context::Cx,
    router::Body,
    view::{View, attributes, component, view},
};

use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::input::input;
use crate::components::select::select;
use crate::db::Db;
use crate::feed::{Broken, Pages};
use crate::ui::section_card;

/// How many rows of a read model one read takes.
const PAGE: i64 = 200;
/// What a spreadsheet opens as UTF-8 only when it leads the file.
const BOM: &str = "\u{feff}";

/// The days an export covers, as its address says them.
#[topcoat::router::query_params(error = bad_request)]
pub struct ExportQuery {
    /// The first and the last day, `YYYY-MM-DD`, both included; without
    /// both, everything.
    pub from: Option<String>,
    pub to: Option<String>,
}

/// What an export covers: the instants, and the part of the file's name
/// that says so (`-2026-10-01-2026-10-31`, or nothing for everything).
pub struct Span {
    pub period: Period,
    pub suffix: String,
}

impl Span {
    /// The days the query names, on the shop's clock, when both are dates
    /// in order; everything otherwise.
    pub fn of(query: &ExportQuery, zone: &TimeZone) -> Self {
        let date = |value: &Option<String>| value.as_deref()?.trim().parse::<Date>().ok();
        match (date(&query.from), date(&query.to)) {
            (Some(first), Some(last)) if first <= last => Self {
                period: crate::app::t::slug::sales::period_of_days(zone, first, last),
                suffix: format!("-{first}-{last}"),
            },
            _ => Self {
                period: Period {
                    from: 0,
                    to: i64::MAX,
                },
                suffix: String::new(),
            },
        }
    }
}

/// A cell someone outside the shop typed, made safe for a spreadsheet: one
/// that starts like a formula is led by an apostrophe, so it is shown, not
/// worked out.
pub fn typed(value: &str) -> String {
    if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.to_owned()
    }
}

/// Writes one line: the cells between commas, one that holds a comma, a
/// quote or a line break between quotes (its quotes doubled), and `\r\n`
/// after the last, as RFC 4180 has it.
pub fn csv_line<S: AsRef<str>>(out: &mut String, cells: &[S]) {
    for (n, cell) in cells.iter().enumerate() {
        if n > 0 {
            out.push(',');
        }
        let cell = cell.as_ref();
        if cell.contains([',', '"', '\n', '\r']) {
            out.push('"');
            out.push_str(&cell.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(cell);
        }
    }
    out.push_str("\r\n");
}

/// Unix seconds as a spreadsheet and a script both read them:
/// `2026-11-16T09:30:00Z`.
fn moment(at: i64) -> String {
    Timestamp::from_second(at).map_or_else(|_| String::new(), |at| at.to_string())
}

fn optional_moment(at: Option<i64>) -> String {
    at.map(moment).unwrap_or_default()
}

/// The columns of the orders file: one line per item of an order, what is
/// the order's own (its total, its shipping, its invoice) said again on
/// each.
pub const ORDER_COLUMNS: [&str; 30] = [
    "reference",
    "placed_at",
    "status",
    "email",
    "name",
    "address",
    "address_2",
    "postal_code",
    "city",
    "country",
    "phone",
    "currency",
    "product",
    "variant",
    "sku",
    "quantity",
    "unit_price",
    "amount",
    "tax_category",
    "discount_code",
    "discount",
    "shipping_method",
    "shipping",
    "order_total",
    "refunded",
    "invoice",
    "vat",
    "source",
    "first_source",
    "note",
];

/// The lines of the orders file one order makes: one per item. Amounts
/// are in the order's currency, VAT included, with two decimals. `invoice`
/// is its invoice, once it has one: `vat` is what that invoice carries.
pub fn order_lines(out: &mut String, order: &OrderState, invoice: Option<&OrderInvoice>) {
    let text = |value: Option<&str>| typed(value.unwrap_or_default());
    let amount = |cents: u64| format_amount(cents);
    for line in &order.lines {
        let cells: [String; 30] = [
            order.reference.clone(),
            moment(order.placed_at as i64),
            order.status.as_str().to_owned(),
            typed(&order.email),
            typed(&order.shipping.name),
            typed(&order.shipping.line1),
            text(order.shipping.line2.as_deref()),
            typed(&order.shipping.postal_code),
            typed(&order.shipping.city),
            typed(&order.shipping.country),
            text(order.shipping.phone.as_deref()),
            order.currency.clone(),
            typed(&line.name),
            text(line.variant_label.as_deref()),
            text(line.sku.as_deref()),
            line.quantity.to_string(),
            amount(line.unit_amount_cents),
            amount(line.amount_cents()),
            line.tax_category().to_owned(),
            text(order.discount.as_ref().map(|d| d.code.as_str())),
            amount(order.discount.as_ref().map_or(0, |d| d.amount_cents)),
            text(order.shipping_charge.as_ref().map(|s| s.name.as_str())),
            amount(order.shipping_charge.as_ref().map_or(0, |s| s.amount_cents)),
            amount(order.total_cents),
            amount(order.refunded_cents),
            invoice.map(|i| i.label.clone()).unwrap_or_default(),
            invoice
                .map(|i| amount(i.vat_cents.max(0) as u64))
                .unwrap_or_default(),
            typed(
                &order
                    .source
                    .as_ref()
                    .map(|s| s.channel())
                    .unwrap_or_default(),
            ),
            typed(
                &order
                    .source
                    .as_ref()
                    .map(|s| s.first.channel())
                    .unwrap_or_default(),
            ),
            text(order.note.as_deref()),
        ];
        csv_line(out, &cells);
    }
}

/// The opening of a file: the mark a spreadsheet wants, and the columns.
fn head(columns: &[&str]) -> String {
    let mut out = String::from(BOM);
    csv_line(&mut out, columns);
    out
}

/// The parts of a file, as they are written.
type Parts = mpsc::Receiver<std::result::Result<Bytes, Broken>>;

/// A file written behind its reader: `pages` gives each next part of it
/// from where the one before ended (`None` first), and `None` once there is
/// no more. A read that fails breaks the file off rather than end it early.
fn written<C, F, Fut>(what: &'static str, tenant_id: String, columns: &[&str], pages: F) -> Parts
where
    C: Send + 'static,
    F: Fn(Option<C>) -> Fut + Send + 'static,
    Fut: Future<Output = anyhow::Result<Option<(C, String)>>> + Send,
{
    let (parts, body) = mpsc::channel::<std::result::Result<Bytes, Broken>>(1);
    let head = head(columns);
    tokio::spawn(async move {
        if parts.send(Ok(head.into())).await.is_err() {
            return;
        }
        let mut after: Option<C> = None;
        loop {
            match pages(after.take()).await {
                Ok(Some((last, text))) => {
                    after = Some(last);
                    // The reader went away: nothing more to write.
                    if !text.is_empty() && parts.send(Ok(text.into())).await.is_err() {
                        return;
                    }
                }
                Ok(None) => return,
                Err(err) => {
                    tracing::error!(tenant = %tenant_id, what, %err, "an export broke off");
                    let _ = parts.send(Err(err.into())).await;
                    return;
                }
            }
        }
    });
    body
}

/// The file as the body of a response.
fn download(parts: Parts) -> Body {
    Body::new(Pages::of(parts))
}

/// The file of `kind` for `period`, whole, to post it somewhere: `None`
/// once it grows past `limit` bytes.
pub async fn file(
    db: Db,
    tenant_id: String,
    kind: ExportKind,
    period: Period,
    limit: usize,
) -> anyhow::Result<Option<Vec<u8>>> {
    let mut parts = match kind {
        ExportKind::Orders => orders_parts(db, tenant_id, period),
        ExportKind::Customers => customers_parts(db, tenant_id, period),
        ExportKind::Newsletter => subscribers_parts(db, tenant_id, period),
    };
    let mut file = Vec::new();
    while let Some(part) = parts.recv().await {
        let part = part.map_err(|err| anyhow::anyhow!("{err}"))?;
        if file.len() + part.len() > limit {
            return Ok(None);
        }
        file.extend_from_slice(&part);
    }
    Ok(Some(file))
}

/// The orders of a tenant placed over `period`, oldest first, one line per
/// item, each order with its invoice's number and VAT once it has one.
pub fn orders_csv(db: Db, tenant_id: String, period: Period) -> Body {
    download(orders_parts(db, tenant_id, period))
}

fn orders_parts(db: Db, tenant_id: String, period: Period) -> Parts {
    let tenant = tenant_id.clone();
    written(
        "orders",
        tenant_id,
        &ORDER_COLUMNS,
        move |after: Option<Cursor>| {
            let (db, tenant) = (db.clone(), tenant.clone());
            async move {
                let orders = orders_placed_over(&db, &tenant, period, after.as_ref(), PAGE).await?;
                let Some(last) = orders.last() else {
                    return Ok(None);
                };
                let last = Cursor {
                    at: last.placed_at as i64,
                    id: last.id.clone(),
                };
                let mut text = String::new();
                for order in &orders {
                    let invoice = invoice_of_order(&db, &tenant, &order.id).await?;
                    order_lines(&mut text, order, invoice.as_ref());
                }
                Ok(Some((last, text)))
            }
        },
    )
}

/// The columns of the customers file.
pub const CUSTOMER_COLUMNS: [&str; 4] = ["email", "customer_since", "orders", "saved_addresses"];

/// The shoppers of a tenant who first signed in over `period`.
pub fn customers_csv(db: Db, tenant_id: String, period: Period) -> Body {
    download(customers_parts(db, tenant_id, period))
}

fn customers_parts(db: Db, tenant_id: String, period: Period) -> Parts {
    let tenant = tenant_id.clone();
    written(
        "customers",
        tenant_id,
        &CUSTOMER_COLUMNS,
        move |after: Option<String>| {
            let (db, tenant) = (db.clone(), tenant.clone());
            async move {
                let rows = shoppers_page(
                    &db,
                    &tenant,
                    (period.from, period.to),
                    after.as_deref(),
                    PAGE,
                )
                .await?;
                let Some(last) = rows.last().map(|row| row.id.clone()) else {
                    return Ok(None);
                };
                let mut text = String::new();
                for row in &rows {
                    csv_line(
                        &mut text,
                        &[
                            typed(&row.email),
                            moment(row.registered_at),
                            row.order_count.to_string(),
                            row.address_count.to_string(),
                        ],
                    );
                }
                Ok(Some((last, text)))
            }
        },
    )
}

/// The columns of the subscribers file.
pub const SUBSCRIBER_COLUMNS: [&str; 8] = [
    "email",
    "status",
    "language",
    "time_zone",
    "asked_from",
    "asked_at",
    "confirmed_at",
    "unsubscribed_at",
];

/// The newsletter subscribers of a tenant who last asked over `period`,
/// whatever their status: the file says it, and only the confirmed ones
/// agreed to be written to.
pub fn subscribers_csv(db: Db, tenant_id: String, period: Period) -> Body {
    download(subscribers_parts(db, tenant_id, period))
}

fn subscribers_parts(db: Db, tenant_id: String, period: Period) -> Parts {
    let tenant = tenant_id.clone();
    written(
        "subscribers",
        tenant_id,
        &SUBSCRIBER_COLUMNS,
        move |after: Option<String>| {
            let (db, tenant) = (db.clone(), tenant.clone());
            async move {
                let rows = subscribers_page(
                    &db,
                    &tenant,
                    (period.from, period.to),
                    after.as_deref(),
                    PAGE,
                )
                .await?;
                let Some(last) = rows.last().map(|row| row.id.clone()) else {
                    return Ok(None);
                };
                let mut text = String::new();
                for row in &rows {
                    csv_line(
                        &mut text,
                        &[
                            typed(&row.email),
                            row.status.clone(),
                            row.language.clone().unwrap_or_default(),
                            row.time_zone.clone().unwrap_or_default(),
                            typed(&row.source),
                            moment(row.requested_at),
                            optional_moment(row.confirmed_at),
                            optional_moment(row.unsubscribed_at),
                        ],
                    );
                }
                Ok(Some((last, text)))
            }
        },
    )
}

/// What a download is called: `orders-acme-2026-10-01-2026-10-31.csv`.
pub fn file_name(what: &str, slug: &str, span: &Span) -> String {
    format!("{what}-{slug}{}.csv", span.suffix)
}

/// The headers of a download: a file to keep, that nothing in between
/// keeps.
pub fn download_headers(name: &str) -> [(topcoat::router::header::HeaderName, String); 3] {
    use topcoat::router::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
    [
        (CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
        (
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        ),
        (CACHE_CONTROL, "private, no-store".to_owned()),
    ]
}

/// The card a list page offers its export from: two optional days and a
/// button. `action` is the export's address, `what` what the days are of
/// ("placed", "signed in", "asked").
#[component]
pub async fn export_card(cx: &Cx, action: String, description: &str) -> Result<impl View> {
    Ok(view! {
        section_card(
            title: Some("Export"),
            description: Some(description),
            <form method="get" action=(&action) class="flex flex-wrap items-end gap-2" data-export="">
                <div class="flex flex-col gap-1">
                    <label class="text-sm" for="export-from">"First day"</label>
                    input(attrs: attributes! { id="export-from" name="from" type="date" class="w-44" })
                </div>
                <div class="flex flex-col gap-1">
                    <label class="text-sm" for="export-to">"Last day"</label>
                    input(attrs: attributes! { id="export-to" name="to" type="date" class="w-44" })
                </div>
                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-9" }, "Export as CSV")
            </form>
            <p class="mt-2 text-sm text-muted-foreground">"Leave both days empty for everything. The file opens in a spreadsheet; its times are in UTC."</p>
        )
    })
}

/// A shop's scheduled export of one kind, as its page shows it.
pub struct ScheduleView {
    /// `orders`, `customers` or `newsletter`.
    pub what: &'static str,
    /// The member looking is an owner of the shop: they alone set it.
    pub owner: bool,
    /// `daily`, `weekly` or `monthly`; `None`: nothing scheduled.
    pub every: Option<&'static str>,
    /// The address told by mail, when it goes by mail.
    pub mail: String,
    /// Whether the file is posted to an address.
    pub posted: bool,
    /// That address, opened for an owner.
    pub url: String,
    /// The secret its files are signed under, for an owner.
    pub secret: Option<String>,
    /// How the last export went, or that one is being tried again.
    pub last: Option<String>,
    /// Whether that was a failure.
    pub failed: bool,
    /// Whether an address can be kept on this installation.
    pub can_post: bool,
}

/// The schedule of `kind` as the member of `access` may see it, read from
/// the tenant's own stream, so a change shows at once.
pub async fn schedule_view(
    cx: &Cx,
    access: &crate::app::t::slug::TenantAccess,
    kind: ExportKind,
) -> Result<ScheduleView> {
    use topcoat::context::context;

    let crate::app::WebhookSealing(keys, _) = context::<crate::app::WebhookSealing>(cx);
    let owner = access.is_owner();
    let mut view = ScheduleView {
        what: kind.key(),
        owner,
        every: None,
        mail: String::new(),
        posted: false,
        url: String::new(),
        secret: None,
        last: None,
        failed: false,
        can_post: keys.is_some(),
    };
    let state = crate::app::t::slug::shipping::orders(access)?
        .load_export_schedule(kind)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let Some(state) = state else {
        return Ok(view);
    };
    let Some(plan) = &state.plan else {
        return Ok(view);
    };
    view.every = Some(plan.every.key());
    match &plan.destination {
        ExportDestination::Mail(address) => view.mail = address.clone(),
        ExportDestination::Url { sealed } => {
            view.posted = true;
            if let (true, Some(keys)) = (owner, keys) {
                view.url =
                    crate::scheduled_exports::open_address(keys, access.tenant_id(), kind, sealed)
                        .unwrap_or_default();
                view.secret = Some(crate::scheduled_exports::signing_secret(
                    keys,
                    access.tenant_id(),
                    kind,
                ));
            }
        }
    }
    let days = |first: Date, last: Date| {
        if first == last {
            first.to_string()
        } else {
            format!("{first} to {last}")
        }
    };
    if let Some(run) = &state.run {
        view.last = Some(format!(
            "The export of {} is being sent (try {}).",
            days(run.first_day, run.last_day),
            run.attempt
        ));
    } else if let Some(ended) = &state.ended {
        match &ended.failure {
            Some(reason) => {
                view.failed = true;
                view.last = Some(format!(
                    "The export of {} was not taken: {reason} Export these days by hand above.",
                    days(ended.first_day, ended.last_day)
                ));
            }
            None => {
                view.last = Some(format!(
                    "Last sent: {}.",
                    days(ended.first_day, ended.last_day)
                ));
            }
        }
    }
    Ok(view)
}

/// What a schedule's rhythm is called.
fn every_label(every: &str) -> &'static str {
    match ExportEvery::parse(every) {
        Some(ExportEvery::Daily) => "Every day, for the day before",
        Some(ExportEvery::Weekly) => "Every Monday, for the week before",
        Some(ExportEvery::Monthly) | None => "Every first of the month, for the month before",
    }
}

/// The card a list page shows its scheduled export in. An owner sets it
/// there; another member sees that there is one, and where it goes, and
/// nothing while there is none.
#[component]
pub async fn schedule_card(cx: &Cx, action: String, schedule: &ScheduleView) -> Result<impl View> {
    let scheduled = schedule.every.is_some();
    let state = match (scheduled, schedule.posted) {
        (false, _) => "none",
        (true, false) => "mail",
        (true, true) => "url",
    };
    let every = schedule.every.unwrap_or("weekly");
    Ok(view! {
        if schedule.owner || scheduled {
            section_card(
                title: Some("Scheduled export"),
                description: Some("The same file on its own, for the days since the one before. Only an owner of the shop sets it."),
                <div class="flex flex-col gap-3" data-export-schedule=(state)>
                    if scheduled {
                        <p class="text-sm" data-export-schedule-summary="">
                            (every_label(every))
                            if schedule.posted {
                                ", posted to an address."
                            } else {
                                ", told by mail to " <strong>(&schedule.mail)</strong> "."
                            }
                        </p>
                    }
                    if let Some(last) = &schedule.last {
                        <p class=(if schedule.failed { "text-sm text-destructive" } else { "text-sm text-muted-foreground" }) data-export-schedule-last="">(last)</p>
                    }
                    if schedule.owner {
                        <form method="post" action=(&action) class="flex max-w-2xl flex-col gap-3">
                            <input type="hidden" name="what" value=(schedule.what)>
                            <div class="flex flex-col gap-1">
                                <label class="text-sm font-medium" for="export-every">"How often"</label>
                                select(
                                    attrs: attributes! { id="export-every" name="every" class="w-80" },
                                    for option in ExportEvery::ALL {
                                        <option value=(option.key()) selected=(option.key() == every)>(every_label(option.key()))</option>
                                    }
                                )
                            </div>
                            <fieldset class="flex flex-col gap-2">
                                <legend class="text-sm font-medium">"Where it goes"</legend>
                                <label class="flex items-center gap-2 text-sm"><input type="radio" name="to" value="mail" checked=(!schedule.posted) class="accent-primary"> " A mail that says it is ready"</label>
                                <label class="flex items-center gap-2 text-sm"><input type="radio" name="to" value="url" checked=(schedule.posted) disabled=(!schedule.can_post) class="accent-primary"> " The file, posted to an address"</label>
                            </fieldset>
                            <div class="flex flex-col gap-1">
                                <label class="text-sm font-medium" for="export-mail">"E-mail address to tell"</label>
                                input(attrs: attributes! { id="export-mail" name="mail" type="email" class="w-80" value=(&schedule.mail) aria-describedby="export-mail-hint" autocomplete="off" })
                                <p id="export-mail-hint" class="text-sm text-muted-foreground">"The mail carries a link, not the file: it opens for a member of the shop signed in, whose role reads these rows."</p>
                            </div>
                            <div class="flex flex-col gap-1">
                                <label class="text-sm font-medium" for="export-url">"Address to post the file to"</label>
                                input(attrs: attributes! { id="export-url" name="url" type="url" class="w-full" value=(&schedule.url) placeholder="https://" aria-describedby="export-url-hint" autocomplete="off" disabled=(!schedule.can_post) })
                                <p id="export-url-hint" class="text-sm text-muted-foreground">
                                    if schedule.can_post {
                                        "An https address on the internet. The CSV file is posted as the request's body, signed in its " <code>"Timada-Signature"</code> " header, with an " <code>"Idempotency-Key"</code> " that is the same when a delivery is tried again."
                                    } else {
                                        "Not available on this installation: its operator has not set the key the addresses are kept under."
                                    }
                                </p>
                            </div>
                            if let Some(secret) = &schedule.secret {
                                <p class="text-sm">"Signing secret: " <code data-export-secret="">(secret)</code></p>
                            }
                            <div class="flex flex-wrap gap-2">
                                button(size: ButtonSize::Sm, attrs: attributes! { type="submit" name="action" value="save" class="h-9" }, "Save the schedule")
                                if scheduled {
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" name="action" value="remove" class="h-9" formnovalidate="" }, "Take it off")
                                }
                            </div>
                        </form>
                    }
                </div>
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_quoted_where_it_has_to_be() {
        let mut out = String::new();
        csv_line(
            &mut out,
            &["plain", "with, comma", "say \"hi\"", "two\nlines", ""],
        );
        assert_eq!(
            out,
            "plain,\"with, comma\",\"say \"\"hi\"\"\",\"two\nlines\",\r\n"
        );
    }

    #[test]
    fn what_a_shopper_typed_is_never_a_formula() {
        assert_eq!(
            typed("=HYPERLINK(\"http://x\")"),
            "'=HYPERLINK(\"http://x\")"
        );
        assert_eq!(typed("+33 6 12 34 56 78"), "'+33 6 12 34 56 78");
        assert_eq!(typed("-1"), "'-1");
        assert_eq!(typed("@ada"), "'@ada");
        assert_eq!(typed("Ada Lovelace"), "Ada Lovelace");
        assert_eq!(typed(""), "");
    }

    #[test]
    fn an_export_covers_the_days_named_or_everything() {
        let paris = TimeZone::get("Europe/Paris").unwrap();
        let span = Span::of(
            &ExportQuery {
                from: Some("2026-10-01".into()),
                to: Some("2026-10-31".into()),
            },
            &paris,
        );
        assert_eq!(span.suffix, "-2026-10-01-2026-10-31");
        assert_eq!(moment(span.period.from), "2026-09-30T22:00:00Z");
        assert_eq!(moment(span.period.to), "2026-10-31T23:00:00Z");
        assert_eq!(
            file_name("orders", "acme", &span),
            "orders-acme-2026-10-01-2026-10-31.csv"
        );
        for (from, to) in [
            (None, None),
            (Some("2026-10-01"), None),
            (Some("2026-10-31"), Some("2026-10-01")),
            (Some("soon"), Some("later")),
        ] {
            let span = Span::of(
                &ExportQuery {
                    from: from.map(str::to_owned),
                    to: to.map(str::to_owned),
                },
                &paris,
            );
            assert_eq!(span.period.from, 0);
            assert_eq!(span.period.to, i64::MAX);
            assert_eq!(file_name("orders", "acme", &span), "orders-acme.csv");
        }
    }

    #[test]
    fn an_order_is_one_line_per_item() {
        use timada_orders::{OrderLine, OrderStatus, ShippingAddress};

        let order = OrderState {
            reference: "K7Q4-MZP2".into(),
            placed_at: 1_794_000_000,
            status: OrderStatus::Paid,
            email: "ada@example.org".into(),
            shipping: ShippingAddress {
                name: "=cmd|' /C calc'!A0".into(),
                line1: "1, rue de la Paix".into(),
                postal_code: "75002".into(),
                city: "Paris".into(),
                country: "FR".into(),
                ..Default::default()
            },
            currency: "EUR".into(),
            total_cents: 3_400,
            refunded_cents: 500,
            lines: vec![
                OrderLine {
                    name: "Mug".into(),
                    unit_amount_cents: 1_250,
                    quantity: 2,
                    ..Default::default()
                },
                OrderLine {
                    name: "Mat".into(),
                    variant_label: Some("Sable".into()),
                    sku: Some("MAT-B".into()),
                    unit_amount_cents: 900,
                    quantity: 1,
                    tax_category: "books".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let invoice = OrderInvoice {
            label: "F2026-000042".into(),
            vat_cents: 464,
        };
        let mut out = String::new();
        order_lines(&mut out, &order, Some(&invoice));
        let lines: Vec<&str> = out.split("\r\n").collect();
        assert_eq!(lines.len(), 3, "two items and the end of the last");
        assert_eq!(
            lines[0],
            "K7Q4-MZP2,2026-11-06T21:20:00Z,paid,ada@example.org,'=cmd|' /C calc'!A0,\
             \"1, rue de la Paix\",,75002,Paris,FR,,EUR,Mug,,,2,12.50,25.00,standard,,0.00,,0.00,\
             34.00,5.00,F2026-000042,4.64,,,"
        );
        assert!(lines[1].contains(",Mat,Sable,MAT-B,1,9.00,9.00,books,"));
        // As many cells as columns: one comma more, the one in the address.
        let mut columns = String::new();
        csv_line(&mut columns, &ORDER_COLUMNS);
        assert_eq!(
            columns.matches(',').count() + 1,
            lines[1].matches(',').count()
        );
        // Without an invoice yet, its two cells are empty.
        let mut out = String::new();
        order_lines(&mut out, &order, None);
        assert!(out.starts_with("K7Q4-MZP2,"));
        assert!(out.contains(",34.00,5.00,,,,,\r\n"));
    }
}
