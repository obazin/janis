//! Every piece of text the user reads, in the languages Janis ships in.
//!
//! The catalogs are the JSON files under `translations/`, compiled into the
//! binary. English is the reference: a key missing from another language falls
//! back to it, and a key missing everywhere shows as the key itself.

use std::collections::HashMap;

const ENGLISH_CATALOG: &str = include_str!("../translations/en.json");
const FRENCH_CATALOG: &str = include_str!("../translations/fr.json");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    English,
    French,
}

impl Language {
    #[cfg(test)]
    pub const ALL: [Language; 2] = [Language::English, Language::French];

    /// The code the preferences row stores.
    pub fn code(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::French => "fr",
        }
    }

    /// Which of a count's two wordings, `.one` or `.other`, the language
    /// uses: English says "1 track" but "0 tracks", French "0 titre" too.
    fn plural_form(self, count: usize) -> &'static str {
        let singular = match self {
            Language::English => count == 1,
            Language::French => count <= 1,
        };
        if singular {
            "one"
        } else {
            "other"
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
        Self::from_catalogs(ENGLISH_CATALOG, FRENCH_CATALOG, current)
    }

    fn from_catalogs(english: &str, french: &str, current: Language) -> Self {
        Self {
            english: parse(english),
            french: parse(french),
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

    /// The wording of `count` things: `key.one` or `key.other`, whichever the
    /// current language uses for that number, with `{count}` replaced.
    pub fn count(&self, key: &str, count: usize) -> String {
        self.count_with(key, count, &[])
    }

    /// `count`, for a wording with other placeholders besides `{count}`.
    pub fn count_with(&self, key: &str, count: usize, parameters: &[(&str, String)]) -> String {
        let form = self.current.plural_form(count);
        let mut all = vec![("count", count.to_string())];
        all.extend_from_slice(parameters);
        self.translate(&format!("{key}.{form}"), &all)
    }

    /// Every key of the reference catalog.
    #[cfg(test)]
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
            translations.translate("error.radio", &[("name", "FIP".to_owned())]),
            "Couldn't connect to FIP"
        );
        translations.set_current(Language::French);
        assert_ne!(translations.text("common.unknownAlbum"), "Unknown album");
    }

    #[test]
    fn a_count_takes_the_wording_its_language_uses_for_that_number() {
        let mut translations = Translations::new(Language::English);
        let english: Vec<String> = [0, 1, 2]
            .map(|count| translations.count("library.tracks", count))
            .into();
        assert_eq!(english, ["0 tracks", "1 track", "2 tracks"]);
        translations.set_current(Language::French);
        let french: Vec<String> = [0, 1, 2]
            .map(|count| translations.count("library.tracks", count))
            .into();
        assert_eq!(french, ["0 titre", "1 titre", "2 titres"]);
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

    #[test]
    fn a_key_missing_from_french_falls_back_to_english() {
        let translations = Translations::from_catalogs(
            r#"{"greeting": "Hello", "farewell": "Bye"}"#,
            r#"{"greeting": "Bonjour"}"#,
            Language::French,
        );
        assert_eq!(translations.text("greeting"), "Bonjour");
        assert_eq!(translations.text("farewell"), "Bye");
    }

    #[test]
    fn both_catalogs_use_the_same_placeholders_for_each_key() {
        let translations = Translations::new(Language::English);
        let placeholders = |text: &str| -> Vec<String> {
            let mut names: Vec<String> = text
                .split('{')
                .skip(1)
                .filter_map(|rest| rest.split_once('}').map(|(name, _)| name.to_owned()))
                .collect();
            names.sort();
            names
        };
        for (key, english) in &translations.english {
            let french = translations.french.get(key).expect("the same keys");
            assert_eq!(placeholders(english), placeholders(french), "{key}");
        }
    }

    /// Every file under `directory` with `extension`, read whole.
    fn sources(directory: &std::path::Path, extension: &str) -> Vec<String> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(directory).expect("a source directory") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                found.extend(sources(&path, extension));
            } else if path.extension().is_some_and(|found| found == extension) {
                found.push(std::fs::read_to_string(&path).expect("a source file"));
            }
        }
        found
    }

    /// The string literals of Rust sources that look like translation keys:
    /// letters and dots, starting with the first part of a catalog key.
    fn key_literals(rust: &[String], prefixes: &[&str]) -> Vec<String> {
        rust.iter()
            .flat_map(|source| source.split('"').skip(1).step_by(2))
            .filter(|literal| {
                literal.contains('.')
                    && !literal.ends_with('.')
                    && literal
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '.')
                    && literal
                        .split_once('.')
                        .is_some_and(|(prefix, _)| prefixes.contains(&prefix))
            })
            .map(str::to_owned)
            .collect()
    }

    fn root() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn every_key_rust_names_is_in_the_catalog() {
        let translations = Translations::new(Language::English);
        let mut prefixes: Vec<&str> = translations
            .keys()
            .filter_map(|key| key.split_once('.').map(|(prefix, _)| prefix))
            .collect();
        prefixes.sort_unstable();
        prefixes.dedup();
        let rust = sources(&root().join("src"), "rs");
        let known = |key: &str| {
            translations.english.contains_key(key)
                || (translations.english.contains_key(&format!("{key}.one"))
                    && translations.english.contains_key(&format!("{key}.other")))
        };
        let missing: Vec<String> = key_literals(&rust, &prefixes)
            .into_iter()
            .filter(|literal| !known(literal))
            .collect();
        assert!(
            missing.is_empty(),
            "keys missing from the catalog: {missing:?}"
        );
    }

    #[test]
    fn every_catalog_key_is_used_somewhere() {
        let translations = Translations::new(Language::English);
        let rust = sources(&root().join("src"), "rs").join("\n");
        let slint = sources(&root().join("ui"), "slint").join("\n");
        let stations = std::fs::read_to_string(root().join("data/stations.json"))
            .expect("the station catalog");
        // Keys built at run time from a prefix: `format!("eq.preset.{name}")`.
        let built_prefixes: Vec<&str> = rust
            .split('"')
            .skip(1)
            .step_by(2)
            .filter_map(|literal| literal.split_once('{').map(|(prefix, _)| prefix))
            .filter(|prefix| prefix.ends_with('.'))
            .collect();
        let used = |key: &str| {
            let base = key
                .strip_suffix(".one")
                .or_else(|| key.strip_suffix(".other"))
                .unwrap_or(key);
            rust.contains(&format!("\"{key}\""))
                || rust.contains(&format!("\"{base}\""))
                || slint.contains(&format!(
                    "Strings.{}",
                    crate::string_names::property_name(key)
                ))
                || stations.contains(&format!("\"{key}\""))
                || built_prefixes.iter().any(|prefix| key.starts_with(prefix))
        };
        let mut unused: Vec<&str> = translations.keys().filter(|key| !used(key)).collect();
        unused.sort_unstable();
        assert!(unused.is_empty(), "catalog keys nothing uses: {unused:?}");
    }
}
