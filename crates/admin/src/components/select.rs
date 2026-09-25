use topcoat::{
    Result,
    view::{Attributes, Child, StaticClass, View, class, component, view},
};

/// The classes for the [`select`] control.
///
/// The height, text size, radius, shadow, and focus ring match [`input`] and
/// the `Md` button, so a select sits flush beside either in a filter bar.
///
/// [`input`]: crate::components::input::input
///
/// Unlike an input, a select is not `w-full`: it has an intrinsic width — its
/// longest option — and a filter bar wants exactly that. A form that lays its
/// controls out in a column passes `w-full` itself.
///
/// The native appearance is kept rather than replaced with a drawn chevron.
/// The theme declares `color-scheme` on both palettes, so the browser already
/// draws the arrow, the popup, and its scrollbar in the operator's scheme —
/// and a popup is one thing a stylesheet cannot reach into.
const SELECT: StaticClass = class!(
    "h-9 min-w-0 rounded-md border border-input bg-background px-3 \
     text-sm shadow-xs transition-[color,border-color,box-shadow] outline-none \
     dark:bg-input/30 \
     focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 \
     disabled:pointer-events-none disabled:opacity-50",
);

/// A select component: a dropdown over a fixed set of options.
///
/// The `attrs` (such as `name`, `id`, `multiple`, or `disabled`) are forwarded
/// to the underlying `<select>`; a `class` among them is appended to the
/// computed classes. Child nodes are the `<option>` elements.
///
/// ```ignore
/// view! {
///     select(
///         attrs: attributes! { id="statut" name="statut" },
///         <option value="">"Tous"</option>
///         <option value="paid" selected=(true)>"Payées"</option>
///     )
/// }
/// ```
#[component]
pub async fn select(
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <select class=(class!(SELECT, attrs.remove("class"))) (attrs)>
            (child)
        </select>
    })
}
