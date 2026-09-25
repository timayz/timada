//! The icons the navigation and the chrome are drawn with.
//!
//! [Lucide] 1.48.0, ISC licensed, copied in by hand. Lucide is the set
//! shadcn/ui draws from, so these are the shapes the design this admin follows
//! was built with. Topcoat can pull an icon set from Iconify instead, but that
//! downloads it during `cargo build`, and a build that needs the network is a
//! build that fails on a train.
//!
//! Every icon is a 24x24 outline that keeps its stroke on the `<svg>` rather
//! than in its body, so [`crate::ui::icon`] supplies the stroke attributes
//! once for all of them. Bodies are verbatim Lucide, reflowed onto one line.
//!
//! `styles.css` excludes this file from the Tailwind scan: path data is not
//! class names, and nothing here carries any.
//!
//! [Lucide]: https://lucide.dev

// The catalogue is complete: one glyph per section of the back office, plus
// the chrome's own. The navigation that draws most of them is still a row of
// text pills, so several are not called yet.
#![allow(dead_code)]

use topcoat::{icon::IconData, view::svg::ViewBox};

/// Every Lucide icon is drawn on the same square.
const BOX: ViewBox = ViewBox::new(0.0, 0.0, 24.0, 24.0);

/// Remboursements.
///
/// Lucide `banknote`.
pub const BANKNOTE: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<rect width="20" height="12" x="2" y="6" rx="2" /> <circle cx="12" cy="12" r="2" /> <path d="M6 12h.01M18 12h.01" />"#,
);

/// The previous page.
///
/// Lucide `chevron-left`.
pub const CHEVRON_LEFT: IconData =
    IconData::unescaped_unchecked(BOX, r#"<path d="m15 18-6-6 6-6" />"#);

/// The next page, and a breadcrumb separator.
///
/// Lucide `chevron-right`.
pub const CHEVRON_RIGHT: IconData =
    IconData::unescaped_unchecked(BOX, r#"<path d="m9 18 6-6-6-6" />"#);

/// Something went wrong.
///
/// Lucide `circle-alert`.
pub const CIRCLE_ALERT: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<circle cx="12" cy="12" r="10" /> <line x1="12" x2="12" y1="8" y2="12" /> <line x1="12" x2="12.01" y1="16" y2="16" />"#,
);

/// Factures.
///
/// Lucide `file-text`.
pub const FILE_TEXT: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z" /> <path d="M14 2v5a1 1 0 0 0 1 1h5" /> <path d="M10 9H8" /> <path d="M16 13H8" /> <path d="M16 17H8" />"#,
);

/// Catégories.
///
/// Lucide `folder-tree`.
pub const FOLDER_TREE: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M20 10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1h-2.5a1 1 0 0 1-.8-.4l-.9-1.2A1 1 0 0 0 15 3h-2a1 1 0 0 0-1 1v5a1 1 0 0 0 1 1Z" /> <path d="M20 21a1 1 0 0 0 1-1v-3a1 1 0 0 0-1-1h-2.9a1 1 0 0 1-.88-.55l-.42-.85a1 1 0 0 0-.92-.6H13a1 1 0 0 0-1 1v5a1 1 0 0 0 1 1Z" /> <path d="M3 5a2 2 0 0 0 2 2h3" /> <path d="M3 3v13a2 2 0 0 0 2 2h3" />"#,
);

/// Journal.
///
/// Lucide `history`.
pub const HISTORY: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" /> <path d="M3 3v5h5" /> <path d="M12 7v5l4 2" />"#,
);

/// An empty list.
///
/// Lucide `inbox`.
pub const INBOX: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<polyline points="22 12 16 12 14 15 10 15 8 12 2 12" /> <path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z" />"#,
);

/// Familles.
///
/// Lucide `layers`.
pub const LAYERS: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83z" /> <path d="M2 12a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 12" /> <path d="M2 17a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 17" />"#,
);

/// Signing out.
///
/// Lucide `log-out`.
pub const LOG_OUT: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="m16 17 5-5-5-5" /> <path d="M21 12H9" /> <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />"#,
);

/// E-mails.
///
/// Lucide `mail`.
pub const MAIL: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="m22 7-8.991 5.727a2 2 0 0 1-2.009 0L2 7" /> <rect x="2" y="4" width="20" height="16" rx="2" />"#,
);

/// Opens the navigation on a narrow screen.
///
/// Lucide `menu`.
pub const MENU: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M4 5h16" /> <path d="M4 12h16" /> <path d="M4 19h16" />"#,
);

/// Questions.
///
/// Lucide `message-circle`.
pub const MESSAGE_CIRCLE: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M2.992 16.342a2 2 0 0 1 .094 1.167l-1.065 3.29a1 1 0 0 0 1.236 1.168l3.413-.998a2 2 0 0 1 1.099.092 10 10 0 1 0-4.777-4.719" />"#,
);

/// The scheme the operating system asks for.
///
/// Lucide `monitor`.
pub const MONITOR: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<rect width="20" height="14" x="2" y="3" rx="2" /> <line x1="8" x2="16" y1="21" y2="21" /> <line x1="12" x2="12" y1="17" y2="21" />"#,
);

/// The dark scheme.
///
/// Lucide `moon`.
pub const MOON: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M20.985 12.486a9 9 0 1 1-9.473-9.472c.405-.022.617.46.402.803a6 6 0 0 0 8.268 8.268c.344-.215.825-.004.803.401" />"#,
);

/// Retours.
///
/// Lucide `package-open`.
pub const PACKAGE_OPEN: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M12 22v-9" /> <path d="M15.17 2.21a1.67 1.67 0 0 1 1.63 0L21 4.57a1.93 1.93 0 0 1 0 3.36L8.82 14.79a1.655 1.655 0 0 1-1.64 0L3 12.43a1.93 1.93 0 0 1 0-3.36z" /> <path d="M20 13v3.87a2.06 2.06 0 0 1-1.11 1.83l-6 3.08a1.93 1.93 0 0 1-1.78 0l-6-3.08A2.06 2.06 0 0 1 4 16.87V13" /> <path d="M21 12.43a1.93 1.93 0 0 0 0-3.36L8.83 2.2a1.64 1.64 0 0 0-1.63 0L3 4.57a1.93 1.93 0 0 0 0 3.36l12.18 6.86a1.636 1.636 0 0 0 1.63 0z" />"#,
);

/// Produits.
///
/// Lucide `package`.
pub const PACKAGE: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M11 21.73a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73z" /> <path d="M12 22V12" /> <polyline points="3.29 7 12 12 20.71 7" /> <path d="m7.5 4.27 9 5.15" />"#,
);

/// Folds the rail down to its icons.
///
/// Lucide `panel-left`.
pub const PANEL_LEFT: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<rect width="18" height="18" x="3" y="3" rx="2" /> <path d="M9 3v18" />"#,
);

/// TVA.
///
/// Lucide `percent`.
pub const PERCENT: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<line x1="19" x2="5" y1="5" y2="19" /> <circle cx="6.5" cy="6.5" r="2.5" /> <circle cx="17.5" cy="17.5" r="2.5" />"#,
);

/// Litiges.
///
/// Lucide `shield-alert`.
pub const SHIELD_ALERT: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" /> <path d="M12 8v4" /> <path d="M12 16h.01" />"#,
);

/// Commandes.
///
/// Lucide `shopping-cart`.
pub const SHOPPING_CART: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="m2.05 2.05 1.099-.028a1 1 0 0 1 1.008.815l2.69 14.347A1 1 0 0 0 7.83 18H18" /> <path d="M4.563 5h16.435a1 1 0 0 1 .981 1.204l-1.026 6.226A2 2 0 0 1 18.962 14H6.25" /> <circle cx="18" cy="20" r="2" /> <circle cx="8" cy="20" r="2" />"#,
);

/// Avis.
///
/// Lucide `star`.
pub const STAR: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.123 2.123 0 0 0 1.595 1.16l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.123 2.123 0 0 0-.611 1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.122 2.122 0 0 0-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.122 2.122 0 0 0-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.122 2.122 0 0 0 1.597-1.16z" />"#,
);

/// The light scheme.
///
/// Lucide `sun`.
pub const SUN: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<circle cx="12" cy="12" r="4" /> <path d="M12 2v2" /> <path d="M12 20v2" /> <path d="m4.93 4.93 1.41 1.41" /> <path d="m17.66 17.66 1.41 1.41" /> <path d="M2 12h2" /> <path d="M20 12h2" /> <path d="m6.34 17.66-1.41 1.41" /> <path d="m19.07 4.93-1.41 1.41" />"#,
);

/// Promotions.
///
/// Lucide `ticket-percent`.
pub const TICKET_PERCENT: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M2 9a3 3 0 1 1 0 6v2a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-2a3 3 0 1 1 0-6V7a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2Z" /> <path d="M9 9h.01" /> <path d="m15 9-6 6" /> <path d="M15 15h.01" />"#,
);

/// Équipe.
///
/// Lucide `user-cog`.
pub const USER_COG: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M10 15H6a4 4 0 0 0-4 4v2" /> <path d="m14.305 16.53.923-.382" /> <path d="m15.228 13.852-.923-.383" /> <path d="m16.852 12.228-.383-.923" /> <path d="m16.852 17.772-.383.924" /> <path d="m19.148 12.228.383-.923" /> <path d="m19.53 18.696-.382-.924" /> <path d="m20.772 13.852.924-.383" /> <path d="m20.772 16.148.924.383" /> <circle cx="18" cy="15" r="3" /> <circle cx="9" cy="7" r="4" />"#,
);

/// Clients.
///
/// Lucide `users`.
pub const USERS: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" /> <path d="M16 3.128a4 4 0 0 1 0 7.744" /> <path d="M22 21v-2a4 4 0 0 0-3-3.87" /> <circle cx="9" cy="7" r="4" />"#,
);

/// Stock.
///
/// Lucide `warehouse`.
pub const WAREHOUSE: IconData = IconData::unescaped_unchecked(
    BOX,
    r#"<path d="M18 21V10a1 1 0 0 0-1-1H7a1 1 0 0 0-1 1v11" /> <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V8a2 2 0 0 1 1.132-1.803l7.95-3.974a2 2 0 0 1 1.837 0l7.948 3.974A2 2 0 0 1 22 8z" /> <path d="M6 13h12" /> <path d="M6 17h12" />"#,
);

/// Closes it.
///
/// Lucide `x`.
pub const X: IconData =
    IconData::unescaped_unchecked(BOX, r#"<path d="M18 6 6 18" /> <path d="m6 6 12 12" />"#);
