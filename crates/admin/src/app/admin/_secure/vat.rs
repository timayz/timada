//! `/{mount}/vat`: the VAT of a quarter, as three returns read it — the
//! shop's own VAT, the one-stop-shop return by member state and rate (with
//! the corrections of earlier quarters) and exports — and the one-stop-shop
//! return as a CSV file. Read from the invoices issued and the credit notes;
//! quarters are UTC.

use timada_core::Money;
use timada_invoice::{VatPeriod, VatReport, oss_csv, vat_report};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params, query_params as query},
    view::{View, component, view},
};

use crate::{
    components::{
        button::{ButtonVariant, button_variants},
        card::{card, card_content, card_header, card_title},
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{empty_state, money, page_header, vat_rate},
};

#[query_params(error = bad_request)]
struct VatQuery {
    /// `2026-T3`; the current quarter when absent or unreadable.
    periode: Option<String>,
}

fn asked_period(cx: &Cx) -> Result<VatPeriod> {
    let asked = query::<VatQuery>(cx)?
        .periode
        .as_deref()
        .and_then(VatPeriod::parse);
    Ok(match asked {
        Some(period) => period,
        None => VatPeriod::of(timada_core::time::now_unix_secs()?),
    })
}

/// A table row: country, rate, base, VAT — already worded.
type Line = (String, String, String, String);

fn lines(report: &VatReport, of: &[timada_invoice::VatReportLine]) -> Vec<Line> {
    of.iter()
        .map(|line| {
            (
                line.country_code.clone(),
                vat_rate(line.rate_bp),
                money(&Money::new(line.base_minor, &report.currency)),
                money(&Money::new(line.vat_minor, &report.currency)),
            )
        })
        .collect()
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let period = asked_period(cx)?;
    let db = &app_context::<AdminServices>(cx).db;
    let report = vat_report(db, period).await?;
    let amount = |minor: i64| money(&Money::new(minor, &report.currency));

    let link = |to: VatPeriod| href!(index).query([("periode", to.code())]).resolve(cx);
    let (previous, next) = (link(period.previous()), link(period.next()));
    let csv = href!(oss_file)
        .query([("periode", period.code())])
        .resolve(cx);
    let title = format!("TVA — {}", period.label());

    let domestic = lines(&report, &report.domestic);
    let oss = lines(&report, &report.oss);
    let corrections: Vec<(String, String, String)> = report
        .oss_corrections
        .iter()
        .map(|correction| {
            (
                correction.corrected.label(),
                correction.country_code.clone(),
                amount(correction.vat_minor),
            )
        })
        .collect();
    let empty = report.currency.is_empty();
    let (unbroken_count, unbroken_total) = (report.unbroken.0, amount(report.unbroken.1));
    // Everything worded before the view, which borrows nothing.
    let domestic_total = amount(report.domestic_vat_minor());
    let oss_sales_total = amount(report.oss.iter().map(|line| line.vat_minor).sum());
    let oss_total = amount(report.oss_vat_minor());
    let exports = amount(report.exports_base_minor);
    let intra_community: Vec<(String, String, String)> = report
        .intra_community
        .iter()
        .map(|line| {
            (
                line.country_code.clone(),
                line.buyer_vat_number.clone(),
                amount(line.base_minor),
            )
        })
        .collect();
    let intra_community_total = amount(
        report
            .intra_community
            .iter()
            .map(|line| line.base_minor)
            .sum(),
    );

    Ok(view! {
        page_header(
            title: &title,
            <a href=(previous) class=(button_variants(ButtonVariant::Outline, Default::default())) rel="prev">"← Trimestre précédent"</a>
            <a href=(next) class=(button_variants(ButtonVariant::Outline, Default::default())) rel="next">"Trimestre suivant →"</a>
        )
        <p class="-mt-4 mb-6 text-sm text-muted-foreground">
            "D'après les factures émises et les avoirs du trimestre (dates UTC). Un avoir sur une facture d'un trimestre antérieur corrige ce trimestre-là dans la déclaration du guichet unique."
        </p>
        if empty {
            empty_state(message: "Aucune facture émise sur ce trimestre.")
        } else {
            <div class="flex flex-col gap-6">
                card(
                    card_header(card_title("TVA française"))
                    card_content(
                        vat_table(rows: &domestic, total: domestic_total, with_country: false)
                    )
                )
                card(
                    card_header(card_title("Guichet unique (OSS) — ventes à distance dans l'Union"))
                    card_content(
                        vat_table(rows: &oss, total: oss_sales_total, with_country: true)
                        if !corrections.is_empty() {
                            <h3 class="mt-6 mb-2 text-sm font-medium">"Corrections de trimestres antérieurs"</h3>
                            table(
                                table_header(table_row(
                                    table_head("Trimestre corrigé") table_head("État membre")
                                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "TVA")
                                ))
                                table_body(
                                    for (corrected, country, vat) in &corrections {
                                        table_row(
                                            table_cell((corrected.clone()))
                                            table_cell((country.clone()))
                                            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (vat.clone()))
                                        )
                                    }
                                )
                            )
                        }
                        <p class="mt-4 text-sm">"TVA à déclarer au guichet unique : " <strong class="tabular-nums">(oss_total)</strong></p>
                        <p class="mt-4"><a href=(csv) class=(button_variants(ButtonVariant::Secondary, Default::default()))>"Télécharger la déclaration OSS (CSV)"</a></p>
                    )
                )
                card(
                    card_header(card_title("Livraisons intracommunautaires — entreprises d'autres États membres"))
                    card_content(
                        if intra_community.is_empty() {
                            <p class="text-sm text-muted-foreground">"Rien à déclarer."</p>
                        } else {
                            table(
                                table_header(table_row(
                                    table_head("État membre") table_head("N° TVA de l'acquéreur")
                                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Base HT")
                                ))
                                table_body(
                                    for (country, buyer, base) in &intra_community {
                                        table_row(
                                            table_cell((country.clone()))
                                            table_cell(<span class="font-mono text-xs">(buyer.clone())</span>)
                                            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (base.clone()))
                                        )
                                    }
                                )
                            )
                            <p class="mt-2 text-right text-sm">"Total HT : " <strong class="tabular-nums">(intra_community_total)</strong></p>
                            <p class="mt-2 text-sm text-muted-foreground">"Ventes exonérées, TVA autoliquidée par l'acquéreur : à reporter sur la déclaration de TVA et l'état récapitulatif des clients."</p>
                        }
                    )
                )
                card(
                    card_header(card_title("Exportations"))
                    card_content(
                        <p class="text-sm">"Ventes hors TVA hors de l'Union : " <strong class="tabular-nums">(exports)</strong> " HT"</p>
                    )
                )
                if report.unconverted > 0 {
                    <p role="alert" class="text-sm text-destructive">
                        (report.unconverted.to_string()) " document(s) en devise étrangère sans cours de change : ils ne sont pas dans ce rapport. Épinglez le cours sur leur commande pour les y faire entrer."
                    </p>
                }
                if unbroken_count > 0 {
                    <p role="alert" class="text-sm text-destructive">
                        (unbroken_count.to_string()) " facture(s) antérieure(s) aux zones fiscales n'ont pas de ventilation de TVA : " (unbroken_total) " TTC à ventiler à la main."
                    </p>
                }
            </div>
        }
    })
}

#[component]
async fn vat_table(rows: &[Line], total: String, with_country: bool) -> Result<impl View> {
    Ok(view! {
        if rows.is_empty() {
            <p class="text-sm text-muted-foreground">"Rien à déclarer."</p>
        } else {
            table(
                table_header(table_row(
                    if with_country { table_head("État membre") }
                    table_head("Taux")
                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Base HT")
                    table_head(attrs: topcoat::view::attributes! { class="text-right" }, "TVA")
                ))
                table_body(
                    for (country, rate, base, vat) in rows {
                        table_row(
                            if with_country { table_cell((country.clone())) }
                            table_cell((rate.clone()))
                            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (base.clone()))
                            table_cell(attrs: topcoat::view::attributes! { class="text-right tabular-nums" }, (vat.clone()))
                        )
                    }
                )
            )
            <p class="mt-2 text-right text-sm">"Total TVA : " <strong class="tabular-nums">(total)</strong></p>
        }
    })
}

pub struct CsvDownload {
    file_name: String,
    content: String,
}

impl topcoat::router::response::IntoResponse for CsvDownload {
    fn into_response(self, _cx: &Cx) -> Result<topcoat::router::response::Response> {
        Ok(topcoat::router::response::Response::builder()
            .header("Content-Type", "text/csv; charset=utf-8")
            .header(
                "Content-Disposition",
                format!("attachment; filename=\"{}\"", self.file_name),
            )
            .header("Cache-Control", "private, no-store")
            .body(topcoat::router::Body::from(self.content.into_bytes()))?)
    }
}

/// The one-stop-shop return of the quarter, for a spreadsheet or the
/// accountant: sales by member state and rate, then the corrections.
#[topcoat::router::route(GET "./oss.csv")]
pub async fn oss_file(cx: &Cx) -> Result<CsvDownload> {
    let period = asked_period(cx)?;
    let db = &app_context::<AdminServices>(cx).db;
    let report = vat_report(db, period).await?;
    Ok(CsvDownload {
        file_name: format!("oss-{}.csv", period.code()),
        content: oss_csv(period, &report),
    })
}
