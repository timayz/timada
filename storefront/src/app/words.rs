//! The storefront's own sentences: every button, heading and message of
//! the pages, in each language this storefront speaks.
//!
//! They live in [Fluent](https://projectfluent.org/) files, one directory
//! per language (`l10n/fr`, `l10n/en`) and one file per part of the shop,
//! under the same keys in each. A page names a sentence by its key,
//! `t!(cart_empty)`, and gives what it holds, `t!(cart_lines, count)`; the
//! sentence is read in the language of the visitor's address, which
//! [`Localized`](super::lang::Localized) puts in the request.
//!
//! To change a sentence, edit it in the files; to speak another language,
//! add its directory and name it in [`SPOKEN`](super::lang::SPOKEN).
//! `topcoat l10n check` tells a key a language lacks.

use std::borrow::Cow;

use topcoat::{
    context::Cx,
    l10n::Translation,
    view::{AttributeValueViewParts, NodeViewParts, PartsWriter},
};

pub(crate) mod catalogue {
    topcoat::l10n::fluent::messages!(pub(crate) "./l10n/{locale}/*.ftl");
}

/// A sentence of the catalogue, by its key and then what it holds:
/// `t!(home_title)`, `t!(cart_lines, count)`, `t!(order_total, total: sum)`.
/// A view shows it as is, as text or as an attribute's value; anywhere else,
/// [`Worded::resolve`] reads it.
macro_rules! t {
    ($($input:tt)*) => {
        $crate::app::words::Worded::new($crate::app::words::catalogue::t!($($input)*))
    };
}
pub(crate) use t;

/// The marks Fluent sets around what a sentence holds, for a name written
/// right to left inside a sentence written left to right.
const ISOLATES: [char; 2] = ['\u{2068}', '\u{2069}'];

/// A sentence and what it holds, read in the visitor's language when it is
/// shown.
#[derive(Clone)]
pub struct Worded<'a>(Translation<'a>);

impl<'a> Worded<'a> {
    pub fn new(sentence: Translation<'a>) -> Self {
        Worded(sentence)
    }

    /// The sentence in the language of the request. Without Fluent's
    /// isolation marks: the languages spoken here are all written left to
    /// right, and a sentence is also an attribute a script reads, a title,
    /// a value compared as it stands.
    pub fn resolve(&self, cx: &Cx) -> Cow<'static, str> {
        match self.0.resolve(cx) {
            Cow::Owned(text) if text.contains(ISOLATES) => Cow::Owned(text.replace(ISOLATES, "")),
            text => text,
        }
    }

    fn push(&self, cx: &Cx, parts: &mut PartsWriter<'_>) {
        match self.resolve(cx) {
            Cow::Borrowed(text) => parts.push_static_str(text),
            Cow::Owned(text) => parts.push_string(text),
        };
    }
}

impl NodeViewParts for Worded<'_> {
    fn into_view_parts(self, cx: &Cx, parts: &mut PartsWriter<'_>) {
        self.push(cx, parts);
    }
}

impl AttributeValueViewParts for Worded<'_> {
    fn attribute_present(&self) -> bool {
        true
    }

    fn into_view_parts(self, cx: &Cx, parts: &mut PartsWriter<'_>) {
        self.push(cx, parts);
    }
}
