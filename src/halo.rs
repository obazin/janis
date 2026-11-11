//! The glow behind the Now Playing art: a ring of the prism colors that fades
//! out toward its edge.
//!
//! The interface has no blur, so the soft edge is drawn into the picture
//! itself, once, and the interface only rotates it.

use std::f32::consts::TAU;

use crate::covers::Pixels;
use slint::Rgba8Pixel;

/// The side of the picture in pixels; it is drawn scaled, and has no detail
/// that scaling could lose.
pub const SIDE: u32 = 384;
/// How strong the glow is at its fullest.
const STRENGTH: f32 = 0.5;
/// The share of the radius within which the glow is at full strength.
const CORE: f32 = 0.5;

/// Draws the glow from the colors met going once around it, as red, green
/// and blue.
pub fn picture(colors: &[[u8; 3]]) -> Pixels {
    let mut pixels = Pixels::new(SIDE, SIDE);
    if colors.is_empty() {
        return pixels;
    }
    let center = SIDE as f32 / 2.0;
    for (index, pixel) in pixels.make_mut_slice().iter_mut().enumerate() {
        let x = (index as u32 % SIDE) as f32 + 0.5 - center;
        let y = (index as u32 / SIDE) as f32 + 0.5 - center;
        // Clockwise from the top, like the conic gradient it stands in for.
        let turn = (x.atan2(-y) / TAU).rem_euclid(1.0) * colors.len() as f32;
        let from = (turn as usize).min(colors.len() - 1);
        let to = (from + 1) % colors.len();
        let blend = turn - from as f32;
        let distance = (x * x + y * y).sqrt() / center;
        let fade = (1.0 - ((distance - CORE) / (1.0 - CORE)).clamp(0.0, 1.0)).powi(2);
        let alpha = STRENGTH * fade;
        let channel = |channel: usize| {
            let value = f32::from(colors[from][channel])
                + (f32::from(colors[to][channel]) - f32::from(colors[from][channel])) * blend;
            (value * alpha) as u8
        };
        *pixel = Rgba8Pixel {
            r: channel(0),
            g: channel(1),
            b: channel(2),
            a: (255.0 * alpha) as u8,
        };
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINK: [u8; 3] = [255, 45, 155];
    const TEAL: [u8; 3] = [34, 224, 210];

    fn pixel(pixels: &Pixels, x: u32, y: u32) -> Rgba8Pixel {
        pixels.as_slice()[(y * SIDE + x) as usize]
    }

    #[test]
    fn the_glow_is_fullest_in_the_middle_and_gone_at_the_edge() {
        let pixels = picture(&[PINK, TEAL]);
        let middle = pixel(&pixels, SIDE / 2, SIDE / 2 - 10);
        assert_eq!(middle.a, (255.0 * STRENGTH) as u8);
        assert_eq!(pixel(&pixels, 0, SIDE / 2).a, 0);
        assert_eq!(pixel(&pixels, 0, 0).a, 0);
    }

    #[test]
    fn the_colors_follow_each_other_around_the_ring_starting_at_the_top() {
        let pixels = picture(&[PINK, TEAL]);
        // Just right of the top the first color leads; at the bottom, the second.
        let top = pixel(&pixels, SIDE / 2 + 1, SIDE / 2 - 40);
        let bottom = pixel(&pixels, SIDE / 2, SIDE / 2 + 40);
        assert!(top.r > top.g, "pink leads at the top");
        assert!(bottom.g > bottom.r, "teal leads at the bottom");
    }

    #[test]
    fn no_colors_draw_nothing() {
        assert!(picture(&[]).as_slice().iter().all(|pixel| pixel.a == 0));
    }
}
