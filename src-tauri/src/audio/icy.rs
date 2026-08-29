//! Turning an ICY `StreamTitle` into structured track info.
//!
//! There is no standard for what a station puts in that string. `Artist -
//! Title` is the common shape, but the curated list also contains WFMU's
//! `"Title" by Artist on Album on WFMU`, Venice Classic's trailing
//! `{+info: …}` annotation, and hosts that never fill it in at all.
//!
//! So this is a heuristic, and it says so: every field is optional and the
//! caller keeps the raw string to fall back on. The tests are real strings
//! captured from the stations Janis ships.

/// What a station is announcing. Only `title` is ever certain.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackInfo {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
}

/// Strings some hosts send when nobody has configured the metadata. Treated
/// as "no information" rather than shown to the listener as a track name.
const PLACEHOLDERS: &[&str] = &["now playing info goes here", "unknown", "no title", "-"];

/// Parses a raw `StreamTitle`. `None` means the station said nothing useful.
pub fn parse(raw: &str) -> Option<TrackInfo> {
    let cleaned = strip_annotation(raw.trim());
    if cleaned.is_empty() || PLACEHOLDERS.contains(&cleaned.to_ascii_lowercase().as_str()) {
        return None;
    }

    if let Some(info) = parse_quoted_by(cleaned) {
        return Some(info);
    }

    // The common case: `Artist - Title`. Split on the first separator, so a
    // title that itself contains " - " stays intact.
    match cleaned.split_once(" - ") {
        Some((artist, title)) => {
            let artist = artist.trim();
            let title = title.trim();
            if artist.is_empty() || title.is_empty() {
                return Some(TrackInfo {
                    title: cleaned.to_string(),
                    ..Default::default()
                });
            }
            Some(TrackInfo {
                title: title.to_string(),
                artist: Some(artist.to_string()),
                album: None,
            })
        }
        None => Some(TrackInfo {
            title: cleaned.to_string(),
            ..Default::default()
        }),
    }
}

/// Drops a trailing `{…}` note. Venice Classic appends its own web address
/// to every title this way, which is not part of the music.
fn strip_annotation(value: &str) -> &str {
    match (value.rfind(" {"), value.ends_with('}')) {
        (Some(at), true) => value[..at].trim_end(),
        _ => value,
    }
}

/// WFMU's shape: `"Title" by Artist on Album on Station`.
fn parse_quoted_by(value: &str) -> Option<TrackInfo> {
    let rest = value.strip_prefix('"')?;
    let (title, rest) = rest.split_once("\" by ")?;
    let title = title.trim();
    if title.is_empty() {
        return None;
    }

    let parts: Vec<&str> = rest.split(" on ").map(str::trim).collect();
    let artist = parts.first().filter(|p| !p.is_empty())?;
    // A third segment is the station's own name, not the record.
    let album = parts
        .get(1)
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string());

    Some(TrackInfo {
        title: title.to_string(),
        artist: Some((*artist).to_string()),
        album,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(title: &str, artist: Option<&str>, album: Option<&str>) -> Option<TrackInfo> {
        Some(TrackInfo {
            title: title.to_string(),
            artist: artist.map(str::to_string),
            album: album.map(str::to_string),
        })
    }

    // Every string below was captured from a station in the curated list.

    #[test]
    fn splits_the_common_artist_title_shape() {
        assert_eq!(
            parse("Alex Cortiz - Vibe 05"),
            info("Vibe 05", Some("Alex Cortiz"), None)
        );
        assert_eq!(
            parse("Robert Plant - Another Tribe"),
            info("Another Tribe", Some("Robert Plant"), None)
        );
        assert_eq!(
            parse("JAMES HYPE - Be Mine"),
            info("Be Mine", Some("JAMES HYPE"), None)
        );
    }

    #[test]
    fn trims_the_padding_stations_leave_behind() {
        assert_eq!(
            parse("Jonah Jones Quartet - The Blues Don't Care "),
            info("The Blues Don't Care", Some("Jonah Jones Quartet"), None)
        );
        assert_eq!(
            parse("Oli Silk - At Your Service feat Julian Vaughn "),
            info("At Your Service feat Julian Vaughn", Some("Oli Silk"), None)
        );
    }

    #[test]
    fn keeps_a_title_that_contains_the_separator() {
        // Only the first " - " separates; the rest belongs to the work.
        assert_eq!(
            parse("Jacques Offenbach (1819-1880) - 'Barbe-Bleue' - Ouverture"),
            info(
                "'Barbe-Bleue' - Ouverture",
                Some("Jacques Offenbach (1819-1880)"),
                None
            )
        );
    }

    #[test]
    fn strips_a_trailing_station_annotation() {
        assert_eq!(
            parse(
                "Jacques Offenbach (1819-1880) - 'Barbe-Bleue' - Ouverture  \
                 {+info: veniceclassicradio.eu}"
            )
            .unwrap()
            .title,
            "'Barbe-Bleue' - Ouverture",
        );
    }

    #[test]
    fn understands_the_quoted_by_shape() {
        assert_eq!(
            parse(r#""Love Me Till the Sun Shines" by Dave Davies on Optical Sound on WFMU"#),
            info(
                "Love Me Till the Sun Shines",
                Some("Dave Davies"),
                Some("Optical Sound")
            ),
            "the trailing station name is not an album"
        );
    }

    #[test]
    fn placeholders_count_as_no_information() {
        assert_eq!(parse("Now Playing info goes here"), None);
        assert_eq!(parse("   "), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("-"), None);
        assert_eq!(parse("Unknown"), None, "matching is case-insensitive");
    }

    #[test]
    fn a_show_name_survives_as_a_plain_title() {
        // DJ radio announces the show, not a track. There is nothing to split
        // and nothing to invent — keep it whole.
        assert_eq!(
            parse("NTS 1 - WE ARE... W/ PAUL CAMO"),
            info("WE ARE... W/ PAUL CAMO", Some("NTS 1"), None)
        );
        assert_eq!(parse("Simulcast"), info("Simulcast", None, None));
    }

    #[test]
    fn a_string_with_no_separator_is_all_title() {
        assert_eq!(
            parse("Tiefblauhorizont"),
            info("Tiefblauhorizont", None, None)
        );
    }

    #[test]
    fn non_ascii_titles_survive_intact() {
        assert_eq!(
            parse("Lola Disco ☀ - Love Your Grooves (Arrow's Loving Mix)"),
            info(
                "Love Your Grooves (Arrow's Loving Mix)",
                Some("Lola Disco ☀"),
                None
            )
        );
    }

    #[test]
    fn a_dangling_separator_leaves_the_string_whole() {
        assert_eq!(parse("Some Artist - "), info("Some Artist -", None, None));
        assert_eq!(parse(" - Some Title"), info("- Some Title", None, None));
    }
}
