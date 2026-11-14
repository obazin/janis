//! The controller's side of the library screens: what the search keeps, the
//! album or artist browsed in place, and the actions that change the library
//! itself — adding files and folders, rescanning, removing a folder.

use std::path::Path;
use std::sync::Arc;

use slint::ComponentHandle;

use super::{
    album_gradient_index, gradient_index, interface_id, post, with_application, Application,
};
use crate::audio;
use crate::covers;
use crate::library::{self, ScanReport, Track, WatchedFolder};
use crate::library_view::{
    self, AlbumGroup, ArtistGroup, LibraryTab, Selection, SelectionKind, SHELF_TILE_COUNT,
};
use crate::persistence::DbState;
use crate::text;
use crate::translations::Translations;
use crate::user_interface::{self, LibraryCard, LibraryTrack};

/// What the Library screen shows, kept so a click on a card or a row finds
/// what it stands for.
#[derive(Default)]
pub(super) struct LibraryScreen {
    pub tab: LibraryTab,
    pub selection: Option<Selection>,
    /// Where the selection's card sits among the cards of the open tab, when
    /// it shows there.
    shown_selection: Option<usize>,
    /// How many times the listener opened, switched or closed a selection.
    selection_changes: i32,
    shown_tracks: Vec<Track>,
    /// The "Recently added" rows, newest first.
    recent_tracks: Vec<Track>,
    shown_albums: Vec<AlbumGroup>,
    shown_artists: Vec<ArtistGroup>,
    local_tracks: Vec<Track>,
    /// The Discover shelves, tile by tile.
    recent_albums: Vec<AlbumGroup>,
    rediscover_albums: Vec<AlbumGroup>,
    shelf_artists: Vec<ArtistGroup>,
}

/// A fresh read of the library, taken off the interface thread.
pub(super) struct LibrarySnapshot {
    tracks: Vec<Track>,
    folders: Vec<WatchedFolder>,
}

impl LibrarySnapshot {
    pub fn read(database: &DbState) -> Result<Self, String> {
        Ok(Self {
            tracks: library::list_tracks(database)?,
            folders: library::list_watched_folders(database)?,
        })
    }
}

impl Application {
    pub(super) fn connect_library(self: &std::rc::Rc<Self>) {
        let view = self.window.global::<user_interface::Library>();
        view.set_albums(self.library_albums.clone().into());
        view.set_artists(self.library_artists.clone().into());
        view.set_songs(self.library_songs.clone().into());
        view.set_recent(self.library_recent.clone().into());
        view.set_selection_tracks(self.library_selection_tracks.clone().into());
        view.set_folding_tracks(self.library_folding_tracks.clone().into());

        view.on_tab_selected(
            self.handler(|application, identifier: slint::SharedString| {
                if let Some(tab) = LibraryTab::from_identifier(&identifier) {
                    {
                        let mut screen = application.library_screen.borrow_mut();
                        screen.tab = tab;
                        screen.selection = None;
                    }
                    application.refresh_library();
                }
            }),
        );
        view.on_album_clicked(self.handler(|application, index: i32| {
            let album = application
                .library_screen
                .borrow()
                .shown_albums
                .get(row(index))
                .cloned();
            let Some(album) = album else {
                return;
            };
            let label = album.album.clone().unwrap_or_else(|| {
                application
                    .translations
                    .borrow()
                    .text("common.unknownAlbum")
            });
            application.toggle_selection(Selection {
                kind: SelectionKind::Album,
                key: album.key,
                label,
                tracks: album.tracks,
            });
        }));
        view.on_artist_clicked(self.handler(|application, index: i32| {
            let artist = application
                .library_screen
                .borrow()
                .shown_artists
                .get(row(index))
                .cloned();
            let Some(artist) = artist else {
                return;
            };
            application.toggle_selection(Selection {
                kind: SelectionKind::Artist,
                key: artist.artist.clone(),
                tracks: library_view::artist_tracks(&artist),
                label: artist.artist,
            });
        }));
        view.on_song_clicked(self.handler(|application, index: i32| {
            let tracks = application.library_screen.borrow().shown_tracks.clone();
            application.play_queue(tracks, row(index));
        }));
        view.on_recent_clicked(self.handler(|application, index: i32| {
            let tracks = application.library_screen.borrow().recent_tracks.clone();
            application.play_queue(tracks, row(index));
        }));
        view.on_selection_track_clicked(self.handler(|application, index: i32| {
            let tracks = application
                .library_screen
                .borrow()
                .selection
                .as_ref()
                .map(|selection| selection.tracks.clone());
            if let Some(tracks) = tracks {
                application.play_queue(tracks, row(index));
            }
        }));
        view.on_selection_closed(self.action(|application| {
            application.change_selection(|selection| *selection = None);
        }));
        view.on_add_music(self.action(Application::add_files));
        view.on_add_folder(self.action(Application::add_folder));
        self.connect_local_files();
        self.connect_discover();
    }

    fn connect_local_files(self: &std::rc::Rc<Self>) {
        let view = self.window.global::<user_interface::Library>();
        view.set_local_tracks(self.library_local.clone().into());
        view.set_folders(self.library_folders.clone().into());
        view.on_local_track_clicked(self.handler(|application, index: i32| {
            let tracks = application.library_screen.borrow().local_tracks.clone();
            application.play_queue(tracks, row(index));
        }));
        view.on_rescan(self.action(Application::rescan));
        view.on_folder_removed(self.handler(|application, id: i32| {
            application.remove_folder(i64::from(id));
        }));
    }

    fn connect_discover(self: &std::rc::Rc<Self>) {
        let view = self.window.global::<user_interface::Library>();
        view.set_discover_recent(self.discover_recent.clone().into());
        view.set_discover_rediscover(self.discover_rediscover.clone().into());
        view.set_discover_artists(self.discover_artists.clone().into());
        view.on_shelf_tile_clicked({
            let weak = std::rc::Rc::downgrade(self);
            move |shelf, index| {
                if let Some(application) = weak.upgrade() {
                    application.play_shelf_tile(&shelf, row(index));
                }
            }
        });
    }

    /// A click on a Discover tile plays the album or the artist it shows.
    fn play_shelf_tile(&self, shelf: &str, index: usize) {
        let tracks = {
            let screen = self.library_screen.borrow();
            match shelf {
                "recent" => screen
                    .recent_albums
                    .get(index)
                    .map(|album| album.tracks.clone()),
                "rediscover" => screen
                    .rediscover_albums
                    .get(index)
                    .map(|album| album.tracks.clone()),
                "artists" => screen
                    .shelf_artists
                    .get(index)
                    .map(|artist| artist.tracks.clone()),
                _ => None,
            }
        };
        if let Some(tracks) = tracks {
            self.play_queue(tracks, 0);
        }
    }

    fn toggle_selection(&self, selection: Selection) {
        self.change_selection(|current| library_view::toggle_selection(current, selection));
    }

    /// Changes the selection as the listener asked and hands the grid the
    /// panel it puts away, tracks included, so that panel folds while the
    /// next one opens.
    fn change_selection(&self, change: impl FnOnce(&mut Option<Selection>)) {
        let (folding, folding_index, changes) = {
            let mut screen = self.library_screen.borrow_mut();
            let folding = screen.selection.clone();
            let folding_index = screen.shown_selection;
            change(&mut screen.selection);
            screen.selection_changes = screen.selection_changes.wrapping_add(1);
            (folding, folding_index, screen.selection_changes)
        };
        self.refresh_library();

        let view = self.window.global::<user_interface::Library>();
        match (folding, folding_index) {
            (Some(folding), Some(index)) => {
                view.set_folding_index(index as i32);
                view.set_folding_heading(folding.label.as_str().into());
                self.library_folding_tracks.set_vec(track_rows(
                    &folding.tracks,
                    Some(folding.kind),
                    &self.translations.borrow(),
                ));
            }
            _ => {
                view.set_folding_index(-1);
                self.library_folding_tracks.set_vec(Vec::new());
            }
        }
        view.set_selection_changes(changes);
    }

    /// Rebuilds what the Library screen shows from the library, the search
    /// and the selection.
    pub(super) fn refresh_library(&self) {
        let translations = self.translations.borrow();
        let tracks = self.tracks.borrow();
        let albums = self.albums.borrow();
        let artists = self.artists.borrow();
        let order = self.library_order.borrow();
        let search = self.search.borrow();
        let mut screen = self.library_screen.borrow_mut();
        let view = self.window.global::<user_interface::Library>();

        screen.shown_tracks = library_view::library_songs(&tracks, &order, &search);
        screen.recent_tracks = library_view::recently_added(&tracks, &search);
        screen.shown_albums = library_view::albums_matching(&albums, &order, &screen.shown_tracks)
            .into_iter()
            .cloned()
            .collect();
        screen.shown_artists =
            library_view::artists_matching(&artists, &order, &screen.shown_tracks)
                .into_iter()
                .cloned()
                .collect();

        view.set_empty(tracks.is_empty());
        view.set_tab(screen.tab.identifier().into());
        self.library_albums.set_vec(
            screen
                .shown_albums
                .iter()
                .map(|album| album_card(album, &translations))
                .collect::<Vec<_>>(),
        );
        self.library_artists.set_vec(
            screen
                .shown_artists
                .iter()
                .map(|artist| artist_card(artist, &translations))
                .collect::<Vec<_>>(),
        );
        self.library_songs
            .set_vec(track_rows(&screen.shown_tracks, None, &translations));
        self.library_recent
            .set_vec(track_rows(&screen.recent_tracks, None, &translations));

        screen.shown_selection = Self::show_selection(
            &screen,
            &view,
            &self.library_selection_tracks,
            &translations,
        );
    }

    /// Pushes the album or artist browsed in place, and where its tile sits
    /// in the grid of the open tab, which it returns.
    fn show_selection(
        screen: &LibraryScreen,
        view: &user_interface::Library<'_>,
        rows: &slint::VecModel<LibraryTrack>,
        translations: &Translations,
    ) -> Option<usize> {
        let selection_index =
            screen
                .selection
                .as_ref()
                .and_then(|selection| match (screen.tab, selection.kind) {
                    (LibraryTab::Albums, SelectionKind::Album) => screen
                        .shown_albums
                        .iter()
                        .position(|album| album.key == selection.key),
                    (LibraryTab::Artists, SelectionKind::Artist) => screen
                        .shown_artists
                        .iter()
                        .position(|artist| artist.artist == selection.key),
                    _ => None,
                });
        view.set_selection_index(selection_index.map_or(-1, |index| index as i32));
        view.set_selection_present(screen.selection.is_some());
        match &screen.selection {
            Some(selection) => {
                view.set_selection_heading(selection.label.as_str().into());
                rows.set_vec(track_rows(
                    &selection.tracks,
                    Some(selection.kind),
                    translations,
                ));
            }
            None => {
                view.set_selection_heading(Default::default());
                rows.set_vec(Vec::new());
            }
        }
        selection_index
    }

    /// Rebuilds the Discover shelves. They are built from the whole library,
    /// not from what the search kept.
    pub(super) fn refresh_discover(&self) {
        let translations = self.translations.borrow();
        let albums = self.albums.borrow();
        let artists = self.artists.borrow();
        let mut screen = self.library_screen.borrow_mut();

        screen.recent_albums = albums.iter().take(SHELF_TILE_COUNT).cloned().collect();
        screen.rediscover_albums = library_view::rediscover(&albums)
            .into_iter()
            .cloned()
            .collect();
        screen.shelf_artists = artists.iter().take(SHELF_TILE_COUNT).cloned().collect();

        self.discover_recent.set_vec(
            screen
                .recent_albums
                .iter()
                .map(|album| album_card(album, &translations))
                .collect::<Vec<_>>(),
        );
        self.discover_rediscover.set_vec(
            screen
                .rediscover_albums
                .iter()
                .map(|album| album_card(album, &translations))
                .collect::<Vec<_>>(),
        );
        self.discover_artists.set_vec(
            screen
                .shelf_artists
                .iter()
                .map(|artist| artist_card(artist, &translations))
                .collect::<Vec<_>>(),
        );
    }

    /// Pushes the Local Files screen's state to the window.
    pub(super) fn refresh_local_files(&self) {
        let translations = self.translations.borrow();
        let tracks = self.tracks.borrow();
        let search = self.search.borrow();
        let mut screen = self.library_screen.borrow_mut();
        let view = self.window.global::<user_interface::Library>();

        screen.local_tracks = library_view::search_local_files(&tracks, &search);
        self.library_local
            .set_vec(track_rows(&screen.local_tracks, None, &translations));
        view.set_scanned_heading(
            translations
                .translate(
                    "local.scanned",
                    &[("count", screen.local_tracks.len().to_string())],
                )
                .into(),
        );
        view.set_scan_report(
            self.last_report
                .get()
                .map(|report| {
                    translations.translate(
                        "local.scanReport",
                        &[
                            ("added", report.added.to_string()),
                            ("updated", report.updated.to_string()),
                            ("skipped", report.skipped.to_string()),
                        ],
                    )
                })
                .unwrap_or_default()
                .into(),
        );
        self.library_folders.set_vec(
            self.folders
                .borrow()
                .iter()
                .map(|folder| user_interface::WatchedFolderRow {
                    id: interface_id(folder.id),
                    path: folder.path.as_str().into(),
                    track_count: translations
                        .translate(
                            "library.tracks",
                            &[("count", folder.track_count.to_string())],
                        )
                        .into(),
                })
                .collect::<Vec<_>>(),
        );
    }

    // ── Changing the library ────────────────────────────────────────────────

    /// File picker → ad-hoc import. The dialog is awaited on the interface
    /// thread's own loop, so the window goes on drawing behind it.
    pub(super) fn add_files(&self) {
        let filter_name = self.translations.borrow().text("local.audioFiles");
        let dialog = rfd::AsyncFileDialog::new()
            .add_filter(filter_name, audio::AUDIO_EXTENSIONS)
            .pick_files();
        await_dialog(async move {
            if let Some(files) = dialog.await {
                let paths = files
                    .iter()
                    .map(|file| file.path().to_string_lossy().into_owned())
                    .collect();
                with_application(|application| application.import_paths(paths));
            }
        });
    }

    /// Folder picker → register and scan.
    pub(super) fn add_folder(&self) {
        let dialog = rfd::AsyncFileDialog::new().pick_folder();
        await_dialog(async move {
            if let Some(folder) = dialog.await {
                let folder = folder.path().to_string_lossy().into_owned();
                with_application(|application| {
                    application
                        .scan(move |database| library::add_watched_folder(database, &folder));
                });
            }
        });
    }

    pub(super) fn rescan(&self) {
        self.scan(library::rescan_library);
    }

    pub(super) fn remove_folder(&self, folder_id: i64) {
        self.scan(move |database| {
            library::remove_watched_folder(database, folder_id).map(|()| ScanReport::default())
        });
    }

    /// Imports the audio files among `paths` — from the file picker, or
    /// dropped on the window. Anything that is not an audio file is left out.
    pub fn import_paths(&self, paths: Vec<String>) {
        let files: Vec<String> = paths
            .into_iter()
            .filter(|path| audio::audio_extension(Path::new(path)).is_some())
            .collect();
        if !files.is_empty() {
            self.scan(move |database| Ok(library::import_files(database, files)));
        }
    }

    /// Runs a change to the library off the interface thread, then takes the
    /// library as the database now has it.
    fn scan(&self, change: impl FnOnce(&DbState) -> Result<ScanReport, String> + Send + 'static) {
        let database = Arc::clone(&self.database);
        self.library_worker.run(move || {
            let outcome = change(&database)
                .and_then(|report| Ok((report, LibrarySnapshot::read(&database)?)));
            post(move |application| application.scan_finished(outcome));
        });
    }

    fn scan_finished(&self, outcome: Result<(ScanReport, LibrarySnapshot), String>) {
        match outcome {
            Ok((report, snapshot)) => {
                self.last_report.set(Some(report));
                self.take_library(snapshot);
            }
            Err(error) => {
                log::error!("library scan: {error}");
                self.notify_error("error.library", vec![("message", error)], None);
            }
        }
        self.refresh_everything();
        // What the scan brought in without a gain is measured next.
        self.measure_loudness();
    }

    /// Replaces the library the screens are built from.
    pub(super) fn take_library(&self, snapshot: LibrarySnapshot) {
        let albums = library_view::albums(&snapshot.tracks);
        let artists = library_view::artists(&snapshot.tracks);
        *self.library_order.borrow_mut() =
            library_view::LibraryOrder::new(&snapshot.tracks, &albums, &artists);
        *self.albums.borrow_mut() = albums;
        *self.artists.borrow_mut() = artists;
        *self.cover_keys.borrow_mut() = covers::CoverKeys::new(&snapshot.tracks);
        *self.tracks.borrow_mut() = snapshot.tracks;
        *self.folders.borrow_mut() = snapshot.folders;
        {
            let mut screen = self.library_screen.borrow_mut();
            screen.selection = screen.selection.take().and_then(|selection| {
                library_view::renewed_selection(
                    selection,
                    &self.albums.borrow(),
                    &self.artists.borrow(),
                )
            });
        }
        // A rescan may have changed any file's art.
        self.thumbnail_epoch.set(self.thumbnail_epoch.get() + 1);
        self.thumbnails.borrow_mut().clear();
        self.cover_requests.borrow_mut().clear();
        self.bump_cover_revision();
    }
}

/// Waits for a system dialog on the interface thread's event loop.
fn await_dialog(dialog: impl std::future::Future<Output = ()> + 'static) {
    if let Err(error) = slint::spawn_local(dialog) {
        log::error!("open a system dialog: {error}");
    }
}

fn row(index: i32) -> usize {
    usize::try_from(index).unwrap_or(usize::MAX)
}

fn album_card(album: &AlbumGroup, translations: &Translations) -> LibraryCard {
    LibraryCard {
        key: album.key.as_str().into(),
        title: album
            .album
            .clone()
            .unwrap_or_else(|| translations.text("common.unknownAlbum"))
            .into(),
        subtitle: album
            .artist
            .clone()
            .unwrap_or_else(|| translations.text("common.unknownArtist"))
            .into(),
        detail: library_view::album_detail(album, translations).into(),
        track_id: album
            .tracks
            .first()
            .map(|track| interface_id(track.id))
            .unwrap_or_default(),
        gradient_index: album_gradient_index(album),
    }
}

fn artist_card(artist: &ArtistGroup, translations: &Translations) -> LibraryCard {
    LibraryCard {
        key: artist.artist.as_str().into(),
        title: artist.artist.as_str().into(),
        subtitle: translations
            .translate(
                "library.tracks",
                &[("count", artist.tracks.len().to_string())],
            )
            .into(),
        detail: Default::default(),
        track_id: artist
            .tracks
            .first()
            .map(|track| interface_id(track.id))
            .unwrap_or_default(),
        gradient_index: text::art_gradient_index(&artist.artist) as i32,
    }
}

/// Rows for a track list. An album's rows are numbered by their place on
/// the record, an artist's are titled album first; `None` is a plain list.
pub(super) fn track_rows(
    tracks: &[Track],
    browsed: Option<SelectionKind>,
    translations: &Translations,
) -> Vec<LibraryTrack> {
    let use_track_number = browsed == Some(SelectionKind::Album);
    let unknown_artist = translations.text("common.unknownArtist");
    tracks
        .iter()
        .enumerate()
        .map(|(index, track)| LibraryTrack {
            track_id: interface_id(track.id),
            number: format!(
                "{:02}",
                library_view::row_number(track, index, use_track_number)
            )
            .into(),
            title: match browsed {
                Some(SelectionKind::Artist) => library_view::artist_row_title(track).into(),
                _ => track.title.as_str().into(),
            },
            artist: track.artist.as_deref().unwrap_or(&unknown_artist).into(),
            composer: track.composer.as_deref().unwrap_or_default().into(),
            format: track.format.as_str().into(),
            duration: text::format_time(track.duration_secs).into(),
            gradient_index: gradient_index(track),
        })
        .collect()
}
