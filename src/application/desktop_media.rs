//! Desktop commands enter on the interface thread and share the window's decisions.

use std::time::Instant;

use super::{post, Application};
use crate::audio::Mode;
use crate::library::Track;
use crate::media_controls::{Command, Metadata, PlaybackStatus, Snapshot};
use crate::player::Player;
use crate::{now_playing, platform};

const TRACK_PREFIX: &str = "track:";

impl Application {
    pub(super) fn connect_media_controls(&self) {
        *self.media_controls.borrow_mut() = platform::media_controls(|command| {
            post(move |application| application.on_media_command(command));
        });
    }

    fn on_media_command(&self, command: Command) {
        match command {
            Command::Raise => {
                self.come_forward();
                return;
            }
            Command::Quit => {
                if let Err(error) = slint::quit_event_loop() {
                    log::warn!("quit through desktop media controls: {error}");
                }
                return;
            }
            _ => {}
        }
        let refresh_lists = matches!(
            &command,
            Command::Play | Command::PlayPause | Command::Stop | Command::SetShuffle(_)
        );
        let effects = {
            let now = Instant::now();
            let mut player = self.player.borrow_mut();
            match command {
                Command::Play => player.play(now),
                Command::Pause => player.pause(now),
                Command::PlayPause => player.toggle(now),
                Command::Stop => player.stop(now),
                Command::Next => player.next(),
                Command::Previous => player.previous(),
                Command::Seek(seconds) => player.seek_relative(seconds, now),
                Command::SetPosition {
                    track_id,
                    position_seconds,
                } => track_identifier(&track_id)
                    .map(|identifier| player.seek_track(identifier, position_seconds, now))
                    .unwrap_or_default(),
                Command::SetVolume(volume) => player.set_volume(volume),
                Command::SetShuffle(enabled) => player.set_shuffle(enabled),
                Command::SetRepeat(enabled) => player.set_repeat(enabled),
                Command::Raise | Command::Quit => unreachable!("root commands returned above"),
            }
        };
        self.run(effects);
        self.refresh_player();
        if refresh_lists {
            self.refresh_now_playing_lists();
        }
    }

    pub(super) fn publish_media_controls(&self, player: &Player) {
        let controls = self.media_controls.borrow();
        let Some(controls) = controls.as_ref() else {
            return;
        };
        let now = Instant::now();
        let mut snapshot = snapshot(player, now);
        if snapshot.metadata.is_some() {
            let art = self.current_art.borrow();
            let shown = now_playing::cover_shown(player.mode(), art.requests.failed());
            snapshot.artwork = if shown.stream {
                art.stream_media_cover.clone()
            } else {
                art.track_media_cover.clone()
            };
        }
        controls.update(snapshot);
    }
}

fn snapshot(player: &Player, now: Instant) -> Snapshot {
    let metadata = match player.mode() {
        Mode::Local => player.current().map(|track| track_metadata(track, player)),
        Mode::Radio => player.station().map(|station| {
            let stream = player.stream();
            let identifier = format!("station:{}", station.id);
            Metadata {
                track_id: identifier,
                title: stream.title.clone().unwrap_or_else(|| station.name.clone()),
                artist: stream.artist.clone().unwrap_or_default(),
                album: stream.album.clone().unwrap_or_else(|| station.name.clone()),
                duration_seconds: None,
                source_url: None,
            }
        }),
        Mode::Idle => None,
    };
    Snapshot {
        playback: if player.playing() {
            PlaybackStatus::Playing
        } else if player.stopped() {
            PlaybackStatus::Stopped
        } else {
            PlaybackStatus::Paused
        },
        metadata,
        artwork: None,
        position_seconds: player.live_position(now),
        observed_at: now,
        volume: player.volume(),
        shuffle: player.shuffle(),
        repeat: player.repeat(),
        can_play: player.can_play() && !player.connecting(),
        can_pause: player.can_play() && !player.connecting(),
        can_seek: player.can_seek(),
        can_go_next: player.can_go_next(),
        can_go_previous: player.can_go_previous(),
        seek_serial: player.seek_serial(),
    }
}

fn track_metadata(track: &Track, player: &Player) -> Metadata {
    // Only disk-backed tracks have public URLs; server paths can contain credentials.
    let source_url = if track.server_id.is_none() {
        reqwest::Url::from_file_path(&track.path)
            .ok()
            .map(|url| url.into())
    } else {
        None
    };
    let duration_seconds = player.duration_seconds();
    Metadata {
        track_id: format!("{TRACK_PREFIX}{:016x}", track.id as u64),
        title: track.title.clone(),
        artist: track.artist.clone().unwrap_or_default(),
        album: track.album.clone().unwrap_or_default(),
        duration_seconds: (duration_seconds.is_finite() && duration_seconds > 0.0)
            .then_some(duration_seconds),
        source_url,
    }
}

fn track_identifier(path: &str) -> Option<i64> {
    let identifier = path.strip_prefix(TRACK_PREFIX)?;
    if identifier.len() != 16
        || !identifier
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    u64::from_str_radix(identifier, 16)
        .ok()
        .map(|identifier| identifier as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::EngineEvent;
    use crate::library::test_support::track;

    #[test]
    fn radio_metadata_uses_live_titles_without_claiming_seek_or_queue_controls() {
        let now = Instant::now();
        let station = crate::radio::Station {
            id: "radio-été".to_owned(),
            name: "Station".to_owned(),
            country: "FR".to_owned(),
            genre_key: "radio.genre.jazz".to_owned(),
            kbps: 128,
            url: "https://station.invalid/live?token=private".to_owned(),
            gradient_index: 0,
            now_playing: None,
        };
        let mut player = Player::new(now);
        player.play_station(station.clone());
        player.apply(
            EngineEvent::State {
                has_next: false,
                playing: true,
                mode: Mode::Radio,
                index: 0,
                queue_len: 0,
                shuffle: false,
                repeat: false,
                station_id: Some(station.id.clone()),
            },
            now,
            &|_| Some(station.clone()),
        );
        player.station_connection_finished(&station, 1, Ok(()));
        let initial = snapshot(&player, now).metadata.expect("station metadata");
        assert_eq!(initial.title, "Station");
        player.apply(
            EngineEvent::StreamMetadata {
                title: Some("Live song".to_owned()),
                artist: Some("Live artist".to_owned()),
                album: Some("Live album".to_owned()),
                cover: None,
            },
            now,
            &|_| None,
        );
        let report = snapshot(&player, now);
        let metadata = report.metadata.expect("live station metadata");
        assert_eq!(metadata.track_id, initial.track_id);
        assert_eq!(metadata.track_id, format!("station:{}", station.id));
        assert_eq!(metadata.title, "Live song");
        assert_eq!(metadata.artist, "Live artist");
        assert_eq!(metadata.album, "Live album");
        assert_eq!(metadata.source_url, None);
        assert_eq!(metadata.duration_seconds, None);
        assert_eq!(report.playback, PlaybackStatus::Playing);
        assert!(report.can_play);
        assert!(report.can_pause);
        assert!(!report.can_seek);
        assert!(!report.can_go_next);
        assert!(!report.can_go_previous);
    }

    #[test]
    fn local_metadata_encodes_file_urls_and_uses_stable_identities() {
        let now = Instant::now();
        let mut song = track(17, "Title").duration(120.0);
        song.path = "/music/a #é.flac".to_owned();
        let mut player = Player::new(now);
        player.play_queue(vec![song], 0, now);
        let report = snapshot(&player, now);
        let metadata = report.metadata.expect("current track metadata");
        assert_eq!(track_identifier(&metadata.track_id), Some(17));
        assert_eq!(
            metadata.source_url.as_deref(),
            Some("file:///music/a%20%23%C3%A9.flac")
        );
        assert_eq!(metadata.duration_seconds, Some(120.0));
        assert!(report.can_seek);
        assert!(!report.can_go_next);
        assert!(report.can_go_previous);
    }

    #[test]
    fn server_metadata_never_publishes_transport_addresses() {
        let now = Instant::now();
        for (identifier, path) in [
            (-17, "https://server.invalid/song?api_key=secret"),
            (17, "https://user:secret@server.invalid/song"),
            (18, "/music/server-song.flac"),
        ] {
            let mut song = track(identifier, "Title");
            song.server_id = Some(1);
            song.path = path.to_owned();
            let mut player = Player::new(now);
            player.play_queue(vec![song], 0, now);
            let metadata = snapshot(&player, now).metadata.expect("selected track");
            assert_eq!(track_identifier(&metadata.track_id), Some(identifier));
            assert_eq!(metadata.source_url, None);
            assert!(!metadata.track_id.contains("secret"));
        }
        assert_eq!(track_identifier("track:wrong"), None);
    }

    #[test]
    fn track_identities_round_trip_signed_ids_and_reject_stale_positions() {
        let now = Instant::now();
        for identifier in [i64::MIN, -17, 0, 17, i64::MAX] {
            let mut player = Player::new(now);
            player.play_queue(vec![track(identifier, "Title").duration(100.0)], 0, now);
            let metadata = snapshot(&player, now).metadata.expect("selected track");
            assert_eq!(track_identifier(&metadata.track_id), Some(identifier));
            assert!(player
                .seek_track(identifier.wrapping_add(1), 10.0, now)
                .is_empty());
            assert_eq!(player.live_position(now), 0.0);
        }
        for malformed in [
            "track:000000000000000A",
            "track:000000000000000",
            "track:00000000000000000",
            "station:0000000000000000",
            "track:../../secret",
        ] {
            assert_eq!(track_identifier(malformed), None);
        }
    }

    #[test]
    fn metadata_preserves_exact_finite_duration_and_omits_unknown_lengths() {
        let now = Instant::now();
        for duration in [123.456_789, 0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut player = Player::new(now);
            player.play_queue(vec![track(1, "Title").duration(duration)], 0, now);
            let metadata = snapshot(&player, now).metadata.expect("selected track");
            let expected = (duration.is_finite() && duration > 0.0).then_some(duration);
            assert_eq!(metadata.duration_seconds, expected);
        }
    }

    #[test]
    fn end_of_queue_and_explicit_stop_are_stopped_not_paused() {
        let now = Instant::now();
        let mut player = Player::new(now);
        assert!(!snapshot(&player, now).can_play);
        player.play_queue(vec![track(1, "Title").duration(100.0)], 0, now);
        player.pause(now);
        assert_eq!(snapshot(&player, now).playback, PlaybackStatus::Paused);
        player.play(now);
        player.apply(
            EngineEvent::State {
                has_next: false,
                playing: false,
                mode: Mode::Local,
                index: 0,
                shuffle: false,
                repeat: false,
                station_id: None,
                queue_len: 1,
            },
            now,
            &|_| None,
        );
        assert_eq!(snapshot(&player, now).playback, PlaybackStatus::Stopped);
        player.stop(now);
        let report = snapshot(&player, now);
        assert_eq!(report.playback, PlaybackStatus::Stopped);
        assert!(report.can_play);
        assert!(report.can_pause);
        assert!(!report.can_seek);
    }
}
