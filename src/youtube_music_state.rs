//! Anonymous YouTube Music search and stale-result rejection.
//! The controller performs every read; this module only decides and holds data.

use std::collections::HashMap;

use crate::effects::{Effect, YouTubeMusicRequest};
use crate::library::{Track, TrackSource};
use crate::youtube_music::{Failure, Song, SongContinuation, SongPage};

#[derive(Clone, Debug)]
pub struct SongRow {
    pub song: Song,
    pub track: Track,
}

pub struct YouTubeMusicState {
    pub query: String,
    pub loading: bool,
    pub failed: bool,
    pub status_key: &'static str,
    pub songs: Vec<SongRow>,
    pub next_songs: Option<SongContinuation>,
    epoch: u64,
    registered: HashMap<String, i64>,
    cover_urls: HashMap<i64, String>,
    next_identifier: i32,
}

impl Default for YouTubeMusicState {
    fn default() -> Self {
        Self {
            query: String::new(),
            loading: false,
            failed: false,
            status_key: "",
            songs: Vec::new(),
            next_songs: None,
            epoch: 0,
            registered: HashMap::new(),
            cover_urls: HashMap::new(),
            next_identifier: -2,
        }
    }
}

impl YouTubeMusicState {
    pub fn is_current(&self, epoch: u64) -> bool {
        self.epoch == epoch
    }

    pub fn search(&mut self, query: &str) -> Vec<Effect> {
        self.query = query.trim().to_owned();
        if self.query.is_empty() {
            self.epoch += 1;
            self.loading = false;
            self.failed = false;
            self.status_key = "";
            self.songs.clear();
            self.next_songs = None;
            return Vec::new();
        }
        self.begin(YouTubeMusicRequest::Search(self.query.clone()), false)
    }

    pub fn more(&mut self) -> Vec<Effect> {
        if self.loading || self.next_songs.is_none() {
            return Vec::new();
        }
        self.begin(YouTubeMusicRequest::MoreSongs, true)
    }

    pub fn has_more(&self) -> bool {
        self.next_songs.is_some()
    }

    fn begin(&mut self, request: YouTubeMusicRequest, append: bool) -> Vec<Effect> {
        self.epoch += 1;
        self.loading = true;
        self.failed = false;
        self.status_key = "youtubeMusic.status.loading";
        if !append {
            self.next_songs = None;
            self.songs.clear();
        }
        vec![Effect::ReadYouTubeMusic {
            request,
            epoch: self.epoch,
        }]
    }

    pub fn songs_finished(
        &mut self,
        epoch: u64,
        outcome: Result<SongPage, Failure>,
        append: bool,
    ) -> Vec<Effect> {
        if !self.is_current(epoch) || !self.loading {
            return Vec::new();
        }
        self.loading = false;
        match outcome {
            Ok(page) => {
                if !append {
                    self.songs.clear();
                }
                for song in page.songs {
                    let row = self.register(song);
                    if !self
                        .songs
                        .iter()
                        .any(|shown| shown.track.id == row.track.id)
                    {
                        self.songs.push(row);
                    }
                }
                self.next_songs = page.next;
                self.failed = false;
                self.status_key = "";
                Vec::new()
            }
            Err(failure) => {
                self.failed = true;
                self.status_key = "youtubeMusic.status.failed";
                vec![Effect::NotifyError {
                    key: failure.translation_key(),
                    parameters: Vec::new(),
                    dedupe_key: Some("youtube-music-search"),
                }]
            }
        }
    }

    fn allocate_identifier(&mut self) -> i64 {
        // Negative ids belong only to transient source rows, not SQLite tracks.
        let identifier = self.next_identifier;
        self.next_identifier = identifier
            .checked_sub(1)
            .expect("transient track identifiers fit the interface");
        i64::from(identifier)
    }

    fn register(&mut self, song: Song) -> SongRow {
        let identifier = self
            .registered
            .get(&song.identifier)
            .copied()
            .unwrap_or_else(|| self.allocate_identifier());
        self.registered.insert(song.identifier.clone(), identifier);
        if let Some(address) = &song.artwork_url {
            self.cover_urls.insert(identifier, address.clone());
        } else {
            self.cover_urls.remove(&identifier);
        }
        SongRow {
            track: song_track(&song, identifier),
            song,
        }
    }

    pub fn cover_url(&self, track_id: i64) -> Option<&str> {
        self.cover_urls.get(&track_id).map(String::as_str)
    }

    pub fn queue(&self) -> Vec<Track> {
        self.songs.iter().map(|row| row.track.clone()).collect()
    }

    pub fn visible_track_id(&self, index: usize) -> Option<i64> {
        self.songs.get(index).map(|row| row.track.id)
    }

    pub fn visible_length(&self) -> usize {
        self.songs.len()
    }
}

/// Transient source songs use a stable provider address, never a signed media URL.
pub fn song_track(song: &Song, identifier: i64) -> Track {
    Track {
        id: identifier,
        source: TrackSource::YouTubeMusic,
        folder_id: None,
        server_id: None,
        artwork_item: None,
        artwork_address: song.artwork_url.clone(),
        server_libraries: Vec::new(),
        path: format!("youtube-music://{}", song.identifier),
        title: song.title.clone(),
        artist: (!song.artist.is_empty()).then(|| song.artist.clone()),
        album: song.album.clone(),
        composer: None,
        duration_secs: f64::from(song.duration_seconds),
        format: String::new(),
        sample_rate: None,
        bit_depth: None,
        channels: None,
        lossless: false,
        added_at: 0,
        track_number: None,
        track_total: None,
        disc_number: None,
        disc_total: None,
        year: None,
        genre: None,
        album_artist: None,
        gain_db: 0.0,
    }
}

pub fn video_identifier(track: &Track) -> Option<&str> {
    (track.source == TrackSource::YouTubeMusic)
        .then(|| track.path.strip_prefix("youtube-music://"))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(identifier: &str) -> Song {
        Song {
            identifier: identifier.into(),
            title: "Song".into(),
            artist: "Artist".into(),
            album: None,
            duration_seconds: 100,
            artwork_url: Some("https://i.ytimg.com/song.jpg".into()),
        }
    }

    fn epoch(effects: &[Effect]) -> u64 {
        match &effects[0] {
            Effect::ReadYouTubeMusic { epoch, .. } => *epoch,
            _ => panic!("a search emits a read effect"),
        }
    }

    fn page(identifiers: &[&str]) -> SongPage {
        SongPage {
            songs: identifiers
                .iter()
                .map(|identifier| song(identifier))
                .collect(),
            next: None,
        }
    }

    #[test]
    fn an_old_search_cannot_replace_a_new_query() {
        let mut state = YouTubeMusicState::default();
        let old = epoch(&state.search("old"));
        let current = epoch(&state.search("new"));
        assert!(state
            .songs_finished(old, Ok(page(&["old"])), false)
            .is_empty());
        assert!(state.songs.is_empty());
        assert!(state.loading);
        state.songs_finished(current, Ok(page(&["new"])), false);
        assert_eq!(state.songs[0].song.identifier, "new");
        assert!(!state.loading);
        assert_eq!(state.status_key, "");
    }

    #[test]
    fn duplicate_page_songs_keep_one_stable_negative_identity() {
        let mut state = YouTubeMusicState::default();
        let request = epoch(&state.search("song"));
        state.songs_finished(request, Ok(page(&["one", "one"])), false);
        let identifier = state.songs[0].track.id;
        assert_eq!(identifier, -2);
        assert_eq!(state.songs.len(), 1);
        let request = epoch(&state.search("again"));
        state.songs_finished(request, Ok(page(&["one", "two"])), false);
        assert_eq!(state.songs[0].track.id, identifier);
        assert!(state.songs[1].track.id < identifier);
        assert_eq!(state.songs[0].track.path, "youtube-music://one");
        assert_eq!(state.songs[0].track.source, TrackSource::YouTubeMusic);
        assert_eq!(
            state.cover_url(identifier),
            state.songs[0].track.artwork_address.as_deref()
        );
        assert_eq!(state.queue().len(), 2);
        assert_eq!(state.visible_track_id(1), Some(state.songs[1].track.id));
        assert_eq!(state.visible_length(), 2);
    }

    #[test]
    fn clearing_a_query_invalidates_its_pending_read() {
        let mut state = YouTubeMusicState::default();
        let old = epoch(&state.search("pending"));
        assert!(state.search("  ").is_empty());
        state.songs_finished(old, Ok(page(&["one"])), false);
        assert!(state.songs.is_empty());
        assert_eq!(state.query, "");
        assert!(!state.loading);
        assert!(!state.failed);
        assert!(!state.has_more());
        assert!(state.more().is_empty());
    }

    #[test]
    fn a_failed_page_keeps_shown_songs_and_notifies_without_account_effects() {
        let mut state = YouTubeMusicState::default();
        let request = epoch(&state.search("song"));
        state.songs_finished(request, Ok(page(&["one"])), false);
        let request = epoch(&state.begin(YouTubeMusicRequest::MoreSongs, true));
        assert_eq!(
            state.songs_finished(request, Err(Failure::Network), true),
            vec![Effect::NotifyError {
                key: "youtubeMusic.error.network",
                parameters: Vec::new(),
                dedupe_key: Some("youtube-music-search"),
            }]
        );
        assert_eq!(state.songs[0].song.identifier, "one");
        assert!(state.failed);
        assert!(!state.loading);
        assert!(state
            .songs_finished(request, Ok(page(&["late"])), true)
            .is_empty());
        assert_eq!(state.songs.len(), 1);
    }

    #[test]
    fn only_youtube_music_tracks_expose_a_video_identifier() {
        let mut track = song_track(&song("dQw4w9WgXcQ"), -2);
        assert_eq!(video_identifier(&track), Some("dQw4w9WgXcQ"));
        track.source = TrackSource::File;
        assert_eq!(video_identifier(&track), None);
    }
}
