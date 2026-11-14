//! The catalog being built: the curated seed, then what each Radio Browser
//! pull adds to it without repeating a station.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::classification::{believable_bitrate, classify_genre, gradient_index, NameNormalizer};
use crate::COUNTRY_CAPS;

/// The bitrate assumed for a station that reports none.
const UNREPORTED_BITRATE: f64 = 128.0;

/// How one Radio Browser answer is taken into the catalog.
pub struct Pull<'pull> {
    /// The most stations to keep from the answer.
    pub cap: usize,
    /// The country every station of a per-country pull gets. Without it the
    /// country comes from the station, and must be one of [`COUNTRY_CAPS`].
    pub fixed_country: Option<&'pull str>,
    /// The genre every station of a tag pull gets: the tag already says what
    /// the station is, so the tag classifier is not asked.
    pub forced_genre: Option<&'pull str>,
}

/// A station kept from an answer, before it is written to the catalog.
struct Candidate {
    id: String,
    name: String,
    country: String,
    bitrate: f64,
    url: String,
    tags: String,
}

pub struct Catalog {
    stations: Vec<Value>,
    seen_urls: HashSet<String>,
    seen_ids: HashSet<String>,
    names: NameNormalizer,
}

impl Catalog {
    /// Starts from the curated stations, kept as they are written.
    pub fn seeded(curated: Vec<Value>) -> Self {
        let mut seen_urls = HashSet::new();
        let mut seen_ids = HashSet::new();
        for station in &curated {
            seen_urls.insert(text(station, "url").trim().to_owned());
            seen_ids.insert(text(station, "id").to_owned());
        }
        Self {
            stations: curated,
            seen_urls,
            seen_ids,
            names: NameNormalizer::new(),
        }
    }

    pub fn into_stations(self) -> Vec<Value> {
        self.stations
    }

    /// Adds up to `pull.cap` stations of a Radio Browser answer, leaving out
    /// what the catalog already has: the same stream, the same id, or — within
    /// the answer — the same station under another bitrate or codec, of which
    /// the best bitrate is kept. Returns how many were added.
    pub fn absorb(&mut self, answer: &[Value], pull: &Pull) -> usize {
        let mut places: HashMap<String, usize> = HashMap::new();
        let mut picked: Vec<Candidate> = Vec::new();
        for station in answer {
            let name = text(station, "name").trim();
            let url = Some(text(station, "url_resolved"))
                .filter(|url| !url.is_empty())
                .unwrap_or_else(|| text(station, "url"))
                .trim();
            let country = match pull.fixed_country {
                Some(country) => country.to_owned(),
                None => text(station, "countrycode").to_uppercase(),
            };
            // What the catalog already holds is left out before the cap is
            // counted, so it does not take the place of a new station.
            if name.is_empty()
                || !is_web_address(url)
                || self.seen_urls.contains(url)
                || self.seen_ids.contains(text(station, "stationuuid"))
            {
                continue;
            }
            // The catalog stays within the countries it has labels for.
            if !COUNTRY_CAPS.iter().any(|(known, _)| *known == country) {
                continue;
            }
            let normalized = self.names.normalized(name);
            if normalized.is_empty() {
                continue;
            }
            let tags = text(station, "tags");
            let bitrate = station["bitrate"].as_f64().unwrap_or(0.0);
            if let Some(place) = places.get(&normalized) {
                let variant = &mut picked[*place];
                variant.tags.push(',');
                variant.tags.push_str(tags);
                if bitrate > variant.bitrate {
                    self.seen_urls.remove(&variant.url);
                    self.seen_urls.insert(url.to_owned());
                    variant.name = name.to_owned();
                    variant.url = url.to_owned();
                    variant.bitrate = bitrate;
                }
                continue;
            }
            self.seen_urls.insert(url.to_owned());
            places.insert(normalized, picked.len());
            picked.push(Candidate {
                id: text(station, "stationuuid").to_owned(),
                name: name.to_owned(),
                country,
                bitrate: if bitrate == 0.0 {
                    UNREPORTED_BITRATE
                } else {
                    bitrate
                },
                url: url.to_owned(),
                tags: tags.to_owned(),
            });
            if picked.len() >= pull.cap {
                break;
            }
        }

        let mut added = 0;
        for candidate in picked {
            if !self.seen_ids.insert(candidate.id.clone()) {
                continue;
            }
            let genre = pull
                .forced_genre
                .unwrap_or_else(|| classify_genre(&candidate.tags, &candidate.name));
            self.stations.push(json!({
                "id": candidate.id,
                "name": candidate.name,
                "country": candidate.country,
                "genreKey": genre,
                "kbps": believable_bitrate(candidate.bitrate),
                "url": candidate.url,
                "gradIndex": gradient_index(&candidate.id),
            }));
            added += 1;
        }
        added
    }
}

/// A text field of a station, empty when absent or not text.
fn text<'station>(station: &'station Value, field: &str) -> &'station str {
    station[field].as_str().unwrap_or_default()
}

fn is_web_address(url: &str) -> bool {
    let lowercase = url.to_ascii_lowercase();
    lowercase.starts_with("http://") || lowercase.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(id: &str, name: &str, url: &str, bitrate: u32) -> Value {
        json!({
            "stationuuid": id,
            "name": name,
            "url_resolved": url,
            "url": "http://unresolved.invalid/",
            "countrycode": "fr",
            "bitrate": bitrate,
            "tags": "jazz",
        })
    }

    fn country_pull(cap: usize) -> Pull<'static> {
        Pull {
            cap,
            fixed_country: Some("FR"),
            forced_genre: None,
        }
    }

    fn names(catalog: Catalog) -> Vec<String> {
        catalog
            .into_stations()
            .iter()
            .map(|station| text(station, "name").to_owned())
            .collect()
    }

    #[test]
    fn a_pulled_station_is_written_with_the_catalog_fields_in_order() {
        let mut catalog = Catalog::seeded(Vec::new());

        let added = catalog.absorb(
            &[station(
                "abc",
                " Jazz FM ",
                " https://jazz.example/stream ",
                192_000,
            )],
            &country_pull(5),
        );

        assert_eq!(added, 1);
        let stations = catalog.into_stations();
        assert_eq!(
            serde_json::to_string(&stations[0]).expect("a station is JSON"),
            r#"{"id":"abc","name":"Jazz FM","country":"FR","genreKey":"radio.genre.jazz","kbps":192,"url":"https://jazz.example/stream","gradIndex":0}"#
        );
    }

    #[test]
    fn a_station_the_catalog_already_has_does_not_use_up_a_place_in_the_cap() {
        let curated = json!({
            "id": "seed",
            "name": "Curated",
            "url": "https://curated.example/stream",
        });
        let mut catalog = Catalog::seeded(vec![curated]);

        let added = catalog.absorb(
            &[
                station("seed", "Same id", "https://elsewhere.example/stream", 128),
                station("one", "One", "https://one.example/stream", 128),
                station("two", "Two", "https://two.example/stream", 128),
            ],
            &country_pull(2),
        );

        assert_eq!(added, 2);
        assert_eq!(names(catalog), ["Curated", "One", "Two"]);
    }

    #[test]
    fn the_curated_seed_is_kept_verbatim_and_never_repeated() {
        let curated = json!({
            "id": "seed",
            "name": "Curated",
            "url": "https://curated.example/stream",
            "nowPlaying": { "kind": "somafm" },
        });
        let mut catalog = Catalog::seeded(vec![curated.clone()]);

        let added = catalog.absorb(
            &[
                station(
                    "other",
                    "Same stream",
                    "https://curated.example/stream",
                    128,
                ),
                station("seed", "Same id", "https://elsewhere.example/stream", 128),
            ],
            &country_pull(5),
        );

        assert_eq!(added, 0);
        assert_eq!(catalog.into_stations(), [curated]);
    }

    #[test]
    fn variants_of_one_station_collapse_onto_the_best_bitrate() {
        let mut catalog = Catalog::seeded(Vec::new());

        catalog.absorb(
            &[
                station("low", "Jazz FM (MP3 64k)", "https://jazz.example/low", 64),
                station("high", "Jazz FM", "https://jazz.example/high", 320),
                station("mid", "Jazz FM AAC", "https://jazz.example/mid", 128),
            ],
            &country_pull(5),
        );

        let stations = catalog.into_stations();
        assert_eq!(stations.len(), 1);
        // The first variant's id, the best variant's name, stream and bitrate.
        assert_eq!(stations[0]["id"], "low");
        assert_eq!(stations[0]["name"], "Jazz FM");
        assert_eq!(stations[0]["url"], "https://jazz.example/high");
        assert_eq!(stations[0]["kbps"], 320);
    }

    #[test]
    fn a_pull_stops_at_its_cap_and_skips_what_cannot_be_played_or_named() {
        let mut catalog = Catalog::seeded(Vec::new());

        catalog.absorb(
            &[
                station("1", "", "https://nameless.example/", 128),
                station("2", "Not a stream", "ftp://files.example/", 128),
                station("3", "★★★", "https://symbols.example/", 128),
                station("4", "First", "https://first.example/", 128),
                station("5", "Second", "https://second.example/", 128),
                station("6", "Third", "https://third.example/", 128),
            ],
            &country_pull(2),
        );

        assert_eq!(names(catalog), ["First", "Second"]);
    }

    #[test]
    fn a_tag_pull_keeps_known_countries_and_labels_them_with_its_genre() {
        let mut catalog = Catalog::seeded(Vec::new());
        let mut elsewhere = station("far", "Far away", "https://far.example/", 128);
        elsewhere["countrycode"] = json!("jp");

        catalog.absorb(
            &[
                elsewhere,
                station("near", "Nearby", "https://near.example/", 0),
            ],
            &Pull {
                cap: 5,
                fixed_country: None,
                forced_genre: Some("radio.genre.blues"),
            },
        );

        let stations = catalog.into_stations();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0]["country"], "FR");
        assert_eq!(stations[0]["genreKey"], "radio.genre.blues");
        assert_eq!(stations[0]["kbps"], 128);
    }

    #[test]
    fn a_station_without_a_resolved_address_uses_its_plain_one() {
        let mut catalog = Catalog::seeded(Vec::new());
        let added = catalog.absorb(
            &[json!({
                "stationuuid": "plain",
                "name": "Plain FM",
                "url_resolved": "",
                "url": "https://plain.example/stream",
                "bitrate": 128,
                "tags": "jazz",
            })],
            &country_pull(5),
        );
        assert_eq!(added, 1);
        assert_eq!(
            catalog.into_stations()[0]["url"],
            "https://plain.example/stream"
        );
    }

    #[test]
    fn the_tags_of_a_stations_variants_are_classified_together() {
        let mut catalog = Catalog::seeded(Vec::new());
        let mut untagged = station("low", "Blue Note", "https://blue.example/64", 64);
        untagged["tags"] = json!("");
        let mut tagged = station("high", "Blue Note", "https://blue.example/128", 128);
        tagged["tags"] = json!("jazz");

        catalog.absorb(&[untagged, tagged], &country_pull(5));

        let stations = catalog.into_stations();
        assert_eq!(stations.len(), 1, "one station for its two variants");
        assert_eq!(stations[0]["genreKey"], "radio.genre.jazz");
        assert_eq!(
            stations[0]["url"], "https://blue.example/128",
            "the best bitrate is kept"
        );
    }
}
