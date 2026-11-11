//! The application controller: the one place where the state modules, the
//! audio engine, the database and the window meet.
//!
//! It lives on the user-interface thread. The state modules decide and return
//! [`Effect`]s; the controller carries them out and pushes what changed to the
//! window. Anything slow — file reads, picture decoding, network connects —
//! runs on another thread and comes back through [`post`].

mod artwork;
mod equalizer_sheet;
mod jellyfin_settings;
mod library_screens;
mod notices;
mod now_playing_screen;
mod radio_screen;
mod settings_screen;

use artwork::{CurrentArt, Thumbnail};
use now_playing_screen::NowPlayingLists;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};

use crate::audio::{self, AudioEngine, EngineBridge, EngineEvent};
use crate::background::Worker;
use crate::covers;
use crate::effects::Effect;
use crate::equalizer::Equalizer;
use crate::favourites::FavouriteStations;
use crate::library::{self, LoudnessStore, ScanReport, Track, WatchedFolder};
use crate::library_view::{self, AlbumGroup, ArtistGroup, QueueView};
use crate::media_server::MediaServerState;
use crate::notifications::Notifications;
use crate::persistence::{self, DbState, Preferences};
use crate::platform;
use crate::playback_options::PlaybackOptions;
use crate::player::Player;
use crate::radio::{Catalog, Station};
use crate::text;
use crate::translations::{Language, Translations};
use crate::user_interface::{self, AppWindow, Shell, Strings, Toast};
use crate::visualizer::Visualizer;

/// How long the volume must rest before it is saved.
const VOLUME_SETTLE: Duration = Duration::from_millis(300);
/// How long the equalizer sliders must rest before they are saved.
const EQUALIZER_SETTLE: Duration = Duration::from_millis(400);
/// How long after the last file of a drop the dropped files are imported.
const DROP_SETTLE: Duration = Duration::from_millis(80);
thread_local! {
    /// The controller, reachable from closures posted to this thread — so
    /// other threads never hold anything that belongs to the interface.
    static APPLICATION: RefCell<Option<Rc<Application>>> = const { RefCell::new(None) };
}

/// Runs `action` on the user-interface thread, with the controller. Callable
/// from any thread; dropped if the application is already gone.
pub fn post(action: impl FnOnce(&Application) + Send + 'static) {
    let posted = slint::invoke_from_event_loop(move || {
        let application = APPLICATION.with(|cell| cell.borrow().clone());
        if let Some(application) = application {
            action(&application);
        }
    });
    if posted.is_err() {
        log::debug!("the event loop is gone; a posted action was dropped");
    }
}

pub struct Application {
    window: AppWindow,
    database: Arc<DbState>,
    engine: Arc<AudioEngine>,
    bridge: Arc<EngineBridge>,
    catalog: Catalog,
    translations: RefCell<Translations>,
    player: RefCell<Player>,
    equalizer: RefCell<Equalizer>,
    playback_options: RefCell<PlaybackOptions>,
    notifications: RefCell<Notifications>,
    visualizer: RefCell<Visualizer>,
    tracks: RefCell<Vec<Track>>,
    albums: RefCell<Vec<AlbumGroup>>,
    artists: RefCell<Vec<ArtistGroup>>,
    current_art: RefCell<CurrentArt>,
    thumbnails: RefCell<covers::BoundedCache<covers::CoverKey, Thumbnail>>,
    /// Which cover each library track shows, renewed with the library.
    cover_keys: RefCell<covers::CoverKeys>,
    /// Bumped when the library is replaced, so a thumbnail read from the
    /// library as it was is recognised and dropped.
    thumbnail_epoch: Cell<u64>,
    queue_view: Cell<QueueView>,
    now_playing_lists: RefCell<NowPlayingLists>,
    queue_rows: Rc<VecModel<user_interface::QueueRow>>,
    spotlight_tracks: Rc<VecModel<user_interface::SpotlightTrack>>,
    spotlight_albums: Rc<VecModel<user_interface::SpotlightAlbum>>,
    equalizer_gains: Rc<VecModel<f32>>,
    equalizer_presets: Rc<VecModel<user_interface::EqualizerPreset>>,
    search: RefCell<String>,
    folders: RefCell<Vec<WatchedFolder>>,
    last_report: Cell<Option<ScanReport>>,
    library_screen: RefCell<library_screens::LibraryScreen>,
    library_albums: Rc<VecModel<user_interface::LibraryCard>>,
    library_artists: Rc<VecModel<user_interface::LibraryCard>>,
    library_songs: Rc<VecModel<user_interface::LibraryTrack>>,
    library_recent: Rc<VecModel<user_interface::LibraryTrack>>,
    library_selection_tracks: Rc<VecModel<user_interface::LibraryTrack>>,
    library_local: Rc<VecModel<user_interface::LibraryTrack>>,
    library_folders: Rc<VecModel<user_interface::WatchedFolderRow>>,
    discover_recent: Rc<VecModel<user_interface::LibraryCard>>,
    discover_rediscover: Rc<VecModel<user_interface::LibraryCard>>,
    discover_artists: Rc<VecModel<user_interface::LibraryCard>>,
    library_worker: Worker,
    favourite_stations: RefCell<FavouriteStations>,
    /// The Jellyfin server the listener signed in to, as Settings shows it.
    media_server: RefCell<MediaServerState>,
    /// Where the calls to the media server run, one after another.
    server_worker: Worker,
    /// How this installation names itself to a media server.
    device_identifier: String,
    radio_screen: RefCell<radio_screen::RadioScreen>,
    radio_countries: Rc<VecModel<user_interface::RadioChip>>,
    radio_genres: Rc<VecModel<user_interface::RadioChip>>,
    radio_stations: Rc<VecModel<user_interface::StationRow>>,
    dropped_files: RefCell<Vec<String>>,
    drop_timer: Timer,
    toasts: Rc<VecModel<Toast>>,
    picture_worker: Worker,
    /// Where stations connect and their streams download.
    station_runtime: tokio::runtime::Runtime,
    frame_timer: Timer,
    last_frame: Cell<Instant>,
    /// How long the animations have run; it stands still while nothing plays.
    animation_time: Cell<Duration>,
    volume_timer: Timer,
    /// The volume waiting for `volume_timer` to be saved.
    unsaved_volume: Cell<Option<f64>>,
    equalizer_timer: Timer,
    /// The gains and preset waiting for `equalizer_timer` to be saved.
    unsaved_equalizer: RefCell<Option<(Vec<f64>, String)>>,
}

impl Application {
    /// Boots everything behind the window and wires the window to it.
    pub fn start(window: AppWindow) -> Result<Rc<Self>, String> {
        let database = Arc::new(persistence::init(platform::data_directory()?)?);
        let preferences = persistence::get_preferences(&database)?;
        let tracks = library::list_tracks(&database)?;
        let folders = library::list_watched_folders(&database)?;
        let mut favourite_stations = FavouriteStations::default();
        favourite_stations.restore(persistence::favourite_station_ids(&database)?);
        let media_server = persistence::media_server(&database)?;
        let device_identifier = persistence::device_identifier(&database)?;

        let bridge = Arc::new(EngineBridge::new(|event| {
            post(move |application| application.on_engine_event(event));
        }));
        let engine = Arc::new(audio::init(
            Arc::new(LoudnessStore(Arc::clone(&database))),
            Arc::clone(&bridge),
        ));

        let toasts = Rc::new(VecModel::default());
        window
            .global::<Shell>()
            .set_toasts(ModelRc::from(toasts.clone()));

        let now = Instant::now();
        let application = Rc::new(Self {
            window,
            database,
            engine,
            bridge,
            catalog: Catalog::bundled(),
            translations: RefCell::new(Translations::new(Language::from_code(
                &preferences.language,
            ))),
            player: RefCell::new(Player::new(now)),
            equalizer: RefCell::new(Equalizer::default()),
            playback_options: RefCell::new(PlaybackOptions::default()),
            notifications: RefCell::new(Notifications::default()),
            visualizer: RefCell::new(Visualizer::default()),
            albums: RefCell::new(library_view::albums(&tracks)),
            artists: RefCell::new(library_view::artists(&tracks)),
            cover_keys: RefCell::new(covers::CoverKeys::new(&tracks)),
            tracks: RefCell::new(tracks),
            current_art: RefCell::new(CurrentArt::default()),
            thumbnails: RefCell::new(covers::BoundedCache::new(covers::THUMBNAIL_LIMIT)),
            thumbnail_epoch: Cell::new(0),
            queue_view: Cell::new(QueueView::default()),
            now_playing_lists: RefCell::new(NowPlayingLists::default()),
            queue_rows: Rc::new(VecModel::default()),
            spotlight_tracks: Rc::new(VecModel::default()),
            spotlight_albums: Rc::new(VecModel::default()),
            equalizer_gains: Rc::new(VecModel::default()),
            equalizer_presets: Rc::new(VecModel::default()),
            search: RefCell::new(String::new()),
            folders: RefCell::new(folders),
            last_report: Cell::new(None),
            library_screen: RefCell::new(library_screens::LibraryScreen::default()),
            library_albums: Rc::new(VecModel::default()),
            library_artists: Rc::new(VecModel::default()),
            library_songs: Rc::new(VecModel::default()),
            library_recent: Rc::new(VecModel::default()),
            library_selection_tracks: Rc::new(VecModel::default()),
            library_local: Rc::new(VecModel::default()),
            library_folders: Rc::new(VecModel::default()),
            discover_recent: Rc::new(VecModel::default()),
            discover_rediscover: Rc::new(VecModel::default()),
            discover_artists: Rc::new(VecModel::default()),
            library_worker: Worker::new("janis library")?,
            favourite_stations: RefCell::new(favourite_stations),
            media_server: RefCell::new(MediaServerState::default()),
            server_worker: Worker::new("janis media server")?,
            device_identifier,
            radio_screen: RefCell::new(radio_screen::RadioScreen::default()),
            radio_countries: Rc::new(VecModel::default()),
            radio_genres: Rc::new(VecModel::default()),
            radio_stations: Rc::new(VecModel::default()),
            dropped_files: RefCell::new(Vec::new()),
            drop_timer: Timer::default(),
            toasts,
            picture_worker: Worker::new("janis pictures")?,
            station_runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("janis stations")
                .enable_all()
                .build()
                .map_err(|error| format!("start the station runtime: {error}"))?,
            frame_timer: Timer::default(),
            last_frame: Cell::new(now),
            animation_time: Cell::new(Duration::ZERO),
            volume_timer: Timer::default(),
            unsaved_volume: Cell::new(None),
            equalizer_timer: Timer::default(),
            unsaved_equalizer: RefCell::new(None),
        });
        APPLICATION.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&application)));

        application.apply_strings();
        application.connect_window();
        application.restore(&preferences);
        // A server kept from the last run is read again in the background;
        // the window opens on the songs already in the library.
        let effects = application.media_server.borrow_mut().restore(media_server);
        application.run(effects);
        // The engine answers with its current state, as events.
        application.engine.describe();
        application.refresh_player();
        application.refresh_now_playing_lists();
        application.refresh_equalizer_presets();
        application.refresh_equalizer();
        application.refresh_library();
        application.refresh_discover();
        application.refresh_local_files();
        application.refresh_radio();
        application.refresh_settings();
        application.refresh_jellyfin();
        Ok(application)
    }

    /// Stops the engine and lets go of the controller. Called once the event
    /// loop has ended, while the process is still alive.
    pub fn shut_down(&self) {
        self.frame_timer.stop();
        // What was still waiting for its slider to settle is saved now.
        self.volume_timer.stop();
        self.save_volume();
        self.equalizer_timer.stop();
        self.save_equalizer();
        audio::shutdown(&self.engine);
        APPLICATION.with(|cell| cell.borrow_mut().take());
    }

    /// Brings the window to the front, for a second launch that found this
    /// instance running.
    pub fn come_forward(&self) {
        platform::bring_forward(self.window.window());
    }

    /// Turns the output down to nothing without touching the saved volume,
    /// so playback can be exercised in development without a sound.
    #[cfg(debug_assertions)]
    pub fn silence(&self) {
        self.engine.set_volume(0.0);
    }

    /// The library as loaded, for the development hook that searches it.
    #[cfg(debug_assertions)]
    pub fn tracks(&self) -> Vec<Track> {
        self.tracks.borrow().clone()
    }

    // ── Boot ────────────────────────────────────────────────────────────────

    fn apply_strings(&self) {
        let translations = self.translations.borrow();
        user_interface::apply_strings(&self.window.global::<Strings>(), &|key| {
            translations.text(key)
        });
    }

    /// Hydrates the state modules from the preferences row, pushing each
    /// setting to the engine without writing it back.
    fn restore(&self, preferences: &Preferences) {
        let effects = self.equalizer.borrow_mut().restore(
            &preferences.eq_gains,
            &preferences.eq_preset,
            preferences.eq_linear_phase,
        );
        self.run(effects);
        let effects = self.player.borrow_mut().restore_volume(preferences.volume);
        self.run(effects);
        let effects = self.playback_options.borrow_mut().restore(preferences);
        self.run(effects);
    }

    fn connect_window(self: &Rc<Self>) {
        let shell = self.window.global::<Shell>();
        shell.set_overlay_title_bar(platform::OVERLAY_TITLE_BAR);
        shell.on_window_drag_started({
            let weak = self.window.as_weak();
            move || {
                if let Some(window) = weak.upgrade() {
                    platform::drag_window(window.window());
                }
            }
        });
        shell.on_toast_dismissed(self.handler(|application, id: i32| {
            application.dismiss_notification(u64::try_from(id).unwrap_or_default());
        }));

        shell.on_search_edited(self.handler(|application, search: slint::SharedString| {
            *application.search.borrow_mut() = search.into();
            let started = Instant::now();
            application.refresh_library();
            application.refresh_local_files();
            application.refresh_radio();
            log::debug!("search refresh: {:?}", started.elapsed());
        }));
        self.connect_library();
        self.connect_radio();
        self.connect_settings();
        self.connect_jellyfin();
        self.connect_player();
        self.connect_equalizer();
        self.connect_covers();
        platform::on_file_dropped(self.window.window(), {
            let weak = Rc::downgrade(self);
            move |path| {
                if let Some(application) = weak.upgrade() {
                    application.file_dropped(path.to_string_lossy().into_owned());
                }
            }
        });
    }

    /// A window callback without arguments, bound to the controller for as
    /// long as it lives.
    fn action(self: &Rc<Self>, run: impl Fn(&Application) + 'static) -> impl FnMut() + 'static {
        let weak = Rc::downgrade(self);
        move || {
            if let Some(application) = weak.upgrade() {
                run(&application);
                application.refresh_player();
            }
        }
    }

    /// A window callback with one argument, bound to the controller for as
    /// long as it lives.
    fn handler<Argument: 'static>(
        self: &Rc<Self>,
        run: impl Fn(&Application, Argument) + 'static,
    ) -> impl FnMut(Argument) + 'static {
        let weak = Rc::downgrade(self);
        move |argument| {
            if let Some(application) = weak.upgrade() {
                run(&application, argument);
                application.refresh_player();
            }
        }
    }

    /// Pushes everything to the window: for the rare changes that touch it
    /// all, like a new library or another language. Actions otherwise refresh
    /// only what they changed — a volume drag must not rebuild the lists.
    fn refresh_everything(&self) {
        self.refresh_player();
        self.refresh_now_playing_lists();
        self.refresh_equalizer_presets();
        self.refresh_equalizer();
        self.refresh_library();
        self.refresh_discover();
        self.refresh_local_files();
        self.refresh_radio();
        self.refresh_settings();
        self.refresh_jellyfin();
        self.refresh_toasts();
    }

    /// One file of a drop on the window. The system reports a drop file by
    /// file, so they are gathered for a moment and imported together.
    fn file_dropped(&self, path: String) {
        self.dropped_files.borrow_mut().push(path);
        self.drop_timer
            .start(TimerMode::SingleShot, DROP_SETTLE, || {
                with_application(|application| {
                    let paths = std::mem::take(&mut *application.dropped_files.borrow_mut());
                    application.import_paths(paths);
                });
            });
    }

    // ── Playback ────────────────────────────────────────────────────────────

    /// Replaces the queue and starts at `index`.
    pub fn play_queue(&self, tracks: Vec<Track>, index: usize) {
        let effects = self
            .player
            .borrow_mut()
            .play_queue(tracks, index, Instant::now());
        self.run(effects);
        self.refresh_player();
        self.refresh_now_playing_lists();
    }

    fn on_engine_event(&self, event: EngineEvent) {
        // The lists follow the current track and the shuffle switch; most events — position reports
        // ten times a second among them — leave it where it was.
        let current_track =
            |player: &Player| (player.mode(), player.current_index(), player.shuffle());
        let (before, effects, after) = {
            let mut player = self.player.borrow_mut();
            let before = current_track(&player);
            let effects = player.apply(event, Instant::now(), &|id| {
                self.catalog.station_by_id(id).cloned()
            });
            (before, effects, current_track(&player))
        };
        self.run(effects);
        self.refresh_player();
        if before != after {
            self.refresh_now_playing_lists();
        }
    }

    /// Carries out what a state module decided.
    fn run(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::LoadQueue { tracks, index } => self.engine.load_queue(
                    tracks
                        .iter()
                        .map(|track| audio::queue_entry(track, self.track_location(track)))
                        .collect(),
                    index,
                ),
                Effect::ConnectStation { station, epoch } => self.connect_station(station, epoch),
                Effect::Toggle => self.engine.toggle(),
                Effect::Next => self.engine.next(),
                Effect::Previous => self.engine.previous(),
                Effect::JumpTo(index) => self.engine.jump_to(index),
                Effect::Seek(position_seconds) => self.engine.seek(position_seconds),
                Effect::SetShuffle(enabled) => self.engine.set_shuffle(enabled),
                Effect::SetRepeat(enabled) => self.engine.set_repeat(enabled),
                Effect::SetVolume(volume) => self.engine.set_volume(volume),
                Effect::SetEqualizerGains(gains) => self.engine.set_eq(gains),
                Effect::SetLinearPhase(enabled) => self.engine.set_fir_eq(enabled),
                Effect::SetGapless(enabled) => self.engine.set_gapless(enabled),
                Effect::SetCrossfade(enabled) => self.engine.set_crossfade(enabled),
                Effect::SetNormalize(enabled) => self.engine.set_normalize(enabled),
                Effect::PersistVolumeDebounced(volume) => {
                    self.unsaved_volume.set(Some(volume));
                    self.volume_timer
                        .start(TimerMode::SingleShot, VOLUME_SETTLE, || {
                            with_application(Application::save_volume);
                        });
                }
                Effect::PersistEqualizer { gains, preset } => {
                    self.equalizer_timer.stop();
                    *self.unsaved_equalizer.borrow_mut() = Some((gains, preset));
                    self.save_equalizer();
                }
                Effect::PersistEqualizerDebounced { gains, preset } => {
                    *self.unsaved_equalizer.borrow_mut() = Some((gains, preset));
                    self.equalizer_timer
                        .start(TimerMode::SingleShot, EQUALIZER_SETTLE, || {
                            with_application(Application::save_equalizer);
                        });
                }
                Effect::PersistLinearPhase(enabled) => self.saved(
                    "eq-linear-phase",
                    persistence::set_eq_linear_phase(&self.database, enabled),
                ),
                Effect::PersistPlaybackOption { option, enabled } => self.saved(
                    "playback-option",
                    persistence::set_playback_option(&self.database, option, enabled),
                ),
                Effect::PersistFavouriteStation {
                    station_id,
                    favourite,
                } => self.saved(
                    "favourite-station",
                    persistence::set_favourite_station(&self.database, &station_id, favourite),
                ),
                Effect::SignInMediaServer {
                    address,
                    user_name,
                    password,
                    epoch,
                } => self.sign_in_media_server(address, user_name, password, epoch),
                Effect::SyncMediaServer { server, epoch } => self.sync_media_server(server, epoch),
                Effect::SignOutMediaServer { server } => self.sign_out_media_server(server),
                Effect::LoadCover(track_id) => self.load_current_cover(track_id),
                Effect::ShowStreamCover(url) => self.show_stream_cover(url),
                Effect::LinearPhaseReported {
                    enabled,
                    latency_seconds,
                } => {
                    log::debug!(
                        "linear-phase equalizer: enabled {enabled}, latency {latency_seconds} s"
                    );
                    self.equalizer
                        .borrow_mut()
                        .apply_engine_report(enabled, latency_seconds);
                    self.refresh_equalizer();
                }
                Effect::NotifyError {
                    key,
                    parameters,
                    dedupe_key,
                } => self.notify_error(key, parameters, dedupe_key),
            }
        }
    }

    /// Connects to a station off the interface thread — the connect waits on
    /// the network — then tells the player how it went. The stream keeps
    /// downloading on the same runtime for as long as the station plays.
    fn connect_station(&self, station: Station, epoch: u64) {
        let engine = Arc::clone(&self.engine);
        self.station_runtime.spawn(async move {
            let outcome = engine
                .play_stream(
                    station.id.clone(),
                    station.url.clone(),
                    station.now_playing.clone(),
                )
                .await;
            post(move |application| {
                let effects = application
                    .player
                    .borrow_mut()
                    .station_connection_finished(&station, epoch, outcome);
                application.run(effects);
                application.refresh_player();
                application.refresh_now_playing_lists();
            });
        });
    }

    /// Saves the volume a drag left waiting, if any.
    fn save_volume(&self) {
        if let Some(volume) = self.unsaved_volume.take() {
            self.saved("volume", persistence::set_volume(&self.database, volume));
        }
    }

    /// Saves the equalizer settings a drag left waiting, if any.
    fn save_equalizer(&self) {
        let unsaved = self.unsaved_equalizer.borrow_mut().take();
        if let Some((gains, preset)) = unsaved {
            self.saved(
                "equalizer",
                persistence::set_eq(&self.database, &gains, &preset),
            );
        }
    }
}

/// Runs `action` with the controller, from a timer on the interface thread;
/// dropped if the application is already gone.
fn with_application(action: impl FnOnce(&Application)) {
    let application = APPLICATION.with(|cell| cell.borrow().clone());
    if let Some(application) = application {
        action(&application);
    }
}

/// A track id as the interface carries it. Ids come from SQLite row ids and
/// stay far below the interface's limit; one beyond it shows as no track.
fn interface_id(track_id: i64) -> i32 {
    i32::try_from(track_id).unwrap_or_default()
}

/// The gradient an album's generated art wears.
fn album_gradient_index(album: &AlbumGroup) -> i32 {
    text::art_gradient_index(&format!(
        "{}-{}",
        album.artist.as_deref().unwrap_or(""),
        album.album.as_deref().unwrap_or("")
    )) as i32
}

/// The gradient a track's generated art wears.
fn gradient_index(track: &Track) -> i32 {
    text::art_gradient_index(&library_view::art_seed(track)) as i32
}
