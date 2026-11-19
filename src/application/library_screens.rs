//! The controller's side of the library screens: what the search keeps, the
//! album or artist browsed in place, and the actions that change the library
//! itself — adding files and folders, rescanning, removing a folder.

use std::path::Path;
use std::sync::Arc;

use slint::{ComponentHandle, Model};

use super::{
    album_gradient_index, gradient_index, interface_id, list_place, post, with_application,
    Application,
};
use crate::audio;
use crate::covers;
use crate::library::{self, LibrarySnapshot, ScanError, ScanReport, Track};
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
    /// Where the open playlist's card sits on the Playlists tab: its section
    /// and its place there.
    pub shown_playlist_selection: Option<(usize, usize)>,
    /// How many times the listener opened, switched or closed a selection.
    selection_changes: i32,
    shown_tracks: Vec<Track>,
    /// The "Recently added" rows, newest first.
    recent_tracks: Vec<Track>,
    /// The Favourites tab's rows, the newest favourite first.
    favourite_tracks: Vec<Track>,
    shown_albums: Vec<AlbumGroup>,
    shown_artists: Vec<ArtistGroup>,
    local_tracks: Vec<Track>,
    /// The Discover shelves, tile by tile.
    recent_albums: Vec<AlbumGroup>,
    rediscover_albums: Vec<AlbumGroup>,
    shelf_artists: Vec<ArtistGroup>,
}

impl Application {
    pub(super) fn connect_library(self: &std::rc::Rc<Self>) {
        let view = self.window.global::<user_interface::Library>();
        view.set_albums(self.library_albums.clone().into());
        view.set_artists(self.library_artists.clone().into());
        view.set_songs(self.library_songs.clone().into());
        view.set_recent(self.library_recent.clone().into());
        view.set_favourites(self.library_favourites.clone().into());
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
            let album = list_place(index).and_then(|place| {
                application
                    .library_screen
                    .borrow()
                    .shown_albums
                    .get(place)
                    .cloned()
            });
            let Some(album) = album else {
                return;
            };
            let selection =
                library_view::album_selection(&album, &application.translations.borrow());
            application.toggle_selection(selection);
        }));
        view.on_artist_clicked(self.handler(|application, index: i32| {
            let artist = list_place(index).and_then(|place| {
                application
                    .library_screen
                    .borrow()
                    .shown_artists
                    .get(place)
                    .cloned()
            });
            let Some(artist) = artist else {
                return;
            };
            application.toggle_selection(library_view::artist_selection(&artist));
        }));
        view.on_song_clicked(self.handler(|application, index: i32| {
            let tracks = application.library_screen.borrow().shown_tracks.clone();
            application.play_list(tracks, index);
        }));
        view.on_recent_clicked(self.handler(|application, index: i32| {
            let tracks = application.library_screen.borrow().recent_tracks.clone();
            application.play_list(tracks, index);
        }));
        view.on_favourite_clicked(self.handler(|application, index: i32| {
            let tracks = application.library_screen.borrow().favourite_tracks.clone();
            application.play_list(tracks, index);
        }));
        view.on_favourite_toggled(self.handler(|application, track_id: i32| {
            application.toggle_favourite(i64::from(track_id));
        }));
        view.on_selection_track_clicked(self.handler(|application, index: i32| {
            let tracks = application
                .library_screen
                .borrow()
                .selection
                .as_ref()
                .map(|selection| selection.tracks.clone());
            if let Some(tracks) = tracks {
                application.play_list(tracks, index);
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
            application.play_list(tracks, index);
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
                let shelf = library_view::Shelf::from_identifier(&shelf);
                if let (Some(application), Some(shelf), Some(place)) =
                    (weak.upgrade(), shelf, list_place(index))
                {
                    application.play_shelf_tile(shelf, place);
                }
            }
        });
    }

    /// A click on a Discover tile plays the album or the artist it shows.
    fn play_shelf_tile(&self, shelf: library_view::Shelf, index: usize) {
        let tracks = {
            let screen = self.library_screen.borrow();
            match shelf {
                library_view::Shelf::RecentAlbums => screen
                    .recent_albums
                    .get(index)
                    .map(|album| album.tracks.clone()),
                library_view::Shelf::Rediscover => screen
                    .rediscover_albums
                    .get(index)
                    .map(|album| album.tracks.clone()),
                library_view::Shelf::Artists => screen
                    .shelf_artists
                    .get(index)
                    .map(|artist| artist.tracks.clone()),
            }
        };
        if let Some(tracks) = tracks {
            self.play_queue(tracks, 0);
        }
    }

    /// Stars a song, or takes its star away, and shows it wherever it is
    /// listed.
    pub(super) fn toggle_favourite(&self, track_id: i64) {
        let track = self
            .all_tracks
            .borrow()
            .iter()
            .find(|track| track.id == track_id)
            .cloned();
        let Some(track) = track else {
            return;
        };
        let effects = self
            .favourite_tracks
            .borrow_mut()
            .toggle(&track, self.media_server.borrow().server());
        self.run(effects);
        self.refresh_favourite(track_id);
    }

    /// Shows a song's star wherever it is listed, and the Favourites tab as
    /// the stars now stand. A star changes nothing else: the other lists keep
    /// their rows.
    pub(super) fn refresh_favourite(&self, track_id: i64) {
        let translations = self.translations.borrow();
        let tracks = self.tracks.borrow();
        let search = self.search.borrow();
        let favourites = self.favourite_tracks.borrow();
        let mut screen = self.library_screen.borrow_mut();
        let view = self.window.global::<user_interface::Library>();

        screen.favourite_tracks =
            library_view::favourite_songs(&tracks, favourites.track_ids(), &search);
        self.library_favourites.set_vec(track_rows(
            &screen.favourite_tracks,
            None,
            |id| favourites.contains(id),
            &translations,
        ));
        view.set_favourites_empty(!library_view::any_favourite_shown(
            &tracks,
            favourites.track_ids(),
        ));

        let favourite = favourites.contains(track_id);
        mark_favourite(
            &self.library_songs,
            &screen.shown_tracks,
            track_id,
            favourite,
        );
        mark_favourite(
            &self.library_recent,
            &screen.recent_tracks,
            track_id,
            favourite,
        );
        if let Some(selection) = &screen.selection {
            mark_favourite(
                &self.library_selection_tracks,
                &selection.tracks,
                track_id,
                favourite,
            );
        }
        // The panel folding away keeps no list of its own; it is a few rows.
        let identifier = interface_id(track_id);
        for place in 0..self.library_folding_tracks.row_count() {
            if let Some(mut row) = self
                .library_folding_tracks
                .row_data(place)
                .filter(|row| row.track_id == identifier)
            {
                row.favourite = favourite;
                self.library_folding_tracks.set_row_data(place, row);
            }
        }
    }

    pub(super) fn toggle_selection(&self, selection: Selection) {
        self.change_selection(|current| library_view::toggle_selection(current, selection));
    }

    /// Changes the selection as the listener asked and hands the grid the
    /// panel it puts away, tracks included, so that panel folds while the
    /// next one opens.
    pub(super) fn change_selection(&self, change: impl FnOnce(&mut Option<Selection>)) {
        let (folding, folding_index, folding_playlist, changes) = {
            let mut screen = self.library_screen.borrow_mut();
            let folding = screen.selection.clone();
            let folding_index = screen.shown_selection;
            let folding_playlist = screen.shown_playlist_selection;
            change(&mut screen.selection);
            screen.selection_changes = screen.selection_changes.wrapping_add(1);
            (
                folding,
                folding_index,
                folding_playlist,
                screen.selection_changes,
            )
        };
        // A playlist's panel folds under its card, in its group's grid.
        self.playlist_screen.borrow_mut().folding = folding.as_ref().and(folding_playlist);
        self.refresh_library();

        let view = self.window.global::<user_interface::Library>();
        // Only a panel that sat under a card folds; another closes at once.
        let folds_in_a_grid = folding_index.is_some();
        let folds = folding_index.or(folding_playlist.map(|_| 0));
        match folding.zip(folds) {
            Some((folding, index)) => {
                view.set_folding_index(if folds_in_a_grid { index as i32 } else { -1 });
                view.set_folding_heading(folding.label.as_str().into());
                self.library_folding_tracks.set_vec(track_rows(
                    &folding.tracks,
                    Some(folding.kind),
                    |id| self.favourite_tracks.borrow().contains(id),
                    &self.translations.borrow(),
                ));
            }
            None => {
                view.set_folding_index(-1);
                self.library_folding_tracks.set_vec(Vec::new());
            }
        }
        view.set_selection_changes(changes);
    }

    /// Rebuilds what the Library screen shows from the library, the search
    /// and the selection: its lists, then its playlists.
    pub(super) fn refresh_library(&self) {
        self.renew_playlist_selection();
        self.refresh_library_lists();
        self.refresh_playlist_sections();
    }

    fn refresh_library_lists(&self) {
        let translations = self.translations.borrow();
        let tracks = self.tracks.borrow();
        let albums = self.albums.borrow();
        let artists = self.artists.borrow();
        let order = self.library_order.borrow();
        let search = self.search.borrow();
        let favourites = self.favourite_tracks.borrow();
        let mut screen = self.library_screen.borrow_mut();
        let view = self.window.global::<user_interface::Library>();

        screen.shown_tracks = library_view::library_songs(&tracks, &order, &search);
        screen.recent_tracks = library_view::recently_added(&tracks, &search);
        screen.favourite_tracks =
            library_view::favourite_songs(&tracks, favourites.track_ids(), &search);
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
        let is_favourite = |id| favourites.contains(id);
        self.library_songs.set_vec(track_rows(
            &screen.shown_tracks,
            None,
            is_favourite,
            &translations,
        ));
        self.library_recent.set_vec(track_rows(
            &screen.recent_tracks,
            None,
            is_favourite,
            &translations,
        ));
        self.library_favourites.set_vec(track_rows(
            &screen.favourite_tracks,
            None,
            is_favourite,
            &translations,
        ));
        // The tab invites a first star only while none shows at all.
        view.set_favourites_empty(!library_view::any_favourite_shown(
            &tracks,
            favourites.track_ids(),
        ));

        screen.shown_selection = Self::show_selection(
            &screen,
            &view,
            &self.library_selection_tracks,
            is_favourite,
            &translations,
        );
    }

    /// Pushes the album or artist browsed in place, and where its tile sits
    /// in the grid of the open tab, which it returns.
    fn show_selection(
        screen: &LibraryScreen,
        view: &user_interface::Library<'_>,
        rows: &slint::VecModel<LibraryTrack>,
        is_favourite: impl Fn(i64) -> bool,
        translations: &Translations,
    ) -> Option<usize> {
        let selection_index = library_view::selection_place(
            screen.tab,
            screen.selection.as_ref(),
            &screen.shown_albums,
            &screen.shown_artists,
        );
        view.set_selection_index(selection_index.map_or(-1, |index| index as i32));
        view.set_selection_present(screen.selection.is_some());
        match &screen.selection {
            Some(selection) => {
                view.set_selection_heading(selection.label.as_str().into());
                rows.set_vec(track_rows(
                    &selection.tracks,
                    Some(selection.kind),
                    is_favourite,
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
        // The Local Files table shows no star.
        self.library_local.set_vec(track_rows(
            &screen.local_tracks,
            None,
            |_| false,
            &translations,
        ));
        view.set_scanned_heading(
            translations
                .count("local.scanned", screen.local_tracks.len())
                .into(),
        );
        view.set_scan_report(
            self.last_report
                .get()
                .map(|report| library_view::scan_report_line(&report, &translations))
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
                        .count(
                            "library.tracks",
                            usize::try_from(folder.track_count).unwrap_or_default(),
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
        self.scan(|database| Ok(library::rescan_library(database)?));
    }

    pub(super) fn remove_folder(&self, folder_id: i64) {
        self.scan(move |database| {
            library::remove_watched_folder(database, folder_id)?;
            Ok(ScanReport::default())
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
    fn scan(
        &self,
        change: impl FnOnce(&DbState) -> Result<ScanReport, ScanError> + Send + 'static,
    ) {
        let database = Arc::clone(&self.database);
        self.library_worker.run(move || {
            let outcome = change(&database)
                .and_then(|report| Ok((report, LibrarySnapshot::read(&database)?)));
            post(move |application| application.scan_finished(outcome));
        });
    }

    fn scan_finished(&self, outcome: Result<(ScanReport, LibrarySnapshot), ScanError>) {
        match outcome {
            Ok((report, snapshot)) => {
                self.last_report.set(Some(report));
                self.take_library(snapshot);
            }
            Err(ScanError::InsideWatched(parent)) => {
                self.notify_error("error.folderInsideWatched", vec![("folder", parent)], None);
            }
            Err(ScanError::Failed(error)) => {
                log::error!("library scan: {error}");
                self.notify_error("error.library", vec![("message", error)], None);
            }
        }
        self.refresh_everything();
        // What the scan brought in without a gain is measured next.
        self.measure_loudness();
    }

    /// Replaces the library the screens are built from — unless something
    /// changed it after the snapshot was read: a star, a playlist edit, the
    /// answer to a push, another job's writes. Taken, that snapshot would
    /// undo them on screen, so the library is read again instead, and the
    /// screens keep what they show until then.
    pub(super) fn take_library(&self, snapshot: LibrarySnapshot) {
        if !snapshot.is_current(&self.database) {
            self.read_library_again();
            return;
        }
        self.media_server
            .borrow_mut()
            .set_libraries(snapshot.libraries);
        *self.cover_keys.borrow_mut() = covers::CoverKeys::new(&snapshot.tracks);
        *self.all_tracks.borrow_mut() = snapshot.tracks;
        *self.folders.borrow_mut() = snapshot.folders;
        self.favourite_tracks
            .borrow_mut()
            .retain_youtube_tracks(&self.all_tracks.borrow());
        self.favourite_tracks
            .borrow_mut()
            .restore(snapshot.favourite_track_ids);
        self.playlists
            .borrow_mut()
            .restore(snapshot.playlists, snapshot.playlist_groups);
        self.group_library();
        // A rescan may have changed any file's art.
        self.thumbnails.borrow_mut().clear();
        self.cover_requests.borrow_mut().clear();
        self.bump_cover_revision();
    }

    /// Reads the library on the library thread, then takes it.
    fn read_library_again(&self) {
        let database = Arc::clone(&self.database);
        self.library_worker.run(move || {
            let outcome = LibrarySnapshot::read(&database);
            post(move |application| match outcome {
                Ok(snapshot) => {
                    application.take_library(snapshot);
                    application.refresh_everything();
                }
                Err(error) => {
                    log::error!("read the library: {error}");
                    application.notify_error("error.library", vec![("message", error)], None);
                }
            });
        });
    }

    /// Builds the tracks, albums and artists the screens show from the whole
    /// library, less the songs of the server libraries the listener hid.
    pub(super) fn group_library(&self) {
        let tracks = library_view::shown_tracks(
            &self.all_tracks.borrow(),
            &self.media_server.borrow().hidden_libraries(),
        );
        let albums = library_view::albums(&tracks);
        let artists = library_view::artists(&tracks);
        *self.library_order.borrow_mut() =
            library_view::LibraryOrder::new(&tracks, &albums, &artists);
        *self.albums.borrow_mut() = albums;
        *self.artists.borrow_mut() = artists;
        *self.tracks.borrow_mut() = tracks;
        {
            let mut screen = self.library_screen.borrow_mut();
            screen.selection = screen.selection.take().and_then(|selection| {
                library_view::renewed_selection(
                    selection,
                    &self.albums.borrow(),
                    &self.artists.borrow(),
                    self.playlists.borrow().playlists(),
                    &self.tracks.borrow(),
                )
            });
        }
    }
}

/// Waits for a system dialog on the interface thread's event loop.
pub(super) fn await_dialog(dialog: impl std::future::Future<Output = ()> + 'static) {
    if let Err(error) = slint::spawn_local(dialog) {
        log::error!("open a system dialog: {error}");
    }
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
        playlist_id: 0,
    }
}

fn artist_card(artist: &ArtistGroup, translations: &Translations) -> LibraryCard {
    LibraryCard {
        key: artist.artist.as_str().into(),
        title: artist.artist.as_str().into(),
        subtitle: translations
            .count("library.tracks", artist.tracks.len())
            .into(),
        detail: Default::default(),
        track_id: artist
            .tracks
            .first()
            .map(|track| interface_id(track.id))
            .unwrap_or_default(),
        gradient_index: text::art_gradient_index(&artist.artist) as i32,
        playlist_id: 0,
    }
}

/// Sets the star of the rows of `track_id` in a list whose rows are
/// `tracks`, one for one.
fn mark_favourite(
    rows: &slint::VecModel<LibraryTrack>,
    tracks: &[Track],
    track_id: i64,
    favourite: bool,
) {
    for (place, _) in tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| track.id == track_id)
    {
        if let Some(mut row) = rows.row_data(place) {
            row.favourite = favourite;
            rows.set_row_data(place, row);
        }
    }
}

/// Rows for a track list. An album's rows are numbered and titled by their
/// place on the record, an artist's are titled album first; `None` is a
/// plain list.
pub(super) fn track_rows(
    tracks: &[Track],
    browsed: Option<SelectionKind>,
    is_favourite: impl Fn(i64) -> bool,
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
                Some(SelectionKind::Album) => library_view::album_row_title(track, index).into(),
                Some(SelectionKind::Artist) => library_view::artist_row_title(track).into(),
                Some(SelectionKind::Playlist) | None => track.title.as_str().into(),
            },
            artist: track.artist.as_deref().unwrap_or(&unknown_artist).into(),
            composer: track.composer.as_deref().unwrap_or_default().into(),
            format: track.format.as_str().into(),
            duration: text::format_time(track.duration_secs).into(),
            gradient_index: gradient_index(track),
            favourite: is_favourite(track.id),
        })
        .collect()
}
