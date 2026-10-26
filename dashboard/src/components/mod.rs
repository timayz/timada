//! Components vendored from the topcoat UI registry with `topcoat ui add`;
//! `components.toml` records what came from where. They are ours to edit, so
//! `topcoat ui list` reports them as drifted. The local edits, kept small so a
//! future `topcoat ui add --overwrite` is a short merge:
//!
//! - `alert.rs`, `badge.rs`: `Success`, `Warning` and `Info` tones.
//! - `card.rs`: `CARD` is public, for `<section>` cards.
//! - `sidebar.rs`: the Katalyst pill (`rounded-full`) menu buttons, the
//!   small-caps group labels, `h-10` medium buttons, and `data-variant`
//!   renamed `data-sidebar-variant` (the product page's variants carry
//!   `data-variant`, and the browser tests look them up by it).
//!
//! Not every helper of every component is used.
#![allow(dead_code)]

pub mod alert;
pub mod avatar;
pub mod badge;
pub mod breadcrumb;
pub mod button;
pub mod card;
pub mod checkbox;
pub mod dialog;
pub mod dropdown_menu;
pub mod field;
pub mod floating;
pub mod input;
pub mod label;
pub mod pagination;
pub mod select;
pub mod separator;
pub mod sheet;
pub mod sidebar;
pub mod skeleton;
pub mod table;
pub mod textarea;
