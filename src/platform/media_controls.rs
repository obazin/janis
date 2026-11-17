//! Session-bus transport. Only this worker touches D-Bus or artwork files.

use std::collections::VecDeque;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use image::ImageEncoder;
use mpris_server::zbus::{self, fdo};
use mpris_server::{
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property, RootInterface,
    Server, Signal, Time, TrackId, Volume,
};
use tokio::sync::watch;

use crate::media_controls::{Artwork, Command, Controls, PlaybackStatus as Status, Snapshot};

const BUS_NAME: &str = "janis";
const MICROSECONDS_PER_SECOND: f64 = 1_000_000.0;
const MAXIMUM_SECONDS: f64 = i64::MAX as f64 / MICROSECONDS_PER_SECOND;
const MAXIMUM_ARTWORK_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn start(handler: impl Fn(Command) + Send + Sync + 'static) -> Option<Controls> {
    let (updates, receiver) = watch::channel(Snapshot::default());
    let worker = std::thread::Builder::new()
        .name("desktop-media-controls".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    log::warn!("Desktop media controls are unavailable: {error}");
                    return;
                }
            };
            let result = runtime.block_on(async {
                let mut shutdown = receiver.clone();
                tokio::select! {
                    result = serve(receiver, Arc::new(handler)) => result,
                    _ = shutdown.wait_for(|_| false) => Ok(()),
                }
            });
            if let Err(error) = result {
                // No reconnect loop: a missing session bus must not flood the log.
                log::warn!("Desktop media controls are unavailable: {error}");
            }
        });
    match worker {
        Ok(worker) => Some(Controls {
            updates: Some(updates),
            worker: Some(worker),
        }),
        Err(error) => {
            log::warn!("Could not start desktop media controls: {error}");
            None
        }
    }
}

#[derive(Clone)]
struct Published {
    snapshot: Snapshot,
    metadata: Arc<Metadata>,
}

struct Player {
    published: RwLock<Published>,
    handler: Arc<dyn Fn(Command) + Send + Sync>,
}

impl Player {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Published> {
        self.published
            .read()
            .expect("the media-control state lock is not poisoned")
    }

    fn send(&self, command: Command) {
        (self.handler)(command);
    }
}

async fn serve(
    mut receiver: watch::Receiver<Snapshot>,
    handler: Arc<dyn Fn(Command) + Send + Sync>,
) -> zbus::Result<()> {
    let mut artwork = ArtworkCache::default();
    let initial = receiver.borrow_and_update().clone();
    let initial_metadata = Arc::new(metadata(&initial, artwork.update(&initial)));
    let server = Server::new(
        BUS_NAME,
        Player {
            published: RwLock::new(Published {
                snapshot: initial,
                metadata: initial_metadata,
            }),
            handler,
        },
    )
    .await?;

    while receiver.changed().await.is_ok() {
        let snapshot = receiver.borrow_and_update().clone();
        let artwork_url = artwork.update(&snapshot);
        let next_metadata = {
            let previous = server.imp().read();
            if previous.snapshot.metadata == snapshot.metadata
                && same_artwork(&previous.snapshot.artwork, &snapshot.artwork)
            {
                Arc::clone(&previous.metadata)
            } else {
                Arc::new(metadata(&snapshot, artwork_url))
            }
        };
        let next = Published {
            metadata: next_metadata,
            snapshot,
        };
        let (properties, seeked) = {
            let mut previous = server
                .imp()
                .published
                .write()
                .expect("the media-control state lock is not poisoned");
            let changes = changed_properties(&previous, &next);
            let seeked = (previous.snapshot.seek_serial != next.snapshot.seek_serial)
                .then(|| position_at(&next.snapshot, Instant::now()));
            *previous = next;
            (changes, seeked)
        };
        server.properties_changed(properties).await?;
        if let Some(position) = seeked {
            server.emit(Signal::Seeked { position }).await?;
        }
    }
    // Releasing explicitly makes lifetime independent of the executor's cleanup.
    server.release_bus_name().await?;
    Ok(())
}

fn unsupported(operation: &str) -> fdo::Error {
    fdo::Error::NotSupported(format!("Janis does not support {operation}"))
}

impl RootInterface for Player {
    async fn raise(&self) -> fdo::Result<()> {
        self.send(Command::Raise);
        Ok(())
    }

    async fn quit(&self) -> fdo::Result<()> {
        self.send(Command::Quit);
        Ok(())
    }

    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_fullscreen(&self, _fullscreen: bool) -> zbus::Result<()> {
        Err(unsupported("fullscreen").into())
    }

    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn identity(&self) -> fdo::Result<String> {
        Ok("Janis".into())
    }

    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok("janis".into())
    }

    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

impl PlayerInterface for Player {
    async fn next(&self) -> fdo::Result<()> {
        if self.read().snapshot.can_go_next {
            self.send(Command::Next);
        }
        Ok(())
    }

    async fn previous(&self) -> fdo::Result<()> {
        if self.read().snapshot.can_go_previous {
            self.send(Command::Previous);
        }
        Ok(())
    }

    async fn pause(&self) -> fdo::Result<()> {
        if self.read().snapshot.can_pause {
            self.send(Command::Pause);
        }
        Ok(())
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        if !self.read().snapshot.can_pause {
            return Err(unsupported("play/pause for the current source"));
        }
        self.send(Command::PlayPause);
        Ok(())
    }

    async fn stop(&self) -> fdo::Result<()> {
        self.send(Command::Stop);
        Ok(())
    }

    async fn play(&self) -> fdo::Result<()> {
        if self.read().snapshot.can_play {
            self.send(Command::Play);
        }
        Ok(())
    }

    async fn seek(&self, offset: Time) -> fdo::Result<()> {
        let command = seek_command(&self.read().snapshot, offset);
        if let Some(command) = command {
            self.send(command);
        }
        Ok(())
    }

    async fn set_position(&self, track_id: TrackId, position: Time) -> fdo::Result<()> {
        let command = set_position_command(&self.read().snapshot, &track_id, position);
        if let Some(command) = command {
            self.send(command);
        }
        Ok(())
    }

    async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Err(unsupported("OpenUri"))
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        Ok(playback_status(self.read().snapshot.playback))
    }

    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(loop_status(self.read().snapshot.repeat))
    }

    async fn set_loop_status(&self, status: LoopStatus) -> zbus::Result<()> {
        match status {
            LoopStatus::None => self.send(Command::SetRepeat(false)),
            LoopStatus::Playlist => self.send(Command::SetRepeat(true)),
            LoopStatus::Track => return Err(unsupported("single-track repeat").into()),
        }
        Ok(())
    }

    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn set_rate(&self, rate: PlaybackRate) -> zbus::Result<()> {
        if !rate.is_finite() {
            return Err(fdo::Error::InvalidArgs("Rate must be finite".into()).into());
        }
        if rate == 0.0 {
            return self.pause().await.map_err(Into::into);
        }
        if rate != 1.0 {
            return Err(unsupported("variable playback rates").into());
        }
        Ok(())
    }

    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(self.read().snapshot.shuffle)
    }

    async fn set_shuffle(&self, shuffle: bool) -> zbus::Result<()> {
        self.send(Command::SetShuffle(shuffle));
        Ok(())
    }

    async fn metadata(&self) -> fdo::Result<Metadata> {
        Ok(self.read().metadata.as_ref().clone())
    }

    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(normalized_volume(self.read().snapshot.volume))
    }

    async fn set_volume(&self, volume: Volume) -> zbus::Result<()> {
        if !volume.is_finite() {
            return Err(fdo::Error::InvalidArgs("Volume must be finite".into()).into());
        }
        // Negative values mean mute in MPRIS. Janis has no amplification gain.
        self.send(Command::SetVolume(volume.clamp(0.0, 1.0)));
        Ok(())
    }

    async fn position(&self) -> fdo::Result<Time> {
        Ok(position_at(&self.read().snapshot, Instant::now()))
    }

    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(self.read().snapshot.can_go_next)
    }

    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(self.read().snapshot.can_go_previous)
    }

    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(self.read().snapshot.can_play)
    }

    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(self.read().snapshot.can_pause)
    }

    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(self.read().snapshot.can_seek)
    }

    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}

fn playback_status(status: Status) -> PlaybackStatus {
    match status {
        Status::Stopped => PlaybackStatus::Stopped,
        Status::Playing => PlaybackStatus::Playing,
        Status::Paused => PlaybackStatus::Paused,
    }
}

fn loop_status(repeat: bool) -> LoopStatus {
    if repeat {
        LoopStatus::Playlist
    } else {
        LoopStatus::None
    }
}

fn normalized_volume(volume: f64) -> f64 {
    if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn duration_seconds(snapshot: &Snapshot) -> Option<f64> {
    snapshot
        .metadata
        .as_ref()?
        .duration_seconds
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0 && *seconds <= MAXIMUM_SECONDS)
}

fn time_from_seconds(seconds: f64) -> Time {
    let seconds = if seconds.is_finite() {
        seconds.clamp(0.0, MAXIMUM_SECONDS)
    } else {
        0.0
    };
    Time::from_micros((seconds * MICROSECONDS_PER_SECOND) as i64)
}

fn position_at(snapshot: &Snapshot, now: Instant) -> Time {
    let mut seconds = if snapshot.position_seconds.is_finite() {
        snapshot.position_seconds.max(0.0)
    } else {
        0.0
    };
    if snapshot.playback == Status::Playing {
        seconds += now
            .saturating_duration_since(snapshot.observed_at)
            .as_secs_f64();
    }
    if let Some(duration) = duration_seconds(snapshot) {
        seconds = seconds.min(duration);
    }
    time_from_seconds(seconds)
}

fn seek_command(snapshot: &Snapshot, offset: Time) -> Option<Command> {
    if !snapshot.can_seek || snapshot.metadata.is_none() {
        return None;
    }
    Some(Command::Seek(
        offset.as_micros() as f64 / MICROSECONDS_PER_SECOND,
    ))
}

fn set_position_command(
    snapshot: &Snapshot,
    track_id: &TrackId,
    position: Time,
) -> Option<Command> {
    let current = snapshot.metadata.as_ref()?;
    let seconds = position.as_micros() as f64 / MICROSECONDS_PER_SECOND;
    if !snapshot.can_seek
        || *track_id == TrackId::NO_TRACK
        || *track_id != wire_track_id(&current.track_id)
        || position.is_negative()
        || duration_seconds(snapshot).is_some_and(|duration| seconds > duration)
    {
        return None;
    }
    Some(Command::SetPosition {
        track_id: current.track_id.clone(),
        position_seconds: seconds,
    })
}

fn wire_track_id(identity: &str) -> TrackId {
    use std::fmt::Write as _;

    let mut path = String::from("/com/olivierbazin/janis/track/t");
    for byte in identity.bytes() {
        let _ = write!(path, "{byte:02x}");
    }
    TrackId::try_from(path).expect("hex-encoded identities form valid object paths")
}

fn metadata(snapshot: &Snapshot, artwork_url: Option<&str>) -> Metadata {
    let Some(current) = &snapshot.metadata else {
        return Metadata::new();
    };
    let track_id = wire_track_id(&current.track_id);
    let mut metadata = Metadata::builder()
        .trackid(track_id)
        .title(current.title.clone())
        .artist([current.artist.clone()])
        .album(current.album.clone())
        .build();
    metadata.set_length(duration_seconds(snapshot).map(time_from_seconds));
    metadata.set_art_url(artwork_url);
    if let Some(source) = current
        .source_url
        .as_deref()
        .filter(|source| public_url(source))
    {
        metadata.set_url(Some(source));
    }
    metadata
}

fn public_url(source: &str) -> bool {
    reqwest::Url::parse(source).is_ok_and(|url| {
        matches!(url.scheme(), "file" | "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn same_artwork(previous: &Option<Arc<Artwork>>, next: &Option<Arc<Artwork>>) -> bool {
    match (previous, next) {
        (None, None) => true,
        (Some(previous), Some(next)) => Arc::ptr_eq(previous, next),
        _ => false,
    }
}

fn changed_properties(previous: &Published, next: &Published) -> Vec<Property> {
    let mut properties = Vec::new();
    let before = &previous.snapshot;
    let after = &next.snapshot;
    if before.playback != after.playback {
        properties.push(Property::PlaybackStatus(playback_status(after.playback)));
    }
    if before.repeat != after.repeat {
        properties.push(Property::LoopStatus(loop_status(after.repeat)));
    }
    if before.shuffle != after.shuffle {
        properties.push(Property::Shuffle(after.shuffle));
    }
    if normalized_volume(before.volume) != normalized_volume(after.volume) {
        properties.push(Property::Volume(normalized_volume(after.volume)));
    }
    if !Arc::ptr_eq(&previous.metadata, &next.metadata) && previous.metadata != next.metadata {
        properties.push(Property::Metadata(next.metadata.as_ref().clone()));
    }
    if before.can_play != after.can_play {
        properties.push(Property::CanPlay(after.can_play));
    }
    if before.can_pause != after.can_pause {
        properties.push(Property::CanPause(after.can_pause));
    }
    if before.can_seek != after.can_seek {
        properties.push(Property::CanSeek(after.can_seek));
    }
    if before.can_go_next != after.can_go_next {
        properties.push(Property::CanGoNext(after.can_go_next));
    }
    if before.can_go_previous != after.can_go_previous {
        properties.push(Property::CanGoPrevious(after.can_go_previous));
    }
    properties
}

#[derive(Default)]
struct ArtworkCache {
    directory: Option<tempfile::TempDir>,
    files: VecDeque<tempfile::NamedTempFile>,
    current: Option<(String, Arc<Artwork>, Option<String>)>,
}

impl ArtworkCache {
    fn update(&mut self, snapshot: &Snapshot) -> Option<&str> {
        let requested = snapshot.metadata.as_ref().zip(snapshot.artwork.as_ref());
        let Some((metadata, artwork)) = requested else {
            self.current = None;
            self.files.clear();
            return None;
        };
        let unchanged = self
            .current
            .as_ref()
            .is_some_and(|(track_id, previous, _)| {
                track_id == &metadata.track_id && Arc::ptr_eq(previous, artwork)
            });
        if !unchanged {
            let url = match self.encode(artwork) {
                Ok(url) => Some(url),
                Err(error) => {
                    log::warn!("Could not publish desktop cover art: {error}");
                    None
                }
            };
            // Remember failed encodes too: position reports are not retry requests.
            self.current = Some((metadata.track_id.clone(), Arc::clone(artwork), url));
        }
        self.current.as_ref().and_then(|(_, _, url)| url.as_deref())
    }

    fn encode(&mut self, artwork: &Artwork) -> Result<String, Box<dyn std::error::Error>> {
        let bytes = (artwork.width as usize)
            .checked_mul(artwork.height as usize)
            .and_then(|pixels| pixels.checked_mul(4));
        if artwork.width == 0
            || artwork.height == 0
            || bytes != Some(artwork.rgba.len())
            || artwork.rgba.len() > MAXIMUM_ARTWORK_BYTES
        {
            return Err("invalid or oversized cover pixels".into());
        }
        if self.directory.is_none() {
            self.directory = Some(
                tempfile::Builder::new()
                    .prefix("janis-artwork-")
                    .permissions(std::fs::Permissions::from_mode(0o700))
                    .tempdir()?,
            );
        }
        let directory = self
            .directory
            .as_ref()
            .expect("the artwork directory exists");
        let mut file = tempfile::Builder::new()
            .prefix("cover-")
            .suffix(".png")
            .tempfile_in(directory.path())?;
        image::codecs::png::PngEncoder::new(file.as_file_mut()).write_image(
            &artwork.rgba,
            artwork.width,
            artwork.height,
            image::ExtendedColorType::Rgba8,
        )?;
        let url = reqwest::Url::from_file_path(file.path())
            .map_err(|()| "the cover path is not absolute")?
            .to_string();
        self.files.push_back(file);
        // Keep the immediately previous URL briefly available to in-flight readers.
        while self.files.len() > 2 {
            self.files.pop_front();
        }
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::time::Duration;

    use zbus::export::futures_core::Stream;

    use super::*;
    use crate::media_controls::Metadata as TrackMetadata;

    fn snapshot() -> Snapshot {
        Snapshot {
            metadata: Some(TrackMetadata {
                track_id: "track:one".into(),
                title: "One".into(),
                artist: "Artist".into(),
                album: "Album".into(),
                duration_seconds: Some(20.0),
                source_url: None,
            }),
            position_seconds: 5.0,
            can_seek: true,
            can_go_next: true,
            ..Snapshot::default()
        }
    }

    fn player(snapshot: Snapshot) -> (Player, Arc<Mutex<Vec<Command>>>) {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let received = Arc::clone(&commands);
        (
            Player {
                published: RwLock::new(Published {
                    metadata: Arc::new(metadata(&snapshot, None)),
                    snapshot,
                }),
                handler: Arc::new(move |command| received.lock().unwrap().push(command)),
            },
            commands,
        )
    }

    #[test]
    fn position_interpolates_only_playing_and_stops_at_duration() {
        let mut snapshot = snapshot();
        let later = snapshot.observed_at + Duration::from_secs(30);
        assert_eq!(position_at(&snapshot, later), Time::from_secs(5));
        snapshot.playback = Status::Playing;
        assert_eq!(position_at(&snapshot, later), Time::from_secs(20));
        assert_eq!(
            position_at(&snapshot, snapshot.observed_at),
            Time::from_secs(5)
        );
        snapshot.position_seconds = f64::NAN;
        snapshot.playback = Status::Paused;
        assert_eq!(position_at(&snapshot, later), Time::ZERO);
        assert_eq!(time_from_seconds(f64::MAX), Time::MAX);
    }

    #[test]
    fn relative_seek_preserves_offsets_for_the_player_boundary() {
        let mut snapshot = snapshot();
        assert!(matches!(
            seek_command(&snapshot, Time::from_secs(-100)),
            Some(Command::Seek(seconds)) if seconds == -100.0
        ));
        assert!(matches!(
            seek_command(&snapshot, Time::from_secs(100)),
            Some(Command::Seek(seconds)) if seconds == 100.0
        ));
        snapshot.can_seek = false;
        assert!(seek_command(&snapshot, Time::from_secs(1)).is_none());
        snapshot.can_seek = true;
        snapshot.metadata = None;
        assert!(seek_command(&snapshot, Time::from_secs(1)).is_none());
    }

    #[test]
    fn absolute_seek_ignores_stale_negative_and_oversized_requests() {
        let snapshot = snapshot();
        let current = wire_track_id(&snapshot.metadata.as_ref().unwrap().track_id);
        assert!(set_position_command(&snapshot, &TrackId::NO_TRACK, Time::ZERO).is_none());
        let stale = TrackId::try_from("/another/track").unwrap();
        assert!(set_position_command(&snapshot, &stale, Time::ZERO).is_none());
        assert!(set_position_command(&snapshot, &current, Time::from_micros(-1)).is_none());
        assert!(set_position_command(&snapshot, &current, Time::MAX).is_none());
        assert!(matches!(
            set_position_command(&snapshot, &current, Time::from_secs(10)),
            Some(Command::SetPosition { position_seconds, .. }) if position_seconds == 10.0
        ));
    }

    #[test]
    fn ordinary_position_reports_do_not_emit_property_changes() {
        let snapshot = snapshot();
        let before = Published {
            metadata: Arc::new(metadata(&snapshot, None)),
            snapshot,
        };
        let mut after = before.clone();
        after.snapshot.position_seconds += 0.25;
        after.snapshot.observed_at += Duration::from_millis(250);
        assert!(changed_properties(&before, &after).is_empty());
        after.snapshot.can_seek = false;
        after.snapshot.playback = Status::Paused;
        assert_eq!(
            changed_properties(&before, &after),
            vec![
                Property::PlaybackStatus(PlaybackStatus::Paused),
                Property::CanSeek(false),
            ]
        );
    }

    #[test]
    fn metadata_uses_object_paths_and_never_exports_credential_urls() {
        let mut snapshot = snapshot();
        let current = snapshot.metadata.as_mut().unwrap();
        current.source_url = Some("https://example.com/song?token=secret".into());
        let published = metadata(&snapshot, None);
        assert!(published.trackid().is_some());
        assert!(published.url().is_none());
        assert!(!public_url("https://user:password@example.com/song"));
        assert!(public_url("file:///music/song.flac"));
        let identity = "/org/mpris/station écho";
        snapshot.metadata.as_mut().unwrap().track_id = identity.into();
        let published = metadata(&snapshot, None);
        assert_eq!(published.trackid(), Some(wire_track_id(identity)));
        assert_eq!(published.title(), Some("One"));
    }

    #[tokio::test]
    async fn unsupported_operations_fail_without_dispatching_commands() {
        let (player, commands) = player(snapshot());
        assert!(matches!(
            player.open_uri("file:///song".into()).await,
            Err(fdo::Error::NotSupported(_))
        ));
        assert!(player.set_fullscreen(true).await.is_err());
        assert!(player.set_loop_status(LoopStatus::Track).await.is_err());
        assert!(matches!(
            player.play_pause().await,
            Err(fdo::Error::NotSupported(_))
        ));
        assert!(player.set_rate(2.0).await.is_err());
        assert!(player.set_rate(f64::NAN).await.is_err());
        assert!(player.set_volume(f64::INFINITY).await.is_err());
        assert!(commands.lock().unwrap().is_empty());
        player.set_volume(-1.0).await.unwrap();
        player.set_volume(f64::MAX).await.unwrap();
        player.set_loop_status(LoopStatus::Playlist).await.unwrap();
        let received = commands.lock().unwrap();
        assert!(matches!(received[0], Command::SetVolume(0.0)));
        assert!(matches!(received[1], Command::SetVolume(1.0)));
        assert!(matches!(received[2], Command::SetRepeat(true)));
    }

    #[tokio::test]
    async fn zero_rate_pauses_without_changing_the_fixed_playback_rate() {
        let mut snapshot = snapshot();
        snapshot.can_play = true;
        snapshot.can_pause = true;
        let (player, commands) = player(snapshot);
        player.set_rate(0.0).await.unwrap();
        assert_eq!(player.rate().await.unwrap(), 1.0);
        assert!(matches!(
            commands.lock().unwrap().as_slice(),
            [Command::Pause]
        ));
    }

    #[test]
    fn artwork_is_private_bounded_track_owned_and_cleared() {
        let mut snapshot = snapshot();
        snapshot.artwork = Some(Arc::new(Artwork {
            width: 1,
            height: 1,
            rgba: Arc::from([255, 0, 0, 255]),
        }));
        let mut cache = ArtworkCache::default();
        let first = cache.update(&snapshot).unwrap().to_owned();
        let path = reqwest::Url::parse(&first).unwrap().to_file_path().unwrap();
        assert_eq!(&std::fs::read(&path).unwrap()[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(cache.update(&snapshot).unwrap(), first);
        for number in 0..5 {
            snapshot.metadata.as_mut().unwrap().track_id = format!("/track/{number}");
            assert_ne!(cache.update(&snapshot).unwrap(), first);
            assert!(cache.files.len() <= 2);
        }
        assert!(!path.exists());
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::metadata(cache.directory.as_ref().unwrap().path())
            .unwrap()
            .permissions();
        assert_eq!(permissions.mode() & 0o077, 0);
        snapshot.artwork = None;
        assert!(cache.update(&snapshot).is_none());
        assert!(cache.files.is_empty());
        assert!(cache.current.is_none());
    }

    #[test]
    #[ignore = "requires dbus-run-session; always starts its own private session bus"]
    fn isolated_session_protocol_and_lifetime() {
        const CHILD: &str = "JANIS_PRIVATE_MPRIS_BUS_PROBE";
        if std::env::var(CHILD).as_deref() != Ok("1") {
            let test_name = concat!(module_path!(), "::isolated_session_protocol_and_lifetime")
                .split_once("::")
                .expect("the module path includes the crate name")
                .1;
            let status = std::process::Command::new("dbus-run-session")
                .arg("--")
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", test_name, "--ignored", "--nocapture"])
                .env(CHILD, "1")
                .status()
                .expect("dbus-run-session must be installed for this opt-in test");
            assert!(status.success());
            return;
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(10), async {
                let connection = zbus::Connection::session().await.unwrap();
                let bus = zbus::Proxy::new(
                    &connection,
                    "org.freedesktop.DBus",
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                )
                .await
                .unwrap();
                let service = "org.mpris.MediaPlayer2.janis";
                let occupied: bool = bus.call("NameHasOwner", &(service,)).await.unwrap();
                assert!(!occupied, "the smoke test must own a fresh private bus");
                let commands = Arc::new(Mutex::new(Vec::new()));
                let received = Arc::clone(&commands);
                let controls =
                    start(move |command| received.lock().unwrap().push(command)).unwrap();
                let mut snapshot = snapshot();
                snapshot.can_play = true;
                snapshot.can_pause = true;
                controls.update(snapshot.clone());
                loop {
                    let owned: bool = bus.call("NameHasOwner", &(service,)).await.unwrap();
                    if owned {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                let player = zbus::proxy::Builder::<zbus::Proxy<'_>>::new(&connection)
                    .destination(service)
                    .path("/org/mpris/MediaPlayer2")
                    .interface("org.mpris.MediaPlayer2.Player")
                    .cache_properties(zbus::proxy::CacheProperties::No)
                    .build()
                    .await
                    .unwrap();
                while !player.get_property::<bool>("CanSeek").await.unwrap() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                let root = zbus::Proxy::new(
                    &connection,
                    service,
                    "/org/mpris/MediaPlayer2",
                    "org.mpris.MediaPlayer2",
                )
                .await
                .unwrap();
                assert_eq!(
                    root.get_property::<String>("Identity").await.unwrap(),
                    "Janis"
                );
                assert_eq!(
                    root.get_property::<String>("DesktopEntry").await.unwrap(),
                    "janis"
                );
                assert!(root.get_property::<bool>("CanQuit").await.unwrap());
                assert!(root.get_property::<bool>("CanRaise").await.unwrap());
                assert!(!root.get_property::<bool>("HasTrackList").await.unwrap());
                assert!(root
                    .get_property::<Vec<String>>("SupportedUriSchemes")
                    .await
                    .unwrap()
                    .is_empty());
                let mut metadata: std::collections::HashMap<String, zbus::OwnedVariant> =
                    player.get_property("Metadata").await.unwrap();
                let path: zbus::OwnedObjectPath = metadata
                    .remove("mpris:trackid")
                    .unwrap()
                    .try_into()
                    .unwrap();
                let track_id = TrackId::try_from(path.as_str()).unwrap();
                assert_eq!(track_id, wire_track_id("track:one"));
                let position: i64 = player.get_property("Position").await.unwrap();
                assert_eq!(position, 5_000_000);
                let rate: f64 = player.get_property("Rate").await.unwrap();
                assert_eq!(rate, 1.0);
                let _: () = player.call("Play", &()).await.unwrap();
                let _: () = player
                    .call("SetPosition", &(TrackId::NO_TRACK, Time::ZERO))
                    .await
                    .unwrap();
                let _: () = player
                    .call("SetPosition", &(track_id, Time::from_secs(10)))
                    .await
                    .unwrap();
                let unsupported: zbus::Result<()> =
                    player.call("OpenUri", &("file:///song",)).await;
                match unsupported {
                    Err(zbus::Error::FDO(error)) => {
                        assert!(matches!(*error, fdo::Error::NotSupported(_)));
                    }
                    other => panic!("OpenUri must fail explicitly, got {other:?}"),
                }
                {
                    let commands = commands.lock().unwrap();
                    assert_eq!(commands.len(), 2);
                    assert!(matches!(commands[0], Command::Play));
                    assert!(matches!(
                        commands[1],
                        Command::SetPosition { ref track_id, position_seconds }
                            if track_id == "track:one" && position_seconds == 10.0
                    ));
                }
                // Subscribe before publishing; query-only checks miss broken signals.
                let properties = zbus::Proxy::new(
                    &connection,
                    service,
                    "/org/mpris/MediaPlayer2",
                    "org.freedesktop.DBus.Properties",
                )
                .await
                .unwrap();
                let mut changes = properties
                    .receive_signal("PropertiesChanged")
                    .await
                    .unwrap();
                let mut seeks = player.receive_signal("Seeked").await.unwrap();
                snapshot.metadata.as_mut().unwrap().title = "Updated title".into();
                snapshot.playback = Status::Paused;
                snapshot.volume = 0.4;
                snapshot.artwork = Some(Arc::new(Artwork {
                    width: 1,
                    height: 1,
                    rgba: Arc::from([255, 0, 0, 255]),
                }));
                controls.update(snapshot.clone());
                let signal = poll_fn(|cx| Pin::new(&mut changes).poll_next(cx))
                    .await
                    .expect("the service must emit PropertiesChanged");
                let (interface, mut changed, invalidated): (
                    String,
                    std::collections::HashMap<String, zbus::OwnedVariant>,
                    Vec<String>,
                ) = signal.body().deserialize().unwrap();
                assert_eq!(interface, "org.mpris.MediaPlayer2.Player");
                assert!(invalidated.is_empty());
                assert_eq!(changed.len(), 3);
                assert!(!changed.contains_key("Position"));
                assert_eq!(
                    String::try_from(changed.remove("PlaybackStatus").unwrap()).unwrap(),
                    "Paused"
                );
                assert_eq!(
                    f64::try_from(changed.remove("Volume").unwrap()).unwrap(),
                    0.4
                );
                let mut metadata: std::collections::HashMap<String, zbus::OwnedVariant> =
                    changed.remove("Metadata").unwrap().try_into().unwrap();
                assert_eq!(
                    String::try_from(metadata.remove("xesam:title").unwrap()).unwrap(),
                    "Updated title"
                );
                let artwork_url =
                    String::try_from(metadata.remove("mpris:artUrl").unwrap()).unwrap();
                let artwork_path = reqwest::Url::parse(&artwork_url)
                    .unwrap()
                    .to_file_path()
                    .unwrap();
                let artwork_directory = artwork_path.parent().unwrap().to_owned();
                assert_eq!(
                    &std::fs::read(&artwork_path).unwrap()[..8],
                    b"\x89PNG\r\n\x1a\n"
                );
                assert!(artwork_directory.is_dir());

                snapshot.position_seconds = 6.0;
                controls.update(snapshot.clone());
                while player.get_property::<i64>("Position").await.unwrap() != 6_000_000 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                tokio::select! {
                    signal = poll_fn(|cx| Pin::new(&mut changes).poll_next(cx)) => {
                        panic!("ordinary progress emitted PropertiesChanged: {signal:?}");
                    }
                    signal = poll_fn(|cx| Pin::new(&mut seeks).poll_next(cx)) => {
                        panic!("ordinary progress emitted Seeked: {signal:?}");
                    }
                    _ = tokio::time::sleep(Duration::from_millis(150)) => {}
                }

                snapshot.position_seconds = 9.0;
                snapshot.seek_serial += 1;
                controls.update(snapshot);
                let signal = poll_fn(|cx| Pin::new(&mut seeks).poll_next(cx))
                    .await
                    .expect("an advanced seek serial must emit Seeked");
                let (position,): (i64,) = signal.body().deserialize().unwrap();
                assert_eq!(position, 9_000_000);
                assert!(
                    tokio::time::timeout(
                        Duration::from_millis(150),
                        poll_fn(|cx| Pin::new(&mut changes).poll_next(cx)),
                    )
                    .await
                    .is_err(),
                    "a seek must not flood PropertiesChanged with Position"
                );
                drop(controls);
                // Drop joins the worker: cleanup must already be complete on return.
                assert!(!artwork_path.exists());
                assert!(!artwork_directory.exists());
                loop {
                    let owned: bool = bus.call("NameHasOwner", &(service,)).await.unwrap();
                    if !owned {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the private-bus service must register, answer, and unregister");
        });
    }
}
