//! The controller's side of Now Playing and the mini player: transport,
//! the queue card, the artist spotlight, and the frame clock of everything
//! that moves during playback.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Image, ModelRc, TimerMode};

use super::{album_gradient_index, gradient_index, interface_id, list_place, Application};
use crate::audio::Mode;
use crate::halo;
use crate::library::Track;
use crate::library_view::{self, AlbumGroup, QueueView};
use crate::now_playing;
use crate::player::Player;
use crate::text;
use crate::translations::Translations;
use crate::user_interface::{self, Tokens};

/// How often the window is redrawn while something plays: the waveform, the
/// glow behind the art and the play button's pulse all move on this one clock.
/// The renderer repaints the whole window for any change, so the rate is what
/// playback costs the graphics processor; thirty frames a second halves it
/// against sixty.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// What the rows of the Now Playing lists stand for, kept so a click on a
/// row finds what to play.
#[derive(Default)]
pub(super) struct NowPlayingLists {
    queue_rows: Vec<library_view::QueueRow>,
    album_tracks: Vec<Track>,
    spotlight_tracks: Vec<Track>,
    spotlight_albums: Vec<AlbumGroup>,
}

impl Application {
    /// Wires the `Player` global: transport, the queue card, the spotlight,
    /// and the clock everything that moves during playback runs on.
    pub(super) fn connect_player(self: &Rc<Self>) {
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
            // Up Next reads differently once the order is the engine's draw.
            application.refresh_now_playing_lists();
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
            application.refresh_now_playing_lists();
        }));
        player.on_queue_row_clicked(self.handler(|application, index: i32| {
            if let Some(place) = list_place(index) {
                application.play_queue_row(place);
            }
        }));
        player.on_spotlight_track_clicked(self.handler(|application, index: i32| {
            if let Some(place) = list_place(index) {
                application.play_spotlight_track(place);
            }
        }));
        player.on_spotlight_album_clicked(self.handler(|application, index: i32| {
            if let Some(place) = list_place(index) {
                application.play_spotlight_album(place);
            }
        }));

        let weak = Rc::downgrade(self);
        self.frame_timer
            .start(TimerMode::Repeated, FRAME_INTERVAL, move || {
                if let Some(application) = weak.upgrade() {
                    application.draw_frame();
                }
            });
    }

    /// A click on a row of the Now Playing queue card.
    pub(super) fn play_queue_row(&self, index: usize) {
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

    pub(super) fn play_spotlight_track(&self, index: usize) {
        let tracks = self.now_playing_lists.borrow().spotlight_tracks.clone();
        let Some(chosen) = tracks.get(index) else {
            return;
        };
        let (tracks, start) = library_view::artist_queue_from(&self.artists.borrow(), chosen);
        self.play_queue(tracks, start);
    }

    pub(super) fn play_spotlight_album(&self, index: usize) {
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

    /// Pushes the playback state to the window. Cheap enough to call after
    /// every action and engine event: setting an unchanged property is free.
    pub(super) fn refresh_player(&self) {
        let player = self.player.borrow();
        let translations = self.translations.borrow();
        let view = self.window.global::<user_interface::Player>();

        view.set_active(player.mode() != Mode::Idle);
        view.set_playing(player.playing());
        view.set_shuffle(player.shuffle());
        view.set_repeat(player.repeat());
        view.set_volume(player.volume() as f32);

        self.show_current(&view, &player, &translations);
        self.refresh_output_readout(&player);
        self.refresh_active_station(&player);
        self.refresh_track_overview(&player);
        // A seek while paused moves the playhead, with no frame to carry it.
        let progress = player.live_progress(Instant::now());
        view.set_progress(progress as f32);
        self.show_overview_playhead(progress);

        // The waveform stops ticking once it has come to rest; anything that
        // sets it moving again starts it back up.
        if !self.frame_timer.running() && !self.visualizer.borrow().at_rest(player.playing()) {
            self.last_frame.set(Instant::now());
            self.frame_timer.restart();
        }
    }

    /// What plays, as the mini player, the art tile and Now Playing show it.
    fn show_current(
        &self,
        view: &user_interface::Player<'_>,
        player: &Player,
        translations: &Translations,
    ) {
        let text = now_playing::now_playing_text(player, translations);
        view.set_title(text.title.into());
        view.set_subtitle(text.subtitle.into());
        view.set_gradient_index(text.gradient_index as i32);
        view.set_local(text.local);
        view.set_radio(text.radio);
        view.set_hero_title(text.hero_title.into());
        view.set_initials(text.initials.into());
        view.set_elapsed(text.elapsed.into());
        view.set_total(text.total.into());
        view.set_eyebrow(text.eyebrow.into());
        view.set_quality(text.quality.into());
        view.set_lossless(text.lossless);
        view.set_artist(text.artist.into());
        view.set_composer(text.composer.into());
        view.set_album(text.album.into());
        view.set_album_line(text.album_line.into());
        view.set_genre(text.genre.into());
        view.set_station_name(text.station_name.into());
        view.set_station_genre(text.station_genre.into());

        let art = self.current_art.borrow();
        let shown = now_playing::cover_shown(player.mode(), art.track_cover_failed);
        let cover = if shown.stream {
            art.stream_cover.clone()
        } else {
            art.track_cover.clone()
        };
        view.set_cover(cover.unwrap_or_default());
        view.set_cover_unavailable(shown.unavailable);
    }

    /// Rebuilds the lists Now Playing shows around the current track: the
    /// queue card's rows and the artist spotlight.
    pub(super) fn refresh_now_playing_lists(&self) {
        let player = self.player.borrow();
        let translations = self.translations.borrow();
        let (queue_rows, album_tracks) = self.refresh_queue_card(&player, &translations);
        let (spotlight_tracks, spotlight_albums) =
            self.refresh_spotlight(player.current(), &translations);
        *self.now_playing_lists.borrow_mut() = NowPlayingLists {
            queue_rows,
            album_tracks,
            spotlight_tracks,
            spotlight_albums,
        };
    }

    /// Fills the queue card for the view it is on, and returns its rows with
    /// the current track's album.
    fn refresh_queue_card(
        &self,
        player: &Player,
        translations: &Translations,
    ) -> (Vec<library_view::QueueRow>, Vec<Track>) {
        let view = self.window.global::<user_interface::Player>();
        let current = player.current();
        let unknown_artist = translations.text("common.unknownArtist");

        let album_tracks = current
            .map(|track| library_view::album_tracks_of(&self.albums.borrow(), track))
            .unwrap_or_default();
        let queue_view = self.queue_view.get();
        let rows = library_view::queue_rows(
            queue_view,
            player.queue(),
            player.current_index(),
            &album_tracks,
            player.shuffle(),
        );
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
                player.shuffle(),
                translations,
            )
            .into(),
        );
        self.queue_rows.set_vec(
            rows.iter()
                .map(|row| user_interface::QueueRow {
                    track_id: interface_id(row.track.id),
                    number: row
                        .display
                        .map(|number| format!("{number:02}"))
                        .unwrap_or_default()
                        .into(),
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
        (rows, album_tracks)
    }

    /// Fills the artist spotlight around `current`, and returns the tracks
    /// and albums it shows.
    fn refresh_spotlight(
        &self,
        current: Option<&Track>,
        translations: &Translations,
    ) -> (Vec<Track>, Vec<AlbumGroup>) {
        let view = self.window.global::<user_interface::Player>();
        let unknown_album = translations.text("common.unknownAlbum");
        let spotlight_tracks = current
            .map(|track| library_view::spotlight_tracks(&self.artists.borrow(), track))
            .unwrap_or_default();
        let spotlight_albums = current
            .map(|track| library_view::spotlight_albums(&self.albums.borrow(), track))
            .unwrap_or_default();

        view.set_spotlight_shown(spotlight_tracks.len() > 1 || !spotlight_albums.is_empty());
        view.set_spotlight_heading(
            translations
                .translate(
                    "now.moreFrom",
                    &[(
                        "artist",
                        current
                            .and_then(|track| track.artist.clone())
                            .unwrap_or_default(),
                    )],
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
                        .count("library.tracks", album.tracks.len())
                        .into(),
                    track_id: album
                        .tracks
                        .first()
                        .map(|track| interface_id(track.id))
                        .unwrap_or_default(),
                    gradient_index: album_gradient_index(album),
                })
                .collect::<Vec<_>>(),
        );
        (spotlight_tracks, spotlight_albums)
    }

    /// The prism colors, read from the design tokens, as red, green and blue.
    pub(super) fn prism_colors(&self) -> Vec<[u8; 3]> {
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
    pub(super) fn draw_frame(&self) {
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
        // While the track's overview stands in for it, the mini trace is not
        // drawn.
        if width > 0.0 && height > 0.0 && !view.get_overview_shown() {
            view.set_mini_wave_commands(visualizer.path_commands(width, height).into());
        }
        let (width, height) = (view.get_big_wave_width(), view.get_big_wave_height());
        if width > 0.0 && height > 0.0 {
            view.set_big_wave_commands(visualizer.path_commands(width, height).into());
        }
        if playing {
            self.animation_time.set(self.animation_time.get() + elapsed);
            view.set_animation_time(
                i64::try_from(self.animation_time.get().as_millis()).unwrap_or_default(),
            );
        }
        let progress = player.live_progress(now) as f32;
        view.set_progress(progress);
        self.show_overview_playhead(f64::from(progress));
        view.set_playhead_level(visualizer.height_at(progress, 1.0));

        if visualizer.at_rest(playing) {
            self.frame_timer.stop();
        }
    }
}
