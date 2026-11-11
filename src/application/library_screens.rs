//! The controller's side of the library screens: what the search keeps, the
//! album or artist browsed in place, and the actions that change the library
//! itself — adding files and folders, rescanning, removing a folder.

use std::path::Path;
use std::sync::Arc;

use slint::ComponentHandle;

use super::{gradient_index, interface_id, post, Application};
use crate::audio;
use crate::library::{self, ScanReport, Track, WatchedFolder};
use crate::library_view::{
    self, AlbumGroup, ArtistGroup, LibraryTab, Selection, SelectionKind, RECENT_TRACK_COUNT,
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
    shown_tracks: Vec<Track>,
    shown_albums: Vec<AlbumGroup>,
    shown_artists: Vec<ArtistGroup>,
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

        view.on_tab_selected(
            self.handler(|application, identifier: slint::SharedString| {
                if let Some(tab) = LibraryTab::from_identifier(&identifier) {
                    let mut screen = application.library_screen.borrow_mut();
                    screen.tab = tab;
                    screen.selection = None;
                }
            }),
        );
        view.on_album_clicked(self.handler(|application, index: i32| {
            let mut screen = application.library_screen.borrow_mut();
            let Some(album) = screen.shown_albums.get(row(index)).cloned() else {
                return;
            };
            let label = album.album.clone().unwrap_or_else(|| {
                application
                    .translations
                    .borrow()
                    .text("common.unknownAlbum")
            });
            library_view::toggle_selection(
                &mut screen.selection,
                Selection {
                    kind: SelectionKind::Album,
                    key: album.key,
                    label,
                    tracks: album.tracks,
                },
            );
        }));
        view.on_artist_clicked(self.handler(|application, index: i32| {
            let mut screen = application.library_screen.borrow_mut();
            let Some(artist) = screen.shown_artists.get(row(index)).cloned() else {
                return;
            };
            library_view::toggle_selection(
                &mut screen.selection,
                Selection {
                    kind: SelectionKind::Artist,
                    key: artist.artist.clone(),
                    label: artist.artist,
                    tracks: artist.tracks,
                },
            );
        }));
        view.on_song_clicked(self.handler(|application, index: i32| {
            let tracks = application.library_screen.borrow().shown_tracks.clone();
            application.play_queue(tracks, row(index));
        }));
        view.on_recent_clicked(self.handler(|application, index: i32| {
            let tracks: Vec<Track> = application
                .library_screen
                .borrow()
                .shown_tracks
                .iter()
                .take(RECENT_TRACK_COUNT)
                .cloned()
                .collect();
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
            application.library_screen.borrow_mut().selection = None;
        }));
        view.on_add_music(self.action(Application::add_files));
        view.on_add_folder(self.action(Application::add_folder));
    }

    /// Rebuilds what the Library screen shows from the library, the search
    /// and the selection.
    pub(super) fn refresh_library(&self) {
        let translations = self.translations.borrow();
        let tracks = self.tracks.borrow();
        let albums = self.albums.borrow();
        let artists = self.artists.borrow();
        let search = self.search.borrow();
        let mut screen = self.library_screen.borrow_mut();
        let view = self.window.global::<user_interface::Library>();

        screen.shown_tracks = library_view::search_library(&tracks, &search);
        screen.shown_albums = library_view::albums_matching(&albums, &screen.shown_tracks)
            .into_iter()
            .cloned()
            .collect();
        screen.shown_artists = library_view::artists_matching(&artists, &screen.shown_tracks)
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
            .set_vec(track_rows(&screen.shown_tracks, false, &translations));
        self.library_recent.set_vec(track_rows(
            &screen.shown_tracks[..screen.shown_tracks.len().min(RECENT_TRACK_COUNT)],
            false,
            &translations,
        ));

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
                self.library_selection_tracks.set_vec(track_rows(
                    &selection.tracks,
                    selection.kind == SelectionKind::Album,
                    &translations,
                ));
            }
            None => {
                view.set_selection_heading(Default::default());
                self.library_selection_tracks.set_vec(Vec::new());
            }
        }
    }

    // ── Changing the library ────────────────────────────────────────────────

    /// File picker → ad-hoc import.
    pub(super) fn add_files(&self) {
        let picked = rfd::FileDialog::new()
            .add_filter("Audio", audio::AUDIO_EXTENSIONS)
            .pick_files();
        if let Some(files) = picked {
            self.import_paths(
                files
                    .iter()
                    .map(|file| file.to_string_lossy().into_owned())
                    .collect(),
            );
        }
    }

    /// Folder picker → register and scan.
    pub(super) fn add_folder(&self) {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            let folder = folder.to_string_lossy().into_owned();
            self.scan(move |database| library::add_watched_folder(database, &folder));
        }
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
        self.scanning.set(true);
        self.refresh_local_files();
        let database = Arc::clone(&self.database);
        self.library_worker.run(move || {
            let outcome = change(&database)
                .and_then(|report| Ok((report, LibrarySnapshot::read(&database)?)));
            post(move |application| application.scan_finished(outcome));
        });
    }

    fn scan_finished(&self, outcome: Result<(ScanReport, LibrarySnapshot), String>) {
        self.scanning.set(false);
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
        self.refresh_after_action();
    }

    /// Replaces the library the screens are built from.
    pub(super) fn take_library(&self, snapshot: LibrarySnapshot) {
        *self.albums.borrow_mut() = library_view::albums(&snapshot.tracks);
        *self.artists.borrow_mut() = library_view::artists(&snapshot.tracks);
        *self.tracks.borrow_mut() = snapshot.tracks;
        *self.folders.borrow_mut() = snapshot.folders;
        // A rescan may have changed any file's art.
        self.thumbnails.borrow_mut().clear();
        self.bump_cover_revision();
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
        gradient_index: text::art_gradient_index(&format!(
            "{}-{}",
            album.artist.as_deref().unwrap_or(""),
            album.album.as_deref().unwrap_or("")
        )) as i32,
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

pub(super) fn track_rows(
    tracks: &[Track],
    use_track_number: bool,
    translations: &Translations,
) -> Vec<LibraryTrack> {
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
            title: track.title.as_str().into(),
            artist: track.artist.as_deref().unwrap_or(&unknown_artist).into(),
            composer: track.composer.as_deref().unwrap_or_default().into(),
            format: track.format.as_str().into(),
            duration: text::format_time(track.duration_secs).into(),
            gradient_index: gradient_index(track),
        })
        .collect()
}
