//! `/companies/{company_id}/oss`: what the company declares on its quarterly
//! One-Stop Shop return: the VAT of other EU countries charged to their
//! consumers by every tenant of it, by country and rate, credit notes
//! deducted. From the shared read model.

use timada_invoicing::{format_amount_fr, format_rate_fr, oss_return, quarter_bounds, quarter_of};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::{CompanyId, require_owned};
use crate::components::button::{ButtonSize, ButtonVariant, button_variants};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{Shell, document, page_header, section_card};

#[topcoat::router::query_params(error = bad_request)]
struct OssQuery {
    /// `2026-Q4`; the current quarter when absent or unreadable.
    quarter: Option<String>,
}

#[derive(serde::Serialize)]
struct OssLink {
    quarter: String,
}

/// The quarter before or after `quarter`.
fn step(quarter: &str, forward: bool) -> String {
    let (from, to) = quarter_bounds(quarter).unwrap_or_default();
    let at = if forward { to } else { from - 1 };
    quarter_of(at.max(0) as u64)
}

#[page]
pub async fn oss(cx: &Cx) -> Result<impl View> {
    let (company, _) = require_owned(cx).await?;
    let query = query_params::<OssQuery>(cx)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let quarter = query
        .quarter
        .clone()
        .filter(|quarter| quarter_bounds(quarter).is_some())
        .unwrap_or_else(|| quarter_of(now));
    let (from, to) = quarter_bounds(&quarter).unwrap_or_default();
    let rows = oss_return(app_context::<Db>(cx), &company.id, from, to)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let id = company.id.clone();
    let link = |quarter: String| {
        href!(oss, CompanyId(id.clone()))
            .query(OssLink { quarter })
            .resolve(cx)
    };
    let previous = link(step(&quarter, false));
    let next = link(step(&quarter, true));
    let name = company.name().to_owned();
    let title = format!("One-Stop Shop {quarter} · {name}");

    Ok(view! {
        document(
            title: &title,
            shell: Shell::page("companies"),
            page_header(
                title: "One-Stop Shop return",
                subtitle: Some(&name),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(href!(super::index, CompanyId(company.id.clone())))>"Back to " (&name)</a>
            )
            section_card(
                description: Some("Sales of every tenant of the company to consumers in other EU countries, taxed at their rates, as invoiced in the quarter (UTC), credit notes deducted. Declare them on the OSS return of impots.gouv.fr."),
                <nav class="mb-4 flex flex-wrap items-center gap-3 text-sm" aria-label="Quarters">
                    <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(previous)>"Previous quarter"</a>
                    <strong class="px-2 text-base tabular-nums" data-oss-quarter="">(&quarter)</strong>
                    <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(next)>"Next quarter"</a>
                </nav>
                table(
                    table_header(
                        table_row(
                            table_head("Country")
                            table_head("Rate")
                            table_head(attrs: attributes! { class="text-right" }, "Taxable amount")
                            table_head(attrs: attributes! { class="text-right" }, "VAT")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            table_row(
                                table_cell(attrs: attributes! { colspan="4" class="text-muted-foreground" data-oss-empty="" }, "Nothing to declare this quarter.")
                            )
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-oss-row=(format!("{}-{}", row.country, row.rate_bps)) },
                                table_cell((&row.country))
                                table_cell((format_rate_fr(row.rate_bps.max(0) as u32)))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" }, (format_amount_fr(row.base_cents.max(0) as u64, &row.currency)))
                                table_cell(attrs: attributes! { class="text-right tabular-nums" data-oss-vat="" }, (format_amount_fr(row.vat_cents.max(0) as u64, &row.currency)))
                            )
                        }
                    )
                )
            )
        )
    })
}
