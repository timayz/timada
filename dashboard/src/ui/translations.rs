//! The forms a product, a category, a page or an article is worded with in the shop's other
//! languages: one card a language, each field showing the main language's
//! wording where it is left empty, which is what a visitor then reads.

use timada_catalog::{
    CategoryQuestion, ImageAlt, MAX_ALT_LEN, MAX_CATEGORY_BODY_LEN, MAX_META_DESCRIPTION_LEN,
    MAX_META_TITLE_LEN, MAX_SLUG_LEN, MAX_WORD_LEN, TranslationState, TranslationText,
};
use timada_core::languages::language_name;
use timada_pages::{TextTranslation, TextTranslationState};
use topcoat::{
    Result,
    context::Cx,
    view::{View, attributes, component, view},
};

use super::page::section_card;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::textarea::textarea;

/// What the thing says in the shop's main language, for the fields to show
/// where a translation says nothing.
pub struct MainWording {
    pub name: String,
    pub description: String,
    pub meta_title: String,
    pub meta_description: String,
    pub name_max: usize,
    pub description_max: usize,
    /// A category's buying guide and its questions (as the one textarea
    /// writes them); `None` for a product, which has neither.
    pub category: Option<(String, String)>,
    /// A product's pictures, in their order: the id of each and what the
    /// main language says it shows. None for a category.
    pub images: Vec<(String, String)>,
}

/// What the fields of the pictures' texts are named after: followed by the
/// picture's id.
pub const ALT_FIELD_PREFIX: &str = "alt:";

/// One language the thing is worded in, or may be.
pub struct TranslationCard {
    pub language: String,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub meta_title: String,
    pub meta_description: String,
    pub body: String,
    pub questions: String,
    /// What the product's pictures show, said in this language, by picture
    /// id.
    pub alts: std::collections::HashMap<String, String>,
    /// Whether the thing has a translation in this language.
    pub set: bool,
}

impl TranslationCard {
    /// What the language says the picture `image_id` shows; empty where it
    /// says nothing of its own.
    pub fn alt(&self, image_id: &str) -> &str {
        self.alts.get(image_id).map_or("", String::as_str)
    }
}

impl TranslationCard {
    /// The card of `language`, filled with the translation there is.
    pub fn of(language: &str, translation: Option<&TranslationState>) -> Self {
        let text = |part: fn(&TranslationState) -> &str| {
            translation.map(part).unwrap_or_default().to_owned()
        };
        TranslationCard {
            language: language.to_owned(),
            name: text(|t| &t.name),
            slug: text(|t| &t.slug),
            description: text(|t| &t.description),
            meta_title: text(|t| &t.meta_title),
            meta_description: text(|t| &t.meta_description),
            body: text(|t| &t.body),
            questions: translation
                .map(|t| questions_text(&t.questions))
                .unwrap_or_default(),
            alts: translation
                .map(|t| {
                    t.alts
                        .iter()
                        .map(|said| (said.image_id.clone(), said.alt.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            set: translation.is_some(),
        }
    }
}

/// The cards of `languages`, each with the translation among
/// `translations` in it.
pub fn translation_cards_of(
    languages: &[String],
    translations: &[TranslationState],
) -> Vec<TranslationCard> {
    languages
        .iter()
        .map(|language| {
            TranslationCard::of(
                language,
                translations.iter().find(|t| &t.language == language),
            )
        })
        .collect()
}

/// A card's form as it is posted.
#[derive(serde::Deserialize)]
pub struct TranslationForm {
    pub language: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub meta_title: String,
    #[serde(default)]
    pub meta_description: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub questions: String,
    /// The pictures' texts, among anything else posted.
    #[serde(flatten)]
    fields: std::collections::HashMap<String, String>,
}

impl TranslationForm {
    /// What the form words, for the catalog.
    pub fn text(&self) -> TranslationText {
        TranslationText {
            name: self.name.clone(),
            slug: Some(self.slug.clone()),
            description: self.description.clone(),
            meta_title: self.meta_title.clone(),
            meta_description: self.meta_description.clone(),
            body: self.body.clone(),
            questions: parse_questions(&self.questions),
            alts: self
                .fields
                .iter()
                .filter_map(|(field, alt)| {
                    Some(ImageAlt {
                        image_id: field.strip_prefix(ALT_FIELD_PREFIX)?.to_owned(),
                        alt: alt.clone(),
                    })
                })
                .collect(),
        }
    }
}

/// The words a product shares with the rest of the shop, as one language
/// reads them: its options, the values they take and the facts of its
/// table.
pub struct WordCard {
    pub language: String,
    /// What the card is headed with.
    title: String,
    /// A word as the main language writes it, then what the language reads
    /// for it; empty while the shop has not worded it there.
    pub words: Vec<(String, String)>,
}

impl WordCard {
    pub fn new(language: &str, words: Vec<(String, String)>) -> Self {
        WordCard {
            language: language.to_owned(),
            title: format!(
                "Options and facts in {}",
                language_name(language).unwrap_or(language)
            ),
            words,
        }
    }
}

/// What the fields of a [`WordCard`]'s form are named after: the word as
/// the main language writes it, and what the language reads for it, both
/// followed by the word's place in the card.
pub const TERM_FIELD_PREFIX: &str = "term:";
pub const WORDING_FIELD_PREFIX: &str = "wording:";

/// A word card's form as it is posted.
#[derive(serde::Deserialize)]
pub struct WordsForm {
    pub language: String,
    /// The words and their wordings, among anything else posted.
    #[serde(flatten)]
    fields: std::collections::HashMap<String, String>,
}

impl WordsForm {
    /// Each word posted with what the language reads for it; empty for a
    /// word left to the main language.
    pub fn words(&self) -> impl Iterator<Item = (&str, &str)> {
        self.fields.iter().filter_map(|(name, term)| {
            let place = name.strip_prefix(TERM_FIELD_PREFIX)?;
            let wording = self
                .fields
                .get(&format!("{WORDING_FIELD_PREFIX}{place}"))
                .map_or("", String::as_str);
            Some((term.as_str(), wording))
        })
    }
}

/// The language a "remove" button names.
#[derive(serde::Deserialize)]
pub struct TranslationLanguageForm {
    pub language: String,
}

/// What a page or an article says in the shop's main language, for the
/// fields to show where a translation says nothing.
pub struct MainText {
    pub title: String,
    pub body: String,
    pub meta_title: String,
    pub meta_description: String,
    /// An article's summary; `None` for a page, which has none and no
    /// address of its own either.
    pub summary: Option<String>,
}

/// One language a page or an article is worded in, or may be.
pub struct TextCard {
    pub language: String,
    pub title: String,
    pub slug: String,
    pub summary: String,
    pub body: String,
    pub meta_title: String,
    pub meta_description: String,
    /// Whether the text has a translation in this language.
    pub set: bool,
}

/// The cards of `languages`, each with the translation among
/// `translations` in it.
pub fn text_cards_of(languages: &[String], translations: &[TextTranslationState]) -> Vec<TextCard> {
    languages
        .iter()
        .map(|language| {
            let translation = translations.iter().find(|t| &t.language == language);
            let text = |part: fn(&TextTranslationState) -> &str| {
                translation.map(part).unwrap_or_default().to_owned()
            };
            TextCard {
                language: language.clone(),
                title: text(|t| &t.title),
                slug: text(|t| &t.slug),
                summary: text(|t| &t.summary),
                body: text(|t| &t.body),
                meta_title: text(|t| &t.meta_title),
                meta_description: text(|t| &t.meta_description),
                set: translation.is_some(),
            }
        })
        .collect()
}

/// A text card's form as it is posted.
#[derive(serde::Deserialize)]
pub struct TextTranslationForm {
    pub language: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub meta_title: String,
    #[serde(default)]
    pub meta_description: String,
}

impl TextTranslationForm {
    /// What the form words, for the pages context.
    pub fn text(&self) -> TextTranslation {
        TextTranslation {
            title: self.title.clone(),
            slug: Some(self.slug.clone()),
            summary: self.summary.clone(),
            body: self.body.clone(),
            meta_title: self.meta_title.clone(),
            meta_description: self.meta_description.clone(),
        }
    }
}

/// One card a language, with the form that words a page or an article in
/// it (posted to `save`) and, once it is worded, the button that drops the
/// wording (posted to `remove`).
#[component]
pub async fn text_translation_cards(
    cx: &Cx,
    editable: bool,
    save: &str,
    remove: &str,
    main: &MainText,
    cards: &[TextCard],
) -> Result<impl View> {
    Ok(view! {
        for card in cards {
            section_card(
                title: Some(language_name(&card.language).unwrap_or(&card.language)),
                description: Some(if card.set {
                    "What is left empty is read in the main language."
                } else {
                    "Not translated yet: visitors in this language read the main wording."
                }),
                attrs: attributes! { data-translation=(&card.language) data-translation-state=(if card.set { "set" } else { "none" }) },
                <form method="post" action=(save) class="flex max-w-3xl flex-col gap-4">
                    <input type="hidden" name="language" value=(&card.language) />
                    <fieldset disabled=(!editable) class="contents">
                        field(
                            field_label(attrs: attributes! { for=(format!("tr-{}-title", card.language)) }, "Translated title")
                            input(attrs: attributes! {
                                id=(format!("tr-{}-title", card.language)) name="title" type="text" required=""
                                maxlength=(timada_pages::MAX_TITLE_LEN) value=(&card.title) placeholder=(&main.title) lang=(&card.language)
                            })
                        )
                        if let Some(summary) = &main.summary {
                            field(
                                field_label(attrs: attributes! { for=(format!("tr-{}-slug", card.language)) }, "Address in this language")
                                field_description(attrs: attributes! { id=(format!("tr-{}-slug-hint", card.language)) }, "The last part of its address under " <code>(format!("/{}/blog/", card.language))</code> ". Leave empty to make it from the title; changing it later redirects the old address.")
                                input(attrs: attributes! {
                                    id=(format!("tr-{}-slug", card.language)) name="slug" type="text"
                                    maxlength=(MAX_SLUG_LEN) value=(&card.slug)
                                    aria-describedby=(format!("tr-{}-slug-hint", card.language)) autocomplete="off"
                                })
                            )
                            field(
                                field_label(attrs: attributes! { for=(format!("tr-{}-summary", card.language)) }, "Translated summary")
                                textarea(
                                    attrs: attributes! {
                                        id=(format!("tr-{}-summary", card.language)) name="summary" rows="2"
                                        maxlength=(timada_pages::MAX_SUMMARY_LEN) placeholder=(summary) lang=(&card.language)
                                    },
                                    (&card.summary)
                                )
                            )
                        }
                        field(
                            field_label(attrs: attributes! { for=(format!("tr-{}-body", card.language)) }, "Translated content (Markdown)")
                            field_description(attrs: attributes! { id=(format!("tr-{}-body-hint", card.language)) }, "Written as the main one is; it may show the same pictures, by the same file names.")
                            textarea(
                                attrs: attributes! {
                                    id=(format!("tr-{}-body", card.language)) name="body" rows="16"
                                    maxlength=(timada_pages::MAX_BODY_LEN) placeholder=(&main.body) lang=(&card.language)
                                    spellcheck="true" class="font-mono text-xs"
                                    aria-describedby=(format!("tr-{}-body-hint", card.language))
                                },
                                (&card.body)
                            )
                        )
                        field(
                            field_label(attrs: attributes! { for=(format!("tr-{}-meta-title", card.language)) }, "Translated title for search engines")
                            input(attrs: attributes! {
                                id=(format!("tr-{}-meta-title", card.language)) name="meta_title" type="text"
                                maxlength=(MAX_META_TITLE_LEN) value=(&card.meta_title) placeholder=(&main.meta_title) lang=(&card.language)
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for=(format!("tr-{}-meta-description", card.language)) }, "Translated description for search engines")
                            textarea(
                                attrs: attributes! {
                                    id=(format!("tr-{}-meta-description", card.language)) name="meta_description" rows="3"
                                    maxlength=(MAX_META_DESCRIPTION_LEN) placeholder=(&main.meta_description) lang=(&card.language)
                                },
                                (&card.meta_description)
                            )
                        )
                        if editable {
                            <div>button(size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save translation")</div>
                        }
                    </fieldset>
                </form>
                if editable && card.set {
                    <form method="post" action=(remove) class="mt-4">
                        <input type="hidden" name="language" value=(&card.language) />
                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Remove translation")
                    </form>
                }
            )
        }
    })
}

/// One card a language, with the form that words what the product shares
/// with the rest of the shop (posted to `save`): a field a word. None for
/// a product without options and facts.
#[component]
pub async fn word_cards(
    cx: &Cx,
    editable: bool,
    save: &str,
    cards: &[WordCard],
) -> Result<impl View> {
    Ok(view! {
        for card in cards {
            if !card.words.is_empty() {
                section_card(
                    title: Some(&card.title),
                    description: Some("A word is translated once for the whole shop: every product that carries it reads it, and so do the filters of the listings. One left empty is read in the main language."),
                    attrs: attributes! { data-words=(&card.language) },
                    <form method="post" action=(save) class="flex max-w-2xl flex-col gap-4">
                        <input type="hidden" name="language" value=(&card.language) />
                        <fieldset disabled=(!editable) class="contents">
                            for (place, (term, wording)) in card.words.iter().enumerate() {
                                field(
                                    field_label(attrs: attributes! { for=(format!("word-{}-{place}", card.language)) }, (term))
                                    <input type="hidden" name=(format!("{TERM_FIELD_PREFIX}{place}")) value=(term) />
                                    input(attrs: attributes! {
                                        id=(format!("word-{}-{place}", card.language)) name=(format!("{WORDING_FIELD_PREFIX}{place}")) type="text"
                                        maxlength=(MAX_WORD_LEN) value=(wording) placeholder=(term) lang=(&card.language) autocomplete="off"
                                    })
                                )
                            }
                            if editable {
                                <div>button(size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save words")</div>
                            }
                        </fieldset>
                    </form>
                )
            }
        }
    })
}

/// The questions as the one textarea shows them: a question on a line, its
/// answer on the lines below, an empty line between two.
pub fn questions_text(questions: &[CategoryQuestion]) -> String {
    questions
        .iter()
        .map(|asked| format!("{}\n{}", asked.question, asked.answer))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// What [`questions_text`] wrote, read back: blocks apart by empty lines,
/// each a question then its answer. A block of one line is a question
/// without an answer, which the catalog refuses.
pub fn parse_questions(text: &str) -> Vec<CategoryQuestion> {
    let text = text.replace("\r\n", "\n");
    let mut questions = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    for line in text.lines().chain([""]) {
        if !line.trim().is_empty() {
            block.push(line);
            continue;
        }
        if let Some((question, answer)) = block.split_first() {
            questions.push(CategoryQuestion {
                question: question.trim().to_owned(),
                answer: answer.join("\n"),
            });
        }
        block.clear();
    }
    questions
}

/// One card a language, with the form that words the thing in it (posted
/// to `save`) and, once it is worded, the button that drops the wording
/// (posted to `remove`).
#[component]
pub async fn translation_cards(
    cx: &Cx,
    editable: bool,
    save: &str,
    remove: &str,
    main: &MainWording,
    cards: &[TranslationCard],
) -> Result<impl View> {
    Ok(view! {
        for card in cards {
            section_card(
                title: Some(language_name(&card.language).unwrap_or(&card.language)),
                description: Some(if card.set {
                    "What is left empty is read in the main language."
                } else {
                    "Not translated yet: visitors in this language read the main wording."
                }),
                attrs: attributes! { data-translation=(&card.language) data-translation-state=(if card.set { "set" } else { "none" }) },
                <form method="post" action=(save) class="flex max-w-2xl flex-col gap-4">
                    <input type="hidden" name="language" value=(&card.language) />
                    <fieldset disabled=(!editable) class="contents">
                        field(
                            field_label(attrs: attributes! { for=(format!("tr-{}-name", card.language)) }, "Translated name")
                            input(attrs: attributes! {
                                id=(format!("tr-{}-name", card.language)) name="name" type="text" required=""
                                maxlength=(main.name_max) value=(&card.name) placeholder=(&main.name) lang=(&card.language)
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for=(format!("tr-{}-slug", card.language)) }, "Address in this language")
                            field_description(attrs: attributes! { id=(format!("tr-{}-slug-hint", card.language)) }, "The last part of its address under " <code>(format!("/{}/", card.language))</code> ". Leave empty to make it from the name; changing it later redirects the old address.")
                            input(attrs: attributes! {
                                id=(format!("tr-{}-slug", card.language)) name="slug" type="text"
                                maxlength=(MAX_SLUG_LEN) value=(&card.slug)
                                aria-describedby=(format!("tr-{}-slug-hint", card.language)) autocomplete="off"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for=(format!("tr-{}-description", card.language)) }, "Translated description")
                            textarea(
                                attrs: attributes! {
                                    id=(format!("tr-{}-description", card.language)) name="description" rows="5"
                                    maxlength=(main.description_max) placeholder=(&main.description) lang=(&card.language)
                                },
                                (&card.description)
                            )
                        )
                        if let Some((body, questions)) = &main.category {
                            field(
                                field_label(attrs: attributes! { for=(format!("tr-{}-body", card.language)) }, "Translated buying guide")
                                textarea(
                                    attrs: attributes! {
                                        id=(format!("tr-{}-body", card.language)) name="body" rows="8"
                                        maxlength=(MAX_CATEGORY_BODY_LEN) placeholder=(body) lang=(&card.language)
                                    },
                                    (&card.body)
                                )
                            )
                            field(
                                field_label(attrs: attributes! { for=(format!("tr-{}-questions", card.language)) }, "Translated questions")
                                field_description(attrs: attributes! { id=(format!("tr-{}-questions-hint", card.language)) }, "A question on a line, its answer below, an empty line between two.")
                                textarea(
                                    attrs: attributes! {
                                        id=(format!("tr-{}-questions", card.language)) name="questions" rows="8"
                                        placeholder=(questions) lang=(&card.language)
                                        aria-describedby=(format!("tr-{}-questions-hint", card.language))
                                    },
                                    (&card.questions)
                                )
                            )
                        }
                        field(
                            field_label(attrs: attributes! { for=(format!("tr-{}-meta-title", card.language)) }, "Translated title for search engines")
                            input(attrs: attributes! {
                                id=(format!("tr-{}-meta-title", card.language)) name="meta_title" type="text"
                                maxlength=(MAX_META_TITLE_LEN) value=(&card.meta_title) placeholder=(&main.meta_title) lang=(&card.language)
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for=(format!("tr-{}-meta-description", card.language)) }, "Translated description for search engines")
                            textarea(
                                attrs: attributes! {
                                    id=(format!("tr-{}-meta-description", card.language)) name="meta_description" rows="3"
                                    maxlength=(MAX_META_DESCRIPTION_LEN) placeholder=(&main.meta_description) lang=(&card.language)
                                },
                                (&card.meta_description)
                            )
                        )
                        for (place, (image_id, alt)) in main.images.iter().enumerate() {
                            field(
                                field_label(attrs: attributes! { for=(format!("tr-{}-alt-{image_id}", card.language)) }, "What picture " (place + 1) " shows, translated")
                                if !alt.is_empty() {
                                    field_description(attrs: attributes! { id=(format!("tr-{}-alt-{image_id}-hint", card.language)) }, "In the main language: " (alt))
                                }
                                input(attrs: attributes! {
                                    id=(format!("tr-{}-alt-{image_id}", card.language)) name=(format!("{ALT_FIELD_PREFIX}{image_id}")) type="text"
                                    maxlength=(MAX_ALT_LEN) value=(card.alt(image_id)) placeholder=(alt) lang=(&card.language)
                                    aria-describedby=((!alt.is_empty()).then(|| format!("tr-{}-alt-{image_id}-hint", card.language)))
                                })
                            )
                        }
                        if editable {
                            <div>button(size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save translation")</div>
                        }
                    </fieldset>
                </form>
                if editable && card.set {
                    <form method="post" action=(remove) class="mt-4">
                        <input type="hidden" name="language" value=(&card.language) />
                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Remove translation")
                    </form>
                }
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_read_back_as_they_were_written() {
        let asked = |question: &str, answer: &str| CategoryQuestion {
            question: question.to_owned(),
            answer: answer.to_owned(),
        };
        let questions = vec![
            asked("Which size?", "Measure the cat.\nAdd 10 cm."),
            asked("Washable?", "Yes."),
        ];
        assert_eq!(parse_questions(&questions_text(&questions)), questions);
        // As a browser posts it, with room around the blocks.
        assert_eq!(
            parse_questions(
                "\r\n Which size? \r\nMeasure the cat.\r\nAdd 10 cm.\r\n\r\n\r\nWashable?\r\nYes.\r\n"
            ),
            questions
        );
        // A question alone has no answer: the catalog refuses it.
        assert_eq!(parse_questions("Why?"), [asked("Why?", "")]);
        assert!(parse_questions(" \n ").is_empty());
    }
}
