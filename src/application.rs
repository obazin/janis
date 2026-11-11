//! The application controller: the one place where the state modules, the
//! audio engine, the database and the window meet.
//!
//! It lives on the user-interface thread. The state modules decide and return
//! [`Effect`]s; the controller carries them out and pushes what changed to the
//! window. Anything slow — file reads, picture decoding, network connects —
//! runs on another thread and comes back through [`post`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Image, ModelRc, Timer, TimerMode, VecModel};

use crate::audio::{self, AudioEngine, EngineBridge, EngineEvent, Mode};
use crate::background::Worker;
use crate::covers::{self, Pixels};
use crate::effects::Effect;
use crate::equalizer::{self, Equalizer};
use crate::halo;
use crate::library::{self, LoudnessStore, Track};
use crate::library_view::{self, AlbumGroup, ArtistGroup, QueueView};
use crate::notifications::{self, Notifications};
use crate::persistence::{self, DbState, Preferences};
use crate::platform;
use crate::playback_options::PlaybackOptions;
use crate::player::Player;
use crate::radio::{Catalog, Station};
use crate::text;
use crate::translations::{Language, Translations};
use crate::user_interface::{self, AppWindow, Covers, Shell, Strings, Toast, Tokens};
use crate::visualizer::Visualizer;

/// How often the waveform is redrawn while it moves.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// How long the volume must rest before it is saved.
const VOLUME_SETTLE: Duration = Duration::from_millis(300);
/// How long the equalizer sliders must rest before they are saved.
const EQUALIZER_SETTLE: Duration = Duration::from_millis(400);
/// The seed of the art shown when nothing is loaded.
const IDLE_ART_SEED: &str = "janis";

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

/// The art of whatever is playing.
#[derive(Default)]
struct CurrentArt {
    /// Bumped on every change of track, so a slow load that finishes after
    /// the listener moved on is recognised and dropped.
    epoch: u64,
    track_cover: Option<Image>,
    track_cover_failed: bool,
    stream_cover: Option<Image>,
}

/// A cover for a list or grid tile.
enum Thumbnail {
    Loading,
    Ready(Image),
    /// Neither the file nor its album carries art.
    Absent,
    /// Art exists but could not be read.
    Failed,
}

/// What the rows of the Now Playing lists stand for, kept so a click on a
/// row finds what to play.
#[derive(Default)]
struct NowPlayingLists {
    queue_rows: Vec<library_view::QueueRow>,
    album_tracks: Vec<Track>,
    spotlight_tracks: Vec<Track>,
    spotlight_albums: Vec<AlbumGroup>,
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
    thumbnails: RefCell<covers::BoundedCache<Thumbnail>>,
    queue_view: Cell<QueueView>,
    now_playing_lists: RefCell<NowPlayingLists>,
    queue_rows: Rc<VecModel<user_interface::QueueRow>>,
    spotlight_tracks: Rc<VecModel<user_interface::SpotlightTrack>>,
    spotlight_albums: Rc<VecModel<user_interface::SpotlightAlbum>>,
    equalizer_gains: Rc<VecModel<f32>>,
    toasts: Rc<VecModel<Toast>>,
    picture_worker: Worker,
    frame_timer: Timer,
    last_frame: Cell<Instant>,
    volume_timer: Timer,
    equalizer_timer: Timer,
}

impl Application {
    /// Boots everything behind the window and wires the window to it.
    pub fn start(window: AppWindow) -> Result<Rc<Self>, String> {
        let database = Arc::new(persistence::init(platform::data_directory()?)?);
        let preferences = persistence::get_preferences(&database)?;
        let tracks = library::list_tracks(&database)?;

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
            tracks: RefCell::new(tracks),
            current_art: RefCell::new(CurrentArt::default()),
            thumbnails: RefCell::new(covers::BoundedCache::new(covers::THUMBNAIL_LIMIT)),
            queue_view: Cell::new(QueueView::default()),
            now_playing_lists: RefCell::new(NowPlayingLists::default()),
            queue_rows: Rc::new(VecModel::default()),
            spotlight_tracks: Rc::new(VecModel::default()),
            spotlight_albums: Rc::new(VecModel::default()),
            equalizer_gains: Rc::new(VecModel::default()),
            toasts,
            picture_worker: Worker::new("janis pictures")?,
            frame_timer: Timer::default(),
            last_frame: Cell::new(now),
            volume_timer: Timer::default(),
            equalizer_timer: Timer::default(),
        });
        APPLICATION.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&application)));

        application.apply_strings();
        application.connect_window();
        application.restore(&preferences);
        // The engine answers with its current state, as events.
        application.engine.describe();
        application.refresh_player();
        application.refresh_now_playing_lists();
        application.refresh_equalizer();
        Ok(application)
    }

    /// Stops the engine and lets go of the controller. Called once the event
    /// loop has ended, while the process is still alive.
    pub fn shut_down(&self) {
        self.frame_timer.stop();
        audio::shutdown(&self.engine);
        APPLICATION.with(|cell| cell.borrow_mut().take());
    }

    /// Turns the output down to nothing without touching the saved volume,
    /// so playback can be exercised in development without a sound.
    #[cfg(debug_assertions)]
    pub fn silence(&self) {
        self.engine.set_volume(0.0);
    }

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

        let player = self.window.global::<user_interface::Player>();
        player.on_toggled(self.action(|application| {
            let effects = application.player.borrow_mut().toggle(Instant::now());
            application.run(effects);
        }));
        player.on_next(self.action(|application| {
            let effects = application.player.borrow().next();
            application.run(effects);
        }));
        player.on_previous(self.action(|application| {
            let effects = application.player.borrow().previous();
            application.run(effects);
        }));
        player.on_shuffle_toggled(self.action(|application| {
            let effects = application.player.borrow_mut().toggle_shuffle();
            application.run(effects);
        }));
        player.on_repeat_toggled(self.action(|application| {
            let effects = application.player.borrow_mut().toggle_repeat();
            application.run(effects);
        }));
        player.on_volume_changed(self.handler(|application, volume: f32| {
            let effects = application
                .player
                .borrow_mut()
                .set_volume(f64::from(volume));
            application.run(effects);
        }));
        player.on_seek(self.handler(|application, fraction: f32| {
            let effects = application
                .player
                .borrow_mut()
                .seek_to(f64::from(fraction), Instant::now());
            application.run(effects);
        }));

        player.set_halo(Image::from_rgba8_premultiplied(halo::picture(
            &self.prism_colors(),
        )));
        player.set_queue_rows(ModelRc::from(self.queue_rows.clone()));
        player.set_spotlight_tracks(ModelRc::from(self.spotlight_tracks.clone()));
        player.set_spotlight_albums(ModelRc::from(self.spotlight_albums.clone()));
        player.on_queue_view_selected(self.handler(|application, view| {
            application.queue_view.set(match view {
                user_interface::QueueView::UpNext => QueueView::UpNext,
                user_interface::QueueView::Playlist => QueueView::Playlist,
                user_interface::QueueView::Album => QueueView::Album,
            });
        }));
        player.on_queue_row_clicked(self.handler(|application, index: i32| {
            application.play_queue_row(usize::try_from(index).unwrap_or_default());
        }));
        player.on_spotlight_track_clicked(self.handler(|application, index: i32| {
            application.play_spotlight_track(usize::try_from(index).unwrap_or_default());
        }));
        player.on_spotlight_album_clicked(self.handler(|application, index: i32| {
            application.play_spotlight_album(usize::try_from(index).unwrap_or_default());
        }));

        let equalizer = self.window.global::<user_interface::Equalizer>();
        equalizer.set_gains(ModelRc::from(self.equalizer_gains.clone()));
        equalizer.set_gain_range(equalizer::GAIN_RANGE as f32);
        equalizer.set_frequency_labels(ModelRc::from(Rc::new(VecModel::from(
            equalizer::frequency_labels()
                .map(slint::SharedString::from)
                .to_vec(),
        ))));
        equalizer.on_band_changed({
            let weak = Rc::downgrade(self);
            move |index, gain| {
                if let Some(application) = weak.upgrade() {
                    let effects = application.equalizer.borrow_mut().set_band(
                        usize::try_from(index).unwrap_or(usize::MAX),
                        f64::from(gain),
                    );
                    application.run(effects);
                    application.refresh_equalizer();
                }
            }
        });
        equalizer.on_preset_selected(self.handler(|application, name: slint::SharedString| {
            let effects = application.equalizer.borrow_mut().set_preset(&name);
            application.run(effects);
        }));
        equalizer.on_reset(self.action(|application| {
            let effects = application.equalizer.borrow_mut().reset();
            application.run(effects);
        }));
        equalizer.on_linear_phase_toggled(self.action(|application| {
            let enabled = !application.equalizer.borrow().linear_phase();
            let effects = application.equalizer.borrow_mut().set_linear_phase(enabled);
            application.run(effects);
        }));

        let covers = self.window.global::<Covers>();
        let weak = Rc::downgrade(self);
        covers.on_thumbnail(move |track_id, _revision| {
            weak.upgrade()
                .map(|application| application.thumbnail(i64::from(track_id)))
                .unwrap_or_default()
        });
        let weak = Rc::downgrade(self);
        covers.on_unavailable(move |track_id, _revision| {
            weak.upgrade().is_some_and(|application| {
                matches!(
                    application.thumbnails.borrow().get(i64::from(track_id)),
                    Some(Thumbnail::Failed)
                )
            })
        });

        let weak = Rc::downgrade(self);
        self.frame_timer
            .start(TimerMode::Repeated, FRAME_INTERVAL, move || {
                if let Some(application) = weak.upgrade() {
                    application.draw_frame();
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
                application.refresh_after_action();
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
                application.refresh_after_action();
            }
        }
    }

    fn refresh_after_action(&self) {
        self.refresh_player();
        self.refresh_now_playing_lists();
        self.refresh_equalizer();
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
        // Position reports arrive ten times a second and change no list.
        let lists_may_change = !matches!(event, EngineEvent::Position { .. });
        let effects = self
            .player
            .borrow_mut()
            .apply(event, Instant::now(), &|id| {
                self.catalog.station_by_id(id).cloned()
            });
        self.run(effects);
        self.refresh_player();
        if lists_may_change {
            self.refresh_now_playing_lists();
        }
    }

    /// Carries out what a state module decided.
    fn run(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::LoadQueue { tracks, index } => self
                    .engine
                    .load_queue(tracks.iter().map(audio::queue_entry).collect(), index),
                Effect::ConnectStation(station) => self.connect_station(station),
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
                    let database = Arc::clone(&self.database);
                    self.volume_timer
                        .start(TimerMode::SingleShot, VOLUME_SETTLE, move || {
                            report_unsaved("volume", persistence::set_volume(&database, volume));
                        });
                }
                Effect::PersistEqualizer { gains, preset } => {
                    self.equalizer_timer.stop();
                    self.saved(
                        "equalizer",
                        persistence::set_eq(&self.database, &gains, &preset),
                    );
                }
                Effect::PersistEqualizerDebounced { gains, preset } => {
                    let database = Arc::clone(&self.database);
                    self.equalizer_timer.start(
                        TimerMode::SingleShot,
                        EQUALIZER_SETTLE,
                        move || {
                            report_unsaved(
                                "equalizer",
                                persistence::set_eq(&database, &gains, &preset),
                            );
                        },
                    );
                }
                Effect::PersistLinearPhase(enabled) => self.saved(
                    "eq-linear-phase",
                    persistence::set_eq_linear_phase(&self.database, enabled),
                ),
                Effect::PersistPlaybackOption { option, enabled } => self.saved(
                    "playback-option",
                    persistence::set_playback_option(&self.database, option, enabled),
                ),
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

    /// Connects to a station on a thread of its own — the connect waits on
    /// the network — then tells the player how it went.
    fn connect_station(&self, station: Station) {
        let engine = Arc::clone(&self.engine);
        let spawned = std::thread::Builder::new()
            .name("janis station connect".to_owned())
            .spawn(move || {
                let outcome = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("start the connect runtime: {error}"))
                    .and_then(|runtime| {
                        runtime.block_on(engine.play_stream(
                            station.id.clone(),
                            station.url.clone(),
                            station.now_playing.clone(),
                        ))
                    });
                post(move |application| {
                    let effects = application
                        .player
                        .borrow_mut()
                        .station_connection_finished(&station, outcome);
                    application.run(effects);
                    application.refresh_player();
                    application.refresh_now_playing_lists();
                });
            });
        if let Err(error) = spawned {
            log::error!("start the station connect thread: {error}");
        }
    }

    /// A click on a row of the Now Playing queue card.
    fn play_queue_row(&self, index: usize) {
        let (list_index, album_tracks) = {
            let lists = self.now_playing_lists.borrow();
            let Some(row) = lists.queue_rows.get(index) else {
                return;
            };
            (row.list_index, lists.album_tracks.clone())
        };
        if self.queue_view.get() == QueueView::Album {
            // The album card can list tracks that are not in the queue at
            // all; playing one replaces the queue with the album.
            self.play_queue(album_tracks, list_index);
        } else {
            // Both queue views show the live queue itself: jump within it
            // rather than reloading it, which would reshuffle the order the
            // listener was just reading.
            let effects = self.player.borrow().jump_to(list_index);
            self.run(effects);
        }
    }

    fn play_spotlight_track(&self, index: usize) {
        let tracks = self.now_playing_lists.borrow().spotlight_tracks.clone();
        let Some(chosen) = tracks.get(index) else {
            return;
        };
        // The whole artist plays, starting from the chosen track.
        let list = self
            .artists
            .borrow()
            .iter()
            .find(|group| Some(&group.artist) == chosen.artist.as_ref())
            .map(|group| group.tracks.clone())
            .unwrap_or_else(|| vec![chosen.clone()]);
        let start = list
            .iter()
            .position(|track| track.id == chosen.id)
            .unwrap_or_default();
        self.play_queue(list, start);
    }

    fn play_spotlight_album(&self, index: usize) {
        let tracks = self
            .now_playing_lists
            .borrow()
            .spotlight_albums
            .get(index)
            .map(|album| album.tracks.clone());
        if let Some(tracks) = tracks {
            self.play_queue(tracks, 0);
        }
    }

    // ── Cover art ───────────────────────────────────────────────────────────

    fn load_current_cover(&self, track_id: Option<i64>) {
        let epoch = {
            let mut art = self.current_art.borrow_mut();
            art.epoch += 1;
            art.track_cover = None;
            art.track_cover_failed = false;
            art.epoch
        };
        let Some(track_id) = track_id else {
            return;
        };
        let database = Arc::clone(&self.database);
        self.picture_worker.run(move || {
            let loaded = covers::track_cover(&database, track_id, covers::HERO_SIDE);
            post(move |application| application.current_cover_loaded(epoch, loaded));
        });
    }

    fn current_cover_loaded(&self, epoch: u64, loaded: Result<Option<Pixels>, String>) {
        {
            let mut art = self.current_art.borrow_mut();
            if art.epoch != epoch {
                return;
            }
            match loaded {
                Ok(pixels) => art.track_cover = pixels.map(Image::from_rgba8),
                Err(error) => {
                    log::error!("current cover: {error}");
                    art.track_cover_failed = true;
                }
            }
        }
        if self.current_art.borrow().track_cover_failed {
            self.notify_cover_unavailable();
        }
        self.refresh_player();
    }

    fn show_stream_cover(&self, url: Option<String>) {
        let Some(url) = url else {
            self.current_art.borrow_mut().stream_cover = None;
            return;
        };
        self.picture_worker.run(move || {
            let decoded = covers::data_url_cover(&url, covers::HERO_SIDE);
            post(move |application| {
                match decoded {
                    Ok(pixels) => {
                        application.current_art.borrow_mut().stream_cover =
                            Some(Image::from_rgba8(pixels));
                    }
                    Err(error) => {
                        log::error!("station cover: {error}");
                        application.notify_cover_unavailable();
                    }
                }
                application.refresh_player();
            });
        });
    }

    /// The cover of a library track for a list or grid tile. Asking for one
    /// that is not cached starts its load; the tile is told when it lands.
    fn thumbnail(&self, track_id: i64) -> Image {
        if track_id <= 0 {
            return Image::default();
        }
        if let Some(thumbnail) = self.thumbnails.borrow().get(track_id) {
            return match thumbnail {
                Thumbnail::Ready(image) => image.clone(),
                Thumbnail::Loading | Thumbnail::Absent | Thumbnail::Failed => Image::default(),
            };
        }
        self.thumbnails
            .borrow_mut()
            .insert(track_id, Thumbnail::Loading);
        let database = Arc::clone(&self.database);
        self.picture_worker.run(move || {
            let loaded = covers::track_cover(&database, track_id, covers::THUMBNAIL_SIDE);
            post(move |application| application.thumbnail_loaded(track_id, loaded));
        });
        Image::default()
    }

    fn thumbnail_loaded(&self, track_id: i64, loaded: Result<Option<Pixels>, String>) {
        let thumbnail = match loaded {
            Ok(Some(pixels)) => Thumbnail::Ready(Image::from_rgba8(pixels)),
            Ok(None) => Thumbnail::Absent,
            Err(error) => {
                log::error!("cover of track {track_id}: {error}");
                self.notify_cover_unavailable();
                Thumbnail::Failed
            }
        };
        self.thumbnails.borrow_mut().insert(track_id, thumbnail);
        let covers = self.window.global::<Covers>();
        covers.set_revision(covers.get_revision().wrapping_add(1));
    }

    fn notify_cover_unavailable(&self) {
        self.notify_error("error.cover", Vec::new(), Some("cover-unavailable"));
    }

    // ── Notifications ───────────────────────────────────────────────────────

    fn notify_error(
        &self,
        key: &'static str,
        parameters: Vec<(&'static str, String)>,
        dedupe_key: Option<&'static str>,
    ) {
        let raised = self
            .notifications
            .borrow_mut()
            .error(key, parameters, dedupe_key);
        let Some(id) = raised else {
            return;
        };
        self.refresh_toasts();
        Timer::single_shot(notifications::DEFAULT_TIMEOUT, move || {
            APPLICATION.with(|cell| {
                if let Some(application) = cell.borrow().clone() {
                    application.dismiss_notification(id);
                }
            });
        });
    }

    fn dismiss_notification(&self, id: u64) {
        self.notifications.borrow_mut().dismiss(id);
        self.refresh_toasts();
    }

    fn refresh_toasts(&self) {
        let translations = self.translations.borrow();
        let toasts: Vec<Toast> = self
            .notifications
            .borrow()
            .items()
            .iter()
            .map(|notification| Toast {
                id: i32::try_from(notification.id).unwrap_or(i32::MAX),
                message: translations
                    .translate(notification.message_key, &notification.parameters)
                    .into(),
            })
            .collect();
        self.toasts.set_vec(toasts);
    }

    /// Tells the user when a choice could not be saved, instead of letting
    /// the next launch silently forget it.
    fn saved(&self, what: &'static str, outcome: Result<(), String>) {
        if let Err(error) = outcome {
            log::error!("save {what}: {error}");
            self.notify_error("error.preferences", Vec::new(), Some(what));
        }
    }

    // ── Window ──────────────────────────────────────────────────────────────

    /// Pushes the playback state to the window. Cheap enough to call after
    /// every action and engine event: setting an unchanged property is free.
    fn refresh_player(&self) {
        let player = self.player.borrow();
        let translations = self.translations.borrow();
        let art = self.current_art.borrow();
        let view = self.window.global::<user_interface::Player>();
        let current = player.current();
        let station = player.station();

        view.set_active(player.mode() != Mode::Idle);
        view.set_playing(player.playing());
        view.set_shuffle(player.shuffle());
        view.set_repeat(player.repeat());
        view.set_volume(player.volume() as f32);

        view.set_title(
            current
                .map(|track| track.title.as_str())
                .or(station.map(|station| station.name.as_str()))
                .unwrap_or("—")
                .into(),
        );
        view.set_subtitle(
            match (current, station) {
                (Some(track), _) => track
                    .artist
                    .clone()
                    .unwrap_or_else(|| translations.text("common.unknownArtist")),
                (None, Some(station)) => translations.text(&station.genre_key),
                (None, None) => String::new(),
            }
            .into(),
        );
        view.set_gradient_index(match (current, station) {
            (_, Some(station)) => station.gradient_index % text::ART_GRADIENT_COUNT,
            (Some(track), None) => text::art_gradient_index(&library_view::art_seed(track)),
            (None, None) => text::art_gradient_index(IDLE_ART_SEED),
        } as i32);
        let cover = if player.mode() == Mode::Radio {
            art.stream_cover.clone()
        } else {
            art.track_cover.clone()
        };
        view.set_cover(cover.unwrap_or_default());
        view.set_cover_unavailable(player.mode() == Mode::Local && art.track_cover_failed);

        let stream = player.stream();
        let hero_title = current
            .map(|track| track.title.as_str())
            .or(stream.title.as_deref())
            .or(station.map(|station| station.name.as_str()))
            .unwrap_or_default();
        view.set_local(current.is_some());
        view.set_radio(station.is_some());
        view.set_hero_title(hero_title.into());
        view.set_initials(text::art_initials(hero_title).into());
        view.set_elapsed(text::format_time(player.position_seconds()).into());
        view.set_total(
            if current.is_some() {
                text::format_time(player.duration_seconds())
            } else {
                translations.text("radio.live")
            }
            .into(),
        );
        view.set_eyebrow(
            match (current, station) {
                (Some(track), _) => match &track.album {
                    Some(album) => {
                        translations.translate("now.eyebrow", &[("album", album.clone())])
                    }
                    None => translations.text("now.eyebrowNoAlbum"),
                },
                (None, Some(station)) => {
                    translations.translate("now.eyebrowRadio", &[("station", station.name.clone())])
                }
                (None, None) => String::new(),
            }
            .into(),
        );
        view.set_quality(current.map(text::quality_label).unwrap_or_default().into());
        view.set_lossless(current.is_some_and(|track| track.lossless));
        view.set_artist(
            match current {
                Some(track) => track
                    .artist
                    .clone()
                    .unwrap_or_else(|| translations.text("common.unknownArtist")),
                None => stream.artist.clone().unwrap_or_default(),
            }
            .into(),
        );
        view.set_composer(
            current
                .and_then(|track| track.composer.clone())
                .unwrap_or_default()
                .into(),
        );
        view.set_album(
            match current {
                Some(track) => track
                    .album
                    .clone()
                    .unwrap_or_else(|| translations.text("common.unknownAlbum")),
                None => stream.album.clone().unwrap_or_default(),
            }
            .into(),
        );
        view.set_album_line(
            current
                .and_then(|track| library_view::album_line(track, &translations))
                .unwrap_or_default()
                .into(),
        );
        view.set_genre(
            current
                .and_then(|track| track.genre.clone())
                .unwrap_or_default()
                .into(),
        );
        view.set_station_name(
            station
                .map(|station| station.name.clone())
                .unwrap_or_default()
                .into(),
        );
        view.set_station_genre(
            station
                .map(|station| translations.text(&station.genre_key))
                .unwrap_or_default()
                .into(),
        );

        // The waveform stops ticking once it has come to rest; anything that
        // sets it moving again starts it back up.
        if !self.frame_timer.running() && !self.visualizer.borrow().at_rest(player.playing()) {
            self.last_frame.set(Instant::now());
            self.frame_timer.restart();
        }
    }

    /// Rebuilds the lists Now Playing shows around the current track: the
    /// queue card's rows and the artist spotlight.
    fn refresh_now_playing_lists(&self) {
        let player = self.player.borrow();
        let translations = self.translations.borrow();
        let albums = self.albums.borrow();
        let artists = self.artists.borrow();
        let view = self.window.global::<user_interface::Player>();
        let current = player.current();
        let unknown_artist = translations.text("common.unknownArtist");
        let unknown_album = translations.text("common.unknownAlbum");

        let album_tracks = current
            .map(|track| library_view::album_tracks_of(&albums, track))
            .unwrap_or_default();
        let queue_view = self.queue_view.get();
        let rows = library_view::queue_rows(queue_view, player.queue(), current, &album_tracks);
        view.set_queue_view(match queue_view {
            QueueView::UpNext => user_interface::QueueView::UpNext,
            QueueView::Playlist => user_interface::QueueView::Playlist,
            QueueView::Album => user_interface::QueueView::Album,
        });
        view.set_queue_heading(
            library_view::queue_heading(
                queue_view,
                player.queue().len(),
                rows.len(),
                current,
                &translations,
            )
            .into(),
        );
        self.queue_rows.set_vec(
            rows.iter()
                .map(|row| user_interface::QueueRow {
                    track_id: interface_id(row.track.id),
                    number: format!("{:02}", row.display).into(),
                    title: row.track.title.as_str().into(),
                    artist: row
                        .track
                        .artist
                        .as_deref()
                        .unwrap_or(&unknown_artist)
                        .into(),
                    duration: text::format_time(row.track.duration_secs).into(),
                    gradient_index: gradient_index(&row.track),
                    current: row.is_current,
                })
                .collect::<Vec<_>>(),
        );

        let artist = current.and_then(|track| track.artist.as_ref());
        let spotlight_tracks: Vec<Track> = artist
            .and_then(|artist| artists.iter().find(|group| &group.artist == artist))
            .map(|group| {
                group
                    .tracks
                    .iter()
                    .take(library_view::SPOTLIGHT_COUNT)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let spotlight_albums: Vec<AlbumGroup> = artist
            .map(|artist| {
                albums
                    .iter()
                    .filter(|album| album.artist.as_ref() == Some(artist))
                    .take(library_view::SPOTLIGHT_COUNT)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        view.set_spotlight_shown(spotlight_tracks.len() > 1 || !spotlight_albums.is_empty());
        view.set_spotlight_heading(
            translations
                .translate(
                    "now.moreFrom",
                    &[("artist", artist.cloned().unwrap_or_default())],
                )
                .into(),
        );
        self.spotlight_tracks.set_vec(
            spotlight_tracks
                .iter()
                .map(|track| user_interface::SpotlightTrack {
                    title: track.title.as_str().into(),
                    album: track.album.as_deref().unwrap_or(&unknown_album).into(),
                })
                .collect::<Vec<_>>(),
        );
        self.spotlight_albums.set_vec(
            spotlight_albums
                .iter()
                .map(|album| user_interface::SpotlightAlbum {
                    title: album.album.as_deref().unwrap_or(&unknown_album).into(),
                    detail: translations
                        .translate(
                            "library.tracks",
                            &[("count", album.tracks.len().to_string())],
                        )
                        .into(),
                    track_id: album
                        .tracks
                        .first()
                        .map(|track| interface_id(track.id))
                        .unwrap_or_default(),
                    gradient_index: text::art_gradient_index(&format!(
                        "{}-{}",
                        album.artist.as_deref().unwrap_or(""),
                        album.album.as_deref().unwrap_or("")
                    )) as i32,
                })
                .collect::<Vec<_>>(),
        );

        *self.now_playing_lists.borrow_mut() = NowPlayingLists {
            queue_rows: rows,
            album_tracks,
            spotlight_tracks,
            spotlight_albums,
        };
    }

    /// Pushes the equalizer's state to its sheet.
    fn refresh_equalizer(&self) {
        let equalizer = self.equalizer.borrow();
        let translations = self.translations.borrow();
        let view = self.window.global::<user_interface::Equalizer>();
        self.equalizer_gains.set_vec(
            equalizer
                .gains()
                .iter()
                .map(|gain| *gain as f32)
                .collect::<Vec<_>>(),
        );
        view.set_preset(equalizer.preset().into());
        view.set_presets(ModelRc::from(Rc::new(VecModel::from(
            equalizer::PRESETS
                .iter()
                .map(|(name, _)| user_interface::EqualizerPreset {
                    name: (*name).into(),
                    label: translations.text(&equalizer::preset_label_key(name)).into(),
                })
                .collect::<Vec<_>>(),
        ))));
        view.set_linear_phase(equalizer.linear_phase());
        view.set_linear_phase_description(
            if equalizer.linear_phase() && equalizer.latency_seconds() > 0.0 {
                let milliseconds = (equalizer.latency_seconds() * 1000.0).round();
                translations.translate(
                    "eq.linearPhase.latency",
                    &[("ms", format!("{milliseconds:.0}"))],
                )
            } else {
                translations.text("eq.linearPhase.desc")
            }
            .into(),
        );
    }

    /// The prism colors, read from the design tokens, as red, green and blue.
    fn prism_colors(&self) -> Vec<[u8; 3]> {
        let tokens = self.window.global::<Tokens>();
        [
            tokens.get_accent(),
            tokens.get_violet(),
            tokens.get_teal(),
            tokens.get_lime(),
        ]
        .map(|color| [color.red(), color.green(), color.blue()])
        .to_vec()
    }

    /// Advances the waveform by one frame and hands each view its trace.
    fn draw_frame(&self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_frame.replace(now));
        let player = self.player.borrow();
        let playing = player.playing();
        let frame = if player.mode() == Mode::Idle {
            None
        } else {
            self.bridge.latest_frame()
        };
        let mut visualizer = self.visualizer.borrow_mut();
        visualizer.tick(
            elapsed.as_secs_f32(),
            frame.as_ref().map(|bytes| bytes.as_slice()),
            playing,
        );

        let view = self.window.global::<user_interface::Player>();
        let (width, height) = (view.get_mini_wave_width(), view.get_mini_wave_height());
        if width > 0.0 && height > 0.0 {
            view.set_mini_wave_commands(visualizer.path_commands(width, height).into());
        }
        let (width, height) = (view.get_big_wave_width(), view.get_big_wave_height());
        if width > 0.0 && height > 0.0 {
            view.set_big_wave_commands(visualizer.path_commands(width, height).into());
        }
        let progress = player.live_progress(now) as f32;
        view.set_progress(progress);
        view.set_playhead_level(visualizer.height_at(progress, 1.0));

        if visualizer.at_rest(playing) {
            self.frame_timer.stop();
        }
    }
}

/// For saves that happen after the action that asked for them has returned.
fn report_unsaved(what: &'static str, outcome: Result<(), String>) {
    APPLICATION.with(|cell| {
        if let Some(application) = cell.borrow().clone() {
            application.saved(what, outcome);
        }
    });
}

/// A track id as the interface carries it. Ids come from SQLite row ids and
/// stay far below the interface's limit; one beyond it shows as no track.
fn interface_id(track_id: i64) -> i32 {
    i32::try_from(track_id).unwrap_or_default()
}

/// The gradient a track's generated art wears.
fn gradient_index(track: &Track) -> i32 {
    text::art_gradient_index(&library_view::art_seed(track)) as i32
}
