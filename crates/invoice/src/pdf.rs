//! The invoice as a PDF file, for when the *server* needs the document — an
//! e-mail attachment, a download that does not depend on the customer's
//! browser. Same [`InvoiceDocument`] as the printable page, so both say the
//! same thing.
//!
//! The page is laid out first, as plain data (text runs and rules with their
//! coordinates), then drawn: the layout is what the tests look at. Text is
//! measured with the shaper the PDF writer uses, so a right-aligned amount
//! ends exactly on its column. Noto Sans (regular and bold, SIL Open Font
//! License, see `fonts/OFL.txt`) is bundled and subset into each file; the
//! same invoice always gives the same bytes.

use krilla::{
    Document,
    color::rgb,
    geom::{PathBuilder, Point},
    metadata::{DateTime, Metadata},
    num::NormalizedF32,
    page::PageSettings,
    paint::{Fill, Stroke},
    text::{Font, TextDirection},
};
use timada_core::{
    Address,
    format::{civil_date, date, money, vat_rate},
};

use crate::document::InvoiceDocument;

static REGULAR: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");
static BOLD: &[u8] = include_bytes!("../fonts/NotoSans-Bold.ttf");

// A4, in points.
const PAGE_WIDTH: f32 = 595.28;
const PAGE_HEIGHT: f32 = 841.89;
const MARGIN: f32 = 48.0;
const RIGHT: f32 = PAGE_WIDTH - MARGIN;
/// Nothing of the body goes below this line: the footer lives under it.
const BODY_BOTTOM: f32 = PAGE_HEIGHT - 84.0;

const BODY: f32 = 9.5;
const SMALL: f32 = 8.0;
const LEADING: f32 = 1.45;

#[derive(Debug, thiserror::Error)]
pub enum InvoicePdfError {
    #[error("the bundled invoice font cannot be read")]
    Font,
    #[error("the invoice PDF could not be written: {0}")]
    Write(String),
}

/// Renders an issued invoice as a PDF file (A4, as many pages as it takes).
pub fn render_invoice_pdf(document: &InvoiceDocument) -> Result<Vec<u8>, InvoicePdfError> {
    let faces = Faces::load()?;
    let pages = lay_out(&faces, document);
    draw(document, pages)
}

/// The file name to offer a download or an attachment under.
pub fn invoice_pdf_file_name(document: &InvoiceDocument) -> String {
    let number: String = document
        .number
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("facture-{number}.pdf")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Weight {
    Regular,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ink {
    Text,
    Muted,
    Rule,
    Strong,
}

impl Ink {
    fn color(self) -> rgb::Color {
        match self {
            Ink::Text | Ink::Strong => rgb::Color::new(17, 24, 39),
            Ink::Muted => rgb::Color::new(99, 108, 122),
            Ink::Rule => rgb::Color::new(209, 213, 219),
        }
    }
}

/// One thing drawn on a page.
#[derive(Debug, Clone, PartialEq)]
enum Mark {
    Text {
        x: f32,
        baseline: f32,
        weight: Weight,
        size: f32,
        ink: Ink,
        text: String,
    },
    Rule {
        x1: f32,
        x2: f32,
        y: f32,
        ink: Ink,
    },
}

struct Face {
    shaper: rustybuzz::Face<'static>,
}

impl Face {
    fn new(data: &'static [u8]) -> Result<Self, InvoicePdfError> {
        rustybuzz::Face::from_slice(data, 0)
            .map(|shaper| Self { shaper })
            .ok_or(InvoicePdfError::Font)
    }

    fn width(&self, text: &str, size: f32) -> f32 {
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let shaped = rustybuzz::shape(&self.shaper, &[], buffer);
        let advance: i32 = shaped.glyph_positions().iter().map(|p| p.x_advance).sum();
        advance as f32 * size / self.shaper.units_per_em() as f32
    }

    fn has(&self, ch: char) -> bool {
        self.shaper.glyph_index(ch).is_some()
    }
}

struct Faces {
    regular: Face,
    bold: Face,
}

impl Faces {
    fn load() -> Result<Self, InvoicePdfError> {
        Ok(Self {
            regular: Face::new(REGULAR)?,
            bold: Face::new(BOLD)?,
        })
    }

    fn of(&self, weight: Weight) -> &Face {
        match weight {
            Weight::Regular => &self.regular,
            Weight::Bold => &self.bold,
        }
    }

    /// Text the font can draw: line breaks and unknown spaces become spaces,
    /// anything else without a glyph a question mark rather than a blank box.
    fn printable(&self, weight: Weight, text: &str) -> String {
        let face = self.of(weight);
        text.chars()
            .map(|ch| match ch {
                ch if ch.is_control() => ' ',
                ch if face.has(ch) => ch,
                ch if ch.is_whitespace() => ' ',
                _ => '?',
            })
            .collect()
    }

    /// Greedy word wrap to `width`; a word wider than the column is cut.
    fn wrap(&self, weight: Weight, size: f32, text: &str, width: f32) -> Vec<String> {
        let face = self.of(weight);
        let text = self.printable(weight, text);
        let mut lines = Vec::new();
        let mut current = String::new();
        for word in text.split(' ').filter(|w| !w.is_empty()) {
            let candidate = if current.is_empty() {
                word.to_owned()
            } else {
                format!("{current} {word}")
            };
            if face.width(&candidate, size) <= width {
                current = candidate;
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            // The word alone: whole if it fits, else cut where it must.
            for ch in word.chars() {
                current.push(ch);
                if face.width(&current, size) > width && current.chars().count() > 1 {
                    let last = current.pop();
                    lines.push(std::mem::take(&mut current));
                    current.extend(last);
                }
            }
        }
        if !current.is_empty() || lines.is_empty() {
            lines.push(current);
        }
        lines
    }
}

/// A table column: where it sits and which way its cells lean.
#[derive(Debug, Clone, Copy)]
struct Column {
    left: f32,
    width: f32,
    numeric: bool,
}

struct Layout<'f> {
    faces: &'f Faces,
    pages: Vec<Vec<Mark>>,
    /// Top of the next thing laid out on the current page.
    y: f32,
}

impl<'f> Layout<'f> {
    fn new(faces: &'f Faces) -> Self {
        Self {
            faces,
            pages: vec![Vec::new()],
            y: MARGIN,
        }
    }

    fn push(&mut self, mark: Mark) {
        if let Some(page) = self.pages.last_mut() {
            page.push(mark);
        }
    }

    /// Starts a new page unless `height` still fits; tells whether it did.
    fn make_room(&mut self, height: f32) -> bool {
        if self.y + height <= BODY_BOTTOM {
            return false;
        }
        self.pages.push(Vec::new());
        self.y = MARGIN;
        true
    }

    fn text_at(&mut self, x: f32, top: f32, weight: Weight, size: f32, ink: Ink, text: &str) {
        let text = self.faces.printable(weight, text);
        if text.trim().is_empty() {
            return;
        }
        self.push(Mark::Text {
            x,
            baseline: top + size,
            weight,
            size,
            ink,
            text,
        });
    }

    fn text_ending_at(
        &mut self,
        right: f32,
        top: f32,
        weight: Weight,
        size: f32,
        ink: Ink,
        text: &str,
    ) {
        let text = self.faces.printable(weight, text);
        let width = self.faces.of(weight).width(&text, size);
        self.text_at(right - width, top, weight, size, ink, &text);
    }

    /// One line of text at the cursor, which moves below it.
    fn line(&mut self, x: f32, weight: Weight, size: f32, ink: Ink, text: &str) {
        self.make_room(size * LEADING);
        self.text_at(x, self.y, weight, size, ink, text);
        self.y += size * LEADING;
    }

    /// A wrapped paragraph over the full width.
    fn paragraph(&mut self, weight: Weight, size: f32, ink: Ink, text: &str) {
        for line in self.faces.wrap(weight, size, text, RIGHT - MARGIN) {
            self.line(MARGIN, weight, size, ink, &line);
        }
    }

    fn rule(&mut self, x1: f32, x2: f32, ink: Ink) {
        let y = self.y;
        self.push(Mark::Rule { x1, x2, y, ink });
    }

    fn caption(&mut self, text: &str) {
        self.make_room(SMALL * LEADING + 40.0);
        self.line(MARGIN, Weight::Regular, SMALL, Ink::Muted, text);
        self.y += 2.0;
    }

    fn table_header(&mut self, columns: &[Column], headings: &[&str]) {
        let top = self.y;
        for (column, heading) in columns.iter().zip(headings) {
            if column.numeric {
                let right = column.left + column.width;
                self.text_ending_at(right, top, Weight::Bold, SMALL, Ink::Text, heading);
            } else {
                self.text_at(column.left, top, Weight::Bold, SMALL, Ink::Text, heading);
            }
        }
        self.y += SMALL * LEADING + 3.0;
        self.rule(MARGIN, RIGHT, Ink::Strong);
        self.y += 4.0;
    }

    /// A table whose cells wrap inside their column. A row is never split
    /// over two pages, and the headings are repeated on each page.
    fn table(&mut self, columns: &[Column], headings: &[&str], rows: &[Vec<String>]) {
        self.make_room(SMALL * LEADING + 7.0 + BODY * LEADING + 8.0);
        self.table_header(columns, headings);
        for row in rows {
            let cells: Vec<Vec<String>> = columns
                .iter()
                .zip(row)
                .map(|(column, cell)| {
                    self.faces
                        .wrap(Weight::Regular, BODY, cell, column.width - 6.0)
                })
                .collect();
            let lines = cells.iter().map(Vec::len).max().unwrap_or(1);
            let height = lines as f32 * BODY * LEADING + 8.0;
            if self.make_room(height) {
                self.table_header(columns, headings);
            }
            let top = self.y;
            for (column, cell) in columns.iter().zip(&cells) {
                for (index, text) in cell.iter().enumerate() {
                    let line_top = top + index as f32 * BODY * LEADING;
                    if column.numeric {
                        let right = column.left + column.width;
                        self.text_ending_at(
                            right,
                            line_top,
                            Weight::Regular,
                            BODY,
                            Ink::Text,
                            text,
                        );
                    } else {
                        self.text_at(
                            column.left,
                            line_top,
                            Weight::Regular,
                            BODY,
                            Ink::Text,
                            text,
                        );
                    }
                }
            }
            self.y += height - 4.0;
            self.rule(MARGIN, RIGHT, Ink::Rule);
            self.y += 4.0;
        }
    }
}

fn address_lines(address: &Address) -> Vec<String> {
    let mut lines = vec![address.full_name(), address.line1.clone()];
    lines.extend(address.line2.clone());
    lines.push(format!("{} {}", address.postal_code, address.city));
    lines.push(address.country_code.clone());
    lines.retain(|line| !line.trim().is_empty());
    lines
}

/// Columns of a table spanning the body: the first takes what the numeric
/// ones, `widths` wide each from the right margin inwards, leave.
fn columns(text_columns: &[f32], numeric_widths: &[f32]) -> Vec<Column> {
    let numeric_total: f32 = numeric_widths.iter().sum();
    let text_total: f32 = text_columns.iter().sum();
    let available = RIGHT - MARGIN - numeric_total;
    let mut left = MARGIN;
    let mut all = Vec::with_capacity(text_columns.len() + numeric_widths.len());
    for share in text_columns {
        let width = available * share / text_total;
        all.push(Column {
            left,
            width,
            numeric: false,
        });
        left += width;
    }
    for width in numeric_widths {
        all.push(Column {
            left,
            width: *width,
            numeric: true,
        });
        left += width;
    }
    all
}

fn lay_out(faces: &Faces, document: &InvoiceDocument) -> Vec<Vec<Mark>> {
    let mut page = Layout::new(faces);
    let incl_vat = document.amounts_include_vat;

    // Who issues it, on the right; what it is, on the left.
    let issuer_left = 340.0;
    let top = page.y;
    page.text_at(
        MARGIN,
        top,
        Weight::Bold,
        18.0,
        Ink::Text,
        &format!("Facture {}", document.number),
    );
    page.y = top + 18.0 * LEADING + 4.0;
    page.line(
        MARGIN,
        Weight::Regular,
        BODY,
        Ink::Text,
        &format!("Date : {}", date(document.issued_at)),
    );
    page.line(
        MARGIN,
        Weight::Regular,
        BODY,
        Ink::Text,
        &format!("Commande : {}", document.order_label),
    );
    let left_bottom = page.y;

    page.y = top;
    let issuer_width = RIGHT - issuer_left;
    for line in faces.wrap(Weight::Bold, BODY, &document.issuer.name, issuer_width) {
        page.line(issuer_left, Weight::Bold, BODY, Ink::Text, &line);
    }
    let mut issuer = document.issuer.address_lines.clone();
    issuer.push(document.issuer.registration.clone());
    issuer.push(format!("TVA {}", document.issuer.vat_number));
    for entry in issuer.iter().filter(|l| !l.trim().is_empty()) {
        for line in faces.wrap(Weight::Regular, BODY, entry, issuer_width) {
            page.line(issuer_left, Weight::Regular, BODY, Ink::Text, &line);
        }
    }
    page.y = page.y.max(left_bottom) + 22.0;

    page.line(MARGIN, Weight::Regular, SMALL, Ink::Muted, "Facturé à");
    page.y += 2.0;
    for entry in address_lines(&document.buyer) {
        for line in faces.wrap(Weight::Regular, BODY, &entry, 280.0) {
            page.line(MARGIN, Weight::Regular, BODY, Ink::Text, &line);
        }
    }
    page.y += 22.0;

    // The lines.
    let (price_heading, total_heading) = if incl_vat {
        ("Prix unitaire TTC", "Total TTC")
    } else {
        ("Prix unitaire HT", "Total HT")
    };
    let rows: Vec<Vec<String>> = document
        .lines
        .iter()
        .map(|l| {
            vec![
                l.label.clone(),
                l.quantity.to_string(),
                money(&l.unit_price),
                money(&l.total),
            ]
        })
        .collect();
    page.table(
        &columns(&[1.0], &[60.0, 95.0, 95.0]),
        &["Désignation", "Quantité", price_heading, total_heading],
        &rows,
    );
    page.y += 10.0;

    // The totals, kept together under the lines.
    let mut totals: Vec<(String, String)> = vec![
        ("Sous-total".into(), money(&document.subtotal)),
        ("Frais de port".into(), money(&document.shipping_fee)),
    ];
    if document.handling_fee.is_positive() {
        totals.push(("Frais de dossier".into(), money(&document.handling_fee)));
    }
    if let Some((label, amount)) = &document.discount {
        totals.push((label.clone(), format!("− {}", money(amount))));
    }
    if let (true, Some(base), Some(vat)) =
        (incl_vat, document.total_excl_vat(), document.vat_total())
    {
        totals.push(("Total HT".into(), money(&base)));
        totals.push(("TVA".into(), money(&vat)));
    }
    let totals_left = RIGHT - 230.0;
    let row_height = BODY * LEADING + 3.0;
    page.make_room(row_height * (totals.len() as f32 + 1.0) + 12.0);
    for (label, amount) in &totals {
        let top = page.y;
        let label = faces
            .wrap(Weight::Regular, BODY, label, 130.0)
            .into_iter()
            .next()
            .unwrap_or_default();
        page.text_at(totals_left, top, Weight::Regular, BODY, Ink::Text, &label);
        page.text_ending_at(RIGHT, top, Weight::Regular, BODY, Ink::Text, amount);
        page.y += row_height;
    }
    page.y += 2.0;
    page.rule(totals_left, RIGHT, Ink::Strong);
    page.y += 5.0;
    let top = page.y;
    page.text_at(
        totals_left,
        top,
        Weight::Bold,
        11.0,
        Ink::Text,
        total_heading,
    );
    page.text_ending_at(
        RIGHT,
        top,
        Weight::Bold,
        11.0,
        Ink::Text,
        &money(&document.total),
    );
    page.y += 11.0 * LEADING + 18.0;

    // The VAT per rate, or why there is none.
    if incl_vat && !document.vat_lines.is_empty() {
        page.caption("TVA par taux");
        let rows: Vec<Vec<String>> = document
            .vat_lines
            .iter()
            .map(|l| {
                vec![
                    vat_rate(l.rate_bp),
                    money(&l.base),
                    money(&l.vat),
                    money(&l.total),
                ]
            })
            .collect();
        page.table(
            &columns(&[1.0], &[110.0, 110.0, 110.0]),
            &["Taux", "Base HT", "TVA", "TTC"],
            &rows,
        );
        page.y += 10.0;
    }
    if let Some(mention) = document.regime_mention {
        page.paragraph(Weight::Regular, BODY, Ink::Text, mention);
        page.y += 12.0;
    }

    if !document.credit_notes.is_empty() {
        page.caption("Avoirs émis sur cette facture");
        let rows: Vec<Vec<String>> = document
            .credit_notes
            .iter()
            .map(|n| {
                vec![
                    n.number.clone(),
                    date(n.issued_at),
                    n.reason.clone(),
                    format!("− {}", money(&n.amount)),
                ]
            })
            .collect();
        page.table(
            &columns(&[1.0, 0.8, 2.2], &[95.0]),
            &["Avoir", "Date", "Motif", "Montant"],
            &rows,
        );
        page.y += 8.0;
        page.make_room(BODY * LEADING);
        let top = page.y;
        page.text_ending_at(
            RIGHT,
            top,
            Weight::Bold,
            BODY,
            Ink::Text,
            &format!(
                "Net après avoirs : {}",
                money(&document.net_after_credit_notes)
            ),
        );
        page.y += BODY * LEADING;
    }

    // The footer of every page, once the number of pages is known.
    let mut pages = page.pages;
    let count = pages.len();
    let identity = format!(
        "{} · {} · TVA {}",
        document.issuer.name, document.issuer.registration, document.issuer.vat_number
    );
    let contact = format!(
        "Une question sur cette facture ? {}",
        document.issuer.contact
    );
    for (index, marks) in pages.iter_mut().enumerate() {
        let mut footer = Layout {
            faces,
            pages: vec![Vec::new()],
            y: PAGE_HEIGHT - 64.0,
        };
        footer.rule(MARGIN, RIGHT, Ink::Rule);
        let top = footer.y + 8.0;
        let line = faces
            .wrap(Weight::Regular, SMALL, &identity, RIGHT - MARGIN - 70.0)
            .into_iter()
            .next()
            .unwrap_or_default();
        footer.text_at(MARGIN, top, Weight::Regular, SMALL, Ink::Muted, &line);
        footer.text_at(
            MARGIN,
            top + SMALL * LEADING,
            Weight::Regular,
            SMALL,
            Ink::Muted,
            &contact,
        );
        footer.text_ending_at(
            RIGHT,
            top,
            Weight::Regular,
            SMALL,
            Ink::Muted,
            &format!("Facture {} · page {}/{count}", document.number, index + 1),
        );
        marks.extend(footer.pages.into_iter().flatten());
    }
    pages
}

fn draw(document: &InvoiceDocument, pages: Vec<Vec<Mark>>) -> Result<Vec<u8>, InvoicePdfError> {
    let regular = Font::new(REGULAR.into(), 0).ok_or(InvoicePdfError::Font)?;
    let bold = Font::new(BOLD.into(), 0).ok_or(InvoicePdfError::Font)?;

    let mut pdf = Document::new();
    let (year, month, day) = civil_date(document.issued_at);
    pdf.set_metadata(
        Metadata::new()
            .title(format!("Facture {}", document.number))
            .authors(vec![document.issuer.name.clone()])
            .language("fr".to_owned())
            .creator("timada-invoice".to_owned())
            // The invoice's own date: rendering it again gives the same file.
            .creation_date(
                DateTime::new(u16::try_from(year).unwrap_or(1970))
                    .month(month)
                    .day(day),
            ),
    );

    for marks in pages {
        let settings = PageSettings::from_wh(PAGE_WIDTH, PAGE_HEIGHT)
            .ok_or_else(|| InvoicePdfError::Write("invalid page size".to_owned()))?;
        let mut page = pdf.start_page_with(settings);
        let mut surface = page.surface();
        for mark in marks {
            match mark {
                Mark::Text {
                    x,
                    baseline,
                    weight,
                    size,
                    ink,
                    text,
                } => {
                    surface.set_stroke(None);
                    surface.set_fill(Some(Fill {
                        paint: ink.color().into(),
                        opacity: NormalizedF32::ONE,
                        rule: Default::default(),
                    }));
                    let font = match weight {
                        Weight::Regular => regular.clone(),
                        Weight::Bold => bold.clone(),
                    };
                    surface.draw_text(
                        Point::from_xy(x, baseline),
                        font,
                        size,
                        &text,
                        false,
                        TextDirection::Auto,
                    );
                }
                Mark::Rule { x1, x2, y, ink } => {
                    let mut path = PathBuilder::new();
                    path.move_to(x1, y);
                    path.line_to(x2, y);
                    let Some(path) = path.finish() else { continue };
                    surface.set_fill(None);
                    surface.set_stroke(Some(Stroke {
                        paint: ink.color().into(),
                        width: if ink == Ink::Strong { 0.9 } else { 0.5 },
                        ..Default::default()
                    }));
                    surface.draw_path(&path);
                }
            }
        }
        surface.finish();
        page.finish();
    }
    pdf.finish()
        .map_err(|err| InvoicePdfError::Write(err.to_string()))
}

#[cfg(test)]
mod tests {
    use timada_core::Money;
    use timada_tax::VatLine;

    use super::*;
    use crate::document::{DocumentCreditNote, DocumentLine, InvoiceIssuer};

    fn invoice(lines: usize) -> InvoiceDocument {
        let line = |index: usize| DocumentLine {
            label: format!("AOC 23.8\" LED - 24G4XE, écran n° {}", index + 1),
            quantity: 2,
            unit_price: Money::eur(11_995),
            total: Money::eur(23_990),
        };
        let goods = 23_990 * lines as i64;
        let total = Money::eur(goods + 590);
        InvoiceDocument {
            issuer: InvoiceIssuer {
                name: "Timada demo SAS".into(),
                address_lines: vec!["1 rue de l'Entrepôt".into(), "31000 Toulouse".into()],
                registration: "SIRET 000 000 000 00000".into(),
                vat_number: "FR00 000000000".into(),
                contact: "facturation@timada.example".into(),
            },
            invoice_id: "invoice-1".into(),
            number: "F2026-000042".into(),
            // 19/09/2026.
            issued_at: 1_789_776_000,
            order_id: "order-1".into(),
            order_label: "C2026-000042".into(),
            customer_id: "customer-1".into(),
            buyer: Address {
                first_name: "Ada".into(),
                last_name: "Lovelace".into(),
                line1: "12 rue des Machines".into(),
                postal_code: "31000".into(),
                city: "Toulouse".into(),
                country_code: "FR".into(),
                ..Address::default()
            },
            lines: (0..lines).map(line).collect(),
            subtotal: Money::eur(goods),
            shipping_fee: Money::eur(590),
            handling_fee: Money::eur(0),
            discount: None,
            amounts_include_vat: true,
            vat_lines: vec![VatLine {
                rate_bp: 2_000,
                base: total.excl_tax(2_000),
                vat: Money::eur(total.minor - total.excl_tax(2_000).minor),
                total: total.clone(),
            }],
            exemption_mention: None,
            regime_mention: None,
            credit_notes: Vec::new(),
            net_after_credit_notes: total.clone(),
            total,
        }
    }

    fn texts(pages: &[Vec<Mark>]) -> Vec<Vec<String>> {
        pages
            .iter()
            .map(|marks| {
                marks
                    .iter()
                    .filter_map(|mark| match mark {
                        Mark::Text { text, .. } => Some(text.clone()),
                        Mark::Rule { .. } => None,
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_font_draws_what_french_invoices_are_made_of() -> Result<(), InvoicePdfError> {
        let faces = Faces::load()?;
        // Narrow no-break space of amounts, the minus of reductions, the dot
        // of the footer.
        for ch in "éèàçùôÉ€°«»’\u{202f}\u{a0}−·".chars() {
            assert!(faces.regular.has(ch), "regular lacks {ch:?}");
            assert!(faces.bold.has(ch), "bold lacks {ch:?}");
        }
        assert_eq!(faces.printable(Weight::Regular, "a\nb\u{1F600}"), "a b?");
        Ok(())
    }

    #[test]
    fn an_invoice_is_laid_out_with_its_amounts_on_their_column() -> Result<(), InvoicePdfError> {
        let faces = Faces::load()?;
        let mut document = invoice(1);
        document.credit_notes = vec![DocumentCreditNote {
            number: "A2026-000001".into(),
            issued_at: 1_789_862_400,
            reason: "return R2026-000003".into(),
            amount: Money::eur(5_000),
        }];
        document.net_after_credit_notes = Money::eur(19_580);
        let pages = lay_out(&faces, &document);
        assert_eq!(pages.len(), 1);

        let all = texts(&pages).concat();
        for expected in [
            "Facture F2026-000042",
            "Date : 19/09/2026",
            "Commande : C2026-000042",
            "Timada demo SAS",
            "TVA FR00 000000000",
            "Ada Lovelace",
            "Prix unitaire TTC",
            "239,90 €",
            "Total HT",
            "204,83 €",
            "245,80 €",
            "20 %",
            "A2026-000001",
            "− 50,00 €",
            "Net après avoirs : 195,80 €",
            "Facture F2026-000042 · page 1/1",
        ] {
            assert!(all.iter().any(|t| t == expected), "{expected}: {all:?}");
        }

        // Every amount of the last column ends on the right margin.
        let ends: Vec<f32> = pages[0]
            .iter()
            .filter_map(|mark| match mark {
                Mark::Text {
                    x,
                    weight,
                    size,
                    text,
                    ..
                } if text.ends_with('€') && !text.starts_with("Net") => {
                    Some(x + faces.of(*weight).width(text, *size))
                }
                _ => None,
            })
            .filter(|end| *end > RIGHT - 60.0)
            .collect();
        assert!(ends.len() >= 6, "{ends:?}");
        assert!(
            ends.iter().all(|end| (end - RIGHT).abs() < 0.01),
            "{ends:?}"
        );
        Ok(())
    }

    #[test]
    fn a_long_invoice_runs_over_pages_that_repeat_the_headings() -> Result<(), InvoicePdfError> {
        let faces = Faces::load()?;
        let pages = lay_out(&faces, &invoice(70));
        assert!(pages.len() >= 3, "{}", pages.len());
        let count = pages.len();
        for (index, page) in texts(&pages).iter().enumerate() {
            let footer = format!("Facture F2026-000042 · page {}/{count}", index + 1);
            assert!(page.contains(&footer), "{page:?}");
            if index + 1 < count {
                assert!(page.iter().any(|t| t == "Désignation"), "{page:?}");
            }
        }
        // Nothing of the body is drawn in the footer's space.
        for marks in &pages {
            for mark in marks {
                if let Mark::Text { baseline, text, .. } = mark {
                    let in_footer = text.contains("page ")
                        || text.starts_with("Timada demo SAS ·")
                        || text.starts_with("Une question");
                    assert!(
                        in_footer || *baseline <= BODY_BOTTOM,
                        "{text} at {baseline}"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn an_export_is_priced_without_vat_and_says_why() -> Result<(), InvoicePdfError> {
        let faces = Faces::load()?;
        let mut document = invoice(1);
        document.amounts_include_vat = false;
        document.exemption_mention = timada_tax::TaxTreatment::Export.exemption_mention();
        document.regime_mention = timada_tax::TaxTreatment::Export.regime_mention();
        let all = texts(&lay_out(&faces, &document)).concat();
        assert!(all.iter().any(|t| t == "Prix unitaire HT"), "{all:?}");
        assert!(!all.iter().any(|t| t == "TVA par taux"), "{all:?}");
        assert!(
            all.iter().any(|t| t.starts_with("Exonération de TVA")),
            "{all:?}"
        );
        Ok(())
    }

    #[test]
    fn the_same_invoice_is_the_same_file() -> Result<(), InvoicePdfError> {
        let document = invoice(3);
        let pdf = render_invoice_pdf(&document)?;
        assert!(pdf.starts_with(b"%PDF-"), "not a PDF");
        assert!(pdf.len() > 5_000);
        // Subset fonts: far from the 1,2 MB of the two font files.
        assert!(pdf.len() < 120_000, "{} bytes", pdf.len());
        assert_eq!(pdf, render_invoice_pdf(&document)?);
        assert_eq!(invoice_pdf_file_name(&document), "facture-F2026-000042.pdf");
        Ok(())
    }
}
