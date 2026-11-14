//! The web-radio catalog and the filters the Radio screen offers over it.
//!
//! The curated station list ships as static JSON (`stations.json`) — a merge of
//! hand-picked flagship streams and the most-listened stations per country
//! from the Radio Browser community database, baked in at build time. No
//! network call is made to list stations.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use crate::audio::Source;
use crate::text::{compare_labels, fold};
use crate::translations::Translations;

const STATIONS: &str = include_str!("../data/stations.json");

/// The genre chip that clears the genre filter.
pub const ALL_GENRES_KEY: &str = "radio.genre.all";
/// The country chip that clears the country filter.
pub const ALL_COUNTRIES_KEY: &str = "radio.country.all";

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Station {
    pub id: String,
    pub name: String,
    /// ISO 3166-1 alpha-2 country code (uppercase) — drives the country filter.
    pub country: String,
    /// Translation key for the genre label — also the genre filter's value.
    pub genre_key: String,
    pub kbps: u32,
    pub url: String,
    /// Index into the generated-art gradients.
    #[serde(rename = "gradIndex")]
    pub gradient_index: usize,
    /// Where the station publishes what it is playing. Absent when it only
    /// carries metadata in the stream itself, or none at all.
    #[serde(default)]
    pub now_playing: Option<Source>,
}

/// Translation key for a country's name, e.g. "US" → `radio.country.us`.
pub fn country_label_key(code: &str) -> String {
    format!("radio.country.{}", code.to_lowercase())
}

pub struct Catalog {
    stations: Vec<Station>,
    /// Every genre present, in order of first appearance.
    genres: Vec<String>,
    /// Every country present, most stations first.
    countries: Vec<String>,
}

impl Catalog {
    /// The catalog compiled into the binary.
    pub fn bundled() -> Self {
        // Compiled in and covered by the tests below, so a parse failure
        // cannot reach a user.
        Self::new(serde_json::from_str(STATIONS).unwrap_or_default())
    }

    pub fn new(stations: Vec<Station>) -> Self {
        let mut genres: Vec<String> = Vec::new();
        let mut countries: Vec<String> = Vec::new();
        let mut station_counts: HashMap<&str, usize> = HashMap::new();
        for station in &stations {
            if !genres.contains(&station.genre_key) {
                genres.push(station.genre_key.clone());
            }
            let count = station_counts.entry(&station.country).or_insert(0);
            if *count == 0 {
                countries.push(station.country.clone());
            }
            *count += 1;
        }
        // Stable, so countries with as many stations keep their first-seen order.
        countries.sort_by_key(|country| std::cmp::Reverse(station_counts[country.as_str()]));
        Self {
            stations,
            genres,
            countries,
        }
    }

    #[cfg(test)]
    pub fn stations(&self) -> &[Station] {
        &self.stations
    }

    pub fn station_by_id(&self, id: &str) -> Option<&Station> {
        self.stations.iter().find(|station| station.id == id)
    }

    /// Country codes for the filter chips, most-populated first.
    pub fn countries(&self) -> &[String] {
        &self.countries
    }

    /// Genre keys for the filter chips, by their label in the current language.
    pub fn genres_by_label(&self, translations: &Translations) -> Vec<String> {
        let mut genres = self.genres.clone();
        genres.sort_by(|left, right| {
            compare_labels(&translations.text(left), &translations.text(right))
        });
        genres
    }

    /// The stations the screen shows. Both filters are multi-select and an
    /// empty one means "all"; `only`, when given, keeps the stations whose id
    /// it holds (the favourites); the search matches the station's name or
    /// its genre label in the current language.
    pub fn filtered(
        &self,
        countries: &[String],
        genres: &[String],
        only: Option<&HashSet<String>>,
        search: &str,
        translations: &Translations,
    ) -> Vec<&Station> {
        let query = fold(search.trim());
        self.stations
            .iter()
            .filter(|station| {
                (countries.is_empty() || countries.contains(&station.country))
                    && (genres.is_empty() || genres.contains(&station.genre_key))
                    && only.is_none_or(|station_ids| station_ids.contains(&station.id))
                    && (query.is_empty()
                        || fold(&station.name).contains(&query)
                        || fold(&translations.text(&station.genre_key)).contains(&query))
            })
            .collect()
    }
}

/// Flips one value of a multi-select filter.
fn toggle_filter(selected: &mut Vec<String>, value: &str) {
    match selected.iter().position(|candidate| candidate == value) {
        Some(position) => {
            selected.remove(position);
        }
        None => selected.push(value.to_owned()),
    }
}

/// The Radio screen's filters: the countries and genres chosen, both
/// multi-select with an empty one meaning "all", and whether only the starred
/// stations show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    pub countries: Vec<String>,
    pub genres: Vec<String>,
    pub favourites_only: bool,
}

impl Filters {
    /// A country chip was clicked: "All" clears the row, another flips.
    pub fn toggle_country(&mut self, key: &str) {
        if key == ALL_COUNTRIES_KEY {
            self.countries.clear();
        } else {
            toggle_filter(&mut self.countries, key);
        }
    }

    /// A genre chip was clicked: "All" clears the row, another flips.
    pub fn toggle_genre(&mut self, key: &str) {
        if key == ALL_GENRES_KEY {
            self.genres.clear();
        } else {
            toggle_filter(&mut self.genres, key);
        }
    }

    pub fn toggle_favourites(&mut self) {
        self.favourites_only = !self.favourites_only;
    }
}

/// A filter chip as the screen shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub key: String,
    pub label: String,
    pub active: bool,
}

impl Catalog {
    /// "All", active with nothing chosen, then every country, most
    /// populated first.
    pub fn country_chips(&self, filters: &Filters, translations: &Translations) -> Vec<Chip> {
        let all = Chip {
            key: ALL_COUNTRIES_KEY.to_owned(),
            label: translations.text(ALL_COUNTRIES_KEY),
            active: filters.countries.is_empty(),
        };
        std::iter::once(all)
            .chain(self.countries().iter().map(|code| Chip {
                key: code.clone(),
                label: translations.text(&country_label_key(code)),
                active: filters.countries.contains(code),
            }))
            .collect()
    }

    /// "All", active with nothing chosen, then every genre by its label.
    pub fn genre_chips(&self, filters: &Filters, translations: &Translations) -> Vec<Chip> {
        let all = Chip {
            key: ALL_GENRES_KEY.to_owned(),
            label: translations.text(ALL_GENRES_KEY),
            active: filters.genres.is_empty(),
        };
        std::iter::once(all)
            .chain(
                self.genres_by_label(translations)
                    .into_iter()
                    .map(|key| Chip {
                        label: translations.text(&key),
                        active: filters.genres.contains(&key),
                        key,
                    }),
            )
            .collect()
    }
}

/// The line under a station's name: its genre and bitrate.
pub fn station_detail(station: &Station, translations: &Translations) -> String {
    format!(
        "{} · {}",
        translations.text(&station.genre_key),
        translations.translate("radio.kbps", &[("kbps", station.kbps.to_string())])
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translations::Language;

    fn station(id: &str, name: &str, country: &str, genre: &str) -> Station {
        Station {
            id: id.to_owned(),
            name: name.to_owned(),
            country: country.to_owned(),
            genre_key: format!("radio.genre.{genre}"),
            kbps: 128,
            url: format!("https://example.invalid/{id}"),
            gradient_index: 0,
            now_playing: None,
        }
    }

    fn small_catalog() -> Catalog {
        Catalog::new(vec![
            station("one", "Groove Salad", "US", "ambient"),
            station("two", "FIP", "FR", "eclectic"),
            station("three", "FIP Jazz", "FR", "jazz"),
            station("four", "Jazz24", "US", "jazz"),
            station("five", "TSF Jazz", "FR", "jazz"),
        ])
    }

    #[test]
    fn the_bundled_catalog_parses_and_every_label_is_translated() {
        let catalog = Catalog::bundled();
        assert!(catalog.stations().len() > 1000);
        let translations = Translations::new(Language::English);
        let keys: Vec<&str> = translations.keys().collect();
        for station in catalog.stations() {
            assert!(
                keys.contains(&station.genre_key.as_str()),
                "{}",
                station.genre_key
            );
            let country = country_label_key(&station.country);
            assert!(keys.contains(&country.as_str()), "{country}");
        }
        assert!(keys.contains(&ALL_GENRES_KEY));
        assert!(keys.contains(&ALL_COUNTRIES_KEY));
    }

    #[test]
    fn the_bundled_catalog_keeps_its_now_playing_providers_and_unique_ids() {
        let catalog = Catalog::bundled();
        let groove_salad = catalog
            .station_by_id("soma-groovesalad")
            .expect("the first curated station");
        assert_eq!(
            groove_salad.now_playing,
            Some(Source::Somafm("groovesalad".to_owned()))
        );
        let mut ids: Vec<&str> = catalog.stations().iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        let total = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), total);
    }

    #[test]
    fn countries_are_listed_most_populated_first() {
        assert_eq!(small_catalog().countries(), ["FR", "US"]);
    }

    #[test]
    fn genres_are_listed_by_their_label_in_the_current_language() {
        let catalog = small_catalog();
        let english = Translations::new(Language::English);
        let genres = catalog.genres_by_label(&english);
        let mut labels: Vec<String> = genres.iter().map(|key| english.text(key)).collect();
        let listed = labels.clone();
        labels.sort_by(|left, right| compare_labels(left, right));
        assert_eq!(listed, labels);
        assert_eq!(genres.len(), 3);
    }

    #[test]
    fn empty_filters_show_everything_and_filters_combine() {
        let catalog = small_catalog();
        let translations = Translations::new(Language::English);
        let ids = |countries: &[&str], genres: &[&str], search: &str| -> Vec<String> {
            let countries: Vec<String> = countries.iter().map(|c| c.to_string()).collect();
            let genres: Vec<String> = genres.iter().map(|g| format!("radio.genre.{g}")).collect();
            catalog
                .filtered(&countries, &genres, None, search, &translations)
                .iter()
                .map(|station| station.id.clone())
                .collect()
        };

        assert_eq!(ids(&[], &[], "").len(), 5);
        assert_eq!(ids(&["FR"], &[], ""), ["two", "three", "five"]);
        assert_eq!(ids(&["FR"], &["jazz"], ""), ["three", "five"]);
        assert_eq!(ids(&["FR", "US"], &["jazz", "ambient"], "").len(), 4);
        assert_eq!(ids(&[], &[], "  fip "), ["two", "three"]);
        // The search also reads the genre label, not only the station name.
        assert_eq!(ids(&[], &[], "ambient"), ["one"]);
    }

    #[test]
    fn the_favourites_filter_keeps_only_starred_stations_within_the_other_filters() {
        let catalog = small_catalog();
        let translations = Translations::new(Language::English);
        let favourites: HashSet<String> = ["two", "four"].map(str::to_owned).into();
        let ids = |countries: &[String], only: Option<&HashSet<String>>| -> Vec<String> {
            catalog
                .filtered(countries, &[], only, "", &translations)
                .iter()
                .map(|station| station.id.clone())
                .collect()
        };

        assert_eq!(ids(&[], Some(&favourites)), ["two", "four"]);
        assert_eq!(ids(&["FR".to_owned()], Some(&favourites)), ["two"]);
        assert!(ids(&[], Some(&HashSet::new())).is_empty());
    }

    #[test]
    fn toggling_a_filter_value_adds_then_removes_it() {
        let mut selected = Vec::new();
        toggle_filter(&mut selected, "FR");
        toggle_filter(&mut selected, "US");
        assert_eq!(selected, ["FR", "US"]);
        toggle_filter(&mut selected, "FR");
        assert_eq!(selected, ["US"]);
    }

    #[test]
    fn the_radio_search_ignores_accents_in_names_and_genre_labels() {
        let catalog = Catalog::new(vec![
            station("a", "Radio Néo", "FR", "jazz"),
            station("b", "Other", "FR", "electronic"),
        ]);
        let translations = Translations::new(Language::French);
        let ids = |search: &str| -> Vec<String> {
            catalog
                .filtered(&[], &[], None, search, &translations)
                .iter()
                .map(|station| station.id.clone())
                .collect()
        };
        assert_eq!(ids("neo"), ["a"]);
        assert_eq!(ids("electro"), ["b"], "the label reads « Électronique »");
    }

    #[test]
    fn choosing_all_clears_its_row_and_a_chip_flips() {
        let mut filters = Filters::default();
        filters.toggle_country("FR");
        filters.toggle_country("DE");
        filters.toggle_genre("radio.genre.jazz");
        assert_eq!(filters.countries, ["FR", "DE"]);
        filters.toggle_country("FR");
        assert_eq!(filters.countries, ["DE"]);
        filters.toggle_country(ALL_COUNTRIES_KEY);
        assert!(filters.countries.is_empty());
        assert_eq!(filters.genres, ["radio.genre.jazz"], "the other row stays");
        filters.toggle_genre(ALL_GENRES_KEY);
        assert!(filters.genres.is_empty());
        filters.toggle_favourites();
        assert!(filters.favourites_only);
    }

    #[test]
    fn the_all_chip_is_active_only_with_nothing_chosen() {
        let catalog = small_catalog();
        let translations = Translations::new(Language::English);
        let mut filters = Filters::default();
        let active = |chips: Vec<Chip>| -> Vec<String> {
            chips
                .into_iter()
                .filter(|chip| chip.active)
                .map(|chip| chip.key)
                .collect()
        };
        assert_eq!(
            active(catalog.country_chips(&filters, &translations)),
            [ALL_COUNTRIES_KEY]
        );
        filters.toggle_country("FR");
        assert_eq!(
            active(catalog.country_chips(&filters, &translations)),
            ["FR"]
        );
        let genres = catalog.genre_chips(&filters, &translations);
        assert_eq!(genres[0].key, ALL_GENRES_KEY);
        assert_eq!(
            genres.len(),
            catalog.genres_by_label(&translations).len() + 1
        );
        assert_eq!(genres[1].label, translations.text(&genres[1].key));
    }

    #[test]
    fn a_station_is_described_by_its_genre_and_bitrate() {
        let translations = Translations::new(Language::English);
        assert_eq!(
            station_detail(&station("a", "A", "FR", "jazz"), &translations),
            format!("{} · 128 kbps", translations.text("radio.genre.jazz"))
        );
    }
}
