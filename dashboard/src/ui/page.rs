//! The furniture a page is built from, over the vendored components: the
//! title row, section cards, stat tiles, status badges, empty states, the
//! pager, filter bars, fact lists and the two-column settings layout.

use topcoat::{
    Result,
    icon::{IconData, icon},
    view::{Attributes, Child, StaticClass, View, attributes, class, component, view},
};

use super::icons;
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonSize, ButtonVariant, button_variants};
use crate::components::card::CARD;
use crate::components::pagination::{
    pagination, pagination_content, pagination_item, pagination_next, pagination_previous,
};

/// The classes of an inline link.
pub const LINK: &str = "font-medium text-primary underline-offset-4 hover:underline";

/// The colour a status or a figure is shown in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Neutral,
    Primary,
    Success,
    Warning,
    Danger,
    Info,
}

impl Tone {
    /// A chip or a tile icon: the colour as text on its own tint.
    fn chip(self) -> StaticClass {
        match self {
            Self::Neutral => class!("bg-muted text-muted-foreground"),
            Self::Primary => class!("bg-primary/10 text-primary"),
            Self::Success => class!("bg-success/10 text-success-text"),
            Self::Warning => class!("bg-warning/10 text-warning-text"),
            Self::Danger => class!("bg-destructive/10 text-destructive-text"),
            Self::Info => class!("bg-info/10 text-info-text"),
        }
    }

    /// The dot in front of a status.
    fn dot(self) -> StaticClass {
        match self {
            Self::Neutral => class!("bg-muted-foreground"),
            Self::Primary => class!("bg-primary"),
            Self::Success => class!("bg-success"),
            Self::Warning => class!("bg-warning"),
            Self::Danger => class!("bg-destructive"),
            Self::Info => class!("bg-info"),
        }
    }

    fn badge(self) -> BadgeVariant {
        match self {
            Self::Neutral => BadgeVariant::Secondary,
            Self::Primary => BadgeVariant::Tinted,
            Self::Success => BadgeVariant::Success,
            Self::Warning => BadgeVariant::Warning,
            Self::Danger => BadgeVariant::Danger,
            Self::Info => BadgeVariant::Info,
        }
    }
}

/// The title row: the page's `<h1>`, an optional line under it, and the
/// actions on the right. A `<div>`, so the topbar stays the page's only
/// `<header>`. `h1_attrs` carries the data attributes a test reads.
#[component]
pub async fn page_header(
    title: &str,
    #[default] subtitle: Option<&str>,
    #[default] h1_attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <div class="mb-6 flex flex-wrap items-end justify-between gap-x-6 gap-y-3">
            <div class="min-w-0">
                <h1 class="text-xl font-bold tracking-tight md:text-2xl" (h1_attrs)>(title)</h1>
                if let Some(subtitle) = subtitle {
                    <p class="mt-1 text-sm text-muted-foreground">(subtitle)</p>
                }
            </div>
            <div class="flex flex-wrap items-center gap-2">(child)</div>
        </div>
    })
}

/// A card that is a `<section>` of the page, with an optional `<h2>`, a
/// line under it, and a primary link button on the right (`action` is its
/// label and address).
#[component]
pub async fn section_card(
    #[default] title: Option<&str>,
    #[default] description: Option<&str>,
    #[default] action: Option<(&str, String)>,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let titled = title.is_some() || description.is_some() || action.is_some();
    Ok(view! {
        <section class=(class!(CARD, "mb-6 gap-4", attrs.remove("class"))) (attrs)>
            if titled {
                <div class="flex flex-wrap items-start justify-between gap-3 px-6">
                    <div class="min-w-0">
                        if let Some(title) = title {
                            <h2 class="text-base font-semibold">(title)</h2>
                        }
                        if let Some(description) = description {
                            <p class="mt-1 text-sm text-muted-foreground">(description)</p>
                        }
                    </div>
                    if let Some((label, href)) = &action {
                        <a class=(button_variants(ButtonVariant::Primary, ButtonSize::Sm)) href=(href)>
                            icon(data: icons::PLUS)
                            (label)
                        </a>
                    }
                </div>
            }
            <div class="px-6">(child)</div>
        </section>
    })
}

/// A figure with its label, linking to the section it counts.
#[component]
pub async fn stat_tile(
    label: &str,
    value: String,
    symbol: IconData,
    href: String,
    #[default] tone: Tone,
    #[default] hint: Option<&str>,
) -> Result<impl View> {
    Ok(view! {
        <a
            href=(href)
            class=(class!(
                CARD,
                "gap-0 py-5 transition-all hover:-translate-y-1 hover:border-primary/40 hover:shadow-lg motion-reduce:transition-none motion-reduce:hover:translate-y-0",
            ))
        >
            <span class="flex items-start gap-3 px-5">
                <span class=(class!("flex size-11 shrink-0 items-center justify-center rounded-xl [&>svg]:size-5", tone.chip()))>
                    icon(data: symbol)
                </span>
                <span class="min-w-0">
                    <span class="block text-xs font-medium text-muted-foreground">(label)</span>
                    <span class="block text-2xl font-bold tracking-tight tabular-nums">(value)</span>
                    if let Some(hint) = hint {
                        <span class="block text-xs text-muted-foreground">(hint)</span>
                    }
                </span>
            </span>
        </a>
    })
}

/// A status: a dot and a word on a tint. `attrs` carries the data attribute
/// a test reads.
#[component]
pub async fn status_badge(
    #[default] tone: Tone,
    #[default] attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        badge(
            variant: tone.badge(),
            attrs: attributes! { class="gap-1.5 rounded-full px-2.5" (attrs) },
            <span class=(class!("size-1.5 shrink-0 rounded-full", tone.dot())) aria-hidden="true"></span>
            (child)
        )
    })
}

/// What a list shows when there is nothing in it yet.
#[component]
pub async fn empty_state(
    title: &str,
    #[default] hint: Option<&str>,
    #[default] symbol: Option<IconData>,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <div class="flex flex-col items-center gap-2 px-4 py-10 text-center">
            if let Some(data) = symbol {
                <span class="flex size-12 items-center justify-center rounded-xl bg-muted text-muted-foreground [&>svg]:size-6">
                    icon(data: data)
                </span>
            }
            <p class="text-sm font-medium">(title)</p>
            if let Some(hint) = hint {
                <p class="max-w-sm text-sm text-muted-foreground">(hint)</p>
            }
            <div class="mt-2 flex flex-wrap justify-center gap-2">(child)</div>
        </div>
    })
}

/// The one row of a table with nothing in it.
#[component]
pub async fn empty_row(colspan: u32, text: &str) -> Result<impl View> {
    Ok(view! {
        <tr><td class="p-3 text-muted-foreground" colspan=(colspan)>(text)</td></tr>
    })
}

/// Previous / "Page X of Y" / Next. Nothing when there is one page.
#[component]
pub async fn pager(
    page: i64,
    pages: i64,
    #[default] previous: Option<String>,
    #[default] next: Option<String>,
) -> Result<impl View> {
    Ok(view! {
        if pages > 1 {
            pagination(
                attrs: attributes! { class="mt-4" },
                pagination_content(
                    if let Some(previous) = previous {
                        pagination_item(pagination_previous(attrs: attributes! { href=(previous) }))
                    }
                    pagination_item(
                        <span class="px-2 text-sm text-muted-foreground">"Page " (page) " of " (pages)</span>
                    )
                    if let Some(next) = next {
                        pagination_item(pagination_next(attrs: attributes! { href=(next) }))
                    }
                )
            )
        }
    })
}

/// A `GET` form of filters above a list.
#[component]
pub async fn filter_bar(
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <form
            method="get"
            role="search"
            class=(class!("mb-4 flex flex-wrap items-center gap-2", attrs.remove("class")))
            (attrs)
        >
            (child)
        </form>
    })
}

/// A list of facts: terms on the left, values on the right.
#[component]
pub async fn facts(#[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <dl class="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[10rem_minmax(0,1fr)]">(child)</dl>
    })
}

/// One fact. `attrs` go on the `<dd>`, for the data attribute a test reads.
#[component]
pub async fn fact(
    term: &str,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <dt class="text-muted-foreground">(term)</dt>
        <dd class=(class!("min-w-0 break-words", attrs.remove("class"))) (attrs)>(child)</dd>
    })
}

/// The settings layout: a sticky card of in-page links on the left, the
/// sections on the right. `sections` pairs each section's `id` with its
/// title; the cards in `child` carry the ids. A `<nav>` in a plain `<div>`,
/// so a test that reads a `<section>` by its text never sees the links.
#[component]
pub async fn settings_layout(
    label: &str,
    sections: &[(&str, &str)],
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <div class="grid items-start gap-6 lg:grid-cols-[14rem_minmax(0,1fr)]">
            <nav aria-label=(label) class=(class!(CARD, "gap-0 px-3 py-3 lg:sticky lg:top-20"))>
                <ul class="flex flex-wrap gap-1 lg:flex-col">
                    for (id, title) in sections {
                        <li>
                            <a
                                class="flex rounded-md px-3 py-2 text-sm font-medium text-muted-foreground hover:bg-muted hover:text-foreground"
                                href=(format!("#{id}"))
                            >(*title)</a>
                        </li>
                    }
                </ul>
            </nav>
            <div class="min-w-0">(child)</div>
        </div>
    })
}
