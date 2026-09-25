//! The stylesheet is its own test.
//!
//! Two things about a theme cannot be caught by reading the pages that use it.
//! The dark palette is declared twice — once on `.dark`, for the operator who
//! asked for it, and once under `prefers-color-scheme`, for the one who asked
//! for nothing — and CSS cannot report a drift between the two: the page
//! simply renders the wrong skin for half the operators. And a color pair that
//! does not carry its text is invisible to every other test in the suite,
//! because the markup is correct and only the reading of it fails.
//!
//! So the file is read back, the two palettes compared, and every pair the
//! components actually put together is measured against WCAG 2.2 AA.

const STYLES: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/styles.css"));

const OPEN: &str = "/* dark:palette */";
const CLOSE: &str = "/* dark:palette:end */";

/// Indentation differs between the two palettes — one is nested a level
/// deeper — so they are compared on their declarations, not on their layout.
fn declarations(block: &str) -> String {
    block.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn dark_palettes() -> Vec<&'static str> {
    STYLES
        .split(OPEN)
        .skip(1)
        .map(|rest| rest.split(CLOSE).next().unwrap_or_default())
        .collect()
}

#[test]
fn the_dark_palette_is_declared_the_same_way_twice() {
    let blocks = dark_palettes();
    assert_eq!(
        blocks.len(),
        2,
        "the palette is marked with `{OPEN}` … `{CLOSE}` on `.dark` and on the media query"
    );
    assert_eq!(
        declarations(blocks[0]),
        declarations(blocks[1]),
        "the two dark palettes have drifted apart"
    );
}

#[test]
fn every_token_the_theme_exposes_has_a_value() {
    // `@theme inline` names the utilities; `:root` and the dark palette give
    // them values. A token exposed but never valued renders as nothing at all
    // — which is how `border-input` came to draw no border.
    let theme = section("@theme inline {");
    let light = section("\n:root {");
    let dark = dark_palettes()[0];

    for line in theme.lines() {
        // `--color-card: var(--card);` — the token is what the utility reads.
        let Some((_, value)) = line.split_once(": var(--") else {
            continue;
        };
        let Some((token, _)) = value.split_once(')') else {
            continue;
        };
        let declaration = format!("--{token}:");
        assert!(
            light.contains(&declaration),
            "`--{token}` is exposed as a utility but has no value on `:root`"
        );
        // The dark palette inherits every token it does not restate, so only
        // what must differ is required of it: a radius and a font stack are
        // the same in both schemes, and the rail's quieter greys are already
        // legible on a surface that barely moves.
        let inherits = [
            "radius",
            "font-",
            "sidebar-foreground-muted",
            "sidebar-icon-",
        ];
        if inherits.iter().any(|prefix| token.starts_with(prefix)) {
            continue;
        }
        assert!(
            dark.contains(&declaration),
            "`--{token}` keeps its light value in the dark scheme"
        );
    }
}

#[test]
fn every_pair_the_components_make_is_legible() {
    for (scheme, tokens) in [("light", light()), ("dark", dark())] {
        for &(foreground, background, floor, what) in PAIRS {
            let ratio = contrast(tokens.get(foreground), tokens.get(background));
            assert!(
                ratio >= floor,
                "{scheme}: `{foreground}` on `{background}` is {ratio:.2}:1, \
                 under the {floor}:1 {what} needs"
            );
        }
        // A status badge tints its own color behind its own text, and a
        // same-hue pair loses more contrast than the numbers above suggest.
        for accent in ["primary", "success", "info", "destructive"] {
            let tint = mix(tokens.get(accent), tokens.get("card"), 0.10);
            let ratio = ratio_of(srgb(tokens.get(accent)), tint);
            assert!(
                ratio >= 4.5,
                "{scheme}: `{accent}` on a tenth of itself is {ratio:.2}:1, \
                 under the 4.5:1 a badge's own label needs"
            );
        }
        // Quiet text on the quiet surfaces the components tint for panels and
        // table stripes.
        for strength in [0.40, 0.50] {
            let surface = mix(tokens.get("muted"), tokens.get("card"), strength);
            let ratio = ratio_of(srgb(tokens.get("muted-foreground")), surface);
            assert!(
                ratio >= 4.5,
                "{scheme}: `muted-foreground` on `bg-muted/{}` is {ratio:.2}:1",
                (strength * 100.0) as u32
            );
        }
    }
}

/// Every pairing the components put on screen: `(text, surface, floor, why)`.
///
/// Text and icons owe 4.5:1 (WCAG 2.2 SC 1.4.3); a focus indicator and a
/// glyph that carries no words owe 3:1 (SC 1.4.11, SC 2.4.13).
const PAIRS: &[(&str, &str, f64, &str)] = &[
    ("foreground", "background", 4.5, "body text"),
    ("card-foreground", "card", 4.5, "text on a card"),
    ("muted-foreground", "card", 4.5, "secondary text"),
    ("muted-foreground", "background", 4.5, "secondary text"),
    (
        "muted-foreground",
        "muted",
        4.5,
        "secondary text on a panel",
    ),
    (
        "primary-foreground",
        "primary",
        4.5,
        "a primary button's label",
    ),
    ("primary", "background", 4.5, "a link"),
    ("primary", "card", 4.5, "a link on a card"),
    (
        "secondary-foreground",
        "secondary",
        4.5,
        "a secondary button",
    ),
    ("accent-foreground", "accent", 4.5, "a hovered item"),
    ("destructive", "background", 4.5, "an error message"),
    ("destructive", "card", 4.5, "an error message on a card"),
    (
        "destructive-foreground",
        "destructive",
        4.5,
        "a destructive button",
    ),
    ("success", "card", 4.5, "a status badge"),
    ("info", "card", 4.5, "a status badge"),
    ("ring", "background", 3.0, "the focus indicator"),
    ("sidebar-foreground", "sidebar", 4.5, "a navigation label"),
    (
        "sidebar-foreground-muted",
        "sidebar",
        4.5,
        "a navigation heading",
    ),
    (
        "sidebar-foreground",
        "sidebar-active",
        4.5,
        "the current section",
    ),
    (
        "sidebar-icon-active",
        "sidebar-active",
        4.5,
        "the current section's icon",
    ),
    ("sidebar-icon", "sidebar", 3.0, "a navigation icon"),
    ("sidebar-icon-muted", "sidebar", 3.0, "a quiet icon"),
];

// ---------------------------------------------------------------------------
// Reading the palette, and measuring it.
// ---------------------------------------------------------------------------

/// The declarations of the block `styles.css` opens with `header`.
fn section(header: &str) -> &'static str {
    STYLES
        .split_once(header)
        .and_then(|(_, rest)| rest.split_once("\n}"))
        .map(|(block, _)| block)
        .unwrap_or_default()
}

/// An opaque color, as `oklch(L C H)`.
#[derive(Clone, Copy, Debug)]
struct Oklch {
    lightness: f64,
    chroma: f64,
    hue: f64,
}

/// The tokens of one color scheme, by name without the leading dashes.
struct Palette(Vec<(String, Oklch)>);

impl Palette {
    fn get(&self, token: &str) -> Oklch {
        self.0
            .iter()
            .find(|(name, _)| name == token)
            .map(|(_, color)| *color)
            .unwrap_or_else(|| panic!("`--{token}` is not declared, or is not an opaque oklch()"))
    }
}

/// Every opaque `--token: oklch(L C H)` in `block`.
///
/// Translucent values (`oklch(1 0 0 / 10%)`, which is what the dark borders
/// are) are skipped: a hairline over an unknown backdrop has no one ratio,
/// and none of the pairs below is a border.
fn palette(block: &str) -> Vec<(String, Oklch)> {
    block
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (name, value) = line.strip_prefix("--")?.split_once(':')?;
            let inside = value.trim().strip_prefix("oklch(")?.strip_suffix(");")?;
            if inside.contains('/') {
                return None;
            }
            let mut parts = inside.split_whitespace();
            let color = Oklch {
                lightness: parts.next()?.parse().ok()?,
                chroma: parts.next()?.parse().ok()?,
                hue: parts.next()?.parse().ok()?,
            };
            parts.next().is_none().then(|| (name.to_owned(), color))
        })
        .collect()
}

fn light() -> Palette {
    Palette(palette(section("\n:root {")))
}

/// The dark scheme as it renders: the light palette with the dark one over it,
/// since the dark block restates only what changes.
fn dark() -> Palette {
    let mut tokens = palette(section("\n:root {"));
    for (name, color) in palette(dark_palettes()[0]) {
        match tokens.iter_mut().find(|(known, _)| *known == name) {
            Some(slot) => slot.1 = color,
            None => tokens.push((name, color)),
        }
    }
    Palette(tokens)
}

/// Oklch to sRGB, gamma-encoded and clipped to the gamut, following the
/// CSS Color 4 conversion.
fn srgb(color: Oklch) -> [f64; 3] {
    let Oklch {
        lightness,
        chroma,
        hue,
    } = color;
    let (a, b) = (
        chroma * hue.to_radians().cos(),
        chroma * hue.to_radians().sin(),
    );
    let long = (lightness + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let medium = (lightness - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let short = (lightness - 0.089_484_177_5 * a - 1.291_485_548_0 * b).powi(3);
    let linear = [
        4.076_741_662_1 * long - 3.307_711_591_3 * medium + 0.230_969_929_2 * short,
        -1.268_438_004_6 * long + 2.609_757_401_1 * medium - 0.341_319_396_5 * short,
        -0.004_196_086_3 * long - 0.703_418_614_7 * medium + 1.707_614_701_0 * short,
    ];
    linear.map(|channel| {
        let channel = channel.clamp(0.0, 1.0);
        if channel <= 0.003_130_8 {
            12.92 * channel
        } else {
            1.055 * channel.powf(1.0 / 2.4) - 0.055
        }
    })
}

/// `strength` of `color` laid over `backdrop`, as a Tailwind opacity modifier
/// composites it.
fn mix(color: Oklch, backdrop: Oklch, strength: f64) -> [f64; 3] {
    let (over, under) = (srgb(color), srgb(backdrop));
    [0, 1, 2].map(|channel| over[channel] * strength + under[channel] * (1.0 - strength))
}

/// Relative luminance, per WCAG 2.2.
fn luminance(rgb: [f64; 3]) -> f64 {
    let [red, green, blue] = rgb.map(|channel| {
        if channel <= 0.040_45 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * red + 0.7152 * green + 0.0722 * blue
}

fn ratio_of(first: [f64; 3], second: [f64; 3]) -> f64 {
    let (a, b) = (luminance(first), luminance(second));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn contrast(foreground: Oklch, background: Oklch) -> f64 {
    ratio_of(srgb(foreground), srgb(background))
}
