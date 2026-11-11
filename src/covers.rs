//! Cover art, from bytes to pixels the interface can draw.
//!
//! Art arrives base64-encoded — embedded in a file, through the audio facade,
//! or announced by a station as a `data:` URL. Decoding and scaling are slow
//! enough to belong on a background thread, so everything here returns a
//! pixel buffer, which can cross threads; the interface thread turns it into
//! an image.

use base64::Engine as _;
use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::library;
use crate::persistence::DbState;

pub type Pixels = SharedPixelBuffer<Rgba8Pixel>;

/// The longest side of the art shown large on Now Playing, in pixels.
pub const HERO_SIDE: u32 = 800;
/// The longest side of the art shown in lists and grids, in pixels.
pub const THUMBNAIL_SIDE: u32 = 256;

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
    let picture =
        image::load_from_memory(&bytes).map_err(|error| format!("decode cover image: {error}"))?;
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
