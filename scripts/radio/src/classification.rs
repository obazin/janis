//! What a station is, worked out from what the crowd says about it: its
//! genre from its tags, a comparable name, a believable bitrate, and the
//! gradient its generated art wears.

use regex::Regex;

/// The genre a station gets when nothing about it says otherwise.
const FALLBACK_GENRE: &str = "radio.genre.variety";

/// Maps crowd tags onto the genre translation keys. A station scores one
/// point per tag that contains a keyword of a genre; the highest score wins
/// and the order here breaks ties, earlier first.
pub const GENRE_RULES: &[(&str, &[&str])] = &[
    (
        "radio.genre.dnb",
        &["drum and bass", "drum & bass", "dnb", "jungle", "liquid"],
    ),
    ("radio.genre.dubstep", &["dubstep"]),
    (
        "radio.genre.bass",
        &["bass music", "bassline", "breakbeat", "breaks"],
    ),
    ("radio.genre.trance", &["trance", "psytrance", "goa"]),
    ("radio.genre.techno", &["techno", "tech house", "minimal"]),
    (
        "radio.genre.house",
        &["house", "deep house", "progressive house"],
    ),
    (
        "radio.genre.synthwave",
        &[
            "synthwave",
            "synth-wave",
            "synthpop",
            "retrowave",
            "darkwave",
        ],
    ),
    ("radio.genre.vaporwave", &["vaporwave"]),
    ("radio.genre.idm", &["idm"]),
    ("radio.genre.industrial", &["industrial", "ebm", "gothic"]),
    (
        "radio.genre.jazz",
        &[
            "jazz",
            "smooth jazz",
            "bebop",
            "swing",
            "bigband",
            "big band",
        ],
    ),
    ("radio.genre.blues", &["blues"]),
    (
        "radio.genre.soul",
        &["soul", "rnb", "r&b", "r n b", "motown", "neo soul"],
    ),
    ("radio.genre.funk", &["funk", "disco", "groove", "boogie"]),
    (
        "radio.genre.hiphop",
        &["hip hop", "hip-hop", "hiphop", "rap", "trap", "urban"],
    ),
    (
        "radio.genre.reggae",
        &["reggae", "dancehall", "ska", "dub", "roots"],
    ),
    ("radio.genre.metal", &["metal", "heavy metal"]),
    (
        "radio.genre.rock",
        &[
            "rock",
            "classic rock",
            "hard rock",
            "punk",
            "grunge",
            "indie rock",
            "alternative rock",
            "rockabilly",
        ],
    ),
    (
        "radio.genre.indie",
        &["indie", "alternative", "shoegaze", "new wave"],
    ),
    ("radio.genre.country", &["country", "bluegrass", "honky"]),
    ("radio.genre.americana", &["americana", "roots rock"]),
    (
        "radio.genre.folk",
        &["folk", "singer-songwriter", "singer/songwriter", "acoustic"],
    ),
    ("radio.genre.celtic", &["celtic", "irish"]),
    ("radio.genre.bossanova", &["bossa nova", "bossa"]),
    (
        "radio.genre.latin",
        &["latin", "salsa", "reggaeton", "cumbia", "flamenco"],
    ),
    (
        "radio.genre.classical",
        &[
            "classical",
            "opera",
            "baroque",
            "symphony",
            "orchestral",
            "klassik",
        ],
    ),
    (
        "radio.genre.soundtrack",
        &["soundtrack", "film score", "game music", "anime"],
    ),
    (
        "radio.genre.world",
        &["world", "world music", "ethnic", "afrobeat"],
    ),
    (
        "radio.genre.electronic",
        &[
            "electronic",
            "electronica",
            "downtempo",
            "trip hop",
            "trip-hop",
            "ambient techno",
        ],
    ),
    (
        "radio.genre.dance",
        &[
            "dance",
            "edm",
            "electro",
            "club",
            "electronic dance",
            "eurodance",
            "pop dance",
        ],
    ),
    ("radio.genre.ambient", &["ambient", "drone", "meditation"]),
    (
        "radio.genre.chillout",
        &["chillout", "chill out", "chill", "relax", "lofi", "lo-fi"],
    ),
    (
        "radio.genre.lounge",
        &["lounge", "easy listening", "easy-listening"],
    ),
    (
        "radio.genre.christian",
        &[
            "christian",
            "gospel",
            "worship",
            "religious",
            "catholic",
            "christ",
        ],
    ),
    (
        "radio.genre.pop",
        &[
            "pop",
            "adult contemporary",
            "contemporary",
            "schlager",
            "kpop",
            "k-pop",
        ],
    ),
    (
        "radio.genre.hits",
        &[
            "hits",
            "top 40",
            "top40",
            "top hits",
            "charts",
            "chart",
            "top 100",
            "greatest hits",
        ],
    ),
    (
        "radio.genre.eighties",
        &["80s", "80's", "80er", "1980s", "eighties"],
    ),
    (
        "radio.genre.nineties",
        &[
            "90s", "90's", "90er", "1990s", "nineties", "2000s", "00s", "2000er", "00er",
        ],
    ),
    (
        "radio.genre.oldies",
        &[
            "oldies",
            "60s",
            "70s",
            "60's",
            "70's",
            "1960s",
            "1970s",
            "60er",
            "70er",
            "retro",
            "evergreen",
            "nostalgie",
        ],
    ),
    (
        "radio.genre.news",
        &[
            "news",
            "information",
            "local news",
            "news talk",
            "actualités",
            "aktuell",
        ],
    ),
    (
        "radio.genre.talk",
        &[
            "talk", "culture", "spoken", "sport", "sports", "politics", "comedy", "podcast",
        ],
    ),
    (
        "radio.genre.variety",
        &[
            "variety",
            "music",
            "public radio",
            "regional",
            "community",
            "mixed",
            "various",
        ],
    ),
];

/// The genre of a station from its comma-separated tags, or failing that
/// from its name.
pub fn classify_genre(tags: &str, name: &str) -> &'static str {
    let tags: Vec<String> = tags
        .split(',')
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect();
    let mut best = None;
    let mut best_score = 0;
    for (genre, keywords) in GENRE_RULES {
        let score = tags
            .iter()
            .filter(|tag| keywords.iter().any(|keyword| tag.contains(keyword)))
            .count();
        if score > best_score {
            best_score = score;
            best = Some(*genre);
        }
    }
    if let Some(genre) = best {
        return genre;
    }
    let name = name.to_lowercase();
    GENRE_RULES
        .iter()
        .find(|(_, keywords)| keywords.iter().any(|keyword| name.contains(keyword)))
        .map_or(FALLBACK_GENRE, |(genre, _)| genre)
}

/// A bitrate in kilobits per second a listener can believe: some stations
/// report bits per second, others nothing at all.
pub fn believable_bitrate(reported: f64) -> i64 {
    if reported.is_nan() || reported <= 0.0 {
        return 128;
    }
    let kilobits = if reported >= 10_000.0 {
        (reported / 1000.0).round()
    } else {
        reported
    };
    kilobits.min(640.0) as i64
}

/// Which of the six art gradients a station wears, stable for its id.
pub fn gradient_index(id: &str) -> u32 {
    id.encode_utf16().fold(0u32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(u32::from(unit))
    }) % 6
}

/// Reduces station names to what tells two stations apart, so the bitrate
/// and codec variants of one station within a country collapse into one.
pub struct NameNormalizer {
    variant_marks: Regex,
    separators: Regex,
    foreign_characters: Regex,
    spaces: Regex,
}

impl NameNormalizer {
    pub fn new() -> Self {
        // The patterns are constants: a mistake in one is a programming error.
        let pattern = |text: &str| Regex::new(text).expect("the pattern is valid");
        Self {
            // Word boundaries and digits are the ASCII ones.
            variant_marks: pattern(
                r"(?-u:\b)(mp3|aac\+?|aacp|ogg|flac|opus|hls|128k?|192k?|256k?|320k?|64k?|96k?|48k?|[0-9]{2,3}\s?kbps)(?-u:\b)",
            ),
            separators: pattern(r"[|(){}\[\]\-–—.]"),
            foreign_characters: pattern(r"[^a-z0-9äöüßàâçéèêëîïôûùüÿñæœ ]"),
            spaces: pattern(r"\s+"),
        }
    }

    pub fn normalized(&self, name: &str) -> String {
        let name = name.to_lowercase();
        let name = self.variant_marks.replace_all(&name, "");
        let name = self.separators.replace_all(&name, " ");
        let name = self.foreign_characters.replace_all(&name, "");
        let name = self.spaces.replace_all(&name, " ");
        name.trim().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_genre_with_the_most_matching_tags_wins_and_the_first_rule_breaks_ties() {
        assert_eq!(
            classify_genre("jazz,smooth jazz,rock", ""),
            "radio.genre.jazz"
        );
        // One tag each for techno and house: techno comes first in the rules.
        assert_eq!(classify_genre("House, Techno", ""), "radio.genre.techno");
        assert_eq!(classify_genre(" , ,", ""), "radio.genre.variety");
    }

    #[test]
    fn a_station_without_telling_tags_is_classified_by_its_name() {
        assert_eq!(
            classify_genre("", "Radio Blues Corner"),
            "radio.genre.blues"
        );
        assert_eq!(classify_genre("unknown tag", "KXYZ"), "radio.genre.variety");
    }

    #[test]
    fn every_rule_names_a_genre_key_and_lowercase_keywords() {
        assert!(GENRE_RULES
            .iter()
            .any(|(genre, _)| *genre == FALLBACK_GENRE));
        for (genre, keywords) in GENRE_RULES {
            assert!(genre.starts_with("radio.genre."), "{genre}");
            assert!(!keywords.is_empty(), "{genre}");
            for keyword in *keywords {
                assert_eq!(keyword.to_lowercase(), *keyword, "{genre}");
            }
        }
    }

    #[test]
    fn a_bitrate_is_made_believable() {
        assert_eq!(believable_bitrate(0.0), 128);
        assert_eq!(believable_bitrate(-5.0), 128);
        assert_eq!(believable_bitrate(192.0), 192);
        // Reported in bits per second.
        assert_eq!(believable_bitrate(128_000.0), 128);
        assert_eq!(believable_bitrate(2000.0), 640);
    }

    #[test]
    fn the_gradient_is_stable_and_one_of_six() {
        assert_eq!(gradient_index(""), 0);
        assert_eq!(gradient_index("a"), 97 % 6);
        assert_eq!(gradient_index("ab"), (97 * 31 + 98) % 6);
        let id = "960559b0-0601-11e8-ae97-52543be04c81";
        assert!(gradient_index(id) < 6);
        assert_eq!(gradient_index(id), gradient_index(id));
    }

    #[test]
    fn names_lose_their_codec_and_bitrate_marks_and_their_punctuation() {
        let normalizer = NameNormalizer::new();
        assert_eq!(
            normalizer.normalized("Radio Swiss Jazz (MP3 128k)"),
            "radio swiss jazz"
        );
        assert_eq!(normalizer.normalized("FIP - 160 kbps"), "fip");
        assert_eq!(
            normalizer.normalized("Antenne Bayern AAC+"),
            "antenne bayern"
        );
        assert_eq!(normalizer.normalized("Ö3 | Hitradio"), "ö3 hitradio");
        assert_eq!(normalizer.normalized("  ★★★  "), "");
        // A mark is only dropped as a whole word.
        assert_eq!(normalizer.normalized("Radio 1280"), "radio 1280");
    }
}
