//! Text helpers shared by the library and radio views: ordering strings the
//! way a reader expects, and formatting what the screens show about a track.

use std::cmp::Ordering;

use crate::library::Track;

/// Compares two labels alphabetically, ignoring case and the accents of the
/// languages Janis ships in — so "Électronique" sits with the E's rather than
/// after Z, where a plain byte comparison would put it — and what leads up to
/// the first letter or digit, so "'Round Midnight" sits with the R's and a
/// stray leading space does not put a title before every other.
pub fn compare_labels(left: &str, right: &str) -> Ordering {
    collation_key(left)
        .cmp(&collation_key(right))
        .then_with(|| left.cmp(right))
}

/// What [`compare_labels`] compares, as a key: sorting a long list by a key
/// worked out once per item spares rebuilding it at every comparison.
pub fn label_key(label: &str) -> (String, String) {
    (collation_key(label), label.to_owned())
}

fn collation_key(label: &str) -> String {
    let start = label.trim_start_matches(|character: char| !character.is_alphanumeric());
    // A label of punctuation only keeps it all, rather than sorting as blank.
    let start = if start.is_empty() { label } else { start };
    let mut key = String::with_capacity(start.len());
    for character in start.chars().flat_map(char::to_lowercase) {
        match character {
            'à'..='å' => key.push('a'),
            'ç' => key.push('c'),
            'è'..='ë' => key.push('e'),
            'ì'..='ï' => key.push('i'),
            'ñ' => key.push('n'),
            'ò'..='ö' => key.push('o'),
            'ù'..='ü' => key.push('u'),
            'ý' | 'ÿ' => key.push('y'),
            'æ' => key.push_str("ae"),
            'œ' => key.push_str("oe"),
            other => key.push(other),
        }
    }
    key
}

/// Seconds → "m:ss" (floors, clamps negatives and non-numbers to 0).
pub fn format_time(seconds: f64) -> String {
    let whole = if seconds.is_finite() {
        seconds.floor().max(0.0) as u64
    } else {
        0
    };
    format!("{}:{:02}", whole / 60, whole % 60)
}

/// The quality badge line for a track, e.g. "Hi-Res · FLAC 24/96",
/// "FLAC 16/44.1", "MP3". Hi-Res = more than 16-bit or above 48 kHz.
pub fn quality_label(track: &Track) -> String {
    let high_resolution = track.bit_depth.is_some_and(|depth| depth > 16)
        || track.sample_rate.is_some_and(|rate| rate > 48_000);
    let mut format = track.format.clone();
    if let (Some(depth), Some(rate)) = (track.bit_depth, track.sample_rate) {
        let rate = if rate % 1000 == 0 {
            (rate / 1000).to_string()
        } else {
            format!("{:.1}", f64::from(rate) / 1000.0)
        };
        format.push_str(&format!(" {depth}/{rate}"));
    }
    if high_resolution {
        format!("Hi-Res · {format}")
    } else {
        format
    }
}

/// Monogram for generated art: first two characters of the title.
pub fn art_initials(title: &str) -> String {
    if title.is_empty() {
        return "?".to_owned();
    }
    title.chars().take(2).flat_map(char::to_uppercase).collect()
}

/// How many gradients the generated art tile chooses between.
pub const ART_GRADIENT_COUNT: usize = 6;

/// Which gradient a seed's generated art uses. Deterministic, so the same
/// album always wears the same colors.
pub fn art_gradient_index(seed: &str) -> usize {
    let hash = seed.encode_utf16().fold(0i32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    });
    (i64::from(hash).unsigned_abs() % ART_GRADIENT_COUNT as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_support::track;

    #[test]
    fn labels_sort_from_their_first_letter_or_digit() {
        let mut labels = vec![
            "Zebra",
            " Telecommunication",
            "\"Live\" At Notodden",
            "'Round Midnight",
            "$ The Weed",
            "'74 Jailbreak",
            "...",
            "Électronique",
        ];
        labels.sort_by(|left, right| compare_labels(left, right));
        assert_eq!(
            labels,
            [
                "...",
                "'74 Jailbreak",
                "Électronique",
                "\"Live\" At Notodden",
                "'Round Midnight",
                " Telecommunication",
                "$ The Weed",
                "Zebra",
            ]
        );
    }

    #[test]
    fn format_time_shows_minutes_and_zero_padded_seconds() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(344.0), "5:44");
        assert_eq!(format_time(611.9), "10:11");
    }

    #[test]
    fn format_time_clamps_negatives_and_tolerates_non_numbers() {
        assert_eq!(format_time(-3.0), "0:00");
        assert_eq!(format_time(f64::NAN), "0:00");
    }

    #[test]
    fn quality_label_marks_high_resolution_when_depth_exceeds_sixteen_bits() {
        let mut flac = track(1, "A");
        flac.bit_depth = Some(24);
        flac.sample_rate = Some(96_000);
        assert_eq!(quality_label(&flac), "Hi-Res · FLAC 24/96");
    }

    #[test]
    fn quality_label_keeps_compact_disc_quality_plain_with_fractional_kilohertz() {
        let mut flac = track(1, "A");
        flac.bit_depth = Some(16);
        flac.sample_rate = Some(44_100);
        assert_eq!(quality_label(&flac), "FLAC 16/44.1");
    }

    #[test]
    fn quality_label_falls_back_to_the_bare_format_without_properties() {
        let mut lossy = track(1, "A");
        lossy.format = "MP3".to_owned();
        assert_eq!(quality_label(&lossy), "MP3");
    }

    #[test]
    fn art_initials_uppercases_the_first_two_characters() {
        assert_eq!(art_initials("Nocturne"), "NO");
        assert_eq!(art_initials(""), "?");
    }

    #[test]
    fn art_gradient_index_is_stable_and_in_range() {
        // The hash is the 31-multiplier string hash over UTF-16 units.
        assert_eq!(art_gradient_index(""), 0);
        assert_eq!(art_gradient_index("a"), 97 % 6);
        assert_eq!(art_gradient_index("ab"), (97 * 31 + 98) % 6);
        for seed in ["Miles Davis-Kind of Blue", "janis", "soma-groovesalad"] {
            assert!(art_gradient_index(seed) < ART_GRADIENT_COUNT);
            assert_eq!(art_gradient_index(seed), art_gradient_index(seed));
        }
    }

    #[test]
    fn labels_sort_without_regard_to_case_or_accents() {
        let mut labels = vec!["Rock", "électronique", "Ambient", "Zouk", "Éclectique"];
        labels.sort_by(|left, right| compare_labels(left, right));
        assert_eq!(
            labels,
            ["Ambient", "Éclectique", "électronique", "Rock", "Zouk"]
        );
    }
}
