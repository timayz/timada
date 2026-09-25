//! Presentation helpers shared by the admin pages.

use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::IconData,
    view::{Attributes, View, class, component, view},
};

use crate::config::AdminConfig;

mod chrome;
mod format;
pub mod icons;
pub mod rail;
pub mod theme;

pub use chrome::{empty_state, page_header, pagination, shell, stylesheet_url};
pub use format::{date, money, order_status_badge, vat_rate};

/// A [Lucide](https://lucide.dev) outline from [`icons`], an em square in the
/// current color.
///
/// Lucide keeps the stroke on the `<svg>` rather than in the icon's body, and
/// topcoat's own `icon` writes only the geometry, so the stroke attributes are
/// applied here — once, where no call site can forget them.
///
/// Without a `label` the icon is decorative and hidden from assistive
/// technology, which is what an icon beside its own text should be. Give a
/// label only when the icon *is* the label.
///
/// The size follows the surrounding text unless a `size-*` class says
/// otherwise: topcoat writes `width`/`height` as attributes, and a class beats
/// a presentation attribute.
#[component]
pub async fn icon(
    cx: &Cx,
    data: IconData,
    #[default]
    #[into]
    label: String,
    #[default] mut attrs: Attributes,
) -> Result<impl View> {
    let class = class!("shrink-0", attrs.remove("class"));
    attrs.insert(cx, "fill", "none");
    attrs.insert(cx, "stroke", "currentColor");
    attrs.insert(cx, "stroke-width", "2");
    attrs.insert(cx, "stroke-linecap", "round");
    attrs.insert(cx, "stroke-linejoin", "round");
    attrs.insert(cx, "class", class);
    Ok(view! { topcoat::icon::icon(data: data, label: label, attrs: attrs) })
}

/// The admin's own mount, as a cookie `Path`: what the operator chose in the
/// back office is the back office's business, not the shop's.
pub(crate) fn mount_path(cx: &Cx) -> String {
    format!(
        "/{}",
        app_context::<AdminConfig>(cx).mount.trim_matches('/')
    )
}
