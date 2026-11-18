//! The playlists: the Library's Playlists tab, the menu that adds a song to
//! one, the dialog that names a new one, the sheet that groups them and
//! gives them pictures, and the changes sent to a server.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image, SharedString};

use super::library_screens::await_dialog;
use super::{gradient_index, interface_id, list_place, post, Application};
use crate::covers::{self, CoverKey};
use crate::jellyfin::{self, Failure, Session};
use crate::library::Track;
use crate::library_view::{self, LibraryTab, SelectionKind};
use crate::persistence::{self, MediaServer};
use crate::playlists::{Playlist, PlaylistChange, PlaylistKey};
use crate::text;
use crate::translations::Translations;
use crate::user_interface::{
    self, LibraryCard, ManagedGroup, ManagedPlaylist, MenuChoice, PlaylistSection,
};

/// What the playlists' menus and dialogs are about, kept so a click on one
/// of their lines finds what it stands for.
#[derive(Default)]
pub(super) struct PlaylistScreen {
    /// The playlists each section's cards stand for, section by section.
    sections: Vec<Vec<PlaylistKey>>,
    /// The section and card of the playlist whose panel folds away.
    pub folding: Option<(usize, usize)>,
    /// The first song on show of each playlist, by row: what its card shows
    /// without a picture.
    first_tracks: HashMap<i64, i64>,
    /// The song the open menu is about, and the playlists it offers.
    menu: Option<(i64, Vec<PlaylistKey>)>,
    /// What the naming dialog names.
    naming: Option<Naming>,
    /// What the deletion dialog deletes.
    deleting: Option<Deletion>,
    manager_open: bool,
    /// The playlist the group menu files, and the group each line stands
    /// for.
    group_menu: Option<(PlaylistKey, Vec<Option<i64>>)>,
}

/// What the naming dialog is for.
#[derive(Clone, Copy, Debug)]
enum Naming {
    NewPlaylist { track_id: i64 },
    RenamePlaylist(PlaylistKey),
    NewGroup { then_file: Option<PlaylistKey> },
    RenameGroup(i64),
}

#[derive(Clone, Copy, Debug)]
enum Deletion {
    Playlist(PlaylistKey),
    Group(i64),
}

impl Application {
    pub(super) fn connect_playlists(self: &Rc<Self>) {
        let view = self.window.global::<user_interface::Library>();
        view.set_playlist_sections(self.playlist_sections.clone().into());
        view.set_menu_playlists(self.menu_playlists.clone().into());
        view.set_managed_groups(self.managed_groups.clone().into());
        view.set_group_menu_choices(self.group_menu_choices.clone().into());

        let weak = Rc::downgrade(self);
        view.on_playlist_clicked(move |section, index| {
            if let Some(application) = weak.upgrade() {
                application.open_playlist(section, index);
            }
        });
        let weak = Rc::downgrade(self);
        view.on_track_menu_requested(move |track_id, x, y, in_playlist| {
            if let Some(application) = weak.upgrade() {
                application.open_track_menu(i64::from(track_id), x, y, in_playlist);
            }
        });
        view.on_menu_closed(self.action(|application| {
            application.close_track_menu();
        }));
        view.on_menu_playlist_chosen(self.handler(Application::add_to_chosen_playlist));
        view.on_menu_new_playlist(self.action(Application::name_new_playlist));
        view.on_menu_favourite_toggled(self.action(|application| {
            if let Some(track_id) = application.close_track_menu() {
                application.toggle_favourite(track_id);
            }
        }));
        view.on_menu_remove_from_playlist(self.action(Application::remove_from_open_playlist));
        view.on_naming_confirmed(self.handler(|application, name: SharedString| {
            application.naming_confirmed(&name);
        }));
        view.on_naming_cancelled(self.action(|application| {
            application.playlist_screen.borrow_mut().naming = None;
            application.refresh_playlist_dialogs();
        }));
        view.on_confirming_confirmed(self.action(Application::deletion_confirmed));
        view.on_confirming_cancelled(self.action(|application| {
            application.playlist_screen.borrow_mut().deleting = None;
            application.refresh_playlist_dialogs();
        }));
        view.on_manager_opened(self.action(|application| {
            application.playlist_screen.borrow_mut().manager_open = true;
            application.refresh_playlists();
        }));
        view.on_manager_closed(self.action(|application| {
            application.playlist_screen.borrow_mut().manager_open = false;
            application.refresh_playlist_dialogs();
        }));
        view.on_group_added(self.action(|application| {
            application.ask_name(Naming::NewGroup { then_file: None });
        }));
        view.on_group_renamed(self.handler(|application, group_id: i32| {
            application.ask_name(Naming::RenameGroup(i64::from(group_id)));
        }));
        view.on_group_moved(move |group_id, earlier| {
            super::with_application(|application| {
                let effects = application
                    .playlists
                    .borrow_mut()
                    .move_group(i64::from(group_id), earlier);
                application.run(effects);
                application.refresh_playlists();
            });
        });
        view.on_group_deleted(self.handler(|application, group_id: i32| {
            application.ask_deletion(Deletion::Group(i64::from(group_id)));
        }));
        view.on_playlist_renamed(self.handler(|application, playlist_id: i32| {
            application.ask_name(Naming::RenamePlaylist(PlaylistKey::Saved(i64::from(
                playlist_id,
            ))));
        }));
        view.on_playlist_deleted(self.handler(|application, playlist_id: i32| {
            application.ask_deletion(Deletion::Playlist(PlaylistKey::Saved(i64::from(
                playlist_id,
            ))));
        }));
        view.on_playlist_picture_chosen(self.handler(|application, playlist_id: i32| {
            application.choose_playlist_picture(i64::from(playlist_id));
        }));
        view.on_playlist_picture_removed(self.handler(|application, playlist_id: i32| {
            let effects = application
                .playlists
                .borrow_mut()
                .remove_picture(i64::from(playlist_id));
            application.run(effects);
            application.refresh_playlists();
        }));
        let weak = Rc::downgrade(self);
        view.on_group_menu_requested(move |playlist_id, x, y| {
            if let Some(application) = weak.upgrade() {
                application.open_group_menu(PlaylistKey::Saved(i64::from(playlist_id)), x, y);
            }
        });
        view.on_group_menu_chosen(self.handler(Application::group_chosen));
        view.on_group_menu_closed(self.action(|application| {
            application.playlist_screen.borrow_mut().group_menu = None;
            application.refresh_playlist_dialogs();
        }));
    }

    /// A click on a playlist's card opens its songs in place, or closes them
    /// when they are open.
    fn open_playlist(&self, section: i32, index: i32) {
        let key = {
            let screen = self.playlist_screen.borrow();
            list_place(section)
                .zip(list_place(index))
                .and_then(|(section, index)| screen.sections.get(section)?.get(index).copied())
        };
        let selection = key.and_then(|key| {
            let playlists = self.playlists.borrow();
            let playlist = playlists.find(key)?;
            Some(library_view::playlist_selection(
                playlist,
                &library_view::tracks_by_id(&self.tracks.borrow()),
            ))
        });
        if let Some(selection) = selection {
            self.toggle_selection(selection);
        }
        self.refresh_player();
    }

    /// The song a row stands for, wherever it is listed.
    fn track_of(&self, track_id: i64) -> Option<Track> {
        self.all_tracks
            .borrow()
            .iter()
            .find(|track| track.id == track_id)
            .cloned()
    }

    /// The playlist open in the Library, when one is.
    fn open_playlist_key(&self) -> Option<PlaylistKey> {
        let screen = self.library_screen.borrow();
        let selection = screen
            .selection
            .as_ref()
            .filter(|selection| selection.kind == SelectionKind::Playlist)?;
        self.playlists
            .borrow()
            .playlists()
            .iter()
            .find(|playlist| library_view::playlist_selection_key(playlist.key) == selection.key)
            .map(|playlist| playlist.key)
    }

    /// Opens a song's menu where it was right-clicked.
    pub(super) fn open_track_menu(&self, track_id: i64, x: f32, y: f32, in_playlist: bool) {
        let Some(track) = self.track_of(track_id) else {
            return;
        };
        let (keys, choices): (Vec<PlaylistKey>, Vec<MenuChoice>) = {
            let playlists = self.playlists.borrow();
            let media_server = self.media_server.borrow();
            playlists
                .accepting(&track, media_server.server())
                .into_iter()
                .map(|playlist| {
                    (
                        playlist.key,
                        MenuChoice {
                            label: playlist.name.as_str().into(),
                            current: playlist.holds(track.id),
                        },
                    )
                })
                .unzip()
        };
        let removable = in_playlist
            && self
                .open_playlist_key()
                .and_then(|key| {
                    self.playlists
                        .borrow()
                        .find(key)
                        .map(|playlist| playlist.holds(track_id))
                })
                .unwrap_or(false);
        self.menu_playlists.set_vec(choices);
        self.playlist_screen.borrow_mut().menu = Some((track_id, keys));
        let view = self.window.global::<user_interface::Library>();
        view.set_menu_x(x);
        view.set_menu_y(y);
        view.set_menu_favourite(self.favourite_tracks.borrow().contains(track_id));
        view.set_menu_in_playlist(removable);
        view.set_menu_submenu_open(false);
        view.set_menu_open(true);
    }

    /// Closes the song's menu and gives the song it was about.
    fn close_track_menu(&self) -> Option<i64> {
        let menu = self.playlist_screen.borrow_mut().menu.take();
        self.window
            .global::<user_interface::Library>()
            .set_menu_open(false);
        menu.map(|(track_id, _)| track_id)
    }

    fn add_to_chosen_playlist(&self, index: i32) {
        let chosen = {
            let screen = self.playlist_screen.borrow();
            screen.menu.as_ref().and_then(|(track_id, keys)| {
                list_place(index)
                    .and_then(|place| keys.get(place))
                    .map(|key| (*track_id, *key))
            })
        };
        self.close_track_menu();
        let Some((track, key)) =
            chosen.and_then(|(track_id, key)| Some((self.track_of(track_id)?, key)))
        else {
            return;
        };
        let effects =
            self.playlists
                .borrow_mut()
                .set(key, &track, true, self.media_server.borrow().server());
        self.run(effects);
        self.refresh_playlists();
    }

    fn remove_from_open_playlist(&self) {
        let track_id = self.close_track_menu();
        let (Some(track), Some(key)) = (
            track_id.and_then(|track_id| self.track_of(track_id)),
            self.open_playlist_key(),
        ) else {
            return;
        };
        let effects = self.playlists.borrow_mut().set(
            key,
            &track,
            false,
            self.media_server.borrow().server(),
        );
        self.run(effects);
        self.refresh_playlists();
    }

    fn name_new_playlist(&self) {
        if let Some(track_id) = self.close_track_menu() {
            self.ask_name(Naming::NewPlaylist { track_id });
        }
    }

    /// Opens the naming dialog, its field holding the name to start from.
    fn ask_name(&self, naming: Naming) {
        let translations = self.translations.borrow();
        let (title, confirm, text) = {
            let playlists = self.playlists.borrow();
            match naming {
                Naming::NewPlaylist { track_id } => (
                    "playlists.name.newTitle",
                    "playlists.name.create",
                    self.track_of(track_id)
                        .map(|track| track.title)
                        .unwrap_or_default(),
                ),
                Naming::RenamePlaylist(key) => (
                    "playlists.name.renameTitle",
                    "playlists.name.save",
                    playlists
                        .find(key)
                        .map(|playlist| playlist.name.clone())
                        .unwrap_or_default(),
                ),
                Naming::NewGroup { .. } => (
                    "playlists.group.newTitle",
                    "playlists.name.create",
                    String::new(),
                ),
                Naming::RenameGroup(group_id) => (
                    "playlists.group.renameTitle",
                    "playlists.name.save",
                    playlists
                        .groups()
                        .iter()
                        .find(|group| group.id == group_id)
                        .map(|group| group.name.clone())
                        .unwrap_or_default(),
                ),
            }
        };
        self.playlist_screen.borrow_mut().naming = Some(naming);
        let view = self.window.global::<user_interface::Library>();
        view.set_naming_title(translations.text(title).into());
        view.set_naming_label(translations.text("playlists.name.label").into());
        view.set_naming_confirm(translations.text(confirm).into());
        view.set_naming_text(text.into());
        view.set_naming_open(true);
    }

    /// The naming dialog was confirmed: a blank name keeps it open.
    fn naming_confirmed(&self, name: &str) {
        if name.trim().is_empty() {
            return;
        }
        let Some(naming) = self.playlist_screen.borrow_mut().naming.take() else {
            return;
        };
        let effects = {
            let server = self.media_server.borrow();
            let server = server.server();
            let mut playlists = self.playlists.borrow_mut();
            match naming {
                Naming::NewPlaylist { track_id } => match self.track_of(track_id) {
                    Some(track) => playlists.create(name, &track, server),
                    None => Vec::new(),
                },
                Naming::RenamePlaylist(key) => playlists.rename(key, name, server),
                Naming::NewGroup { then_file } => playlists.create_group(name, then_file),
                Naming::RenameGroup(group_id) => playlists.rename_group(group_id, name),
            }
        };
        self.refresh_playlist_dialogs();
        self.run(effects);
        self.refresh_playlists();
    }

    fn ask_deletion(&self, deletion: Deletion) {
        let translations = self.translations.borrow();
        let (title, message) = {
            let playlists = self.playlists.borrow();
            match deletion {
                Deletion::Playlist(key) => {
                    let Some(playlist) = playlists.find(key) else {
                        return;
                    };
                    let message = match &playlist.server {
                        Some(_) => translations.translate(
                            "playlists.delete.server",
                            &[(
                                "server",
                                self.media_server
                                    .borrow()
                                    .server()
                                    .map(|server| server.server_name.clone())
                                    .unwrap_or_default(),
                            )],
                        ),
                        None => translations.text("playlists.delete.local"),
                    };
                    (
                        translations.translate(
                            "playlists.delete.title",
                            &[("name", playlist.name.clone())],
                        ),
                        message,
                    )
                }
                Deletion::Group(group_id) => {
                    let Some(group) = playlists.groups().iter().find(|group| group.id == group_id)
                    else {
                        return;
                    };
                    (
                        translations.translate(
                            "playlists.group.deleteTitle",
                            &[("name", group.name.clone())],
                        ),
                        translations.text("playlists.group.deleteMessage"),
                    )
                }
            }
        };
        self.playlist_screen.borrow_mut().deleting = Some(deletion);
        let view = self.window.global::<user_interface::Library>();
        view.set_confirming_title(title.into());
        view.set_confirming_message(message.into());
        view.set_confirming_confirm(translations.text("playlists.delete.confirm").into());
        view.set_confirming_open(true);
    }

    fn deletion_confirmed(&self) {
        let Some(deletion) = self.playlist_screen.borrow_mut().deleting.take() else {
            return;
        };
        let effects = match deletion {
            Deletion::Playlist(key) => {
                // Its panel closes at once: the card it would fold under
                // goes with it.
                if self.open_playlist_key() == Some(key) {
                    self.change_selection(|selection| *selection = None);
                    self.playlist_screen.borrow_mut().folding = None;
                }
                self.playlists
                    .borrow_mut()
                    .delete(key, self.media_server.borrow().server())
            }
            Deletion::Group(group_id) => self.playlists.borrow_mut().remove_group(group_id),
        };
        self.refresh_playlist_dialogs();
        self.run(effects);
        self.refresh_library();
    }

    /// Opens the menu that files a playlist in a group, under its button.
    fn open_group_menu(&self, key: PlaylistKey, x: f32, y: f32) {
        let translations = self.translations.borrow();
        let (groups, choices): (Vec<Option<i64>>, Vec<MenuChoice>) = {
            let playlists = self.playlists.borrow();
            let current = playlists.find(key).and_then(|playlist| playlist.group_id);
            std::iter::once((None, translations.text("playlists.manage.noGroup")))
                .chain(
                    playlists
                        .groups()
                        .iter()
                        .map(|group| (Some(group.id), group.name.clone())),
                )
                .map(|(group_id, label)| {
                    (
                        group_id,
                        MenuChoice {
                            label: label.into(),
                            current: group_id == current,
                        },
                    )
                })
                .unzip()
        };
        self.group_menu_choices.set_vec(choices);
        self.playlist_screen.borrow_mut().group_menu = Some((key, groups));
        let view = self.window.global::<user_interface::Library>();
        view.set_group_menu_x(x);
        view.set_group_menu_y(y);
        view.set_group_menu_open(true);
    }

    fn group_chosen(&self, index: i32) {
        let Some((key, groups)) = self.playlist_screen.borrow_mut().group_menu.take() else {
            return;
        };
        self.refresh_playlist_dialogs();
        if index < 0 {
            self.ask_name(Naming::NewGroup {
                then_file: Some(key),
            });
            return;
        }
        let Some(group_id) = list_place(index).and_then(|place| groups.get(place).copied()) else {
            return;
        };
        let effects = self.playlists.borrow_mut().set_group(key, group_id);
        self.run(effects);
        self.refresh_playlists();
    }

    /// Asks the system for a picture, then keeps it for the playlist.
    fn choose_playlist_picture(&self, playlist_id: i64) {
        let filter_name = self.translations.borrow().text("playlists.manage.pictures");
        let dialog = rfd::AsyncFileDialog::new()
            .add_filter(filter_name, &["png", "jpg", "jpeg", "webp", "gif", "bmp"])
            .pick_file();
        await_dialog(async move {
            let Some(file) = dialog.await else {
                return;
            };
            let path = file.path().to_path_buf();
            super::with_application(move |application| {
                application.keep_playlist_picture(playlist_id, path);
            });
        });
    }

    /// Reads, scales and keeps a picture off the interface thread, then shows
    /// it on the playlist's card.
    fn keep_playlist_picture(&self, playlist_id: i64, path: std::path::PathBuf) {
        let database = Arc::clone(&self.database);
        self.picture_worker.run(move || {
            let kept = std::fs::read(&path)
                .map_err(|error| format!("read {}: {error}", path.display()))
                .and_then(covers::playlist_picture)
                .and_then(|picture| {
                    persistence::set_playlist_picture(&database, playlist_id, Some(&picture))
                });
            post(move |application| match kept {
                Ok(()) => {
                    application
                        .playlists
                        .borrow_mut()
                        .picture_saved(playlist_id);
                    application.forget_playlist_picture(playlist_id);
                    application.refresh_playlists();
                }
                Err(error) => {
                    log::warn!("keep a playlist picture: {error}");
                    application.notify_error("error.playlistPicture", Vec::new(), None);
                }
            });
        });
    }

    /// Saves a playlist just made — by Janis, or by its server — then tells
    /// the playlists its row.
    pub(super) fn save_playlist(
        &self,
        creation: u64,
        name: String,
        server: Option<(i64, String)>,
        track_ids: Vec<i64>,
    ) {
        let saved = persistence::create_playlist(
            &self.database,
            &name,
            server
                .as_ref()
                .map(|(server_id, item)| (*server_id, item.as_str())),
            &track_ids,
        );
        let playlist_id = match saved {
            Ok(playlist_id) => playlist_id,
            Err(error) => {
                self.saved("playlist", Err(error));
                None
            }
        };
        let effects = self.playlists.borrow_mut().creation_saved(
            creation,
            playlist_id,
            self.media_server.borrow().server(),
        );
        self.run(effects);
        self.refresh_playlists();
    }

    /// Saves a new group, then tells the playlists its row.
    pub(super) fn create_playlist_group(&self, name: String, then_file: Option<PlaylistKey>) {
        match persistence::create_playlist_group(&self.database, &name) {
            Ok(group_id) => {
                let effects = self
                    .playlists
                    .borrow_mut()
                    .group_created(group_id, name, then_file);
                self.run(effects);
            }
            Err(error) => self.saved("playlist-group", Err(error)),
        }
        self.refresh_playlists();
    }

    /// Drops the picture of a playlist from the covers, so its card asks for
    /// what stands for it now.
    pub(super) fn forget_playlist_picture(&self, playlist_id: i64) {
        self.thumbnails
            .borrow_mut()
            .remove(&CoverKey::PlaylistPicture(playlist_id));
        self.bump_cover_revision();
    }

    /// The cover of a playlist's card: the picture the listener chose, the
    /// server's, or its first song's on show.
    pub(super) fn playlist_cover(&self, playlist_id: i64) -> Image {
        let key = {
            let playlists = self.playlists.borrow();
            let Some(playlist) = playlists.find(PlaylistKey::Saved(playlist_id)) else {
                return Image::default();
            };
            let item = playlist
                .server
                .as_ref()
                .and_then(|home| home.item.clone())
                .filter(|_| playlist.server_picture);
            if playlist.picture {
                Some(CoverKey::PlaylistPicture(playlist_id))
            } else if let Some(item) = item {
                Some(CoverKey::ServerPicture(item))
            } else {
                self.playlist_screen
                    .borrow()
                    .first_tracks
                    .get(&playlist_id)
                    .map(|track_id| self.cover_keys.borrow().key(*track_id))
            }
        };
        match key {
            Some(key) => self.thumbnail_of(key),
            None => Image::default(),
        }
    }

    /// Makes, changes or deletes a playlist on its server, behind whatever
    /// call to the server is already queued; then tells the playlists how it
    /// went, and the Jellyfin card when the server refused the token.
    pub(super) fn push_playlist_change(
        &self,
        server: MediaServer,
        playlist: PlaylistKey,
        item: Option<String>,
        change: PlaylistChange,
        sequence: u64,
    ) {
        let session = Session::new(&server, &self.device_identifier);
        self.server_worker.run(move || {
            let outcome = send_playlist_change(&session, item.as_deref(), &change);
            post(move |application| {
                let token_rejected = matches!(outcome, Err(Failure::TokenRejected));
                let mut effects = application
                    .playlists
                    .borrow_mut()
                    .push_finished(playlist, change, sequence, outcome);
                if token_rejected {
                    effects.extend(
                        application
                            .media_server
                            .borrow_mut()
                            .token_rejected(&server),
                    );
                    application.refresh_jellyfin();
                }
                if effects.is_empty() {
                    return;
                }
                application.run(effects);
                application.refresh_playlists();
            });
        });
    }

    /// Brings the open playlist's songs up to date with the playlists and the
    /// library; gives whether a playlist is open.
    pub(super) fn renew_playlist_selection(&self) -> bool {
        let mut screen = self.library_screen.borrow_mut();
        if !screen
            .selection
            .as_ref()
            .is_some_and(|selection| selection.kind == SelectionKind::Playlist)
        {
            return false;
        }
        let playlists = self.playlists.borrow();
        let tracks = self.tracks.borrow();
        screen.selection = screen.selection.take().and_then(|selection| {
            library_view::renewed_selection(selection, &[], &[], playlists.playlists(), &tracks)
        });
        true
    }

    /// Pushes the playlists as they now are, after a change to them: the
    /// open playlist's songs, the Playlists tab, and the sheet when it is
    /// open.
    pub(super) fn refresh_playlists(&self) {
        if self.renew_playlist_selection() {
            self.show_playlist_selection();
        }
        self.refresh_playlist_sections();
    }

    /// Pushes the open playlist's heading and songs.
    fn show_playlist_selection(&self) {
        let screen = self.library_screen.borrow();
        let view = self.window.global::<user_interface::Library>();
        view.set_selection_present(screen.selection.is_some());
        match &screen.selection {
            Some(selection) => {
                let favourites = self.favourite_tracks.borrow();
                view.set_selection_heading(selection.label.as_str().into());
                self.library_selection_tracks
                    .set_vec(super::library_screens::track_rows(
                        &selection.tracks,
                        Some(selection.kind),
                        |id| favourites.contains(id),
                        &self.translations.borrow(),
                    ));
            }
            None => {
                view.set_selection_heading(SharedString::default());
                self.library_selection_tracks.set_vec(Vec::new());
            }
        }
    }

    /// Pushes the Playlists tab — a section per group, from the playlists,
    /// the search and the selection — and the sheet when it is open. A
    /// section's grid is updated in place, so the grids stay and an opened
    /// card unfolds as an album's does; they are rebuilt only when the
    /// sections themselves change in number.
    pub(super) fn refresh_playlist_sections(&self) {
        let translations = self.translations.borrow();
        let playlists = self.playlists.borrow();
        let tracks = self.tracks.borrow();
        let search = self.search.borrow();
        let mut screen = self.library_screen.borrow_mut();
        let mut playlist_screen = self.playlist_screen.borrow_mut();
        let view = self.window.global::<user_interface::Library>();
        let shown = library_view::tracks_by_id(&tracks);

        let sections =
            library_view::playlist_sections(playlists.playlists(), playlists.groups(), &search);
        let selected = if screen.tab == LibraryTab::Playlists {
            library_view::playlist_selection_place(&sections, screen.selection.as_ref())
        } else {
            None
        };
        screen.shown_playlist_selection = selected;
        let folding = playlist_screen.folding;
        let grouped = !playlists.groups().is_empty();
        let ungrouped = translations.text("library.playlists.ungrouped");

        let mut card_models = self.playlist_section_cards.borrow_mut();
        let rebuilt = card_models.len() != sections.len();
        if rebuilt {
            *card_models = sections
                .iter()
                .map(|_| Rc::new(slint::VecModel::default()))
                .collect();
        }
        let rows: Vec<PlaylistSection> = sections
            .iter()
            .enumerate()
            .map(|(place, section)| {
                let cards: Vec<LibraryCard> = section
                    .playlists
                    .iter()
                    .map(|playlist| playlist_card(playlist, &shown, &translations))
                    .collect();
                let model = &card_models[place];
                if rebuilt || !slint::Model::iter(&**model).eq(cards.iter().cloned()) {
                    model.set_vec(cards);
                }
                PlaylistSection {
                    heading: match &section.group {
                        Some(group) => group.name.as_str().into(),
                        None if grouped => ungrouped.as_str().into(),
                        None => SharedString::default(),
                    },
                    cards: model.clone().into(),
                    selection_index: index_in(selected, place),
                    folding_index: index_in(folding, place),
                }
            })
            .collect();
        if rebuilt {
            self.playlist_sections.set_vec(rows);
        } else {
            for (place, row) in rows.into_iter().enumerate() {
                if slint::Model::row_data(&*self.playlist_sections, place).as_ref() != Some(&row) {
                    slint::Model::set_row_data(&*self.playlist_sections, place, row);
                }
            }
        }
        playlist_screen.sections = sections
            .iter()
            .map(|section| {
                section
                    .playlists
                    .iter()
                    .map(|playlist| playlist.key)
                    .collect()
            })
            .collect();
        playlist_screen.first_tracks = playlists
            .playlists()
            .iter()
            .filter_map(|playlist| {
                let playlist_id = playlist.key.saved()?;
                let first = playlist
                    .track_ids
                    .iter()
                    .copied()
                    .find(|track_id| shown.contains_key(track_id))?;
                Some((playlist_id, first))
            })
            .collect();
        view.set_playlists_empty(playlists.playlists().is_empty());
        view.set_selection_is_playlist(
            screen
                .selection
                .as_ref()
                .is_some_and(|selection| selection.kind == SelectionKind::Playlist),
        );
        if playlist_screen.manager_open {
            self.managed_groups
                .set_vec(managed_groups(&playlists, &shown, &translations));
        }
        drop(card_models);
        drop(playlist_screen);
        drop(screen);
        drop(search);
        drop(tracks);
        drop(playlists);
        drop(translations);
        self.refresh_playlist_dialogs();
    }

    /// Opens and closes the sheet, the dialogs and the menus as the
    /// playlists' screen state says.
    pub(super) fn refresh_playlist_dialogs(&self) {
        let screen = self.playlist_screen.borrow();
        let view = self.window.global::<user_interface::Library>();
        view.set_manager_open(screen.manager_open);
        view.set_naming_open(screen.naming.is_some());
        view.set_confirming_open(screen.deleting.is_some());
        view.set_group_menu_open(screen.group_menu.is_some());
        view.set_menu_open(screen.menu.is_some());
    }
}

/// The place of a card in section `section`, or -1 when the card is in
/// another, or none.
fn index_in(place: Option<(usize, usize)>, section: usize) -> i32 {
    match place {
        Some((in_section, index)) if in_section == section => index as i32,
        _ => -1,
    }
}

/// The gradient a playlist's card wears without a cover: its first song's,
/// or one of its own.
fn playlist_gradient_index(playlist: &Playlist, songs: &[Track]) -> i32 {
    songs
        .first()
        .map(gradient_index)
        .unwrap_or_else(|| text::art_gradient_index(&playlist.name) as i32)
}

fn playlist_card(
    playlist: &Playlist,
    shown: &HashMap<i64, &Track>,
    translations: &Translations,
) -> LibraryCard {
    let songs = library_view::playlist_tracks(playlist, shown);
    LibraryCard {
        key: library_view::playlist_selection_key(playlist.key).into(),
        title: playlist.name.as_str().into(),
        subtitle: library_view::playlist_detail(&songs, translations).into(),
        detail: SharedString::default(),
        track_id: songs
            .first()
            .map(|track| interface_id(track.id))
            .unwrap_or_default(),
        gradient_index: playlist_gradient_index(playlist, &songs),
        playlist_id: playlist.key.saved().map(interface_id).unwrap_or_default(),
    }
}

/// The sheet's sections (`library_view::managed_sections`) as its rows.
fn managed_groups(
    playlists: &crate::playlists::Playlists,
    shown: &HashMap<i64, &Track>,
    translations: &Translations,
) -> Vec<ManagedGroup> {
    let no_group = translations.text("playlists.manage.noGroup");
    let sections = library_view::managed_sections(playlists.playlists(), playlists.groups());
    let group_count = playlists.groups().len();
    sections
        .iter()
        .enumerate()
        .map(|(place, section)| {
            let group_name = section
                .group
                .as_ref()
                .map_or(no_group.as_str(), |group| group.name.as_str());
            let rows: Vec<ManagedPlaylist> = section
                .playlists
                .iter()
                .map(|playlist| {
                    let songs = library_view::playlist_tracks(playlist, shown);
                    ManagedPlaylist {
                        playlist_id: playlist.key.saved().map(interface_id).unwrap_or_default(),
                        name: playlist.name.as_str().into(),
                        detail: library_view::playlist_detail(&songs, translations).into(),
                        group: group_name.into(),
                        picture: playlist.picture,
                        track_id: songs
                            .first()
                            .map(|track| interface_id(track.id))
                            .unwrap_or_default(),
                        gradient_index: playlist_gradient_index(playlist, &songs),
                    }
                })
                .collect();
            ManagedGroup {
                group_id: section
                    .group
                    .as_ref()
                    .map(|group| interface_id(group.id))
                    .unwrap_or_default(),
                name: group_name.into(),
                first: place == 0,
                last: place + 1 >= group_count,
                playlists: Rc::new(slint::VecModel::from(rows)).into(),
            }
        })
        .collect()
}

/// Sends one change to the server. A creation gives the item it made.
fn send_playlist_change(
    session: &Session,
    item: Option<&str>,
    change: &PlaylistChange,
) -> Result<Option<String>, Failure> {
    let playlist = || item.ok_or_else(|| Failure::Other("the playlist has no item".to_owned()));
    match change {
        PlaylistChange::Create {
            name, addresses, ..
        } => jellyfin::create_playlist(session, name, addresses).map(Some),
        PlaylistChange::Add { address, .. } => {
            jellyfin::add_to_playlist(session, playlist()?, address).map(|()| None)
        }
        PlaylistChange::Remove { address, .. } => {
            jellyfin::remove_from_playlist(session, playlist()?, address).map(|()| None)
        }
        PlaylistChange::Rename { name } => {
            jellyfin::rename_playlist(session, playlist()?, name).map(|()| None)
        }
        PlaylistChange::Delete => jellyfin::delete_playlist(session, playlist()?).map(|()| None),
    }
}
