use topcoat::{
    Result,
    view::{Attributes, StaticClass, View, class, component, view},
};

/// The classes for the [`input`] control.
///
/// The height, text size, radius, shadow, and focus ring match the `Md`
/// button, so an input and a button sit flush in a row. File inputs restyle
/// the browser's upload button into quiet, borderless text.
///
/// The border is `--input`, not `--border`: a control's edge is a shade of its
/// own, and in the dark scheme it is a translucent white over a tinted fill.
/// Focus recolors that border to the ring and lays a translucent halo outside
/// it, so the indicator itself is full strength.
const INPUT: StaticClass = class!(
    "h-9 w-full min-w-0 rounded-md border border-input bg-background px-3 \
     text-sm shadow-xs transition-[color,border-color,box-shadow] outline-none \
     placeholder:text-muted-foreground dark:bg-input/30 \
     file:mr-3 file:h-full file:border-0 file:bg-transparent file:text-sm file:font-medium \
     focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 \
     disabled:pointer-events-none disabled:opacity-50",
);

/// A text input component.
///
/// The `attrs` (such as `type`, `name`, `placeholder`, `disabled`, or event
/// handlers) are forwarded to the underlying `<input>`; a `class` among them
/// is appended to the computed classes. The input fills its container, so
/// size it through the container or with a width class.
///
/// ```ignore
/// view! {
///     input(attrs: attributes! { type="email" placeholder="you@example.com" })
/// }
/// ```
#[component]
pub async fn input(#[default] mut attrs: Attributes) -> Result<impl View> {
    Ok(view! { <input class=(class!(INPUT, attrs.remove("class"))) (attrs)> })
}
