//! The shape of the whole track the mini player draws, as a DAW draws an
//! audio clip: a lighter peak envelope around a solid RMS body, mirrored
//! about the middle line, left to right from the start of the track to its
//! end.
//!
//! The engine reads the track into slices of a hundredth of a second
//! (`audio::OverviewSlice`); this module rasterises them into an alpha mask
//! at the view's pixel size. The mask is white with the shape in its alpha,
//! so the window colours it with its tokens: no colour lives here.

use crate::audio::{OverviewSlice, OVERVIEW_SLICES_PER_SECOND};
use crate::covers::Pixels;
use slint::Rgba8Pixel;

/// Below this peak a track is treated as near-silent and not scaled up, so
/// hiss does not fill the view.
const MINIMUM_NORMALISATION_PEAK: f32 = 0.1;
/// The alpha of the peak envelope (the lighter outer shape) and of the RMS
/// body (the solid inner shape), out of 255.
const PEAK_ALPHA: f32 = 130.0;
const RMS_ALPHA: f32 = 255.0;

/// How long the slices last, in seconds.
pub fn seconds_of(slices: &[OverviewSlice]) -> f64 {
    slices.len() as f64 / f64::from(OVERVIEW_SLICES_PER_SECOND)
}

/// Rasterises `slices` into a `width` × `height` mask spanning
/// `duration_seconds` from left to right — the track's length, so the shape
/// lines up with the playhead even while the end is still being read.
/// Columns not read yet stay transparent; silence draws a one-pixel middle
/// line. Amplitudes are normalised to the loudest peak read so far, so a
/// quiet track stays readable.
pub fn mask(
    slices: &[OverviewSlice],
    duration_seconds: f64,
    width: usize,
    height: usize,
) -> Pixels {
    let mut pixels = Pixels::new(width as u32, height as u32);
    if slices.is_empty() || width == 0 || height == 0 {
        return pixels;
    }
    // A track whose length is unknown spans what was read.
    let duration_seconds = if duration_seconds.is_finite() && duration_seconds > 0.0 {
        duration_seconds
    } else {
        seconds_of(slices)
    };
    let peak = slices
        .iter()
        .map(|slice| slice.maximum.max(-slice.minimum))
        .fold(0.0, f32::max);
    let gain = 1.0 / peak.max(MINIMUM_NORMALISATION_PEAK);
    // The middle of a pixel row, so silence is one sharp row whether the
    // view is an odd or an even number of pixels high.
    let middle = (height / 2) as f32 + 0.5;
    // Half a pixel of headroom, so a full-scale peak is not cut.
    let half_height = (height as f32 / 2.0 - 0.5).max(0.5);
    let slices_per_column = duration_seconds * f64::from(OVERVIEW_SLICES_PER_SECOND) / width as f64;
    let to_row = |amplitude: f32| middle - (amplitude * gain).clamp(-1.0, 1.0) * half_height;

    let buffer = pixels.make_mut_slice();
    for column in 0..width {
        let first = (column as f64 * slices_per_column).floor() as usize;
        if first >= slices.len() {
            break;
        }
        let end = (((column + 1) as f64 * slices_per_column).ceil() as usize)
            .clamp(first + 1, slices.len());
        let range = &slices[first..end];
        let minimum = range.iter().map(|slice| slice.minimum).fold(0.0, f32::min);
        let maximum = range.iter().map(|slice| slice.maximum).fold(0.0, f32::max);
        let rms = (range.iter().map(|slice| slice.rms * slice.rms).sum::<f32>()
            / range.len() as f32)
            .sqrt();

        let peak_span = at_least_a_pixel(to_row(maximum), to_row(minimum));
        // The folds start at zero, so minimum ≤ 0 ≤ maximum: the RMS body
        // stays inside the peak envelope.
        let rms_span = at_least_a_pixel(to_row(rms.min(maximum)), to_row(-rms.min(-minimum)));
        for row in 0..height {
            let alpha = (coverage(row, peak_span) * PEAK_ALPHA)
                .max(coverage(row, rms_span) * RMS_ALPHA)
                .round() as u8;
            // Premultiplied white: the alpha is all the window reads.
            buffer[row * width + column] = Rgba8Pixel::new(alpha, alpha, alpha, alpha);
        }
    }
    pixels
}

/// `progress` (0 to 1) rounded down to a whole pixel of a view `width`
/// pixels wide, so a playhead only moves when it reaches the next pixel.
pub fn pixel_progress(progress: f64, width: usize) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    if width == 0 {
        return progress;
    }
    (progress * width as f64).floor() / width as f64
}

/// Widens a vertical span to one pixel about its centre, so silence still
/// draws a line.
fn at_least_a_pixel(top: f32, bottom: f32) -> (f32, f32) {
    let missing = (1.0 - (bottom - top)).max(0.0) / 2.0;
    (top - missing, bottom + missing)
}

/// How much of pixel row `row` (from `row` to `row + 1`) the vertical span
/// covers, from 0 to 1: the envelope's edges are anti-aliased.
fn coverage(row: usize, (top, bottom): (f32, f32)) -> f32 {
    let row_top = row as f32;
    (bottom.min(row_top + 1.0) - top.max(row_top)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOUD: OverviewSlice = OverviewSlice {
        minimum: -1.0,
        maximum: 1.0,
        rms: 0.7,
    };
    const SILENT: OverviewSlice = OverviewSlice {
        minimum: 0.0,
        maximum: 0.0,
        rms: 0.0,
    };

    /// One second of `slice`.
    fn second(slice: OverviewSlice) -> Vec<OverviewSlice> {
        vec![slice; OVERVIEW_SLICES_PER_SECOND as usize]
    }

    fn column(pixels: &Pixels, index: usize) -> Vec<u8> {
        let width = pixels.width() as usize;
        pixels
            .as_slice()
            .iter()
            .skip(index)
            .step_by(width)
            .map(|pixel| pixel.a)
            .collect()
    }

    fn drawn_rows(pixels: &Pixels, index: usize) -> usize {
        column(pixels, index)
            .iter()
            .filter(|alpha| **alpha > 0)
            .count()
    }

    #[test]
    fn a_loud_second_fills_its_columns_and_a_silent_one_is_a_middle_line() {
        let mut slices = second(LOUD);
        slices.extend(second(SILENT));
        let (width, height) = (20, 11);
        let pixels = mask(&slices, 2.0, width, height);

        let loud = column(&pixels, 2);
        // Every row is drawn, the outer two as anti-aliased half pixels.
        assert!(loud.iter().all(|alpha| *alpha > 0), "{loud:?}");
        // The RMS body is solid; the peak envelope around it is lighter.
        assert_eq!(loud[height / 2], 255);
        assert_eq!(loud[1], PEAK_ALPHA as u8, "{loud:?}");

        let silent = column(&pixels, 15);
        assert_eq!(drawn_rows(&pixels, 15), 1);
        assert!(silent[height / 2] > 0, "middle row drawn: {silent:?}");
    }

    #[test]
    fn silence_is_one_sharp_row_in_a_view_an_even_number_of_pixels_high() {
        let pixels = mask(&second(SILENT), 1.0, 4, 88);
        let silent = column(&pixels, 1);
        assert_eq!(drawn_rows(&pixels, 1), 1, "{silent:?}");
        assert_eq!(silent.iter().copied().max(), Some(255));
    }

    #[test]
    fn what_is_not_read_yet_stays_transparent() {
        // One second read of a four-second track.
        let pixels = mask(&second(LOUD), 4.0, 8, 6);
        assert!(drawn_rows(&pixels, 1) > 0);
        assert_eq!(drawn_rows(&pixels, 5), 0);
    }

    #[test]
    fn a_quiet_track_is_normalised_but_near_silence_is_not() {
        let quiet = |level: f32| OverviewSlice {
            minimum: -level,
            maximum: level,
            rms: level * 0.7,
        };
        assert_eq!(drawn_rows(&mask(&second(quiet(0.3)), 1.0, 4, 21), 1), 21);
        assert!(drawn_rows(&mask(&second(quiet(0.01)), 1.0, 4, 21), 1) <= 3);
    }

    #[test]
    fn an_unknown_length_spans_what_was_read() {
        let mut slices = second(LOUD);
        slices.extend(second(SILENT));
        let pixels = mask(&slices, 0.0, 10, 9);
        assert_eq!(drawn_rows(&pixels, 2), 9);
        assert_eq!(drawn_rows(&pixels, 9), 1);
    }

    #[test]
    fn nothing_to_draw_gives_an_empty_mask() {
        let pixels = mask(&[], 10.0, 8, 4);
        assert!(pixels.as_slice().iter().all(|pixel| pixel.a == 0));
        assert_eq!(mask(&second(LOUD), 1.0, 0, 0).as_slice().len(), 0);
    }

    #[test]
    fn the_playhead_moves_by_whole_pixels() {
        assert_eq!(pixel_progress(0.0, 100), 0.0);
        assert_eq!(pixel_progress(0.019, 100), 0.01);
        assert_eq!(pixel_progress(0.5, 100), 0.5);
        assert_eq!(pixel_progress(1.5, 100), 1.0);
        // A view that has not reported its size yet gets the exact value.
        assert_eq!(pixel_progress(0.25, 0), 0.25);
    }

    #[test]
    fn the_slices_say_how_long_they_last() {
        assert_eq!(seconds_of(&second(LOUD)), 1.0);
    }
}
