//! The listener's playlists, and the groups they sort them into.
//!
//! A playlist lives where its first song does. Made from a file, it is
//! Janis's own, holds files only, and every change to it is saved at once.
//! Made from a song of the signed-in media server, it lives on the server,
//! holds that server's songs only, and is kept as the server has it: every
//! sync brings the server's playlists back into `janis.db`, and a change made
//! in Janis shows at once but is saved once the server has it. Should the
//! server refuse it, or not answer, the playlist shows what the server has,
//! and the listener is told — the bookkeeping is `PendingPushes`'s, as for
//! the stars of `FavouriteTracks`.
//!
//! A playlist on its way to being created on the server has no row yet: it
//! shows under a key of its own, takes changes in Janis alone, and once the
//! server has made it and its row is saved, whatever changed meanwhile is
//! sent after it.
//!
//! A song is in a playlist at most once. Groups and the pictures the
//! listener chooses are Janis's own: no server keeps them.
//! YouTube Music playlists are imported read-only. Their provider owns names
//! and songs; the listener can still choose their Janis group and picture.

use std::collections::HashMap;

use crate::effects::Effect;
use crate::jellyfin::Failure;
use crate::library::{Track, TrackSource};
use crate::persistence::{MediaServer, PlaylistGroup, PlaylistSource, SavedPlaylist};
use crate::server_pushes::PendingPushes;
use crate::text::compare_labels;

/// Which playlist: its row, or — while its server creates it — the number
/// of its creation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaylistKey {
    Saved(i64),
    Creating(u64),
}

impl PlaylistKey {
    pub fn saved(self) -> Option<i64> {
        match self {
            PlaylistKey::Saved(playlist_id) => Some(playlist_id),
            PlaylistKey::Creating(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playlist {
    pub key: PlaylistKey,
    pub name: String,
    pub source: PlaylistSource,
    /// The Jellyfin server it lives on; none for other providers.
    pub server: Option<ServerHome>,
    /// The server has a picture of it.
    pub server_picture: bool,
    pub group_id: Option<i64>,
    /// The listener chose a picture for it.
    pub picture: bool,
    pub track_ids: Vec<i64>,
}

/// Where a server's playlist lives: the server's row, and the playlist's
/// item there once the server has made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerHome {
    pub server_id: i64,
    pub item: Option<String>,
}

impl Playlist {
    fn from_saved(saved: SavedPlaylist) -> Self {
        Self {
            key: PlaylistKey::Saved(saved.id),
            name: saved.name,
            source: saved.source,
            server: saved.server.map(|(server_id, item)| ServerHome {
                server_id,
                item: Some(item),
            }),
            server_picture: saved.server_picture,
            group_id: saved.group_id,
            picture: saved.picture,
            track_ids: saved.track_ids,
        }
    }

    /// Whether provider-owned content may change. Grouping and pictures are
    /// Janis-local choices for every provider.
    pub fn can_edit_content(&self) -> bool {
        match self.source {
            PlaylistSource::Local | PlaylistSource::Jellyfin => true,
            #[cfg(feature = "youtube-music")]
            PlaylistSource::YouTubeMusic => false,
        }
    }

    /// A local playlist takes persistent files and YouTube Music songs; a Jellyfin
    /// playlist takes songs of its signed-in server. Imported playlists take none.
    pub fn takes(&self, track: &Track, server: Option<&MediaServer>) -> bool {
        if track.id <= 0 {
            return false;
        }
        match (self.source, track.source, &self.server, track.server_id) {
            (PlaylistSource::Local, TrackSource::File, _, _) => true,
            #[cfg(feature = "youtube-music")]
            (PlaylistSource::Local, TrackSource::YouTubeMusic, _, _) => true,
            (PlaylistSource::Jellyfin, TrackSource::Jellyfin, Some(home), Some(server_id)) => {
                home.server_id == server_id && server.is_some_and(|server| server.id == server_id)
            }
            _ => false,
        }
    }

    pub fn holds(&self, track_id: i64) -> bool {
        self.track_ids.contains(&track_id)
    }
}

/// What is sent to a server about one of its playlists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaylistChange {
    /// Make the playlist, holding these songs.
    Create {
        name: String,
        track_ids: Vec<i64>,
        /// The songs' `jellyfin://<server>/<item>` addresses.
        addresses: Vec<String>,
    },
    Add {
        track_id: i64,
        address: String,
    },
    Remove {
        track_id: i64,
        address: String,
    },
    Rename {
        name: String,
    },
    Delete,
}

/// A playlist a server is making.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Creation {
    server_id: i64,
    /// What the server was asked to make it with.
    name: String,
    track_ids: Vec<i64>,
    /// The address of every song it held at any point, so what changed
    /// meanwhile can be sent once the server has made it.
    addresses: HashMap<i64, String>,
    /// The playlist, when the listener deleted it meanwhile.
    deleted: Option<Playlist>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Playlists {
    /// Every playlist on show, those being created included.
    playlists: Vec<Playlist>,
    groups: Vec<PlaylistGroup>,
    /// A song's place in a server's playlist, by playlist row and track id.
    entries: PendingPushes<(i64, i64), bool>,
    /// A server's playlist's name, by row.
    names: PendingPushes<i64, String>,
    /// Whether a server's playlist still exists, by row.
    deletions: PendingPushes<i64, bool>,
    /// The playlists whose deletion is on its way, out of sight meanwhile.
    deleting: HashMap<i64, Playlist>,
    creations: HashMap<u64, Creation>,
    last_creation: u64,
}

impl Playlists {
    /// Takes the playlists and groups as `janis.db` keeps them: at boot, and
    /// after every read of the library. Changes still on their way to a
    /// server win over what was read, and the playlists still being created
    /// stay.
    pub fn restore(&mut self, saved: Vec<SavedPlaylist>, groups: Vec<PlaylistGroup>) {
        let creating: Vec<Playlist> = self
            .playlists
            .drain(..)
            .filter(|playlist| matches!(playlist.key, PlaylistKey::Creating(_)))
            .collect();
        self.playlists = saved.into_iter().map(Playlist::from_saved).collect();
        self.groups = groups;

        let entries: Vec<((i64, i64), bool)> = self
            .entries
            .wanted()
            .map(|(key, present)| (*key, *present))
            .collect();
        for ((playlist_id, track_id), present) in entries {
            if let Some(playlist) = self
                .find_mut(PlaylistKey::Saved(playlist_id))
                .filter(|playlist| playlist.source == PlaylistSource::Jellyfin)
            {
                set_entry(playlist, track_id, present, None);
            }
        }
        let names: Vec<(i64, String)> = self
            .names
            .wanted()
            .map(|(playlist_id, name)| (*playlist_id, name.clone()))
            .collect();
        for (playlist_id, name) in names {
            if let Some(playlist) = self
                .find_mut(PlaylistKey::Saved(playlist_id))
                .filter(|playlist| playlist.source == PlaylistSource::Jellyfin)
            {
                playlist.name = name;
            }
        }
        let deleted: Vec<i64> = self
            .deletions
            .wanted()
            .filter(|(_, exists)| !**exists)
            .map(|(playlist_id, _)| *playlist_id)
            .collect();
        for playlist_id in deleted {
            if let Some(place) = self
                .place_of(PlaylistKey::Saved(playlist_id))
                .filter(|place| self.playlists[*place].source == PlaylistSource::Jellyfin)
            {
                let playlist = self.playlists.remove(place);
                self.deleting.insert(playlist_id, playlist);
            }
        }
        self.playlists.extend(creating);
    }

    #[cfg(feature = "youtube-music")]
    /// Refreshes imported membership without replacing local presentation or
    /// changes still on their way to another provider.
    pub fn restore_youtube_memberships(&mut self, memberships: Vec<(i64, Vec<i64>)>) {
        for (playlist_id, track_ids) in memberships {
            if let Some(playlist) = self
                .find_mut(PlaylistKey::Saved(playlist_id))
                .filter(|playlist| playlist.source == PlaylistSource::YouTubeMusic)
            {
                playlist.track_ids = track_ids;
            }
        }
    }

    /// Every playlist on show, the oldest first.
    pub fn playlists(&self) -> &[Playlist] {
        &self.playlists
    }

    pub fn groups(&self) -> &[PlaylistGroup] {
        &self.groups
    }

    pub fn find(&self, key: PlaylistKey) -> Option<&Playlist> {
        self.playlists.iter().find(|playlist| playlist.key == key)
    }

    /// The playlists the listener can put `track` in, by name.
    pub fn accepting(&self, track: &Track, server: Option<&MediaServer>) -> Vec<&Playlist> {
        let mut accepting: Vec<&Playlist> = self
            .playlists
            .iter()
            .filter(|playlist| playlist.takes(track, server))
            .collect();
        accepting.sort_by(|left, right| compare_labels(&left.name, &right.name));
        accepting
    }

    /// Makes a playlist named `name` holding `track`. A file or saved YouTube Music
    /// song makes one of Janis's own, saved at once; a song of the signed-in server
    /// makes one on the server, saved once the server has made it.
    pub fn create(
        &mut self,
        name: &str,
        track: &Track,
        server: Option<&MediaServer>,
    ) -> Vec<Effect> {
        let name = name.trim().to_owned();
        if name.is_empty() || track.id <= 0 {
            return Vec::new();
        }
        let home = match track.source {
            TrackSource::File => None,
            #[cfg(feature = "youtube-music")]
            TrackSource::YouTubeMusic => None,
            TrackSource::Jellyfin => {
                match server.filter(|server| Some(server.id) == track.server_id) {
                    Some(server) => Some(server),
                    None => return Vec::new(),
                }
            }
        };
        self.last_creation += 1;
        let creation = self.last_creation;
        self.playlists.push(Playlist {
            key: PlaylistKey::Creating(creation),
            name: name.clone(),
            source: if home.is_some() {
                PlaylistSource::Jellyfin
            } else {
                PlaylistSource::Local
            },
            server: home.map(|server| ServerHome {
                server_id: server.id,
                item: None,
            }),
            server_picture: false,
            group_id: None,
            picture: false,
            track_ids: vec![track.id],
        });
        let Some(server) = home else {
            return vec![Effect::SavePlaylist {
                creation,
                name,
                server: None,
                track_ids: vec![track.id],
            }];
        };
        self.creations.insert(
            creation,
            Creation {
                server_id: server.id,
                name: name.clone(),
                track_ids: vec![track.id],
                addresses: HashMap::from([(track.id, track.path.clone())]),
                deleted: None,
            },
        );
        vec![Effect::PushPlaylistChange {
            server: server.clone(),
            playlist: PlaylistKey::Creating(creation),
            item: None,
            change: PlaylistChange::Create {
                name,
                track_ids: vec![track.id],
                addresses: vec![track.path.clone()],
            },
            sequence: creation,
        }]
    }

    /// The playlist made by `creation` was saved as row `playlist_id`, or
    /// could not be — its server is gone. It now goes by its row, and what
    /// changed while the server made it is sent after it.
    pub fn creation_saved(
        &mut self,
        creation: u64,
        playlist_id: Option<i64>,
        server: Option<&MediaServer>,
    ) -> Vec<Effect> {
        let made = self.creations.remove(&creation);
        let place = self.place_of(PlaylistKey::Creating(creation));
        let Some(playlist_id) = playlist_id else {
            if let Some(place) = place {
                self.playlists.remove(place);
            }
            return Vec::new();
        };
        let key = PlaylistKey::Saved(playlist_id);
        // A sync that read the new playlist may have saved it first.
        if let (Some(saved_place), Some(_)) = (self.place_of(key), place) {
            self.playlists.remove(saved_place);
        }
        let Some(made) = made else {
            // Janis's own: saved as it is.
            if let Some(place) = self.place_of(PlaylistKey::Creating(creation)) {
                self.playlists[place].key = key;
            }
            return Vec::new();
        };
        let wanted = match (
            made.deleted.clone(),
            self.place_of(PlaylistKey::Creating(creation)),
        ) {
            (Some(deleted), _) => deleted,
            (None, Some(place)) => self.playlists.remove(place),
            (None, None) => return Vec::new(),
        };
        let item = wanted.server.as_ref().and_then(|home| home.item.clone());
        let mut saved = Playlist {
            key,
            name: made.name.clone(),
            track_ids: made.track_ids.clone(),
            ..wanted.clone()
        };
        saved.server = Some(ServerHome {
            server_id: made.server_id,
            item,
        });
        let place = self.playlists.len();
        self.playlists.push(saved);

        let mut effects = Vec::new();
        if let Some(group_id) = wanted.group_id {
            effects.push(Effect::PersistPlaylistGroup {
                playlist_id,
                group_id: Some(group_id),
            });
        }
        if made.deleted.is_some() {
            effects.extend(self.delete(key, server));
            return effects;
        }
        if wanted.name != made.name {
            effects.extend(self.rename(key, &wanted.name, server));
        }
        for track_id in &made.track_ids {
            if !wanted.holds(*track_id) {
                effects.extend(self.change_entry(place, *track_id, &made, false, server));
            }
        }
        for track_id in &wanted.track_ids {
            if !made.track_ids.contains(track_id) {
                effects.extend(self.change_entry(place, *track_id, &made, true, server));
            }
        }
        effects
    }

    /// Sends one song's change, made while the server created its playlist.
    fn change_entry(
        &mut self,
        place: usize,
        track_id: i64,
        made: &Creation,
        present: bool,
        server: Option<&MediaServer>,
    ) -> Vec<Effect> {
        let Some(address) = made.addresses.get(&track_id) else {
            return Vec::new();
        };
        let key = self.playlists[place].key;
        self.place_entry(key, track_id, address, present, server)
    }

    /// Puts `track` at the end of a playlist, or takes it out. A playlist of
    /// Janis's own is saved at once; a server's is sent to it, and saved
    /// once it answers. A song the playlist cannot take changes nothing.
    pub fn set(
        &mut self,
        key: PlaylistKey,
        track: &Track,
        present: bool,
        server: Option<&MediaServer>,
    ) -> Vec<Effect> {
        let takes = self
            .find(key)
            .is_some_and(|playlist| playlist.takes(track, server));
        if present && !takes {
            return Vec::new();
        }
        self.place_entry(key, track.id, &track.path, present, server)
    }

    fn place_entry(
        &mut self,
        key: PlaylistKey,
        track_id: i64,
        address: &str,
        present: bool,
        server: Option<&MediaServer>,
    ) -> Vec<Effect> {
        let Some(place) = self.place_of(key).filter(|_| self.changeable(key, server)) else {
            return Vec::new();
        };
        let playlist = &mut self.playlists[place];
        let was_present = playlist.holds(track_id);
        if was_present == present {
            return Vec::new();
        }
        let entry_place = playlist.track_ids.iter().position(|kept| *kept == track_id);
        set_entry(playlist, track_id, present, None);
        let playlist_id = match key {
            PlaylistKey::Creating(creation) => {
                if let Some(made) = self.creations.get_mut(&creation) {
                    made.addresses.insert(track_id, address.to_owned());
                }
                return Vec::new();
            }
            PlaylistKey::Saved(playlist_id) => playlist_id,
        };
        let Some((server, item)) = server_of(&self.playlists[place], server) else {
            return vec![Effect::PersistPlaylistEntry {
                playlist_id,
                track_id,
                present,
            }];
        };
        let sequence =
            self.entries
                .push((playlist_id, track_id), present, was_present, entry_place);
        let address = address.to_owned();
        vec![Effect::PushPlaylistChange {
            server: server.clone(),
            playlist: key,
            item: Some(item),
            change: if present {
                PlaylistChange::Add { track_id, address }
            } else {
                PlaylistChange::Remove { track_id, address }
            },
            sequence,
        }]
    }

    /// Renames a playlist; a blank name changes nothing.
    pub fn rename(
        &mut self,
        key: PlaylistKey,
        name: &str,
        server: Option<&MediaServer>,
    ) -> Vec<Effect> {
        let name = name.trim().to_owned();
        let Some(place) = self.place_of(key).filter(|_| self.changeable(key, server)) else {
            return Vec::new();
        };
        if name.is_empty() || self.playlists[place].name == name {
            return Vec::new();
        }
        let old_name = std::mem::replace(&mut self.playlists[place].name, name.clone());
        let Some(playlist_id) = key.saved() else {
            return Vec::new();
        };
        let Some((server, item)) = server_of(&self.playlists[place], server) else {
            return vec![Effect::PersistPlaylistName { playlist_id, name }];
        };
        let sequence = self.names.push(playlist_id, name.clone(), old_name, None);
        vec![Effect::PushPlaylistChange {
            server: server.clone(),
            playlist: key,
            item: Some(item),
            change: PlaylistChange::Rename { name },
            sequence,
        }]
    }

    /// Deletes a playlist: one of Janis's own at once, a server's once the
    /// server has; it is out of sight meanwhile.
    pub fn delete(&mut self, key: PlaylistKey, server: Option<&MediaServer>) -> Vec<Effect> {
        let Some(place) = self.place_of(key).filter(|_| self.changeable(key, server)) else {
            return Vec::new();
        };
        let playlist = self.playlists.remove(place);
        let playlist_id = match key {
            PlaylistKey::Creating(creation) => {
                if let Some(made) = self.creations.get_mut(&creation) {
                    made.deleted = Some(playlist);
                }
                return Vec::new();
            }
            PlaylistKey::Saved(playlist_id) => playlist_id,
        };
        let Some((server, item)) = server_of(&playlist, server) else {
            return vec![Effect::RemovePlaylist { playlist_id }];
        };
        let server = server.clone();
        self.deleting.insert(playlist_id, playlist);
        let sequence = self.deletions.push(playlist_id, false, true, Some(place));
        vec![Effect::PushPlaylistChange {
            server,
            playlist: key,
            item: Some(item),
            change: PlaylistChange::Delete,
            sequence,
        }]
    }

    /// The server answered the push numbered `sequence`, which carried
    /// `change`; a creation's answer names the item the server made. Once a
    /// change's last push has answered, what the server holds is saved; when
    /// it is not what the listener asked for last, the playlist shows it
    /// again and the listener is told — but not of a refused token, which
    /// signing in again answers.
    pub fn push_finished(
        &mut self,
        key: PlaylistKey,
        change: PlaylistChange,
        sequence: u64,
        outcome: Result<Option<String>, Failure>,
    ) -> Vec<Effect> {
        if let Err(failure) = &outcome {
            log::warn!("change a playlist on the server: {failure:?}");
        }
        let took = outcome.is_ok();
        let token_rejected = matches!(outcome, Err(Failure::TokenRejected));
        let mut effects = Vec::new();
        let mut refused: Option<&'static str> = None;
        match (key, change) {
            (
                PlaylistKey::Creating(creation),
                PlaylistChange::Create {
                    name, track_ids, ..
                },
            ) => {
                let Some(made) = self.creations.get(&creation) else {
                    return Vec::new();
                };
                let server_id = made.server_id;
                match outcome.ok().flatten() {
                    Some(item) => {
                        let shown = self
                            .playlists
                            .iter_mut()
                            .find(|playlist| playlist.key == key);
                        let deleted = self
                            .creations
                            .get_mut(&creation)
                            .and_then(|made| made.deleted.as_mut());
                        for playlist in [shown, deleted].into_iter().flatten() {
                            if let Some(home) = playlist.server.as_mut() {
                                home.item = Some(item.clone());
                            }
                        }
                        effects.push(Effect::SavePlaylist {
                            creation,
                            name,
                            server: Some((server_id, item)),
                            track_ids,
                        });
                    }
                    None => {
                        self.creations.remove(&creation);
                        if let Some(place) = self.place_of(key) {
                            self.playlists.remove(place);
                        }
                        refused = Some("error.playlistCreate");
                    }
                }
            }
            (PlaylistKey::Saved(playlist_id), PlaylistChange::Add { track_id, .. }) => {
                refused =
                    self.entry_answered(playlist_id, track_id, true, sequence, took, &mut effects);
            }
            (PlaylistKey::Saved(playlist_id), PlaylistChange::Remove { track_id, .. }) => {
                refused =
                    self.entry_answered(playlist_id, track_id, false, sequence, took, &mut effects);
            }
            (PlaylistKey::Saved(playlist_id), PlaylistChange::Rename { name }) => {
                let Some(settled) = self.names.answer(&playlist_id, sequence, name, took) else {
                    return Vec::new();
                };
                if self
                    .find(key)
                    .is_some_and(|playlist| playlist.source != PlaylistSource::Jellyfin)
                {
                    return Vec::new();
                }
                effects.push(Effect::PersistPlaylistName {
                    playlist_id,
                    name: settled.server_value.clone(),
                });
                if !settled.as_wanted() {
                    if let Some(playlist) = self.find_mut(key) {
                        playlist.name = settled.server_value;
                    }
                    refused = Some("error.playlistRename");
                }
            }
            (PlaylistKey::Saved(playlist_id), PlaylistChange::Delete) => {
                let Some(settled) = self.deletions.answer(&playlist_id, sequence, false, took)
                else {
                    return Vec::new();
                };
                let playlist = self.deleting.remove(&playlist_id);
                if self
                    .find(key)
                    .is_some_and(|playlist| playlist.source != PlaylistSource::Jellyfin)
                {
                    return Vec::new();
                }
                if settled.server_value {
                    if let Some(playlist) = playlist {
                        let place = settled
                            .place
                            .unwrap_or(self.playlists.len())
                            .min(self.playlists.len());
                        self.playlists.insert(place, playlist);
                    }
                    refused = Some("error.playlistDelete");
                } else {
                    effects.push(Effect::RemovePlaylist { playlist_id });
                }
            }
            _ => return Vec::new(),
        }
        if let Some(key) = refused.filter(|_| !token_rejected) {
            effects.push(Effect::NotifyError {
                key,
                parameters: Vec::new(),
                dedupe_key: Some("jellyfin-playlist"),
            });
        }
        effects
    }

    /// Settles a song's place in a server's playlist once its last push
    /// has answered, and gives the message to tell when the server holds
    /// otherwise.
    fn entry_answered(
        &mut self,
        playlist_id: i64,
        track_id: i64,
        present: bool,
        sequence: u64,
        took: bool,
        effects: &mut Vec<Effect>,
    ) -> Option<&'static str> {
        let settled = self
            .entries
            .answer(&(playlist_id, track_id), sequence, present, took)?;
        if self
            .find(PlaylistKey::Saved(playlist_id))
            .is_some_and(|playlist| playlist.source != PlaylistSource::Jellyfin)
        {
            return None;
        }
        effects.push(Effect::PersistPlaylistEntry {
            playlist_id,
            track_id,
            present: settled.server_value,
        });
        if settled.as_wanted() {
            return None;
        }
        if let Some(playlist) = self.find_mut(PlaylistKey::Saved(playlist_id)) {
            set_entry(playlist, track_id, settled.server_value, settled.place);
        }
        Some(if settled.wanted {
            "error.playlistAdd"
        } else {
            "error.playlistRemove"
        })
    }

    /// The listener chose a picture for a playlist, now saved.
    pub fn picture_saved(&mut self, playlist_id: i64) {
        if let Some(playlist) = self.find_mut(PlaylistKey::Saved(playlist_id)) {
            playlist.picture = true;
        }
    }

    /// Forgets the picture the listener chose for a playlist.
    pub fn remove_picture(&mut self, playlist_id: i64) -> Vec<Effect> {
        match self.find_mut(PlaylistKey::Saved(playlist_id)) {
            Some(playlist) if playlist.picture => {
                playlist.picture = false;
                vec![Effect::RemovePlaylistPicture { playlist_id }]
            }
            _ => Vec::new(),
        }
    }

    /// Files a playlist in a group, or in none.
    pub fn set_group(&mut self, key: PlaylistKey, group_id: Option<i64>) -> Vec<Effect> {
        if group_id.is_some_and(|group_id| !self.groups.iter().any(|group| group.id == group_id)) {
            return Vec::new();
        }
        let Some(playlist) = self.find_mut(key) else {
            return Vec::new();
        };
        if playlist.group_id == group_id {
            return Vec::new();
        }
        playlist.group_id = group_id;
        match key.saved() {
            Some(playlist_id) => vec![Effect::PersistPlaylistGroup {
                playlist_id,
                group_id,
            }],
            // Filed once its row is saved.
            None => Vec::new(),
        }
    }

    /// Makes a group after the others, and files `then_file` in it once it
    /// is saved; a blank name makes none.
    pub fn create_group(&mut self, name: &str, then_file: Option<PlaylistKey>) -> Vec<Effect> {
        let name = name.trim();
        if name.is_empty() {
            return Vec::new();
        }
        vec![Effect::CreatePlaylistGroup {
            name: name.to_owned(),
            then_file,
        }]
    }

    /// The group was saved as row `group_id`.
    pub fn group_created(
        &mut self,
        group_id: i64,
        name: String,
        then_file: Option<PlaylistKey>,
    ) -> Vec<Effect> {
        self.groups.push(PlaylistGroup { id: group_id, name });
        match then_file {
            Some(key) => self.set_group(key, Some(group_id)),
            None => Vec::new(),
        }
    }

    pub fn rename_group(&mut self, group_id: i64, name: &str) -> Vec<Effect> {
        let name = name.trim();
        let Some(group) = self.groups.iter_mut().find(|group| group.id == group_id) else {
            return Vec::new();
        };
        if name.is_empty() || group.name == name {
            return Vec::new();
        }
        group.name = name.to_owned();
        vec![Effect::PersistPlaylistGroupName {
            group_id,
            name: name.to_owned(),
        }]
    }

    /// Removes a group; its playlists are then in none.
    pub fn remove_group(&mut self, group_id: i64) -> Vec<Effect> {
        let before = self.groups.len();
        self.groups.retain(|group| group.id != group_id);
        if self.groups.len() == before {
            return Vec::new();
        }
        for playlist in self
            .playlists
            .iter_mut()
            .chain(self.deleting.values_mut())
            .filter(|playlist| playlist.group_id == Some(group_id))
        {
            playlist.group_id = None;
        }
        vec![Effect::RemovePlaylistGroup { group_id }]
    }

    /// Moves a group one place earlier (`earlier`) or later.
    pub fn move_group(&mut self, group_id: i64, earlier: bool) -> Vec<Effect> {
        let Some(place) = self.groups.iter().position(|group| group.id == group_id) else {
            return Vec::new();
        };
        let other = if earlier {
            place.checked_sub(1)
        } else {
            Some(place + 1).filter(|other| *other < self.groups.len())
        };
        let Some(other) = other else {
            return Vec::new();
        };
        self.groups.swap(place, other);
        vec![Effect::PersistPlaylistGroupOrder {
            group_ids: self.groups.iter().map(|group| group.id).collect(),
        }]
    }

    /// Content of imported playlists is read-only. Local content is editable;
    /// Jellyfin changes require its signed-in server, except during creation.
    fn changeable(&self, key: PlaylistKey, server: Option<&MediaServer>) -> bool {
        self.find(key)
            .is_some_and(|playlist| match playlist.source {
                PlaylistSource::Local => true,
                PlaylistSource::Jellyfin => {
                    matches!(key, PlaylistKey::Creating(_)) || server_of(playlist, server).is_some()
                }
                #[cfg(feature = "youtube-music")]
                PlaylistSource::YouTubeMusic => false,
            })
    }

    fn place_of(&self, key: PlaylistKey) -> Option<usize> {
        self.playlists
            .iter()
            .position(|playlist| playlist.key == key)
    }

    fn find_mut(&mut self, key: PlaylistKey) -> Option<&mut Playlist> {
        self.playlists
            .iter_mut()
            .find(|playlist| playlist.key == key)
    }
}

/// The signed-in server a playlist lives on, and its item there; none for a
/// playlist of Janis's own.
fn server_of<'server>(
    playlist: &Playlist,
    server: Option<&'server MediaServer>,
) -> Option<(&'server MediaServer, String)> {
    if playlist.source != PlaylistSource::Jellyfin {
        return None;
    }
    let home = playlist.server.as_ref()?;
    let server = server.filter(|server| server.id == home.server_id)?;
    Some((server, home.item.clone()?))
}

/// Puts a song in a playlist — at `place`, or last — or takes it out.
fn set_entry(playlist: &mut Playlist, track_id: i64, present: bool, place: Option<usize>) {
    if present == playlist.holds(track_id) {
        return;
    }
    if present {
        let place = place
            .unwrap_or(playlist.track_ids.len())
            .min(playlist.track_ids.len());
        playlist.track_ids.insert(place, track_id);
    } else {
        playlist.track_ids.retain(|kept| *kept != track_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_support::track;

    fn server() -> MediaServer {
        MediaServer {
            id: 1,
            address: "http://jellyfin.local:8096".to_owned(),
            server_identifier: "server".to_owned(),
            server_name: "Home".to_owned(),
            user_identifier: "user".to_owned(),
            user_name: "listener".to_owned(),
            access_token: "token".to_owned(),
            last_sync_at: None,
        }
    }

    fn server_song(id: i64) -> Track {
        let mut song = track(id, "Song").server(1, &format!("item{id}"));
        song.path = format!("jellyfin://server/item{id}");
        song
    }

    fn local(id: i64, name: &str, track_ids: &[i64]) -> SavedPlaylist {
        SavedPlaylist {
            id,
            name: name.to_owned(),
            source: PlaylistSource::Local,
            server: None,
            server_picture: false,
            group_id: None,
            picture: false,
            track_ids: track_ids.to_vec(),
        }
    }

    fn on_server(id: i64, name: &str, track_ids: &[i64]) -> SavedPlaylist {
        SavedPlaylist {
            source: PlaylistSource::Jellyfin,
            server: Some((1, format!("list{id}"))),
            ..local(id, name, track_ids)
        }
    }

    fn with(saved: Vec<SavedPlaylist>) -> Playlists {
        let mut playlists = Playlists::default();
        playlists.restore(saved, Vec::new());
        playlists
    }

    fn songs_of(playlists: &Playlists, key: PlaylistKey) -> Vec<i64> {
        playlists
            .find(key)
            .map(|playlist| playlist.track_ids.clone())
            .unwrap_or_default()
    }

    fn unreachable<T>() -> Result<T, Failure> {
        Err(Failure::Unreachable("down".to_owned()))
    }

    fn told(key: &'static str) -> Effect {
        Effect::NotifyError {
            key,
            parameters: Vec::new(),
            dedupe_key: Some("jellyfin-playlist"),
        }
    }

    /// The one push among `effects`: its key, change and sequence.
    fn pushed(effects: &[Effect]) -> (PlaylistKey, PlaylistChange, u64) {
        match effects {
            [Effect::PushPlaylistChange {
                playlist,
                change,
                sequence,
                ..
            }] => (*playlist, change.clone(), *sequence),
            other => panic!("expected one push, got {other:?}"),
        }
    }

    #[test]
    fn a_playlist_made_from_a_file_is_janiss_own_and_saved_at_once() {
        let mut playlists = Playlists::default();
        let effects = playlists.create("  Road  ", &track(4, "Four"), Some(&server()));
        assert_eq!(
            effects,
            [Effect::SavePlaylist {
                creation: 1,
                name: "Road".to_owned(),
                server: None,
                track_ids: vec![4],
            }]
        );
        assert!(playlists
            .creation_saved(1, Some(10), Some(&server()))
            .is_empty());
        let playlist = playlists.find(PlaylistKey::Saved(10)).expect("saved");
        assert_eq!(playlist.name, "Road");
        assert_eq!(playlist.server, None);
        assert_eq!(playlist.track_ids, [4]);
        assert_eq!(playlists.playlists().len(), 1);
    }

    #[test]
    fn a_blank_name_makes_no_playlist() {
        let mut playlists = Playlists::default();
        assert!(playlists.create("   ", &track(4, "Four"), None).is_empty());
        assert!(playlists.playlists().is_empty());
    }

    #[test]
    fn a_playlist_made_from_a_server_song_is_made_on_the_server_then_saved() {
        let mut playlists = Playlists::default();
        let effects = playlists.create("Road", &server_song(5), Some(&server()));
        let (key, change, sequence) = pushed(&effects);
        assert_eq!(key, PlaylistKey::Creating(1));
        assert_eq!(
            change,
            PlaylistChange::Create {
                name: "Road".to_owned(),
                track_ids: vec![5],
                addresses: vec!["jellyfin://server/item5".to_owned()],
            }
        );
        assert_eq!(songs_of(&playlists, key), [5], "it shows at once");

        let effects = playlists.push_finished(key, change, sequence, Ok(Some("made".to_owned())));
        assert_eq!(
            effects,
            [Effect::SavePlaylist {
                creation: 1,
                name: "Road".to_owned(),
                server: Some((1, "made".to_owned())),
                track_ids: vec![5],
            }]
        );
        assert!(playlists
            .creation_saved(1, Some(10), Some(&server()))
            .is_empty());
        let playlist = playlists.find(PlaylistKey::Saved(10)).expect("saved");
        assert_eq!(
            playlist.server,
            Some(ServerHome {
                server_id: 1,
                item: Some("made".to_owned()),
            })
        );
        assert_eq!(playlists.playlists().len(), 1);
    }

    #[test]
    fn a_creation_the_server_refused_takes_the_playlist_away_and_tells_the_listener() {
        let mut playlists = Playlists::default();
        let (key, change, sequence) =
            pushed(&playlists.create("Road", &server_song(5), Some(&server())));
        let effects = playlists.push_finished(key, change, sequence, unreachable());
        assert_eq!(effects, [told("error.playlistCreate")]);
        assert!(playlists.playlists().is_empty());

        let (key, change, sequence) =
            pushed(&playlists.create("Road", &server_song(5), Some(&server())));
        let effects = playlists.push_finished(key, change, sequence, Err(Failure::TokenRejected));
        assert!(
            effects.is_empty(),
            "signing in again answers a refused token"
        );
        assert!(playlists.playlists().is_empty());
    }

    #[test]
    fn what_changed_while_the_server_made_a_playlist_is_sent_once_it_is_saved() {
        let mut playlists = Playlists::default();
        let (key, change, sequence) =
            pushed(&playlists.create("Road", &server_song(5), Some(&server())));
        assert!(playlists
            .set(key, &server_song(6), true, Some(&server()))
            .is_empty());
        assert!(playlists
            .set(key, &server_song(5), false, Some(&server()))
            .is_empty());
        assert!(playlists
            .rename(key, "Road trip", Some(&server()))
            .is_empty());
        assert_eq!(songs_of(&playlists, key), [6]);

        playlists.push_finished(key, change, sequence, Ok(Some("made".to_owned())));
        let effects = playlists.creation_saved(1, Some(10), Some(&server()));
        let changes: Vec<PlaylistChange> = effects
            .iter()
            .map(|effect| match effect {
                Effect::PushPlaylistChange { change, item, .. } => {
                    assert_eq!(item.as_deref(), Some("made"));
                    change.clone()
                }
                other => panic!("expected pushes, got {other:?}"),
            })
            .collect();
        assert_eq!(
            changes,
            [
                PlaylistChange::Rename {
                    name: "Road trip".to_owned()
                },
                PlaylistChange::Remove {
                    track_id: 5,
                    address: "jellyfin://server/item5".to_owned()
                },
                PlaylistChange::Add {
                    track_id: 6,
                    address: "jellyfin://server/item6".to_owned()
                },
            ]
        );
        let playlist = playlists.find(PlaylistKey::Saved(10)).expect("saved");
        assert_eq!(playlist.name, "Road trip");
        assert_eq!(playlist.track_ids, [6]);
    }

    #[test]
    fn a_playlist_deleted_while_the_server_made_it_is_deleted_once_it_is_saved() {
        let mut playlists = Playlists::default();
        let (key, change, sequence) =
            pushed(&playlists.create("Road", &server_song(5), Some(&server())));
        assert!(playlists.delete(key, Some(&server())).is_empty());
        assert!(playlists.playlists().is_empty());

        playlists.push_finished(key, change, sequence, Ok(Some("made".to_owned())));
        let effects = playlists.creation_saved(1, Some(10), Some(&server()));
        let (key, change, _) = pushed(&effects);
        assert_eq!(
            (key, change),
            (PlaylistKey::Saved(10), PlaylistChange::Delete)
        );
        assert!(playlists.playlists().is_empty());
    }

    #[test]
    fn a_new_playlist_a_sync_saved_first_shows_once() {
        let mut playlists = Playlists::default();
        let (key, change, sequence) =
            pushed(&playlists.create("Road", &server_song(5), Some(&server())));
        playlists.push_finished(key, change, sequence, Ok(Some("list10".to_owned())));
        // The sync read the server once it had made the playlist.
        playlists.restore(vec![on_server(10, "Road", &[5])], Vec::new());
        assert_eq!(playlists.playlists().len(), 2, "until its row is known");
        playlists.creation_saved(1, Some(10), Some(&server()));
        assert_eq!(playlists.playlists().len(), 1);
    }

    #[test]
    fn a_playlist_whose_server_left_before_it_was_saved_goes() {
        let mut playlists = Playlists::default();
        let (key, change, sequence) =
            pushed(&playlists.create("Road", &server_song(5), Some(&server())));
        playlists.push_finished(key, change, sequence, Ok(Some("made".to_owned())));
        assert!(playlists.creation_saved(1, None, None).is_empty());
        assert!(playlists.playlists().is_empty());
    }

    #[test]
    fn a_file_goes_last_in_a_playlist_of_janiss_own_once_and_is_saved_at_once() {
        let mut playlists = with(vec![local(1, "Mine", &[3])]);
        let key = PlaylistKey::Saved(1);
        assert_eq!(
            playlists.set(key, &track(4, "Four"), true, None),
            [Effect::PersistPlaylistEntry {
                playlist_id: 1,
                track_id: 4,
                present: true,
            }]
        );
        assert!(
            playlists.set(key, &track(4, "Four"), true, None).is_empty(),
            "a song is in a playlist once"
        );
        assert!(
            playlists
                .set(key, &server_song(5), true, Some(&server()))
                .is_empty(),
            "a server's song does not go in a playlist of Janis's own"
        );
        assert_eq!(songs_of(&playlists, key), [3, 4]);
        assert_eq!(
            playlists.set(key, &track(3, "Three"), false, None),
            [Effect::PersistPlaylistEntry {
                playlist_id: 1,
                track_id: 3,
                present: false,
            }]
        );
        assert_eq!(songs_of(&playlists, key), [4]);
    }

    #[test]
    fn a_song_put_in_a_servers_playlist_shows_at_once_and_is_saved_once_the_server_has_it() {
        let mut playlists = with(vec![on_server(1, "Road", &[5])]);
        let key = PlaylistKey::Saved(1);
        assert!(
            playlists.set(key, &track(4, "Four"), true, None).is_empty(),
            "a file does not go in a server's playlist"
        );
        let effects = playlists.set(key, &server_song(6), true, Some(&server()));
        let (_, change, sequence) = pushed(&effects);
        assert!(matches!(
            &effects[0],
            Effect::PushPlaylistChange { item: Some(item), .. } if item == "list1"
        ));
        assert_eq!(songs_of(&playlists, key), [5, 6]);
        assert_eq!(
            playlists.push_finished(key, change, sequence, Ok(None)),
            [Effect::PersistPlaylistEntry {
                playlist_id: 1,
                track_id: 6,
                present: true,
            }]
        );
        assert_eq!(songs_of(&playlists, key), [5, 6]);
    }

    #[test]
    fn a_song_the_server_would_not_take_out_goes_back_to_its_place_and_the_listener_is_told() {
        let mut playlists = with(vec![on_server(1, "Road", &[5, 6, 7])]);
        let key = PlaylistKey::Saved(1);
        let (_, change, sequence) =
            pushed(&playlists.set(key, &server_song(6), false, Some(&server())));
        assert_eq!(songs_of(&playlists, key), [5, 7]);
        let effects = playlists.push_finished(key, change, sequence, unreachable());
        assert_eq!(
            effects,
            [
                Effect::PersistPlaylistEntry {
                    playlist_id: 1,
                    track_id: 6,
                    present: true,
                },
                told("error.playlistRemove"),
            ]
        );
        assert_eq!(songs_of(&playlists, key), [5, 6, 7]);
    }

    #[test]
    fn a_song_the_server_would_not_take_leaves_the_playlist() {
        let mut playlists = with(vec![on_server(1, "Road", &[5])]);
        let key = PlaylistKey::Saved(1);
        let (_, change, sequence) =
            pushed(&playlists.set(key, &server_song(6), true, Some(&server())));
        let effects = playlists.push_finished(key, change, sequence, unreachable());
        assert_eq!(effects.last(), Some(&told("error.playlistAdd")));
        assert_eq!(songs_of(&playlists, key), [5]);
    }

    #[test]
    fn only_the_answer_to_a_songs_last_push_settles_it() {
        let mut playlists = with(vec![on_server(1, "Road", &[])]);
        let key = PlaylistKey::Saved(1);
        let first = pushed(&playlists.set(key, &server_song(6), true, Some(&server())));
        let second = pushed(&playlists.set(key, &server_song(6), false, Some(&server())));
        assert!(playlists
            .push_finished(key, first.1, first.2, Ok(None))
            .is_empty());
        assert!(songs_of(&playlists, key).is_empty());
        // The server took the song, then could not take it out: it still has
        // it, and so does Janis.
        let effects = playlists.push_finished(key, second.1, second.2, unreachable());
        assert_eq!(songs_of(&playlists, key), [6]);
        assert_eq!(effects.last(), Some(&told("error.playlistRemove")));
    }

    #[test]
    fn a_library_read_before_a_push_answers_keeps_what_the_listener_did() {
        let mut playlists = with(vec![on_server(1, "Road", &[5]), on_server(2, "Gym", &[])]);
        let road = PlaylistKey::Saved(1);
        playlists.set(road, &server_song(6), true, Some(&server()));
        playlists.rename(road, "Road trip", Some(&server()));
        playlists.delete(PlaylistKey::Saved(2), Some(&server()));
        playlists.create("New", &server_song(7), Some(&server()));

        playlists.restore(
            vec![on_server(1, "Road", &[5]), on_server(2, "Gym", &[])],
            Vec::new(),
        );
        let names: Vec<&str> = playlists
            .playlists()
            .iter()
            .map(|playlist| playlist.name.as_str())
            .collect();
        assert_eq!(names, ["Road trip", "New"]);
        assert_eq!(songs_of(&playlists, road), [5, 6]);
    }

    #[test]
    fn a_rename_is_saved_at_once_for_janiss_own_and_taken_back_when_the_server_refuses() {
        let mut playlists = with(vec![local(1, "Mine", &[]), on_server(2, "Road", &[])]);
        assert_eq!(
            playlists.rename(PlaylistKey::Saved(1), " Ours ", None),
            [Effect::PersistPlaylistName {
                playlist_id: 1,
                name: "Ours".to_owned(),
            }]
        );
        assert!(playlists
            .rename(PlaylistKey::Saved(1), "  ", None)
            .is_empty());

        let road = PlaylistKey::Saved(2);
        let (_, change, sequence) = pushed(&playlists.rename(road, "Road trip", Some(&server())));
        let effects = playlists.push_finished(road, change, sequence, unreachable());
        assert_eq!(
            effects,
            [
                Effect::PersistPlaylistName {
                    playlist_id: 2,
                    name: "Road".to_owned(),
                },
                told("error.playlistRename"),
            ]
        );
        assert_eq!(
            playlists.find(road).map(|playlist| playlist.name.as_str()),
            Some("Road")
        );
    }

    #[test]
    fn a_servers_playlist_deleted_hides_until_the_server_answers_and_comes_back_when_it_refuses() {
        let mut playlists = with(vec![
            local(1, "Mine", &[]),
            on_server(2, "Road", &[]),
            local(3, "Last", &[]),
        ]);
        assert_eq!(
            playlists.delete(PlaylistKey::Saved(1), None),
            [Effect::RemovePlaylist { playlist_id: 1 }]
        );
        let road = PlaylistKey::Saved(2);
        let (_, change, sequence) = pushed(&playlists.delete(road, Some(&server())));
        assert_eq!(playlists.playlists().len(), 1);
        let effects = playlists.push_finished(road, change.clone(), sequence, unreachable());
        assert_eq!(effects, [told("error.playlistDelete")]);
        let keys: Vec<PlaylistKey> = playlists
            .playlists()
            .iter()
            .map(|playlist| playlist.key)
            .collect();
        assert_eq!(keys, [road, PlaylistKey::Saved(3)], "back in its place");

        let (_, change, sequence) = pushed(&playlists.delete(road, Some(&server())));
        assert_eq!(
            playlists.push_finished(road, change, sequence, Ok(None)),
            [Effect::RemovePlaylist { playlist_id: 2 }]
        );
        assert_eq!(playlists.playlists().len(), 1);
    }

    #[test]
    fn a_servers_playlist_takes_no_change_while_its_server_is_not_signed_in_to() {
        let mut playlists = with(vec![on_server(1, "Road", &[5])]);
        let road = PlaylistKey::Saved(1);
        assert!(playlists.set(road, &server_song(5), false, None).is_empty());
        assert!(playlists.rename(road, "Road trip", None).is_empty());
        assert!(playlists.delete(road, None).is_empty());
        let playlist = playlists.find(road).expect("still there");
        assert_eq!(
            (playlist.name.as_str(), playlist.track_ids.as_slice()),
            ("Road", &[5][..])
        );
    }

    #[cfg(feature = "youtube-music")]
    #[test]
    fn imported_content_is_read_only_but_its_local_group_and_picture_are_editable() {
        let mut saved = local(1, "Imported", &[5]);
        saved.source = PlaylistSource::YouTubeMusic;
        let mut playlists = with(vec![saved]);
        let imported = PlaylistKey::Saved(1);
        let mut song = track(5, "YouTube song");
        song.source = TrackSource::YouTubeMusic;
        song.path = "youtube-music://abcdefghijk".to_owned();
        let before = playlists.clone();
        assert!(!playlists
            .find(imported)
            .expect("imported")
            .can_edit_content());
        assert!(playlists.accepting(&song, Some(&server())).is_empty());
        assert!(playlists
            .set(imported, &song, false, Some(&server()))
            .is_empty());
        assert!(playlists
            .set(imported, &track(6, "File"), true, None)
            .is_empty());
        assert!(playlists
            .rename(imported, "Changed", Some(&server()))
            .is_empty());
        assert!(playlists.delete(imported, Some(&server())).is_empty());
        assert_eq!(playlists, before);

        playlists.group_created(7, "Moods".to_owned(), None);
        assert_eq!(
            playlists.set_group(imported, Some(7)),
            [Effect::PersistPlaylistGroup {
                playlist_id: 1,
                group_id: Some(7)
            }]
        );
        playlists.picture_saved(1);
        assert!(playlists.find(imported).expect("imported").picture);
        assert_eq!(
            playlists.remove_picture(1),
            [Effect::RemovePlaylistPicture { playlist_id: 1 }]
        );
        assert_eq!(
            playlists.find(imported).expect("imported").group_id,
            Some(7)
        );
    }

    #[cfg(feature = "youtube-music")]
    #[test]
    fn imported_membership_refresh_preserves_every_other_playlist_choice() {
        let mut imported = local(1, "Imported", &[5]);
        imported.source = PlaylistSource::YouTubeMusic;
        let mut untouched = local(4, "Other import", &[9]);
        untouched.source = PlaylistSource::YouTubeMusic;
        let mut playlists = with(vec![
            imported,
            local(2, "Mine", &[6]),
            on_server(3, "Server", &[7]),
            untouched,
        ]);
        playlists.group_created(8, "Moods".to_owned(), None);
        playlists.set_group(PlaylistKey::Saved(1), Some(8));
        playlists.picture_saved(1);
        playlists.rename(PlaylistKey::Saved(3), "Pending name", Some(&server()));
        playlists.create("Creating", &track(10, "File"), None);
        let mut expected = playlists.clone();
        expected
            .find_mut(PlaylistKey::Saved(1))
            .expect("imported playlist")
            .track_ids = vec![11, 12];
        playlists.restore_youtube_memberships(vec![
            (1, vec![11, 12]),
            (2, vec![]),
            (3, vec![]),
            (99, vec![13]),
        ]);
        assert_eq!(playlists, expected);
        playlists.restore_youtube_memberships(vec![(1, vec![])]);
        assert!(playlists
            .find(PlaylistKey::Saved(1))
            .expect("empty imported playlist")
            .track_ids
            .is_empty());
    }

    #[cfg(feature = "youtube-music")]
    #[test]
    fn local_playlists_mix_persistent_files_and_youtube_music_but_reject_anonymous_songs() {
        let mut playlists = with(vec![local(1, "Mixed", &[5]), on_server(2, "Server", &[])]);
        let mixed = PlaylistKey::Saved(1);
        let mut remote = track(6, "Saved YouTube Music song");
        remote.source = TrackSource::YouTubeMusic;
        assert_eq!(playlists.accepting(&remote, Some(&server())).len(), 1);
        assert_eq!(
            playlists.set(mixed, &remote, true, None),
            [Effect::PersistPlaylistEntry {
                playlist_id: 1,
                track_id: 6,
                present: true,
            }]
        );
        assert_eq!(songs_of(&playlists, mixed), [5, 6]);
        assert!(playlists
            .set(PlaylistKey::Saved(2), &remote, true, Some(&server()))
            .is_empty());
        assert!(playlists
            .set(mixed, &server_song(7), true, Some(&server()))
            .is_empty());
        let effects = playlists.create("Saved songs", &remote, None);
        assert!(matches!(
            effects.as_slice(),
            [Effect::SavePlaylist { server: None, track_ids, .. }] if track_ids == &[6]
        ));
        let creation = match playlists.playlists().last().expect("created playlist").key {
            PlaylistKey::Creating(creation) => creation,
            PlaylistKey::Saved(_) => panic!("new playlist awaits persistence"),
        };
        playlists.creation_saved(creation, Some(3), None);
        let created = PlaylistKey::Saved(3);
        assert_eq!(
            playlists.set(created, &track(8, "File"), true, None),
            [Effect::PersistPlaylistEntry {
                playlist_id: 3,
                track_id: 8,
                present: true,
            }]
        );
        assert_eq!(songs_of(&playlists, created), [6, 8]);
        for source in [TrackSource::File, TrackSource::YouTubeMusic] {
            let mut transient = track(-1, "Anonymous catalog song");
            transient.source = source;
            assert!(playlists.accepting(&transient, None).is_empty());
            assert!(playlists.set(mixed, &transient, true, None).is_empty());
            assert!(playlists
                .create("Not persisted", &transient, None)
                .is_empty());
        }
        assert_eq!(songs_of(&playlists, mixed), [5, 6]);
    }

    #[cfg(feature = "youtube-music")]
    #[test]
    fn imported_playlists_never_inherit_a_reused_rows_pending_jellyfin_changes() {
        for action in ["rename", "remove", "delete"] {
            let mut playlists = with(vec![on_server(1, "Server playlist", &[5])]);
            let key = PlaylistKey::Saved(1);
            let effects = match action {
                "rename" => playlists.rename(key, "Pending name", Some(&server())),
                "remove" => playlists.set(key, &server_song(5), false, Some(&server())),
                "delete" => playlists.delete(key, Some(&server())),
                _ => unreachable!("the test enumerates its actions"),
            };
            let (key, change, sequence) = pushed(&effects);
            let mut imported = local(1, "Imported", &[5]);
            imported.source = PlaylistSource::YouTubeMusic;
            playlists.restore(vec![imported.clone()], Vec::new());
            assert_eq!(
                playlists.playlists(),
                [Playlist::from_saved(imported.clone())],
                "{action}: pending content does not override a new provider"
            );
            assert!(playlists
                .push_finished(key, change, sequence, unreachable())
                .is_empty());
            assert_eq!(
                playlists.playlists(),
                [Playlist::from_saved(imported)],
                "{action}: a late failure does not roll back imported content"
            );
        }
    }

    #[test]
    fn a_song_is_offered_the_playlists_that_can_take_it_by_name() {
        let playlists = with(vec![
            local(1, "zebra", &[]),
            on_server(2, "Road", &[5]),
            local(3, "Apple", &[4]),
        ]);
        let names = |track: &Track, server: Option<&MediaServer>| -> Vec<(String, bool)> {
            playlists
                .accepting(track, server)
                .into_iter()
                .map(|playlist| (playlist.name.clone(), playlist.holds(track.id)))
                .collect()
        };
        assert_eq!(
            names(&track(4, "Four"), Some(&server())),
            [("Apple".to_owned(), true), ("zebra".to_owned(), false)]
        );
        assert_eq!(
            names(&server_song(5), Some(&server())),
            [("Road".to_owned(), true)]
        );
        assert!(
            names(&server_song(5), None).is_empty(),
            "nor while signed out of its server"
        );
    }

    #[test]
    fn groups_are_made_named_ordered_and_removed_and_their_playlists_filed() {
        let mut playlists = with(vec![local(1, "Mine", &[])]);
        assert_eq!(
            playlists.create_group(" Moods ", Some(PlaylistKey::Saved(1))),
            [Effect::CreatePlaylistGroup {
                name: "Moods".to_owned(),
                then_file: Some(PlaylistKey::Saved(1)),
            }]
        );
        assert!(playlists.create_group(" ", None).is_empty());
        assert_eq!(
            playlists.group_created(7, "Moods".to_owned(), Some(PlaylistKey::Saved(1))),
            [Effect::PersistPlaylistGroup {
                playlist_id: 1,
                group_id: Some(7),
            }]
        );
        playlists.group_created(8, "Sport".to_owned(), None);
        assert!(
            playlists
                .set_group(PlaylistKey::Saved(1), Some(99))
                .is_empty(),
            "no group 99"
        );
        assert_eq!(
            playlists.move_group(8, true),
            [Effect::PersistPlaylistGroupOrder {
                group_ids: vec![8, 7],
            }]
        );
        assert!(playlists.move_group(8, true).is_empty(), "already first");
        assert_eq!(
            playlists.rename_group(7, "Feelings"),
            [Effect::PersistPlaylistGroupName {
                group_id: 7,
                name: "Feelings".to_owned(),
            }]
        );
        assert_eq!(
            playlists.remove_group(7),
            [Effect::RemovePlaylistGroup { group_id: 7 }]
        );
        assert_eq!(
            playlists
                .find(PlaylistKey::Saved(1))
                .and_then(|playlist| playlist.group_id),
            None
        );
        let names: Vec<&str> = playlists
            .groups()
            .iter()
            .map(|group| group.name.as_str())
            .collect();
        assert_eq!(names, ["Sport"]);
    }

    #[test]
    fn a_chosen_picture_is_kept_until_the_listener_removes_it() {
        let mut playlists = with(vec![local(1, "Mine", &[])]);
        assert!(playlists.remove_picture(1).is_empty(), "there is none yet");
        playlists.picture_saved(1);
        assert!(playlists
            .find(PlaylistKey::Saved(1))
            .is_some_and(|playlist| playlist.picture));
        assert_eq!(
            playlists.remove_picture(1),
            [Effect::RemovePlaylistPicture { playlist_id: 1 }]
        );
    }
}
