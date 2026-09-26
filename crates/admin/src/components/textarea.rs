use topcoat::{
    Result,
    view::{Attributes, Child, StaticClass, View, class, component, view},
};

/// The classes for the [`textarea`] control.
///
/// The text size, radius, shadow, and focus ring match the input control.
/// `field-sizing-content` lets the control grow with its content, from the
/// two-line minimum height; browsers without support keep the fixed minimum
/// and scroll.
///
/// `aria-invalid="true"` recolors the border and focus halo to `--destructive`,
/// the same way the input control marks a rejected value.
const TEXTAREA: StaticClass = class!(
    "field-sizing-content min-h-16 w-full rounded-md border border-input \
     bg-background px-3 py-2 text-sm shadow-xs transition-[color,border-color,box-shadow] outline-none \
     placeholder:text-muted-foreground dark:bg-input/30 \
     focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 \
     aria-invalid:border-destructive aria-invalid:focus-visible:ring-destructive/50 \
     disabled:pointer-events-none disabled:opacity-50",
);

/// A multi-line text input component.
///
/// The `attrs` (such as `name`, `placeholder`, `rows`, `disabled`, or event
/// handlers) are forwarded to the underlying `<textarea>`; a `class` among
/// them is appended to the computed classes. Child nodes become the control's
/// initial value. The textarea fills its container, so size it through the
/// container or with a width class; it grows with its content from a
/// two-line minimum.
///
/// ```ignore
/// view! {
///     textarea(attrs: attributes! { name="feedback" placeholder="Tell us more" })
/// }
/// ```
#[component]
pub async fn textarea(
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <textarea class=(class!(TEXTAREA, attrs.remove("class"))) (attrs)>
            (child)
        </textarea>
    })
}
