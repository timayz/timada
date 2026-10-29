//! Pages bounded context: the shop's own pages, the ones a storefront links
//! from its footer (legal notice, terms of sale, privacy policy, about us).
//!
//! A page is a title and a body the merchant writes in Markdown, under a
//! slug unique in the shop (`/pages/{slug}`). It stays a draft, seen in the
//! back office only, until it is published; a published page may be listed
//! in the storefront's footer, at a position of its own, and carries the
//! title and description search engines show. Its pictures are uploaded
//! beside it and written into the body by file name
//! (`![The workshop](01J….webp)`).
//!
//! The store keeps the Markdown as written; [`render_markdown`] turns it into
//! HTML each time a page is shown, which keeps no raw HTML and no picture
//! from elsewhere. Pure domain code: no permission checks, no HTTP.

pub mod aggregator;
mod command;
mod error;
mod markdown;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use markdown::*;
pub use query::*;
pub use value_object::*;
