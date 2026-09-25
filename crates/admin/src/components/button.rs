use topcoat::{
    Result,
    view::{Attributes, Child, Class, StaticClass, View, class, component, view},
};

/// The visual style of a [`button`].
///
/// [`Default`] is `ButtonVariant::Primary`, used when no variant is given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ButtonVariant {
    /// The primary-filled button for the main action.
    #[default]
    Primary,
    /// A muted, tinted fill for secondary actions.
    Secondary,
    /// A hairline-bordered button on the page background.
    Outline,
    /// No fill until hovered, for toolbars and inline actions.
    Ghost,
    /// A destructive-filled button for actions such as deleting data.
    Destructive,
}

impl ButtonVariant {
    /// The Tailwind classes for this variant.
    ///
    /// Hover and press states apply the fill or foreground color at reduced
    /// opacity, so they hold up in both color schemes without `dark:`
    /// overrides — except `Outline`, whose whole point is its edge: on a dark
    /// page a hairline over the page color disappears, so it takes a tinted
    /// fill and the control border instead. Every variant with a resting fill
    /// or border casts the theme's control shadow; `Ghost` is flat until
    /// hovered, so it casts none.
    ///
    /// Each variant sets its own border color rather than inheriting a
    /// transparent one from [`BASE`]: with two border-color classes on the
    /// same element, stylesheet order (not class order) would decide the
    /// winner.
    fn classes(self) -> StaticClass {
        match self {
            Self::Primary => class!(
                "border-transparent bg-primary text-primary-foreground shadow-xs \
                 hover:bg-primary/90 active:bg-primary/80",
            ),
            Self::Secondary => class!(
                "border-transparent bg-secondary text-secondary-foreground shadow-xs \
                 hover:bg-secondary/80 active:bg-secondary/70",
            ),
            Self::Outline => class!(
                "border-border bg-background text-foreground shadow-xs \
                 hover:bg-accent hover:text-accent-foreground \
                 dark:border-input dark:bg-input/30 dark:hover:bg-input/50",
            ),
            Self::Ghost => class!(
                "border-transparent text-foreground hover:bg-accent \
                 hover:text-accent-foreground active:bg-accent/70",
            ),
            Self::Destructive => class!(
                "border-transparent bg-destructive text-destructive-foreground shadow-xs \
                 hover:bg-destructive/90 active:bg-destructive/80",
            ),
        }
    }
}

/// The size of a [`button`].
///
/// [`Default`] is `ButtonSize::Md`, used when no size is given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ButtonSize {
    /// A compact button.
    Sm,
    /// The standard button size.
    #[default]
    Md,
    /// A prominent button.
    Lg,
    /// A square button sized for a single icon.
    Icon,
}

impl ButtonSize {
    /// The Tailwind classes for this size.
    ///
    /// Each size sets a text size, which also scales any icons inside: the
    /// `icon` component is `1em` square by default.
    fn classes(self) -> StaticClass {
        match self {
            Self::Sm => class!("h-8 gap-1.5 rounded-md px-3 text-xs"),
            Self::Md => class!("h-9 gap-2 rounded-md px-4 text-sm"),
            Self::Lg => class!("h-10 gap-2 rounded-md px-5 text-base"),
            Self::Icon => class!("size-9 rounded-md text-base"),
        }
    }
}

/// The classes shared by every button, regardless of variant or size.
///
/// Every button carries a border (colored per variant) so that the `Outline`
/// variant, which only recolors it, does not change the button's dimensions.
///
/// Focus recolors that border to the ring and lays a translucent halo outside
/// it. The border is the indicator and it is full strength; the halo is what
/// makes it carry across a busy toolbar.
///
/// The transition names its properties rather than using `transition-colors`,
/// which does not cover the ring: the ring is a box shadow, and a halo that
/// appears instantly while the fill fades reads as two separate events.
const BASE: StaticClass = class!(
    "inline-flex shrink-0 items-center justify-center border \
     font-medium whitespace-nowrap outline-none select-none \
     transition-[color,background-color,border-color,box-shadow] \
     focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 \
     disabled:pointer-events-none disabled:opacity-50 [&>svg]:shrink-0",
);

/// Builds the full class list for a button of the given `variant` and `size`.
///
/// Use it to give button styling to an element that is not a `<button>`, such
/// as a link styled as a button:
///
/// ```ignore
/// view! {
///     <a href="/login" class=(button_variants(ButtonVariant::Outline, ButtonSize::Md))>
///         "Sign in"
///     </a>
/// }
/// ```
#[must_use]
pub fn button_variants(
    variant: ButtonVariant,
    size: ButtonSize,
) -> Class<(StaticClass, StaticClass, StaticClass)> {
    class!(BASE, variant.classes(), size.classes())
}

/// A button component.
///
/// The `variant` and `size` parameters select the styling, defaulting to
/// `Primary` and `Md`. The `attrs` (such as `class`, `type`, `disabled`, or
/// event handlers) are forwarded to the underlying `<button>`; a `class` among
/// them is appended to the computed classes. Child nodes become the button's
/// content.
///
/// ```ignore
/// view! {
///     button(
///         variant: ButtonVariant::Destructive,
///         attrs: attributes! { type="submit" },
///         "Delete"
///     )
/// }
/// ```
///
/// To style a non-`<button>` element like a button, use [`button_variants`]
/// directly.
#[component]
pub async fn button(
    #[default] variant: ButtonVariant,
    #[default] size: ButtonSize,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <button
            class=(class!(
                BASE,
                variant.classes(),
                size.classes(),
                attrs.remove("class"),
            ))
            (attrs)
        >
            (child)
        </button>
    })
}
