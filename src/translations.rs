//! Every piece of text the user reads, in the languages Janis ships in.
//!
//! The catalogs are the JSON files under `src/lib/i18n/`, compiled into the
//! binary. English is the reference: a key missing from another language falls
//! back to it, and a key missing everywhere shows as the key itself.

use std::collections::HashMap;

const ENGLISH_CATALOG: &str = include_str!("lib/i18n/en.json");
const FRENCH_CATALOG: &str = include_str!("lib/i18n/fr.json");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    English,
    French,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::English, Language::French];

    /// The code the preferences row stores.
    pub fn code(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::French => "fr",
        }
    }

    /// The language a stored code names; anything unknown is English.
    pub fn from_code(code: &str) -> Language {
        match code {
            "fr" => Language::French,
            _ => Language::English,
        }
    }
}

pub struct Translations {
    english: HashMap<String, String>,
    french: HashMap<String, String>,
    current: Language,
}

impl Translations {
    pub fn new(current: Language) -> Self {
        Self {
            english: parse(ENGLISH_CATALOG),
            french: parse(FRENCH_CATALOG),
            current,
        }
    }

    pub fn current(&self) -> Language {
        self.current
    }

    pub fn set_current(&mut self, language: Language) {
        self.current = language;
    }

    /// The text for `key` in the current language.
    pub fn text(&self, key: &str) -> String {
        self.translate(key, &[])
    }

    /// The text for `key`, with each `{name}` placeholder replaced.
    pub fn translate(&self, key: &str, parameters: &[(&str, String)]) -> String {
        let mut value = self
            .catalog(self.current)
            .get(key)
            .or_else(|| self.english.get(key))
            .cloned()
            .unwrap_or_else(|| key.to_owned());
        for (name, replacement) in parameters {
            value = value.replace(&format!("{{{name}}}"), replacement);
        }
        value
    }

    /// Every key of the reference catalog.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.english.keys().map(String::as_str)
    }

    fn catalog(&self, language: Language) -> &HashMap<String, String> {
        match language {
            Language::English => &self.english,
            Language::French => &self.french,
        }
    }
}

fn parse(catalog: &str) -> HashMap<String, String> {
    // The catalogs are compiled in and covered by the tests below, so a parse
    // failure cannot reach a user; an empty catalog shows keys instead of text.
    serde_json::from_str(catalog).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_covers_exactly_the_reference_keys() {
        let translations = Translations::new(Language::English);
        assert!(
            !translations.english.is_empty(),
            "the English catalog parses"
        );
        let mut english: Vec<&String> = translations.english.keys().collect();
        let mut french: Vec<&String> = translations.french.keys().collect();
        english.sort();
        french.sort();
        assert_eq!(english, french);
    }

    #[test]
    fn placeholders_are_replaced_in_the_current_language() {
        let mut translations = Translations::new(Language::English);
        assert_eq!(
            translations.translate("library.tracks", &[("count", "12".to_owned())]),
            "12 tracks"
        );
        translations.set_current(Language::French);
        assert_ne!(translations.text("common.unknownAlbum"), "Unknown album");
    }

    #[test]
    fn an_unknown_key_shows_as_itself() {
        let translations = Translations::new(Language::French);
        assert_eq!(translations.text("no.such.key"), "no.such.key");
    }

    #[test]
    fn stored_codes_round_trip_and_unknown_ones_fall_back_to_english() {
        for language in Language::ALL {
            assert_eq!(Language::from_code(language.code()), language);
        }
        assert_eq!(Language::from_code("de"), Language::English);
    }
}
