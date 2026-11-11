//! Cover art, from bytes to pixels the interface can draw.
//!
//! Art arrives base64-encoded — embedded in a file, through the audio facade,
//! or announced by a station as a `data:` URL — or as a picture file a media
//! server sends. Decoding and scaling are slow enough to belong on a
//! background thread, so everything here returns a pixel buffer, which can
//! cross threads; the interface thread turns it into an image.

use std::collections::{HashMap, VecDeque};

use base64::Engine as _;
use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::library;
use crate::persistence::DbState;

pub type Pixels = SharedPixelBuffer<Rgba8Pixel>;

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

/// A track's cover, scaled to fit `longest_side`. `Ok(None)` when neither the
/// file nor its album carries art; `Err` when art exists but cannot be read.
pub fn track_cover(
    database: &DbState,
    track_id: i64,
    longest_side: u32,
) -> Result<Option<Pixels>, String> {
    match library::track_cover(database, track_id) {
        Some(art) => decode_base64(&art.data_base64, longest_side).map(Some),
        None => Ok(None),
    }
}

/// The picture in a `data:` URL, scaled to fit `longest_side`.
pub fn data_url_cover(url: &str, longest_side: u32) -> Result<Pixels, String> {
    let (_, encoded) = url
        .split_once(";base64,")
        .ok_or_else(|| "cover is not a base64 data URL".to_owned())?;
    decode_base64(encoded, longest_side)
}

fn decode_base64(encoded: &str, longest_side: u32) -> Result<Pixels, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("decode cover base64: {error}"))?;
    picture_cover(bytes, longest_side)
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

/// A cache that forgets its oldest entries past a limit.
///
/// Covers are pinned in memory while cached, so a long session scrolling a
/// big library must not keep every cover it ever showed; what falls out is
/// simply loaded again if it scrolls back in.
pub struct BoundedCache<Value> {
    entries: HashMap<i64, Value>,
    /// Keys from oldest to newest insertion.
    order: VecDeque<i64>,
    limit: usize,
}

impl<Value> BoundedCache<Value> {
    pub fn new(limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            limit,
        }
    }

    pub fn get(&self, key: i64) -> Option<&Value> {
        self.entries.get(&key)
    }

    /// Stores `value`, replacing what the key held without renewing its age.
    pub fn insert(&mut self, key: i64, value: Value) {
        if self.entries.insert(key, value).is_none() {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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

        assert_eq!(cache.get(1), None);
        assert_eq!(cache.get(2), Some(&"two"));
        assert_eq!(cache.get(3), Some(&"three"));

        cache.clear();
        assert_eq!(cache.get(3), None);
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
}
