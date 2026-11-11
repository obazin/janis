//! What the screens show of the track library: albums and artists grouped
//! from the flat track list, the search over them, and the lists Now Playing
//! builds around the current track. Pure functions over [`Track`]s — the
//! database stays in `library`.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::library::Track;
use crate::text::{compare_labels, format_time, label_key};
use crate::translations::Translations;

/// Rows in the Library screen's "Recently added" list.
pub const RECENT_TRACK_COUNT: usize = 8;
/// Tiles on each Discover shelf.
pub const SHELF_TILE_COUNT: usize = 8;
/// Tracks and albums the Now Playing artist spotlight shows.
pub const SPOTLIGHT_COUNT: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct AlbumGroup {
    pub key: String,
    pub album: Option<String>,
    pub artist: Option<String>,
    /// In album order: disc, then track, then title for untagged files.
    pub tracks: Vec<Track>,
    pub newest_added_at: i64,
    pub year: Option<u32>,
    pub genre: Option<String>,
    /// Combined runtime, for the album's detail line.
    pub duration_seconds: f64,
    /// True once every track carries a position, so the order is trustworthy.
    pub ordered: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtistGroup {
    pub artist: String,
    pub tracks: Vec<Track>,
}

/// Album running order. Untagged tracks sort last rather than to the front,
/// where a missing number would otherwise read as track zero.
pub fn compare_in_album(left: &Track, right: &Track) -> Ordering {
    left.disc_number
        .unwrap_or(1)
        .cmp(&right.disc_number.unwrap_or(1))
        .then_with(|| {
            left.track_number
                .unwrap_or(u32::MAX)
                .cmp(&right.track_number.unwrap_or(u32::MAX))
        })
        .then_with(|| compare_labels(&left.title, &right.title))
}

/// Albums, newest addition first, each in its own running order.
///
/// Grouped by album + album artist so a compilation stays one album instead
/// of splintering into one entry per featured artist.
pub fn albums(tracks: &[Track]) -> Vec<AlbumGroup> {
    let mut groups: Vec<AlbumGroup> = Vec::new();
    let mut positions: HashMap<String, usize> = HashMap::new();
    for track in tracks {
        let artist = track.album_artist.clone().or_else(|| track.artist.clone());
        let key = format!(
            "{}::{}",
            track.album.as_deref().unwrap_or(""),
            artist.as_deref().unwrap_or("")
        );
        let position = *positions.entry(key.clone()).or_insert_with(|| {
            groups.push(AlbumGroup {
                key,
                album: track.album.clone(),
                artist,
                tracks: Vec::new(),
                newest_added_at: 0,
                year: None,
                genre: None,
                duration_seconds: 0.0,
                ordered: true,
            });
            groups.len() - 1
        });
        let group = &mut groups[position];
        group.newest_added_at = group.newest_added_at.max(track.added_at);
        group.duration_seconds += track.duration_secs;
        group.year = group.year.or(track.year);
        if group.genre.is_none() {
            group.genre.clone_from(&track.genre);
        }
        if track.track_number.is_none() {
            group.ordered = false;
        }
        group.tracks.push(track.clone());
    }
    for group in &mut groups {
        group.tracks.sort_by(compare_in_album);
    }
    groups.sort_by_key(|group| std::cmp::Reverse(group.newest_added_at));
    groups
}

/// Artists with their tracks, most tracks first.
pub fn artists(tracks: &[Track]) -> Vec<ArtistGroup> {
    let mut groups: Vec<ArtistGroup> = Vec::new();
    let mut positions: HashMap<&str, usize> = HashMap::new();
    for track in tracks {
        let Some(artist) = track.artist.as_deref().filter(|artist| !artist.is_empty()) else {
            continue;
        };
        let position = *positions.entry(artist).or_insert_with(|| {
            groups.push(ArtistGroup {
                artist: artist.to_owned(),
                tracks: Vec::new(),
            });
            groups.len() - 1
        });
        groups[position].tracks.push(track.clone());
    }
    groups.sort_by_key(|group| std::cmp::Reverse(group.tracks.len()));
    groups
}

/// The search field's text as the filters compare it.
pub fn normalized_query(search: &str) -> String {
    search.trim().to_lowercase()
}

/// The tracks the Library screen's search keeps, in the order given: any of
/// title, artist, album, composer, album artist or genre containing the
/// query.
pub fn search_library(tracks: &[Track], search: &str) -> Vec<Track> {
    let query = normalized_query(search);
    tracks
        .iter()
        .filter(|track| library_match(track, &query))
        .cloned()
        .collect()
}

/// Whether the Library search keeps `track`; `query` is normalised.
fn library_match(track: &Track, query: &str) -> bool {
    query.is_empty()
        || [
            Some(&track.title),
            track.artist.as_ref(),
            track.album.as_ref(),
            track.composer.as_ref(),
            track.album_artist.as_ref(),
            track.genre.as_ref(),
        ]
        .into_iter()
        .flatten()
        .any(|field| field.to_lowercase().contains(query))
}

/// The order of the Library tabs: songs by title, albums by name, artists by
/// name, alphabetically as a reader expects (`text::compare_labels`). Kept as
/// positions into the library's tracks, albums and artists, worked out once
/// per library, so a search only filters. Discover, Local Files, the
/// "Recently added" rows and Now Playing keep the library's own order, newest
/// addition first.
#[derive(Debug, Default)]
pub struct LibraryOrder {
    songs: Vec<usize>,
    albums: Vec<usize>,
    artists: Vec<usize>,
}

impl LibraryOrder {
    pub fn new(tracks: &[Track], albums: &[AlbumGroup], artists: &[ArtistGroup]) -> Self {
        Self {
            // Songs of the same name, then by artist and album.
            songs: sorted_positions(tracks, |track| {
                (
                    label_key(&track.title),
                    missing_last(track.artist.as_deref()),
                    missing_last(track.album.as_deref()),
                )
            }),
            // Albums of the same name, by artist; untitled albums last.
            albums: sorted_positions(albums, |album| {
                (
                    missing_last(album.album.as_deref()),
                    missing_last(album.artist.as_deref()),
                )
            }),
            artists: sorted_positions(artists, |artist| label_key(&artist.artist)),
        }
    }
}

/// The positions of `items` in the order of their keys, each key worked out
/// once. Equal keys keep the items' own order.
fn sorted_positions<Item, Key: Ord>(items: &[Item], key: impl Fn(&Item) -> Key) -> Vec<usize> {
    let mut positions: Vec<usize> = (0..items.len()).collect();
    positions.sort_by_cached_key(|&position| key(&items[position]));
    positions
}

/// A label's sort key, an absent or blank label after every present one.
fn missing_last(label: Option<&str>) -> (bool, (String, String)) {
    match label.filter(|label| !label.trim().is_empty()) {
        Some(label) => (false, label_key(label)),
        None => (true, Default::default()),
    }
}

/// The Songs tab: the tracks the search keeps, in alphabetical order.
pub fn library_songs(tracks: &[Track], order: &LibraryOrder, search: &str) -> Vec<Track> {
    let query = normalized_query(search);
    order
        .songs
        .iter()
        .filter_map(|&position| tracks.get(position))
        .filter(|track| library_match(track, &query))
        .cloned()
        .collect()
}

/// The "Recently added" rows: the newest additions the search keeps.
pub fn recently_added(tracks: &[Track], search: &str) -> Vec<Track> {
    let query = normalized_query(search);
    tracks
        .iter()
        .filter(|track| library_match(track, &query))
        .take(RECENT_TRACK_COUNT)
        .cloned()
        .collect()
}

/// The tracks the Local Files screen's search keeps: files on disk whose
/// title or artist matches. A media server's songs are in the library, not
/// among the local files.
pub fn search_local_files(tracks: &[Track], search: &str) -> Vec<Track> {
    let query = normalized_query(search);
    tracks
        .iter()
        .filter(|track| track.server_id.is_none())
        .filter(|track| {
            query.is_empty()
                || track.title.to_lowercase().contains(&query)
                || track
                    .artist
                    .as_ref()
                    .is_some_and(|artist| artist.to_lowercase().contains(&query))
        })
        .cloned()
        .collect()
}

/// The albums with at least one track the search kept, in alphabetical order.
pub fn albums_matching<'groups>(
    groups: &'groups [AlbumGroup],
    order: &LibraryOrder,
    kept: &[Track],
) -> Vec<&'groups AlbumGroup> {
    let kept: HashSet<i64> = kept.iter().map(|track| track.id).collect();
    order
        .albums
        .iter()
        .filter_map(|&position| groups.get(position))
        .filter(|group| group.tracks.iter().any(|track| kept.contains(&track.id)))
        .collect()
}

/// The artists with at least one track the search kept, in alphabetical order.
pub fn artists_matching<'groups>(
    groups: &'groups [ArtistGroup],
    order: &LibraryOrder,
    kept: &[Track],
) -> Vec<&'groups ArtistGroup> {
    let kept: HashSet<i64> = kept.iter().map(|track| track.id).collect();
    order
        .artists
        .iter()
        .filter_map(|&position| groups.get(position))
        .filter(|group| group.tracks.iter().any(|track| kept.contains(&track.id)))
        .collect()
}

/// Year · track count · runtime, dropping whatever the tags did not say.
pub fn album_detail(album: &AlbumGroup, translations: &Translations) -> String {
    let mut parts = Vec::with_capacity(3);
    if let Some(year) = album.year.filter(|year| *year != 0) {
        parts.push(year.to_string());
    }
    parts.push(translations.translate(
        "library.tracks",
        &[("count", album.tracks.len().to_string())],
    ));
    parts.push(format_time(album.duration_seconds));
    parts.join(" · ")
}

/// The Discover shelf of albums worth another listen: the oldest additions.
pub fn rediscover(groups: &[AlbumGroup]) -> Vec<&AlbumGroup> {
    groups.iter().rev().take(SHELF_TILE_COUNT).collect()
}

/// The current track's album in running order, or the track alone when the
/// library no longer has its album. Matched on album artist, which is how
/// albums are keyed — on a compilation the track's own artist is not the
/// album's.
pub fn album_tracks_of(groups: &[AlbumGroup], track: &Track) -> Vec<Track> {
    let artist = track.album_artist.as_ref().or(track.artist.as_ref());
    groups
        .iter()
        .find(|group| group.album == track.album && group.artist.as_ref() == artist)
        .map(|group| group.tracks.clone())
        .unwrap_or_else(|| vec![track.clone()])
}

/// The tracks the artist spotlight shows around `track`: the first few of
/// its artist.
pub fn spotlight_tracks(groups: &[ArtistGroup], track: &Track) -> Vec<Track> {
    artist_group_of(groups, track)
        .map(|group| group.tracks.iter().take(SPOTLIGHT_COUNT).cloned().collect())
        .unwrap_or_default()
}

/// The albums the artist spotlight shows around `track`: the first few
/// under its artist's name.
pub fn spotlight_albums(groups: &[AlbumGroup], track: &Track) -> Vec<AlbumGroup> {
    let Some(artist) = track.artist.as_ref() else {
        return Vec::new();
    };
    groups
        .iter()
        .filter(|album| album.artist.as_ref() == Some(artist))
        .take(SPOTLIGHT_COUNT)
        .cloned()
        .collect()
}

/// What a click on a spotlight track plays: everything by its artist,
/// starting from the chosen track — or the track alone when the library no
/// longer has its artist.
pub fn artist_queue_from(groups: &[ArtistGroup], chosen: &Track) -> (Vec<Track>, usize) {
    let tracks = artist_group_of(groups, chosen)
        .map(|group| group.tracks.clone())
        .unwrap_or_else(|| vec![chosen.clone()]);
    let start = tracks
        .iter()
        .position(|track| track.id == chosen.id)
        .unwrap_or_default();
    (tracks, start)
}

fn artist_group_of<'groups>(
    groups: &'groups [ArtistGroup],
    track: &Track,
) -> Option<&'groups ArtistGroup> {
    let artist = track.artist.as_ref()?;
    groups.iter().find(|group| &group.artist == artist)
}

/// Position on the record and its year — whatever the tags gave us.
pub fn album_line(track: &Track, translations: &Translations) -> Option<String> {
    let mut parts = Vec::with_capacity(3);
    if let Some(number) = track.track_number {
        parts.push(match track.track_total {
            Some(total) => translations.translate(
                "now.trackOf",
                &[("n", number.to_string()), ("total", total.to_string())],
            ),
            None => translations.translate("now.trackNo", &[("n", number.to_string())]),
        });
    }
    if let (Some(total), Some(number)) = (track.disc_total, track.disc_number) {
        if total > 1 {
            parts.push(translations.translate(
                "now.discOf",
                &[("n", number.to_string()), ("total", total.to_string())],
            ));
        }
    }
    if let Some(year) = track.year {
        parts.push(year.to_string());
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The seed the generated art of whatever is playing derives its colors from.
pub fn art_seed(track: &Track) -> String {
    format!(
        "{}-{}",
        track.artist.as_deref().unwrap_or(""),
        track.album.as_deref().unwrap_or(&track.title)
    )
}

/// The tabs of the Library screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LibraryTab {
    Playlists,
    Artists,
    #[default]
    Albums,
    Songs,
}

impl LibraryTab {
    /// The name the interface knows the tab by.
    pub fn identifier(self) -> &'static str {
        match self {
            LibraryTab::Playlists => "playlists",
            LibraryTab::Artists => "artists",
            LibraryTab::Albums => "albums",
            LibraryTab::Songs => "songs",
        }
    }

    pub fn from_identifier(identifier: &str) -> Option<Self> {
        [Self::Playlists, Self::Artists, Self::Albums, Self::Songs]
            .into_iter()
            .find(|tab| tab.identifier() == identifier)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    Album,
    Artist,
}

/// The album or artist browsed in the Library grid instead of played.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub kind: SelectionKind,
    pub key: String,
    pub label: String,
    pub tracks: Vec<Track>,
}

/// Selects `next`, or clears the selection when it is already the selected
/// one — clicking the open tile again is how the listener backs out of it.
pub fn toggle_selection(current: &mut Option<Selection>, next: Selection) {
    let same = current
        .as_ref()
        .is_some_and(|selected| selected.kind == next.kind && selected.key == next.key);
    *current = if same { None } else { Some(next) };
}

/// The selection as a replaced library has it: the same album or artist with
/// its tracks as they are now, or nothing when the library no longer holds it.
pub fn renewed_selection(
    selection: Selection,
    albums: &[AlbumGroup],
    artists: &[ArtistGroup],
) -> Option<Selection> {
    let tracks = match selection.kind {
        SelectionKind::Album => albums
            .iter()
            .find(|album| album.key == selection.key)
            .map(|album| album.tracks.clone()),
        SelectionKind::Artist => artists
            .iter()
            .find(|artist| artist.artist == selection.key)
            .map(|artist| artist.tracks.clone()),
    }?;
    Some(Selection {
        tracks,
        ..selection
    })
}

/// The number a track row shows: its place in the list, or — in an album —
/// the position its tags give it.
pub fn row_number(track: &Track, index: usize, use_track_number: bool) -> u32 {
    track
        .track_number
        .filter(|_| use_track_number)
        .unwrap_or(index as u32 + 1)
}

/// The three lists the Now Playing queue card switches between.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QueueView {
    /// Everything after the current track, wrapping once.
    #[default]
    UpNext,
    /// The whole play queue, in order.
    Playlist,
    /// The current track's album.
    Album,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueueRow {
    pub track: Track,
    /// The number shown beside the row; none where the rows are in no
    /// particular order.
    pub display: Option<u32>,
    /// Where the row's track sits in the list a click plays from: the album
    /// in the album view, the play queue otherwise.
    pub list_index: usize,
    pub is_current: bool,
}

/// The rows of the Now Playing queue card. `current_index` is the place of
/// the current track in `queue`, when a local track is current.
///
/// The engine draws the shuffled order itself and does not say what it
/// drew: with `shuffle` on, Up Next still lists what is left to hear, but
/// unnumbered, since their place in the list is not when they will play.
pub fn queue_rows(
    view: QueueView,
    queue: &[Track],
    current_index: Option<usize>,
    album_tracks: &[Track],
    shuffle: bool,
) -> Vec<QueueRow> {
    let current = current_index.and_then(|index| queue.get(index));
    match view {
        QueueView::Album => album_tracks
            .iter()
            .enumerate()
            .map(|(index, track)| QueueRow {
                track: track.clone(),
                // The album's own numbering, so a partially-ripped album shows
                // the positions the sleeve does.
                display: Some(track.track_number.unwrap_or(index as u32 + 1)),
                list_index: index,
                is_current: current.is_some_and(|current| current.id == track.id),
            })
            .collect(),
        QueueView::Playlist => queue
            .iter()
            .enumerate()
            .map(|(index, track)| QueueRow {
                track: track.clone(),
                display: Some(index as u32 + 1),
                list_index: index,
                is_current: current_index == Some(index),
            })
            .collect(),
        QueueView::UpNext => {
            // With nothing current the ring is simply the whole queue.
            let after = current_index.map_or(0, |index| (index + 1).min(queue.len()));
            let before = current_index.unwrap_or(0).min(queue.len());
            queue[after..]
                .iter()
                .chain(&queue[..before])
                .enumerate()
                .map(|(index, track)| QueueRow {
                    track: track.clone(),
                    display: (!shuffle).then_some(index as u32 + 1),
                    list_index: (after + index) % queue.len(),
                    is_current: false,
                })
                .collect()
        }
    }
}

/// The heading above the queue card's rows.
pub fn queue_heading(
    view: QueueView,
    queue_length: usize,
    row_count: usize,
    current: Option<&Track>,
    shuffle: bool,
    translations: &Translations,
) -> String {
    match view {
        QueueView::Album => current
            .and_then(|track| track.album.clone())
            .unwrap_or_else(|| translations.text("common.unknownAlbum")),
        QueueView::Playlist => {
            translations.translate("now.queueHeading", &[("count", queue_length.to_string())])
        }
        QueueView::UpNext => {
            let key = if shuffle {
                "now.upNextShuffledHeading"
            } else {
                "now.upNextHeading"
            };
            translations.translate(key, &[("count", row_count.to_string())])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_support::track;
    use crate::translations::Language;

    fn album_track(id: i64, title: &str, album: &str, artist: &str, number: Option<u32>) -> Track {
        let mut track = track(id, title);
        track.album = Some(album.to_owned());
        track.artist = Some(artist.to_owned());
        track.track_number = number;
        track
    }

    #[test]
    fn local_files_lists_only_the_files_on_disk() {
        let file = track(1, "Alive");
        let mut song = track(2, "Alive again");
        song.server_id = Some(7);
        let tracks = vec![file.clone(), song];

        assert_eq!(search_local_files(&tracks, ""), vec![file.clone()]);
        assert_eq!(search_local_files(&tracks, "alive"), vec![file]);
        assert!(search_local_files(&tracks, "nothing").is_empty());
    }

    fn titles(tracks: &[Track]) -> Vec<&str> {
        tracks.iter().map(|track| track.title.as_str()).collect()
    }

    #[test]
    fn an_album_plays_in_disc_then_track_order_with_untagged_tracks_last() {
        let mut second_disc = album_track(1, "Later", "A", "X", Some(1));
        second_disc.disc_number = Some(2);
        let tracks = [
            second_disc,
            album_track(2, "zeta", "A", "X", None),
            album_track(3, "Second", "A", "X", Some(2)),
            album_track(4, "Alpha", "A", "X", None),
            album_track(5, "First", "A", "X", Some(1)),
        ];

        let groups = albums(&tracks);

        assert_eq!(groups.len(), 1);
        assert_eq!(
            titles(&groups[0].tracks),
            ["First", "Second", "Alpha", "zeta", "Later"]
        );
        assert!(!groups[0].ordered);
    }

    #[test]
    fn a_compilation_stays_one_album_under_its_album_artist() {
        let mut first = album_track(1, "One", "Hits", "Artist A", Some(1));
        first.album_artist = Some("Various".to_owned());
        let mut second = album_track(2, "Two", "Hits", "Artist B", Some(2));
        second.album_artist = Some("Various".to_owned());
        let other = album_track(3, "Three", "Hits", "Someone Else", Some(1));

        let groups = albums(&[first, second, other]);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].artist.as_deref(), Some("Various"));
        assert_eq!(groups[0].tracks.len(), 2);
        assert!(groups[0].ordered);
    }

    #[test]
    fn albums_list_newest_addition_first_and_sum_what_their_tracks_say() {
        let mut old = album_track(1, "Old", "Old Album", "X", Some(1));
        old.added_at = 10;
        let mut new_first = album_track(2, "New 1", "New Album", "X", Some(1));
        new_first.added_at = 20;
        new_first.duration_secs = 100.0;
        let mut new_second = album_track(3, "New 2", "New Album", "X", Some(2));
        new_second.added_at = 30;
        new_second.duration_secs = 50.0;
        new_second.year = Some(1971);
        new_second.genre = Some("Rock".to_owned());

        let groups = albums(&[old, new_first, new_second]);

        assert_eq!(groups[0].album.as_deref(), Some("New Album"));
        assert_eq!(groups[0].newest_added_at, 30);
        assert_eq!(groups[0].duration_seconds, 150.0);
        assert_eq!(groups[0].year, Some(1971));
        assert_eq!(groups[0].genre.as_deref(), Some("Rock"));
        assert_eq!(
            album_detail(&groups[0], &Translations::new(Language::English)),
            "1971 · 2 tracks · 2:30"
        );
    }

    #[test]
    fn artists_group_by_track_artist_most_tracks_first_skipping_the_nameless() {
        let tracks = [
            album_track(1, "a", "A", "Solo", Some(1)),
            album_track(2, "b", "B", "Prolific", Some(1)),
            album_track(3, "c", "C", "Prolific", Some(1)),
            track(4, "untagged"),
        ];

        let groups = artists(&tracks);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].artist, "Prolific");
        assert_eq!(groups[0].tracks.len(), 2);
    }

    #[test]
    fn the_library_search_reads_six_fields_and_ignores_case_and_padding() {
        let mut by_composer = track(1, "Prelude");
        by_composer.composer = Some("Claude Debussy".to_owned());
        let mut by_genre = track(2, "Song");
        by_genre.genre = Some("Impressionist".to_owned());
        let unrelated = album_track(3, "Other", "Album", "Artist", None);
        let tracks = [by_composer, by_genre, unrelated];

        assert_eq!(titles(&search_library(&tracks, "  DEBUSSY ")), ["Prelude"]);
        assert_eq!(titles(&search_library(&tracks, "impression")), ["Song"]);
        assert_eq!(search_library(&tracks, "   ").len(), 3);
        // Local Files reads only the title and the artist.
        assert!(search_local_files(&tracks, "debussy").is_empty());
        assert_eq!(titles(&search_local_files(&tracks, "artist")), ["Other"]);
    }

    #[test]
    fn a_search_keeps_the_albums_and_artists_that_hold_a_match() {
        let tracks = [
            album_track(1, "Needle", "Haystack", "Farmer", Some(1)),
            album_track(2, "Straw", "Haystack", "Farmer", Some(2)),
            album_track(3, "Elsewhere", "Barn", "Builder", Some(1)),
        ];
        let album_groups = albums(&tracks);
        let artist_groups = artists(&tracks);

        let order = LibraryOrder::new(&tracks, &album_groups, &artist_groups);

        let kept = search_library(&tracks, "needle");

        let matching_albums = albums_matching(&album_groups, &order, &kept);
        assert_eq!(matching_albums.len(), 1);
        assert_eq!(matching_albums[0].album.as_deref(), Some("Haystack"));
        let matching_artists = artists_matching(&artist_groups, &order, &kept);
        assert_eq!(matching_artists.len(), 1);
        assert_eq!(matching_artists[0].artist, "Farmer");
    }

    #[test]
    fn the_library_tabs_are_alphabetical_ignoring_case_and_accents() {
        // Newest addition first, as the library is read.
        let mut untitled = track(5, "Unnamed");
        untitled.artist = Some("zed".to_owned());
        let tracks = [
            untitled,
            album_track(4, "beta", "Zénith", "Émile", Some(1)),
            album_track(3, "Alpha", "apogée", "zed", Some(1)),
            album_track(2, "Éclair", "Bridge", "Abel", Some(1)),
            album_track(1, "Alpha", "Bridge", "Abel", Some(2)),
        ];
        let album_groups = albums(&tracks);
        let artist_groups = artists(&tracks);
        let order = LibraryOrder::new(&tracks, &album_groups, &artist_groups);

        let song_identifiers: Vec<i64> = library_songs(&tracks, &order, "")
            .iter()
            .map(|track| track.id)
            .collect();
        assert_eq!(
            song_identifiers,
            [1, 3, 4, 2, 5],
            "songs by title, the same title by artist"
        );
        let album_names: Vec<Option<&str>> = albums_matching(&album_groups, &order, &tracks)
            .iter()
            .map(|album| album.album.as_deref())
            .collect();
        assert_eq!(
            album_names,
            [Some("apogée"), Some("Bridge"), Some("Zénith"), None],
            "albums by name, an untitled album last"
        );
        let artist_names: Vec<&str> = artists_matching(&artist_groups, &order, &tracks)
            .iter()
            .map(|artist| artist.artist.as_str())
            .collect();
        assert_eq!(artist_names, ["Abel", "Émile", "zed"]);
        assert_eq!(
            titles(&library_songs(&tracks, &order, "bridge")),
            ["Alpha", "Éclair"],
            "a search filters without reordering"
        );
    }

    #[test]
    fn recently_added_keeps_the_newest_additions_the_search_keeps() {
        let tracks: Vec<Track> = (0..12)
            .rev()
            .map(|id| track(id, &format!("Song {id}")))
            .collect();

        let recent = recently_added(&tracks, "");
        assert_eq!(recent.len(), RECENT_TRACK_COUNT);
        assert_eq!(recent[0].id, 11, "the newest first");
        assert_eq!(
            titles(&recently_added(&tracks, "song 1")),
            ["Song 11", "Song 10", "Song 1"]
        );
    }

    #[test]
    fn the_album_line_shows_only_what_the_tags_gave() {
        let translations = Translations::new(Language::English);
        let mut tagged = track(1, "A");
        assert_eq!(album_line(&tagged, &translations), None);

        tagged.track_number = Some(3);
        assert_eq!(
            album_line(&tagged, &translations).as_deref(),
            Some("Track 3")
        );

        tagged.track_total = Some(9);
        tagged.disc_number = Some(1);
        tagged.disc_total = Some(1);
        tagged.year = Some(1959);
        assert_eq!(
            album_line(&tagged, &translations).as_deref(),
            Some("Track 3 of 9 · 1959")
        );

        tagged.disc_total = Some(2);
        assert_eq!(
            album_line(&tagged, &translations).as_deref(),
            Some("Track 3 of 9 · Disc 1 of 2 · 1959")
        );
    }

    #[test]
    fn the_current_album_is_found_by_album_artist_or_falls_back_to_the_track() {
        let mut first = album_track(1, "One", "Hits", "Artist A", Some(1));
        first.album_artist = Some("Various".to_owned());
        let mut second = album_track(2, "Two", "Hits", "Artist B", Some(2));
        second.album_artist = Some("Various".to_owned());
        let groups = albums(&[first.clone(), second]);

        assert_eq!(titles(&album_tracks_of(&groups, &first)), ["One", "Two"]);

        let stray = album_track(9, "Stray", "Gone", "Nobody", None);
        assert_eq!(titles(&album_tracks_of(&groups, &stray)), ["Stray"]);
    }

    #[test]
    fn up_next_reads_as_a_ring_starting_after_the_current_track() {
        let queue = [track(1, "a"), track(2, "b"), track(3, "c"), track(4, "d")];

        let rows = queue_rows(QueueView::UpNext, &queue, Some(1), &[], false);

        let listed: Vec<(&str, Option<u32>, usize)> = rows
            .iter()
            .map(|row| (row.track.title.as_str(), row.display, row.list_index))
            .collect();
        assert_eq!(
            listed,
            [("c", Some(1), 2), ("d", Some(2), 3), ("a", Some(3), 0)]
        );
        assert!(rows.iter().all(|row| !row.is_current));

        // With nothing current the ring is the whole queue, from its top.
        let rows = queue_rows(QueueView::UpNext, &queue, None, &[], false);
        let places: Vec<usize> = rows.iter().map(|row| row.list_index).collect();
        assert_eq!(places, [0, 1, 2, 3]);
        assert!(queue_rows(QueueView::UpNext, &[], None, &[], false).is_empty());
    }

    #[test]
    fn a_shuffled_up_next_lists_what_is_left_without_claiming_an_order() {
        let queue = [track(1, "a"), track(2, "b"), track(3, "c")];

        let rows = queue_rows(QueueView::UpNext, &queue, Some(0), &[], true);

        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.display.is_none()));
        // A click still plays the row's own track.
        assert_eq!(rows[1].list_index, 2);
        // The playlist is the queue as loaded, whatever the engine draws.
        let playlist = queue_rows(QueueView::Playlist, &queue, Some(0), &[], true);
        assert_eq!(playlist[2].display, Some(3));
        assert_eq!(
            queue_heading(
                QueueView::UpNext,
                3,
                2,
                Some(&queue[0]),
                true,
                &Translations::new(Language::English)
            ),
            "Up next, shuffled · 2 songs"
        );
    }

    #[test]
    fn the_spotlight_gathers_the_artist_and_a_click_plays_them_from_the_chosen_track() {
        let tracks: Vec<Track> = (1..=6)
            .map(|id| album_track(id, &format!("t{id}"), &format!("Album {id}"), "X", Some(1)))
            .chain([album_track(7, "other", "Elsewhere", "Y", Some(1))])
            .collect();
        let artist_groups = artists(&tracks);
        let album_groups = albums(&tracks);

        assert_eq!(
            spotlight_tracks(&artist_groups, &tracks[0]).len(),
            SPOTLIGHT_COUNT
        );
        let shown_albums = spotlight_albums(&album_groups, &tracks[0]);
        assert_eq!(shown_albums.len(), SPOTLIGHT_COUNT);
        assert!(shown_albums
            .iter()
            .all(|album| album.artist.as_deref() == Some("X")));

        let (queue, start) = artist_queue_from(&artist_groups, &tracks[2]);
        assert_eq!(queue.len(), 6);
        assert_eq!(queue[start].id, 3);

        // A track with no artist, or one the library lost, plays alone.
        let stray = track(9, "stray");
        assert!(spotlight_tracks(&artist_groups, &stray).is_empty());
        assert!(spotlight_albums(&album_groups, &stray).is_empty());
        assert_eq!(artist_queue_from(&artist_groups, &stray), (vec![stray], 0));
    }

    #[test]
    fn the_playlist_and_album_views_mark_the_current_track() {
        let queue = [track(1, "a"), track(2, "b")];
        let mut album_only = track(7, "not queued");
        album_only.track_number = Some(5);
        let album = [queue[1].clone(), album_only];

        let playlist = queue_rows(QueueView::Playlist, &queue, Some(1), &album, false);
        assert_eq!(
            playlist
                .iter()
                .map(|row| row.is_current)
                .collect::<Vec<_>>(),
            [false, true]
        );

        let album_rows = queue_rows(QueueView::Album, &queue, Some(1), &album, false);
        assert!(album_rows[0].is_current);
        // Untagged rows count from their place; tagged ones keep the sleeve's number.
        assert_eq!(album_rows[0].display, Some(1));
        assert_eq!(album_rows[1].display, Some(5));
        assert_eq!(album_rows[1].list_index, 1);
    }

    #[test]
    fn the_queue_heading_follows_the_view() {
        let translations = Translations::new(Language::English);
        let mut current = track(1, "a");
        assert_eq!(
            queue_heading(
                QueueView::UpNext,
                4,
                3,
                Some(&current),
                false,
                &translations
            ),
            "Up next · 3 songs"
        );
        assert_eq!(
            queue_heading(
                QueueView::Playlist,
                4,
                4,
                Some(&current),
                false,
                &translations
            ),
            "Play queue · 4 songs"
        );
        assert_eq!(
            queue_heading(QueueView::Album, 4, 1, Some(&current), false, &translations),
            "Unknown album"
        );
        current.album = Some("Blue".to_owned());
        assert_eq!(
            queue_heading(QueueView::Album, 4, 1, Some(&current), false, &translations),
            "Blue"
        );
    }

    #[test]
    fn clicking_the_selected_tile_again_clears_the_selection() {
        let album = |key: &str| Selection {
            kind: SelectionKind::Album,
            key: key.to_owned(),
            label: key.to_owned(),
            tracks: Vec::new(),
        };
        let key_of =
            |selection: &Option<Selection>| selection.as_ref().map(|selected| selected.key.clone());
        let mut selection = None;

        toggle_selection(&mut selection, album("a"));
        assert_eq!(key_of(&selection).as_deref(), Some("a"));
        toggle_selection(&mut selection, album("b"));
        assert_eq!(key_of(&selection).as_deref(), Some("b"));
        toggle_selection(&mut selection, album("b"));
        assert_eq!(selection, None);

        // An artist that happens to share an album's key is a different tile.
        toggle_selection(&mut selection, album("x"));
        toggle_selection(
            &mut selection,
            Selection {
                kind: SelectionKind::Artist,
                ..album("x")
            },
        );
        assert_eq!(
            selection.map(|selected| selected.kind),
            Some(SelectionKind::Artist)
        );
    }

    #[test]
    fn a_selection_follows_a_replaced_library_or_closes_when_its_subject_is_gone() {
        let before = [album_track(1, "One", "A", "X", Some(1))];
        let after = [
            album_track(1, "One", "A", "X", Some(1)),
            album_track(2, "Two", "A", "X", Some(2)),
        ];
        let selection = Selection {
            kind: SelectionKind::Album,
            key: albums(&before)[0].key.clone(),
            label: "A".to_owned(),
            tracks: before.to_vec(),
        };

        let renewed = renewed_selection(selection.clone(), &albums(&after), &artists(&after))
            .expect("the album is still there");
        assert_eq!(titles(&renewed.tracks), ["One", "Two"]);
        assert_eq!(renewed.label, "A");

        assert_eq!(renewed_selection(selection, &[], &[]), None);
    }

    #[test]
    fn rows_are_numbered_by_place_or_by_the_album_position() {
        let mut tagged = track(1, "a");
        tagged.track_number = Some(7);
        assert_eq!(row_number(&tagged, 0, false), 1);
        assert_eq!(row_number(&tagged, 0, true), 7);
        assert_eq!(row_number(&track(2, "b"), 4, true), 5);
    }

    #[test]
    fn tabs_round_trip_through_their_identifiers() {
        for tab in [
            LibraryTab::Playlists,
            LibraryTab::Artists,
            LibraryTab::Albums,
            LibraryTab::Songs,
        ] {
            assert_eq!(LibraryTab::from_identifier(tab.identifier()), Some(tab));
        }
        assert_eq!(LibraryTab::from_identifier("nowhere"), None);
    }

    #[test]
    fn rediscover_offers_the_oldest_additions() {
        let tracks: Vec<Track> = (0..12)
            .map(|index| {
                let mut track = album_track(index, "t", &format!("Album {index}"), "X", Some(1));
                track.added_at = index;
                track
            })
            .collect();
        let groups = albums(&tracks);

        let shelf = rediscover(&groups);

        assert_eq!(shelf.len(), SHELF_TILE_COUNT);
        assert_eq!(shelf[0].album.as_deref(), Some("Album 0"));
    }
}
