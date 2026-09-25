//! The furniture a page is built from.
//!
//! Every shape here was already in the tree a dozen times over, written out by
//! hand each time and drifting a little with each copy: two recipes for a fact
//! list, three for a filter bar's submit button, a detail layout repeated in
//! twelve files. Naming them is what makes the next page look like the last
//! one without anybody having to remember how.

use topcoat::{
    Result,
    context::Cx,
    view::{Attributes, Child, View, attributes, class, component, view},
};

use crate::{
    components::{
        button::{ButtonSize, ButtonVariant, button_variants},
        input::input,
    },
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
///
/// `class` belongs to the value, which is often an id and wants a monospace.
#[component]
pub async fn fact(
    term: &str,
    #[default]
    #[into]
    class: String,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <dt class="text-muted-foreground">(term)</dt>
        <dd class=((!class.is_empty()).then_some(class))>(child)</dd>
    })
}

/// The filter bar of a list page: a GET form whose fields a page names and
/// whose submit button it does not.
///
/// Wrapping, because a filter bar with four fields does not fit a phone, and
/// `items-end` so a labelled field and a bare control sit on the same line.
#[component]
pub async fn filter_bar(
    /// What the button says. "Filtrer" narrows a list; a bar that is only a
    /// search box says "Rechercher" instead.
    #[default("Filtrer")]
    submit: &str,
    /// Where the form goes, when it is not the page it is on.
    #[default]
    #[into]
    action: String,
    #[default]
    #[into]
    class: String,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <form
            method="get"
            action=((!action.is_empty()).then_some(action))
            class=(class!("flex flex-wrap items-end gap-2 text-sm", class))
        >
            (child)
            <button
                type="submit"
                class=(button_variants(ButtonVariant::Outline, ButtonSize::Md))
            >
                (submit)
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

/// A labelled text input, which is most of what a form is.
///
/// The `name` is the id too, so the label always points at its control and
/// nobody has to keep the two in step. `attrs` carries the type, the value and
/// whatever else the input needs.
///
/// This lived in `products` and was reached for from two other files, where it
/// collided with [`field`]. It is the same idea one step more specific: a
/// [`field`] whose child is always an input.
#[component]
pub async fn text_field(
    cx: &Cx,
    name: &str,
    label_text: &str,
    #[default] mut attrs: Attributes,
) -> Result<impl View> {
    attrs.insert(cx, "id", name.to_owned());
    attrs.insert(cx, "name", name.to_owned());
    Ok(view! {
        field(label: label_text, control: name, input(attrs: attrs))
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

/// What went wrong, in the operator's language.
///
/// Takes children rather than a string: half the messages in the back office
/// are a count and a sentence about it, and one of those is not a `&str`.
///
/// `items-start` and not `items-center`: several of these run to two lines, and
/// a centred glyph beside a paragraph floats in the middle of it.
#[component]
pub async fn form_error(
    #[default]
    #[into]
    class: String,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <p
            role="alert"
            class=(class!("flex items-start gap-2 text-sm text-destructive", class))
        >
            icon(data: icons::CIRCLE_ALERT, attrs: attributes! { class="mt-0.5 size-4" })
            <span>(child)</span>
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
