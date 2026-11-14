//! What Now Playing and the mini player say about what plays: the titles, the
//! lines under them, the times and the tags, worked out from the player's
//! mirror. Pure: the controller only sets what this returns.

use crate::audio::Mode;
use crate::library_view;
use crate::player::Player;
use crate::text;
use crate::translations::Translations;

/// The seed of the art shown when nothing is loaded.
const IDLE_ART_SEED: &str = "janis";

/// Every text Now Playing and the mini player show about what plays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlayingText {
    /// The mini player's title: the track, else the station, else a dash.
    pub title: String,
    /// The line under it: the artist, or the station's genre.
    pub subtitle: String,
    /// The gradient of the generated art.
    pub gradient_index: usize,
    /// Whether a local track is current, and whether a station is.
    pub local: bool,
    pub radio: bool,
    /// The big title: the track, else the song the station announces, else
    /// the station.
    pub hero_title: String,
    pub initials: String,
    pub elapsed: String,
    /// The track's length, or "Live" for a station.
    pub total: String,
    pub eyebrow: String,
    pub quality: String,
    pub lossless: bool,
    pub artist: String,
    pub composer: String,
    pub album: String,
    pub album_line: String,
    pub genre: String,
    pub station_name: String,
    pub station_genre: String,
}

/// Which cover the art tile shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoverShown {
    /// The cover a station announced, rather than the track's.
    pub stream: bool,
    /// The track's cover failed to load: the tile says so rather than show
    /// the gradient art meant for a track with no cover.
    pub unavailable: bool,
}

pub fn now_playing_text(player: &Player, translations: &Translations) -> NowPlayingText {
    let current = player.current();
    let station = player.station();
    let stream = player.stream();
    let unknown_artist = || translations.text("common.unknownArtist");
    let hero_title = current
        .map(|track| track.title.clone())
        .or_else(|| stream.title.clone())
        .or_else(|| station.map(|station| station.name.clone()))
        .unwrap_or_default();
    NowPlayingText {
        title: current
            .map(|track| track.title.clone())
            .or_else(|| station.map(|station| station.name.clone()))
            .unwrap_or_else(|| "—".to_owned()),
        subtitle: match (current, station) {
            (Some(track), _) => track.artist.clone().unwrap_or_else(unknown_artist),
            (None, Some(station)) => translations.text(&station.genre_key),
            (None, None) => String::new(),
        },
        gradient_index: match (current, station) {
            (_, Some(station)) => station.gradient_index % text::ART_GRADIENT_COUNT,
            (Some(track), None) => text::art_gradient_index(&library_view::art_seed(track)),
            (None, None) => text::art_gradient_index(IDLE_ART_SEED),
        },
        local: current.is_some(),
        radio: station.is_some(),
        initials: text::art_initials(&hero_title),
        hero_title,
        elapsed: text::format_time(player.position_seconds()),
        total: if current.is_some() {
            text::format_time(player.duration_seconds())
        } else {
            translations.text("radio.live")
        },
        eyebrow: match (current, station) {
            (Some(track), _) => match &track.album {
                Some(album) => translations.translate("now.eyebrow", &[("album", album.clone())]),
                None => translations.text("now.eyebrowNoAlbum"),
            },
            (None, Some(station)) => {
                translations.translate("now.eyebrowRadio", &[("station", station.name.clone())])
            }
            (None, None) => String::new(),
        },
        quality: current.map(text::quality_label).unwrap_or_default(),
        lossless: current.is_some_and(|track| track.lossless),
        artist: match current {
            Some(track) => track.artist.clone().unwrap_or_else(unknown_artist),
            None => stream.artist.clone().unwrap_or_default(),
        },
        composer: current
            .and_then(|track| track.composer.clone())
            .unwrap_or_default(),
        album: match current {
            Some(track) => track
                .album
                .clone()
                .unwrap_or_else(|| translations.text("common.unknownAlbum")),
            None => stream.album.clone().unwrap_or_default(),
        },
        album_line: current
            .and_then(|track| library_view::album_line(track, translations))
            .unwrap_or_default(),
        genre: current
            .and_then(|track| track.genre.clone())
            .unwrap_or_default(),
        station_name: station
            .map(|station| station.name.clone())
            .unwrap_or_default(),
        station_genre: station
            .map(|station| translations.text(&station.genre_key))
            .unwrap_or_default(),
    }
}

/// The cover a station announced plays on radio; a local track shows its own,
/// and marks it unavailable when it failed to load.
pub fn cover_shown(mode: Mode, track_cover_failed: bool) -> CoverShown {
    CoverShown {
        stream: mode == Mode::Radio,
        unavailable: mode == Mode::Local && track_cover_failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::EngineEvent;
    use crate::library::test_support::track;
    use crate::radio::Station;
    use crate::translations::Language;
    use std::time::Instant;

    fn station() -> Station {
        Station {
            id: "fip".to_owned(),
            name: "FIP".to_owned(),
            country: "FR".to_owned(),
            genre_key: "radio.genre.eclectic".to_owned(),
            kbps: 128,
            url: "https://example.invalid/fip".to_owned(),
            gradient_index: 9,
            now_playing: None,
        }
    }

    fn english() -> Translations {
        Translations::new(Language::English)
    }

    fn on_the_radio() -> Player {
        let now = Instant::now();
        let mut player = Player::new(now);
        player.play_station(station());
        player.apply(
            EngineEvent::State {
                playing: true,
                mode: Mode::Radio,
                index: 0,
                queue_len: 0,
                shuffle: false,
                repeat: false,
                station_id: Some("fip".to_owned()),
            },
            now,
            &|_| Some(station()),
        );
        player
    }

    #[test]
    fn nothing_playing_shows_a_dash_on_the_idle_gradient() {
        let text = now_playing_text(&Player::new(Instant::now()), &english());
        assert_eq!(text.title, "—");
        assert_eq!(text.subtitle, "");
        assert_eq!(text.hero_title, "");
        assert_eq!(text.eyebrow, "");
        assert_eq!(text.gradient_index, text::art_gradient_index(IDLE_ART_SEED));
        assert!(!text.local && !text.radio);
    }

    #[test]
    fn a_tagged_track_spells_out_its_tags() {
        let now = Instant::now();
        let mut player = Player::new(now);
        let song = track(1, "So What")
            .artist("Miles Davis")
            .album("Kind of Blue")
            .composer("Miles Davis")
            .genre("Jazz")
            .number(1)
            .duration(562.0)
            .quality("FLAC", 24, 96_000);
        player.play_queue(vec![song.clone()], 0, now);

        let text = now_playing_text(&player, &english());

        assert_eq!(text.title, "So What");
        assert_eq!(text.subtitle, "Miles Davis");
        assert_eq!(text.hero_title, "So What");
        assert_eq!(text.initials, "SO");
        assert_eq!(text.total, "9:22");
        assert_eq!(text.eyebrow, "Now playing · From Kind of Blue");
        assert_eq!(text.quality, "Hi-Res · FLAC 24/96");
        assert!(text.lossless && text.local && !text.radio);
        assert_eq!(
            (
                text.artist.as_str(),
                text.album.as_str(),
                text.genre.as_str()
            ),
            ("Miles Davis", "Kind of Blue", "Jazz")
        );
        assert_eq!(text.album_line, "Track 1");
        assert_eq!(
            text.gradient_index,
            text::art_gradient_index(&library_view::art_seed(&song))
        );
    }

    #[test]
    fn an_untagged_track_reads_unknown_artist_and_unknown_album() {
        let now = Instant::now();
        let mut player = Player::new(now);
        player.play_queue(vec![track(1, "untitled")], 0, now);

        let text = now_playing_text(&player, &english());

        assert_eq!(text.subtitle, "Unknown artist");
        assert_eq!(text.artist, "Unknown artist");
        assert_eq!(text.album, "Unknown album");
        assert_eq!(text.eyebrow, english().text("now.eyebrowNoAlbum"));
    }

    #[test]
    fn a_station_reads_live_and_names_itself_in_the_eyebrow() {
        let text = now_playing_text(&on_the_radio(), &english());
        assert_eq!(text.title, "FIP");
        assert_eq!(text.hero_title, "FIP");
        assert_eq!(text.subtitle, english().text("radio.genre.eclectic"));
        assert_eq!(text.total, english().text("radio.live"));
        assert_eq!(
            text.eyebrow,
            english().translate("now.eyebrowRadio", &[("station", "FIP".to_owned())])
        );
        assert_eq!(text.station_name, "FIP");
        assert_eq!(text.gradient_index, 9 % text::ART_GRADIENT_COUNT);
        assert!(text.radio && !text.local);
    }

    #[test]
    fn a_station_announcing_a_song_shows_it_as_the_hero_title() {
        let mut player = on_the_radio();
        player.apply(
            EngineEvent::StreamMetadata {
                title: Some("Lapse".to_owned()),
                artist: Some("Fresh Moods".to_owned()),
                album: Some("Orfine No. 6".to_owned()),
                cover: None,
            },
            Instant::now(),
            &|_| None,
        );

        let text = now_playing_text(&player, &english());

        assert_eq!(text.hero_title, "Lapse");
        assert_eq!(text.title, "FIP", "the mini player keeps the station");
        assert_eq!(text.artist, "Fresh Moods");
        assert_eq!(text.album, "Orfine No. 6");
    }

    #[test]
    fn only_a_local_track_whose_cover_failed_reads_unavailable() {
        assert_eq!(
            cover_shown(Mode::Local, true),
            CoverShown {
                stream: false,
                unavailable: true
            }
        );
        assert_eq!(
            cover_shown(Mode::Radio, true),
            CoverShown {
                stream: true,
                unavailable: false
            }
        );
        assert!(!cover_shown(Mode::Idle, true).unavailable);
        assert!(!cover_shown(Mode::Local, false).unavailable);
    }
}
