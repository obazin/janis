//! Cover art, from bytes to pixels the interface can draw.
//!
//! Art arrives base64-encoded — embedded in a file, through the audio facade,
//! or announced by a station as a `data:` URL — or as a picture file a media
//! server sends. Decoding and scaling are slow enough to belong on a
//! background thread, so everything here returns a pixel buffer, which can
//! cross threads; the interface thread turns it into an image.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::Arc;

use base64::Engine as _;
use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::library::Track;

pub type Pixels = SharedPixelBuffer<Rgba8Pixel>;

/// Which loads of the art of whatever plays are still wanted. Each change of
/// track, and each art a station announces, moves an epoch on, so a slow load
/// that ends after the listener moved on is recognised and dropped.
#[derive(Debug, Default)]
pub struct CurrentArtRequests {
    epoch: u64,
    /// The track whose cover is loaded or loading.
    track_id: Option<i64>,
    failed: bool,
    stream_epoch: u64,
}

/// What asking for the cover of a track leads to.
#[derive(Debug, PartialEq, Eq)]
pub enum CoverRequest {
    /// The cover shown already belongs to that track: nothing to do.
    Unchanged,
    /// The cover shown is cleared, and loaded under the epoch given when
    /// there is a track.
    Cleared { load: Option<u64> },
}

impl CurrentArtRequests {
    /// The cover of `track_id`, or none.
    pub fn request(&mut self, track_id: Option<i64>) -> CoverRequest {
        // The engine confirms a track the click already showed: its cover is
        // not read a second time, unless it failed.
        if track_id.is_some() && self.track_id == track_id && !self.failed {
            return CoverRequest::Unchanged;
        }
        self.track_id = track_id;
        self.epoch += 1;
        self.failed = false;
        CoverRequest::Cleared {
            load: track_id.map(|_| self.epoch),
        }
    }

    /// A cover load ended. True when it is still wanted; a failure is then
    /// remembered, so the art tile says so and the cover is read again when
    /// asked for again.
    pub fn loaded(&mut self, epoch: u64, succeeded: bool) -> bool {
        if epoch != self.epoch {
            return false;
        }
        self.failed = !succeeded;
        true
    }

    /// Whether the cover of the current track failed to load.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// A station announced other art, or none: the epoch its decode runs under.
    pub fn request_stream(&mut self) -> u64 {
        self.stream_epoch += 1;
        self.stream_epoch
    }

    /// Whether a station art decode started under `epoch` is still wanted.
    pub fn stream_is_current(&self, epoch: u64) -> bool {
        epoch == self.stream_epoch
    }
}

/// The longest side of the art shown large on Now Playing, in pixels.
pub const HERO_SIDE: u32 = 800;
/// The longest side of the art shown in lists and grids, in pixels.
pub const THUMBNAIL_SIDE: u32 = 256;
/// The longest side a cover's file may have, in pixels, to be decoded at all.
const LARGEST_SOURCE_SIDE: u32 = 8192;
/// The most memory decoding one cover may take.
const LARGEST_DECODE_BYTES: u64 = 512 * 1024 * 1024;
/// Covers kept in memory at once for lists and grids.
pub const THUMBNAIL_LIMIT: usize = 300;

/// The picture in a `data:` URL, scaled to fit `longest_side`.
pub fn data_url_cover(url: &str, longest_side: u32) -> Result<Pixels, String> {
    let (_, encoded) = url
        .split_once(";base64,")
        .ok_or_else(|| "cover is not a base64 data URL".to_owned())?;
    base64_cover(encoded, longest_side)
}

/// A base64-encoded picture, as a file embeds it, scaled to fit
/// `longest_side`.
pub fn base64_cover(encoded: &str, longest_side: u32) -> Result<Pixels, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("decode cover base64: {error}"))?;
    picture_cover(bytes, longest_side)
}

/// The longest side of the picture kept for a playlist: enough for its
/// card at any size the grid draws it.
pub const PLAYLIST_PICTURE_SIDE: u32 = 600;

/// A picture the listener chose for a playlist, as it is kept: decoded with
/// the same limits as any cover, brought within `PLAYLIST_PICTURE_SIDE`, and
/// encoded as a JPEG.
pub fn playlist_picture(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    let pixels = picture_cover(bytes, PLAYLIST_PICTURE_SIDE)?;
    let picture =
        image::RgbaImage::from_raw(pixels.width(), pixels.height(), pixels.as_bytes().to_vec())
            .ok_or_else(|| "a decoded picture of the wrong size".to_owned())?;
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(picture)
        .to_rgb8()
        .write_to(&mut encoded, image::ImageFormat::Jpeg)
        .map_err(|error| format!("encode a playlist picture: {error}"))?;
    Ok(encoded.into_inner())
}

/// A picture file's bytes — as a media server sends them — scaled to fit
/// `longest_side`.
pub fn picture_cover(bytes: Vec<u8>, longest_side: u32) -> Result<Pixels, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("recognise cover image: {error}"))?;
    // Art comes from files and stations Janis does not control: a picture
    // that would decode to more than this is refused rather than allocated.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(LARGEST_SOURCE_SIDE);
    limits.max_image_height = Some(LARGEST_SOURCE_SIDE);
    limits.max_alloc = Some(LARGEST_DECODE_BYTES);
    reader.limits(limits);
    let picture = reader
        .decode()
        .map_err(|error| format!("decode cover image: {error}"))?;
    let picture = if picture.width().max(picture.height()) > longest_side {
        picture.thumbnail(longest_side, longest_side)
    } else {
        picture
    };
    let picture = picture.to_rgba8();
    Ok(Pixels::clone_from_slice(
        picture.as_raw(),
        picture.width(),
        picture.height(),
    ))
}

/// Tracks sharing a provider picture share one cached image.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum CoverKey {
    /// A track's own cover, read from its file or its album's files.
    Track(i64),
    /// A media server's picture of this item.
    ServerPicture(String),
    /// The picture the listener chose for this playlist.
    PlaylistPicture(i64),
    /// A provider image address, never a signed media address.
    Address(Arc<str>),
}

impl std::fmt::Debug for CoverKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Track(identifier) => formatter.debug_tuple("Track").field(identifier).finish(),
            Self::ServerPicture(item) => {
                formatter.debug_tuple("ServerPicture").field(item).finish()
            }
            Self::PlaylistPicture(identifier) => formatter
                .debug_tuple("PlaylistPicture")
                .field(identifier)
                .finish(),
            Self::Address(_) => formatter.write_str("Address { .. }"),
        }
    }
}

/// The cover key of every track in the library, so a tile that names a
/// track finds the cover another track of its album already loaded.
#[derive(Default)]
pub struct CoverKeys {
    shared_pictures: HashMap<i64, CoverKey>,
}

impl CoverKeys {
    pub fn new(tracks: &[Track]) -> Self {
        Self {
            shared_pictures: tracks
                .iter()
                .filter_map(|track| {
                    let key = if track.source == crate::library::TrackSource::YouTubeMusic {
                        CoverKey::Address(Arc::from(track.artwork_address.as_deref()?))
                    } else if track.server_id.is_some() {
                        CoverKey::ServerPicture(track.artwork_item.clone()?)
                    } else {
                        return None;
                    };
                    Some((track.id, key))
                })
                .collect(),
        }
    }

    /// A track this does not know — or a server song without a picture —
    /// keys by its own id; its lookup then finds what there is.
    pub fn key(&self, track_id: i64) -> CoverKey {
        self.shared_pictures
            .get(&track_id)
            .cloned()
            .unwrap_or(CoverKey::Track(track_id))
    }
}

/// A cache that forgets its oldest entries past a limit.
///
/// Covers are pinned in memory while cached, so a long session scrolling a
/// big library must not keep every cover it ever showed; what falls out is
/// simply loaded again if it scrolls back in.
pub struct BoundedCache<Key, Value> {
    entries: HashMap<Key, Value>,
    /// Keys from oldest to newest insertion.
    order: VecDeque<Key>,
    limit: usize,
}

impl<Key: Clone + Eq + Hash, Value> BoundedCache<Key, Value> {
    pub fn new(limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            limit,
        }
    }

    pub fn get(&self, key: &Key) -> Option<&Value> {
        self.entries.get(key)
    }

    /// Stores `value`, replacing what the key held without renewing its age.
    pub fn insert(&mut self, key: Key, value: Value) {
        if self.entries.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > self.limit {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    /// Forgets what the key held, so it is loaded again when next asked for.
    pub fn remove(&mut self, key: &Key) {
        if self.entries.remove(key).is_some() {
            self.order.retain(|kept| kept != key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chosen_playlist_picture_is_kept_as_a_jpeg_no_larger_than_needed() {
        let picture = image::RgbaImage::from_pixel(1200, 800, image::Rgba([10, 200, 30, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        picture
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode a test picture");
        let kept = playlist_picture(bytes.into_inner()).expect("a picture is kept");
        let decoded = image::load_from_memory_with_format(&kept, image::ImageFormat::Jpeg)
            .expect("the kept picture is a JPEG");
        assert_eq!((decoded.width(), decoded.height()), (600, 400));
        assert!(playlist_picture(b"not a picture".to_vec()).is_err());
    }

    fn encoded_png(width: u32, height: u32) -> String {
        let picture = image::RgbaImage::from_pixel(width, height, image::Rgba([255, 45, 155, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        picture
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode a test picture");
        base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
    }

    #[test]
    fn a_large_cover_is_scaled_down_keeping_its_shape() {
        let url = format!("data:image/png;base64,{}", encoded_png(400, 200));

        let pixels = data_url_cover(&url, 100).expect("a valid picture decodes");

        assert_eq!((pixels.width(), pixels.height()), (100, 50));
    }

    #[test]
    fn the_cache_forgets_its_oldest_entries_past_the_limit() {
        let mut cache = BoundedCache::new(2);
        cache.insert(1, "one");
        cache.insert(2, "two");
        // Replacing an entry does not make it newer.
        cache.insert(1, "uno");
        cache.insert(3, "three");

        assert_eq!(cache.get(&1), None);
        assert_eq!(cache.get(&2), Some(&"two"));
        assert_eq!(cache.get(&3), Some(&"three"));

        cache.clear();
        assert_eq!(cache.get(&3), None);
    }

    #[test]
    fn the_songs_of_an_album_on_a_server_share_its_picture() {
        let mut first = crate::library::test_support::track(1, "One");
        first.server_id = Some(7);
        first.artwork_item = Some("album".to_owned());
        let mut second = crate::library::test_support::track(2, "Two");
        second.server_id = Some(7);
        second.artwork_item = Some("album".to_owned());

        let keys = CoverKeys::new(&[first, second]);

        assert_eq!(keys.key(1), CoverKey::ServerPicture("album".to_owned()));
        assert_eq!(keys.key(1), keys.key(2));
    }

    #[test]
    fn a_file_and_a_server_song_without_a_picture_key_by_their_own_id() {
        let file = crate::library::test_support::track(1, "On disk");
        let mut bare = crate::library::test_support::track(2, "No picture");
        bare.server_id = Some(7);

        let keys = CoverKeys::new(&[file, bare]);

        assert_eq!(keys.key(1), CoverKey::Track(1));
        assert_eq!(keys.key(2), CoverKey::Track(2));
        assert_eq!(keys.key(99), CoverKey::Track(99), "an unknown track too");
    }

    #[test]
    fn provider_artwork_is_shared_without_using_a_server_picture() {
        let address = "https://i.ytimg.com/album.jpg";
        let mut first = crate::library::test_support::track(1, "One");
        first.source = crate::library::TrackSource::YouTubeMusic;
        first.artwork_address = Some(address.to_owned());
        let mut second = crate::library::test_support::track(2, "Two");
        second.source = crate::library::TrackSource::YouTubeMusic;
        second.artwork_address = Some(address.to_owned());
        let server = crate::library::test_support::track(3, "Server").server(7, address);
        let keys = CoverKeys::new(&[first, second, server]);
        let mut cache = BoundedCache::new(2);
        cache.insert(keys.key(1), "decoded artwork");
        assert_eq!(cache.get(&keys.key(2)), Some(&"decoded artwork"));
        assert_eq!(cache.get(&keys.key(3)), None);
    }

    #[test]
    fn an_oversized_cover_is_refused() {
        let url = format!(
            "data:image/png;base64,{}",
            encoded_png(LARGEST_SOURCE_SIDE + 1, 1)
        );
        assert!(data_url_cover(&url, 100).is_err());
    }

    #[test]
    fn a_small_cover_keeps_its_size() {
        let url = format!("data:image/png;base64,{}", encoded_png(40, 40));
        let pixels = data_url_cover(&url, 100).expect("a valid picture decodes");
        assert_eq!((pixels.width(), pixels.height()), (40, 40));
    }

    #[test]
    fn what_is_not_a_picture_is_an_error_rather_than_a_blank() {
        assert!(data_url_cover("https://example.invalid/cover.png", 100).is_err());
        assert!(data_url_cover("data:image/png;base64,AAAA", 100).is_err());
        assert!(data_url_cover("data:image/png;base64,***", 100).is_err());
    }

    #[test]
    fn a_confirmed_track_keeps_its_cover_and_a_failed_one_is_read_again() {
        let mut requests = CurrentArtRequests::default();
        let CoverRequest::Cleared { load: Some(epoch) } = requests.request(Some(1)) else {
            panic!("the first cover loads");
        };
        assert_eq!(requests.request(Some(1)), CoverRequest::Unchanged);
        assert!(requests.loaded(epoch, false));
        assert!(requests.failed());
        assert!(matches!(
            requests.request(Some(1)),
            CoverRequest::Cleared { load: Some(_) }
        ));
        assert!(!requests.failed());
    }

    #[test]
    fn a_cover_for_a_track_the_listener_left_is_dropped() {
        let mut requests = CurrentArtRequests::default();
        let CoverRequest::Cleared { load: Some(first) } = requests.request(Some(1)) else {
            panic!("the first cover loads");
        };
        requests.request(Some(2));
        assert!(!requests.loaded(first, false));
        assert!(
            !requests.failed(),
            "a stale failure is not the current one's"
        );
        assert_eq!(requests.request(None), CoverRequest::Cleared { load: None });
    }

    #[test]
    fn station_art_announced_again_drops_the_decode_still_in_flight() {
        let mut requests = CurrentArtRequests::default();
        let first = requests.request_stream();
        assert!(requests.stream_is_current(first));
        let second = requests.request_stream();
        assert!(!requests.stream_is_current(first));
        assert!(requests.stream_is_current(second));
    }

    #[test]
    fn a_tall_cover_is_scaled_down_keeping_its_shape_and_one_too_tall_is_refused() {
        let url = format!("data:image/png;base64,{}", encoded_png(100, 400));
        let pixels = data_url_cover(&url, 100).expect("a valid picture decodes");
        assert_eq!((pixels.width(), pixels.height()), (25, 100));

        let url = format!(
            "data:image/png;base64,{}",
            encoded_png(1, LARGEST_SOURCE_SIDE + 1)
        );
        assert!(data_url_cover(&url, 100).is_err());
    }

    #[test]
    fn a_file_with_a_picture_item_but_no_server_keys_by_its_own_id() {
        let file = Track {
            artwork_item: Some("item".to_owned()),
            ..crate::library::test_support::track(1, "a")
        };
        assert_eq!(CoverKeys::new(&[file]).key(1), CoverKey::Track(1));
    }

    #[test]
    fn a_cache_with_no_room_keeps_nothing_and_a_key_evicted_comes_back_when_inserted() {
        let mut empty = BoundedCache::new(0);
        empty.insert(1, "one");
        assert_eq!(empty.get(&1), None);

        let mut cache = BoundedCache::new(1);
        cache.insert(1, "one");
        cache.insert(2, "two");
        cache.insert(1, "one again");
        assert_eq!(cache.get(&1), Some(&"one again"));
        assert_eq!(cache.get(&2), None);
    }

    proptest::proptest! {
        #[test]
        fn the_cache_never_holds_more_than_its_limit(
            limit in 0usize..8,
            keys in proptest::collection::vec(0u8..16, 0..64),
        ) {
            let mut cache = BoundedCache::new(limit);
            for key in keys {
                cache.insert(key, ());
                proptest::prop_assert!(cache.entries.len() <= limit);
                proptest::prop_assert_eq!(cache.entries.len(), cache.order.len());
            }
        }
    }
}
