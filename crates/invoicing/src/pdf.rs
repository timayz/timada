//! A document drawn as PDF/A, from its event alone: the same document draws
//! the same PDF for as long as it is kept, so the PDF itself is never
//! stored. Values are written the French way here; the Typst template of
//! the document's layout version places them.

use std::sync::OnceLock;

use typst::foundations::{Array, Dict, IntoValue, Value};
use typst_as_lib::{TypstEngine, TypstTemplateMainFile};
use typst_pdf::{PdfOptions, PdfStandard, PdfStandards};

use crate::command::{DocumentKind, IssuedDocument};
use crate::value_object::{
    Address, FRANCHISE_MENTION, PAYMENT_METHOD_CARD, civil_date, format_amount_fr, format_date_fr,
    format_rate_fr,
};

/// Layout version 1 (see [`TEMPLATE_VERSION`](crate::TEMPLATE_VERSION)).
const TEMPLATE_V1: &str = include_str!("../templates/invoice.typ");

/// The engine of layout version 1, with its fonts loaded once.
fn engine_v1() -> &'static TypstEngine<TypstTemplateMainFile> {
    static ENGINE: OnceLock<TypstEngine<TypstTemplateMainFile>> = OnceLock::new();
    ENGINE.get_or_init(|| {
        TypstEngine::builder()
            .main_file(TEMPLATE_V1)
            .fonts(typst_assets::fonts())
            .build()
    })
}

/// The file name a browser saves it under: `F2026-000042.pdf`.
pub fn file_name(document: &IssuedDocument) -> String {
    format!("{}.pdf", document.label)
}

/// Draws a document as PDF/A-2b. Typst works synchronously and takes some
/// milliseconds: a server calls it off its async threads.
pub fn render(document: &IssuedDocument) -> anyhow::Result<Vec<u8>> {
    let engine = match document.template {
        1 => engine_v1(),
        other => anyhow::bail!("no layout version {other}"),
    };
    let compiled = engine
        .compile_with_input(inputs(document))
        .output
        .map_err(|err| anyhow::anyhow!("the document does not compile: {err:?}"))?;
    let options = PdfOptions {
        standards: PdfStandards::new(&[PdfStandard::A_2b])
            .map_err(|err| anyhow::anyhow!("{err:?}"))?,
        ..PdfOptions::default()
    };
    typst_pdf::pdf(&compiled, &options).map_err(|err| anyhow::anyhow!("no PDF: {err:?}"))
}

fn text(value: impl Into<String>) -> Value {
    value.into().into_value()
}

fn lines(values: impl IntoIterator<Item = String>) -> Value {
    Value::Array(values.into_iter().map(text).collect::<Array>())
}

/// The lines of an address: name, street, postal code and city, country.
fn address_lines(address: &Address) -> Vec<String> {
    let mut out = vec![address.name.clone(), address.line1.clone()];
    out.extend(address.line2.clone());
    out.push(format!("{} {}", address.postal_code, address.city));
    if address.country != "FR" {
        out.push(address.country.clone());
    }
    out
}

/// `SAS au capital de 10 000,00 €`, or the form alone.
fn form_and_capital(document: &IssuedDocument) -> String {
    let seller = &document.seller;
    match seller.share_capital_cents {
        Some(capital) => format!(
            "{} au capital de {}",
            seller.legal_form,
            format_amount_fr(capital, "EUR")
        ),
        None => seller.legal_form.clone(),
    }
}

fn inputs(document: &IssuedDocument) -> Dict {
    let money = |cents: u64| format_amount_fr(cents, &document.currency);
    let seller = &document.seller;
    let credit = document.kind == DocumentKind::CreditNote;
    let mut d = Dict::new();
    let mut put = |key: &str, value: Value| {
        d.insert(key.into(), value);
    };
    let kind_title = if credit { "AVOIR" } else { "FACTURE" };
    put(
        "title",
        text(format!(
            "{} {}",
            if credit { "Avoir" } else { "Facture" },
            document.label
        )),
    );
    put("kind_title", text(kind_title));
    put("label", text(&document.label));
    put("issued_on", text(format_date_fr(document.issued_at)));
    // The PDF's own date (PDF/A asks for one): the day of issue, never the
    // day it is drawn, so the file stays the same.
    let (year, month, day) = civil_date(document.issued_at);
    put("issued_year", Value::Int(year));
    put("issued_month", Value::Int(i64::from(month)));
    put("issued_day", Value::Int(i64::from(day)));
    put(
        "sale_on",
        text(document.paid_at.map(format_date_fr).unwrap_or_default()),
    );
    put(
        "corrects",
        text(document.invoice_label.clone().unwrap_or_default()),
    );
    put("order_reference", text(&document.order_reference));
    put("seller_name", text(&seller.address.name));
    put(
        "seller_address",
        lines(address_lines(&seller.address).into_iter().skip(1)),
    );
    put("seller_email", text(&seller.contact_email));
    put("seller_siret", text(&seller.siret));
    put(
        "seller_rcs",
        text(
            seller
                .rcs_city
                .as_ref()
                .map(|city| format!("RCS {city} {}", seller.siren()))
                .unwrap_or_default(),
        ),
    );
    put(
        "seller_vat",
        text(seller.vat_number.clone().unwrap_or_default()),
    );
    put("seller_form_capital", text(form_and_capital(document)));
    put("buyer_address", lines(address_lines(&document.buyer)));
    put("buyer_email", text(&document.buyer_email));
    put(
        "delivery_address",
        lines(
            document
                .delivery
                .as_ref()
                .map(address_lines)
                .unwrap_or_default(),
        ),
    );
    put("category", text("livraison de biens"));

    let rows: Vec<Value> = if credit {
        vec![
            Dict::from_iter([
                (
                    "name".into(),
                    text(format!(
                        "Remboursement sur la facture {}",
                        document.invoice_label.as_deref().unwrap_or_default()
                    )),
                ),
                (
                    "detail".into(),
                    text(format!("Commande {}", document.order_reference)),
                ),
                ("quantity".into(), text("1")),
                (
                    "unit_excl".into(),
                    text(money(document.total_excl_vat_cents)),
                ),
                (
                    "rate".into(),
                    text(if document.vat.is_empty() {
                        "—".to_owned()
                    } else {
                        "selon taux".to_owned()
                    }),
                ),
                ("total".into(), text(money(document.total_cents))),
            ])
            .into_value(),
        ]
    } else {
        document
            .lines
            .iter()
            .map(|line| {
                let detail = [
                    line.variant_label.clone(),
                    line.sku.as_ref().map(|sku| format!("Réf. {sku}")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                Dict::from_iter([
                    ("name".into(), text(&line.name)),
                    ("detail".into(), text(detail)),
                    ("quantity".into(), text(line.quantity.to_string())),
                    ("unit_excl".into(), text(money(line.unit_excl_vat_cents()))),
                    (
                        "rate".into(),
                        text(line.vat_rate_bps.map_or("—".to_owned(), format_rate_fr)),
                    ),
                    ("total".into(), text(money(line.total_cents()))),
                ])
                .into_value()
            })
            .collect()
    };
    put("lines", Value::Array(rows.into_iter().collect()));
    let vat: Vec<Value> = document
        .vat
        .iter()
        .map(|subtotal| {
            Dict::from_iter([
                ("rate".into(), text(format_rate_fr(subtotal.rate_bps))),
                ("base".into(), text(money(subtotal.base_cents))),
                ("vat".into(), text(money(subtotal.vat_cents))),
                ("total".into(), text(money(subtotal.total_cents()))),
            ])
            .into_value()
        })
        .collect();
    put("vat", Value::Array(vat.into_iter().collect()));
    put("total_excl", text(money(document.total_excl_vat_cents)));
    put("total_vat", text(money(document.total_vat_cents)));
    put("total", text(money(document.total_cents)));
    put(
        "total_title",
        text(if credit {
            "Total remboursé TTC"
        } else {
            "Total TTC"
        }),
    );
    put(
        "vat_mention",
        text(if document.is_vat_exempt() {
            FRANCHISE_MENTION
        } else {
            ""
        }),
    );
    let settlement = if credit {
        format!(
            "Montant remboursé le {} sur le moyen de paiement d'origine.",
            format_date_fr(document.issued_at)
        )
    } else {
        let method = match document.payment_method.as_deref() {
            Some(PAYMENT_METHOD_CARD) | None => "carte bancaire",
            Some(other) => other,
        };
        format!(
            "Facture acquittée : payée par {method} le {}.",
            format_date_fr(document.paid_at.unwrap_or(document.issued_at))
        )
    };
    put("settlement", text(settlement));
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value_object::{InvoiceLine, Seller, VatSubtotal, lines_breakdown};

    fn invoice(vat_number: Option<&str>) -> IssuedDocument {
        let rate = vat_number.map(|_| 2000);
        let lines = vec![
            InvoiceLine {
                name: "Mug bleu".into(),
                variant_label: Some("Grand".into()),
                sku: Some("MUG-L".into()),
                quantity: 2,
                unit_amount_cents: 1250,
                vat_rate_bps: rate,
            },
            InvoiceLine {
                name: "Livre".into(),
                quantity: 1,
                unit_amount_cents: 1999,
                vat_rate_bps: vat_number.map(|_| 550),
                ..Default::default()
            },
        ];
        let vat = lines_breakdown(&lines);
        let total_vat_cents: u64 = vat.iter().map(|v| v.vat_cents).sum();
        IssuedDocument {
            document_id: "doc".into(),
            kind: DocumentKind::Invoice,
            series: "F2026".into(),
            number: 42,
            label: "F2026-000042".into(),
            issued_at: 1_792_011_600,
            order_id: "order".into(),
            order_reference: "K7Q4-MZP2".into(),
            paid_at: Some(1_792_011_590),
            payment_method: Some(PAYMENT_METHOD_CARD.into()),
            seller: Seller {
                address: Address {
                    name: "Acme Boutique SAS".into(),
                    line1: "1 rue de la Paix".into(),
                    postal_code: "75002".into(),
                    city: "Paris".into(),
                    country: "FR".into(),
                    ..Default::default()
                },
                legal_form: "SAS".into(),
                share_capital_cents: Some(1_000_000),
                siret: "12002701600092".into(),
                rcs_city: Some("Paris".into()),
                contact_email: "hello@acme.example".into(),
                vat_number: vat_number.map(str::to_owned),
            },
            buyer: Address {
                name: "Ada Lovelace".into(),
                line1: "12 Baker Street".into(),
                postal_code: "NW1".into(),
                city: "London".into(),
                country: "GB".into(),
                ..Default::default()
            },
            buyer_email: "ada@example.org".into(),
            delivery: Some(Address {
                name: "Ada Lovelace".into(),
                line1: "12 Baker Street".into(),
                postal_code: "NW1".into(),
                city: "London".into(),
                country: "GB".into(),
                ..Default::default()
            }),
            currency: "EUR".into(),
            total_excl_vat_cents: 4499 - total_vat_cents,
            total_vat_cents,
            total_cents: 4499,
            lines,
            vat,
            template: 1,
            ..Default::default()
        }
    }

    fn get(d: &Dict, key: &str) -> String {
        match d.get(key).unwrap() {
            Value::Str(s) => s.to_string(),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn an_invoice_says_what_the_law_asks() {
        let d = inputs(&invoice(Some("FR85120027016")));
        assert_eq!(get(&d, "kind_title"), "FACTURE");
        assert_eq!(get(&d, "label"), "F2026-000042");
        assert_eq!(get(&d, "issued_on"), "14/10/2026");
        assert_eq!(get(&d, "sale_on"), "14/10/2026");
        assert_eq!(get(&d, "seller_siret"), "12002701600092");
        assert_eq!(get(&d, "seller_rcs"), "RCS Paris 120027016");
        assert_eq!(get(&d, "seller_vat"), "FR85120027016");
        assert_eq!(
            get(&d, "seller_form_capital"),
            "SAS au capital de 10\u{a0}000,00\u{a0}€"
        );
        assert_eq!(get(&d, "category"), "livraison de biens");
        assert_eq!(get(&d, "vat_mention"), "");
        assert_eq!(get(&d, "total"), "44,99\u{a0}€");
        assert!(
            get(&d, "settlement")
                .starts_with("Facture acquittée : payée par carte bancaire le 14/10/2026")
        );
        assert!(get(&d, "vat").contains("5,5"));
    }

    #[test]
    fn an_exempt_seller_says_so() {
        let d = inputs(&invoice(None));
        assert_eq!(get(&d, "vat_mention"), FRANCHISE_MENTION);
        assert_eq!(get(&d, "seller_vat"), "");
        assert_eq!(get(&d, "total_vat"), "0,00\u{a0}€");
    }

    #[test]
    fn a_document_draws_the_same_pdf_every_time() {
        let document = invoice(Some("FR85120027016"));
        let first = render(&document).unwrap();
        assert!(first.starts_with(b"%PDF-"));
        assert_eq!(render(&document).unwrap(), first);
        assert_eq!(file_name(&document), "F2026-000042.pdf");

        let credit = IssuedDocument {
            kind: DocumentKind::CreditNote,
            label: "A2026-000001".into(),
            invoice_label: Some("F2026-000042".into()),
            refund_id: Some("re_1".into()),
            lines: Vec::new(),
            delivery: None,
            paid_at: None,
            vat: vec![VatSubtotal {
                rate_bps: 2000,
                base_cents: 1000,
                vat_cents: 200,
            }],
            ..document
        };
        assert!(render(&credit).unwrap().starts_with(b"%PDF-"));
        if let Ok(dir) = std::env::var("INVOICE_PDF_DIR") {
            std::fs::write(format!("{dir}/invoice.pdf"), &first).unwrap();
            std::fs::write(format!("{dir}/credit.pdf"), render(&credit).unwrap()).unwrap();
            std::fs::write(
                format!("{dir}/franchise.pdf"),
                render(&invoice(None)).unwrap(),
            )
            .unwrap();
        }
    }
}
