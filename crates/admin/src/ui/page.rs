//! The furniture a page is built from.
//!
//! Every shape here was already in the tree a dozen times over, written out by
//! hand each time and drifting a little with each copy: two recipes for a fact
//! list, three for a filter bar's submit button, a detail layout repeated in
//! twelve files. Naming them is what makes the next page look like the last
//! one without anybody having to remember how.

use topcoat::{
    Result,
    view::{Child, View, attributes, class, component, view},
};

use crate::{
    components::button::{ButtonSize, ButtonVariant, button_variants},
    ui::{icon, icons},
};

/// The two-column layout of a record's page: the record on the left, its facts
/// and the actions on it down the right.
#[component]
pub async fn detail_grid(#[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! { <div class="grid gap-6 lg:grid-cols-3">(child)</div> })
}

/// The wide column of a [`detail_grid`].
#[component]
pub async fn detail_main(#[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! { <div class="flex flex-col gap-6 lg:col-span-2">(child)</div> })
}

/// The narrow column of a [`detail_grid`].
#[component]
pub async fn detail_side(#[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! { <div class="flex flex-col gap-6">(child)</div> })
}

/// A list of facts about a record: each term quiet, each value plain.
///
/// A two-column grid rather than a stack, so the values line up down the page
/// and a long term does not push its value out of sight.
#[component]
pub async fn facts(#[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm">(child)</dl>
    })
}

/// One fact: its term, and whatever is known about it.
#[component]
pub async fn fact(term: &str, #[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <dt class="text-muted-foreground">(term)</dt>
        <dd>(child)</dd>
    })
}

/// The filter bar of a list page: a GET form whose fields a page names and
/// whose submit button it does not.
///
/// Wrapping, because a filter bar with four fields does not fit a phone, and
/// `items-end` so a labelled field and a bare control sit on the same line.
#[component]
pub async fn filter_bar(#[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <form method="get" class="flex flex-wrap items-end gap-2 text-sm">
            (child)
            <button
                type="submit"
                class=(button_variants(ButtonVariant::Outline, ButtonSize::Md))
            >
                "Filtrer"
            </button>
        </form>
    })
}

/// A labelled control, stacked. `control` is the id of what it labels, so the
/// label is a real label and not a caption sitting near one.
///
/// A width belongs on the field, not on the control inside it: the controls are
/// `w-full`, and a `w-40` handed to one of them loses to that — same property,
/// same specificity, so stylesheet order decides and the narrower class is
/// simply dropped.
#[component]
pub async fn field(
    label: &str,
    control: &str,
    #[default]
    #[into]
    class: String,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <div class=(class!("flex flex-col gap-1.5", class))>
            <label for=(control) class="text-muted-foreground">(label)</label>
            (child)
        </div>
    })
}

/// A list page's table, on a card.
///
/// The table keeps its own scroll container; this is the surface it sits on.
/// `overflow-hidden` is what stops the first row's corners from squaring off
/// the card's.
#[component]
pub async fn table_card(#[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <div class="overflow-hidden rounded-xl border border-border bg-card shadow-sm">
            (child)
        </div>
    })
}

/// What went wrong with a form, in the operator's language.
#[component]
pub async fn form_error(message: &str) -> Result<impl View> {
    Ok(view! {
        <p role="alert" class="flex items-center gap-2 text-sm text-destructive">
            icon(data: icons::CIRCLE_ALERT, attrs: attributes! { class="size-4" })
            (message)
        </p>
    })
}

/// An inline link.
///
/// `text-primary`, which the theme sets dark enough to carry body text: at
/// Katalyst's own lightness it would be 3.3:1 on the page, which is why the
/// palette moved it and why there is no second colour for links.
#[component]
pub async fn link(
    href: String,
    #[default]
    #[into]
    class: String,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <a
            href=(href)
            class=(class!("text-primary underline-offset-4 hover:underline", class))
        >
            (child)
        </a>
    })
}
