//! Rebuilds the web-radio catalog at `data/stations.json`.
//!
//! It merges two sources:
//!   1. `curated.json` — the hand-picked flagship stations (SomaFM, Radio
//!      France, Radio Paradise, …), kept verbatim including their now-playing
//!      wiring.
//!   2. The Radio Browser community database (<https://www.radio-browser.info>):
//!      the most-listened working stations per target country, plus
//!      tag-targeted top-ups for a few genres (see [`GENRE_PULLS`]), fetched
//!      live, deduplicated, and mapped onto the genre translation keys.
//!
//! The result is static data compiled into the application — nothing here
//! runs when Janis does. Run it to refresh the catalog: `just stations`.
//!
//! New genre keys must also exist in `translations/{en,fr}.json`, and new
//! country codes need a `radio.country.<cc>` key in both; the application's
//! tests check it. See `README.md` beside this package.

mod catalog;
mod classification;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use catalog::{Catalog, Pull};

const USER_AGENT: &str = "Janis/0.1 (+https://github.com/obazin/janis)";
const MIRRORS: [&str; 2] = [
    "https://de1.api.radio-browser.info",
    "https://nl1.api.radio-browser.info",
];
/// The wait between two requests, to be polite to the mirror.
const PAUSE_BETWEEN_REQUESTS: Duration = Duration::from_millis(400);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// How many freshly pulled stations to keep per country (the curated seed adds
/// on top), in the order the countries are pulled. Sums to about 880; with
/// the hundred or so curated ones that lands the catalog near a thousand.
pub const COUNTRY_CAPS: [(&str, usize); 22] = [
    ("US", 130),
    ("GB", 80),
    ("DE", 80),
    ("FR", 55),
    ("IT", 55),
    ("ES", 50),
    ("BR", 45),
    ("NL", 45),
    ("PL", 40),
    ("GR", 35),
    ("BE", 30),
    ("SE", 30),
    ("PT", 30),
    ("CH", 25),
    ("AT", 25),
    ("IE", 25),
    ("DK", 25),
    ("NO", 25),
    ("FI", 25),
    ("CZ", 25),
    ("RO", 25),
    ("HU", 20),
];

/// A tag-targeted top-up for a genre the per-country lists leave thin (a
/// country's most-clicked stations skew pop and news). Pulled by exact tag,
/// kept to the [`COUNTRY_CAPS`] countries, and labelled with `genre` outright.
struct GenrePull {
    tag: &'static str,
    genre: &'static str,
    cap: usize,
}

const GENRE_PULLS: [GenrePull; 7] = [
    GenrePull {
        tag: "blues",
        genre: "radio.genre.blues",
        cap: 45,
    },
    GenrePull {
        tag: "funk",
        genre: "radio.genre.funk",
        cap: 35,
    },
    GenrePull {
        tag: "jazz",
        genre: "radio.genre.jazz",
        cap: 45,
    },
    GenrePull {
        tag: "rock",
        genre: "radio.genre.rock",
        cap: 45,
    },
    GenrePull {
        tag: "pop",
        genre: "radio.genre.pop",
        cap: 45,
    },
    GenrePull {
        tag: "classical",
        genre: "radio.genre.classical",
        cap: 45,
    },
    GenrePull {
        tag: "bossa nova",
        genre: "radio.genre.bossanova",
        cap: 30,
    },
];

/// Where the tool reads from and writes to.
struct Options {
    mirrors: Vec<String>,
    output: PathBuf,
}

impl Options {
    /// `--mirror <url>` asks one Radio Browser mirror instead of the built-in
    /// ones; `--output <file>` writes somewhere other than the catalog.
    fn from_arguments(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Self {
            mirrors: MIRRORS.map(str::to_owned).to_vec(),
            output: package_directory().join("../../data/stations.json"),
        };
        let mut arguments = arguments;
        while let Some(argument) = arguments.next() {
            let mut value = || {
                arguments
                    .next()
                    .ok_or_else(|| format!("{argument} needs a value"))
            };
            match argument.as_str() {
                "--mirror" => options.mirrors = vec![value()?.trim_end_matches('/').to_owned()],
                "--output" => options.output = PathBuf::from(value()?),
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(options)
    }
}

fn package_directory() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("\n{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let options = Options::from_arguments(std::env::args().skip(1))?;
    let agent = ureq::AgentBuilder::new()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .build();

    let curated_path = package_directory().join("curated.json");
    let curated = std::fs::read_to_string(&curated_path)
        .map_err(|error| format!("read {}: {error}", curated_path.display()))?;
    let curated: Vec<Value> = serde_json::from_str(&curated)
        .map_err(|error| format!("parse {}: {error}", curated_path.display()))?;

    // The curated seed first: highest priority, it keeps its ids and wiring.
    let mut catalog = Catalog::seeded(curated);

    // Then the fresh Radio Browser pulls, country by country.
    for (country, cap) in COUNTRY_CAPS {
        let path = format!(
            "/json/stations/bycountrycodeexact/{country}?order=clickcount&reverse=true&hidebroken=true&limit=150"
        );
        let stations = fetch(&agent, &options.mirrors, &path, country)?;
        let added = catalog.absorb(
            &stations,
            &Pull {
                cap,
                fixed_country: Some(country),
                forced_genre: None,
            },
        );
        eprint!("{country}:{added} ");
        std::thread::sleep(PAUSE_BETWEEN_REQUESTS);
    }

    // Then the tag-targeted pulls. Most results of a tag fall outside the
    // countries kept, so the request casts wide and `absorb` narrows it.
    for pull in &GENRE_PULLS {
        let path = format!(
            "/json/stations/bytagexact/{}?order=clickcount&reverse=true&hidebroken=true&limit=300",
            percent_encoded(pull.tag)
        );
        let stations = fetch(&agent, &options.mirrors, &path, pull.tag)?;
        let added = catalog.absorb(
            &stations,
            &Pull {
                cap: pull.cap,
                fixed_country: None,
                forced_genre: Some(pull.genre),
            },
        );
        eprint!("{}:{added} ", pull.tag);
        std::thread::sleep(PAUSE_BETWEEN_REQUESTS);
    }
    eprintln!();

    let stations = catalog.into_stations();
    let mut text = serde_json::to_string_pretty(&stations)
        .map_err(|error| format!("write the catalog as JSON: {error}"))?;
    text.push('\n');
    std::fs::write(&options.output, text)
        .map_err(|error| format!("write {}: {error}", options.output.display()))?;

    println!("TOTAL {}", stations.len());
    println!("COUNTRIES {}", breakdown(&stations, "country", ""));
    println!(
        "GENRES {}",
        breakdown(&stations, "genreKey", "radio.genre.")
    );
    println!("wrote {}", options.output.display());
    Ok(())
}

/// Asks each mirror in turn for `path` and returns the first answer.
fn fetch(
    agent: &ureq::Agent,
    mirrors: &[String],
    path: &str,
    label: &str,
) -> Result<Vec<Value>, String> {
    let mut last_error = String::from("no mirror to ask");
    for mirror in mirrors {
        let answer = agent
            .get(&format!("{mirror}{path}"))
            .call()
            .map_err(|error| error.to_string())
            .and_then(|response| {
                response
                    .into_json::<Vec<Value>>()
                    .map_err(|error| error.to_string())
            });
        match answer {
            Ok(stations) => return Ok(stations),
            Err(error) => last_error = error,
        }
    }
    Err(format!("fetch {label} failed: {last_error}"))
}

/// `text` as one segment of a URL path: everything but unreserved characters
/// is written as percent-encoded bytes.
fn percent_encoded(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// How many stations carry each value of `field`, most first, as
/// `value:count` pairs; `prefix` is dropped from the values shown.
fn breakdown(stations: &[Value], field: &str, prefix: &str) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for station in stations {
        let value = station[field].as_str().unwrap_or_default();
        match counts.iter_mut().find(|(counted, _)| *counted == value) {
            Some((_, count)) => *count += 1,
            None => counts.push((value, 1)),
        }
    }
    // Stable: values with as many stations keep their first-seen order.
    counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    counts
        .iter()
        .map(|(value, count)| format!("{}:{count}", value.strip_prefix(prefix).unwrap_or(value)))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn arguments(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|argument| (*argument).to_owned())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn a_tag_is_encoded_as_a_path_segment() {
        assert_eq!(percent_encoded("bossa nova"), "bossa%20nova");
        assert_eq!(percent_encoded("r&b"), "r%26b");
        assert_eq!(percent_encoded("actualités"), "actualit%C3%A9s");
        assert_eq!(percent_encoded("rock"), "rock");
    }

    #[test]
    fn the_breakdown_lists_the_most_frequent_first_and_ties_as_first_seen() {
        let stations = [
            json!({ "genreKey": "radio.genre.rock" }),
            json!({ "genreKey": "radio.genre.jazz" }),
            json!({ "genreKey": "radio.genre.pop" }),
            json!({ "genreKey": "radio.genre.jazz" }),
        ];
        assert_eq!(
            breakdown(&stations, "genreKey", "radio.genre."),
            "jazz:2 rock:1 pop:1"
        );
    }

    #[test]
    fn the_catalog_is_written_beside_the_application_unless_told_otherwise() {
        let defaults = Options::from_arguments(arguments(&[])).expect("no argument is fine");
        assert!(defaults.output.ends_with("data/stations.json"));
        assert_eq!(defaults.mirrors.len(), MIRRORS.len());

        let chosen = Options::from_arguments(arguments(&[
            "--mirror",
            "http://localhost:8000/",
            "--output",
            "/tmp/stations.json",
        ]))
        .expect("both options are known");
        assert_eq!(chosen.mirrors, ["http://localhost:8000"]);
        assert_eq!(chosen.output, PathBuf::from("/tmp/stations.json"));

        assert!(Options::from_arguments(arguments(&["--mirror"])).is_err());
        assert!(Options::from_arguments(arguments(&["--elsewhere"])).is_err());
    }

    #[test]
    fn every_genre_pull_names_a_genre_the_rules_know() {
        for pull in &GENRE_PULLS {
            assert!(
                classification::GENRE_RULES
                    .iter()
                    .any(|(genre, _)| *genre == pull.genre),
                "{}",
                pull.genre
            );
        }
    }
}
