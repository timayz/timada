//! The theme in `styles.css` is read back and measured: the dark palette is
//! written twice (for the `dark` class and for the OS preference) and must
//! not drift, every block must declare the same tokens, and the text /
//! background pairs the pages rely on must meet WCAG 2.2 AA (4.5:1).
//!
//! Colours are `oklch(L C H [/ A])`; the conversion to sRGB luminance is
//! Björn Ottosson's OKLab matrices. Alpha is ignored (the only translucent
//! tokens are borders, which no pair here measures).

use std::collections::{BTreeMap, BTreeSet};

const STYLES: &str = include_str!("../styles.css");

type Palette = BTreeMap<String, String>;

/// The declarations of the first `{ … }` block whose opening line contains
/// `selector`.
fn block(selector: &str) -> Palette {
    let start = STYLES
        .find(selector)
        .unwrap_or_else(|| panic!("no `{selector}` block in styles.css"));
    let body_start = STYLES[start..].find('{').expect("opening brace") + start + 1;
    let mut depth = 1;
    let mut end = body_start;
    for (i, c) in STYLES[body_start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = body_start + i;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &STYLES[body_start..end];
    body.split(';')
        .filter_map(|decl| {
            let (name, value) = decl.split_once(':')?;
            let name = name.trim();
            name.starts_with("--").then(|| {
                let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
                (name.to_owned(), value)
            })
        })
        .collect()
}

fn light() -> Palette {
    block(":root {")
}

fn dark() -> Palette {
    block(".dark {")
}

fn dark_media() -> Palette {
    block(":root:not(.light) {")
}

#[test]
fn dark_palette_is_written_twice_identically() {
    let class = dark();
    let media = dark_media();
    assert!(!class.is_empty(), "the .dark block is empty");
    for (name, value) in &class {
        assert_eq!(
            media.get(name),
            Some(value),
            "{name} differs between `.dark` and the prefers-color-scheme copy"
        );
    }
    for name in media.keys() {
        assert!(class.contains_key(name), "{name} is only in the media copy");
    }
}

#[test]
fn every_scheme_declares_the_same_tokens() {
    let light: BTreeSet<_> = light().into_keys().filter(|n| n != "--radius").collect();
    let dark: BTreeSet<_> = dark().into_keys().collect();
    assert_eq!(
        light, dark,
        "the light and dark blocks declare different tokens"
    );
}

#[test]
fn the_markers_fence_the_dark_palette() {
    assert_eq!(
        STYLES.matches("/* dark:palette */").count(),
        3,
        "the dark palette is fenced by three `/* dark:palette */` markers"
    );
}

/// `oklch(L C H)` → linear sRGB, clamped to the gamut.
fn linear_srgb(value: &str) -> [f64; 3] {
    let inner = value
        .strip_prefix("oklch(")
        .and_then(|v| v.strip_suffix(')'))
        .unwrap_or_else(|| panic!("not an oklch colour: {value}"));
    let inner = inner.split('/').next().unwrap();
    let mut parts = inner.split_whitespace().map(|p| {
        if let Some(pct) = p.strip_suffix('%') {
            pct.parse::<f64>().unwrap() / 100.0
        } else {
            p.parse::<f64>().unwrap()
        }
    });
    let (l, c, h) = (
        parts.next().unwrap(),
        parts.next().unwrap(),
        parts.next().unwrap_or(0.0),
    );
    let (a, b) = (c * h.to_radians().cos(), c * h.to_radians().sin());
    let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
    let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
    let s_ = l - 0.089_484_177_5 * a - 1.291_485_548 * b;
    let (l3, m3, s3) = (l_.powi(3), m_.powi(3), s_.powi(3));
    [
        4.076_741_662_1 * l3 - 3.307_711_591_3 * m3 + 0.230_969_929_2 * s3,
        -1.268_438_004_6 * l3 + 2.609_757_401_1 * m3 - 0.341_319_396_5 * s3,
        -0.004_196_086_3 * l3 - 0.703_418_614_7 * m3 + 1.707_614_701 * s3,
    ]
    .map(|v| v.clamp(0.0, 1.0))
}

fn luminance(rgb: [f64; 3]) -> f64 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

fn contrast(a: f64, b: f64) -> f64 {
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// The luminance of `tone` at 10 % over `background`, as `bg-<tone>/10`
/// composites it (a linear-light mix, close enough to the browser's).
fn tint(tone: [f64; 3], background: [f64; 3]) -> f64 {
    let mixed = [0, 1, 2].map(|i| 0.1 * tone[i] + 0.9 * background[i]);
    luminance(mixed)
}

fn check(scheme: &str, palette: &Palette) {
    let lum = |name: &str| {
        let value = palette
            .get(name)
            .unwrap_or_else(|| panic!("{scheme}: no {name} token"));
        luminance(linear_srgb(value))
    };
    let rgb = |name: &str| linear_srgb(&palette[name]);
    let mut failures = Vec::new();
    let mut pair = |text: &str, surface: &str, ratio: f64| {
        if ratio < 4.5 {
            failures.push(format!("{scheme}: {text} on {surface} is {ratio:.2}:1"));
        }
    };
    for (text, surface) in [
        ("--foreground", "--background"),
        ("--muted-foreground", "--background"),
        ("--card-foreground", "--card"),
        ("--muted-foreground", "--card"),
        ("--popover-foreground", "--popover"),
        ("--primary-foreground", "--primary"),
        ("--secondary-foreground", "--secondary"),
        ("--accent-foreground", "--accent"),
        ("--destructive-foreground", "--destructive"),
        ("--success-foreground", "--success"),
        ("--warning-foreground", "--warning"),
        ("--info-foreground", "--info"),
        // Links and plain tinted text on the page.
        ("--primary", "--background"),
        ("--destructive-text", "--background"),
        ("--success-text", "--background"),
        ("--warning-text", "--background"),
        ("--info-text", "--background"),
        ("--topbar-foreground", "--topbar"),
        ("--sidebar-foreground", "--sidebar"),
        ("--sidebar-muted", "--sidebar"),
        ("--sidebar-icon", "--sidebar"),
        ("--sidebar-accent-foreground", "--sidebar-accent"),
        ("--sidebar-primary-foreground", "--sidebar-primary"),
        ("--sidebar-primary", "--sidebar"),
    ] {
        pair(text, surface, contrast(lum(text), lum(surface)));
    }
    // The status badges: `bg-<tone>/10 text-<tone>-text` on a card.
    for tone in [
        "--primary",
        "--destructive",
        "--success",
        "--warning",
        "--info",
    ] {
        let text = if tone == "--primary" {
            "--primary".to_owned()
        } else {
            format!("{tone}-text")
        };
        let surface = tint(rgb(tone), rgb("--card"));
        pair(
            &text,
            &format!("{tone}/10 over card"),
            contrast(lum(&text), surface),
        );
    }
    assert!(
        failures.is_empty(),
        "WCAG AA failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn light_scheme_meets_wcag_aa() {
    check("light", &light());
}

#[test]
fn dark_scheme_meets_wcag_aa() {
    check("dark", &dark());
}
