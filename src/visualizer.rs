//! The oscilloscope trace both waveform views draw.
//!
//! The engine publishes a frame roughly sixty times a second whose first
//! [`WAVE_POINTS`] bytes are the trace, already windowed in the engine; 128 is
//! silence. Every playing source feeds it — radio decodes through the same
//! engine path as a local file.
//!
//! With no frame — nothing loaded, or paused — the trace falls back to a
//! synthetic animation whose energy eases toward zero, so the views never care
//! which source fed them.

use std::f32::consts::TAU;
use std::fmt::Write as _;

/// Points in the trace: the leading bytes of the engine's visual frame.
pub const WAVE_POINTS: usize = 160;

/// The share of a view's half-height the trace swings through at full scale.
const AMPLITUDE: f32 = 0.42;
/// The longest step one tick advances the animation by, so a stalled window
/// resumes where it was instead of leaping ahead.
const LONGEST_STEP_SECONDS: f32 = 0.05;
const IDLE_ENERGY: f32 = 0.04;
const REST_TOLERANCE: f32 = 0.002;

pub struct Visualizer {
    wave: [f32; WAVE_POINTS],
    energy: f32,
    seconds: f32,
}

impl Default for Visualizer {
    fn default() -> Self {
        Self {
            wave: [0.0; WAVE_POINTS],
            energy: 0.05,
            seconds: 0.0,
        }
    }
}

impl Visualizer {
    /// Advances the trace by `elapsed_seconds`, from the engine's frame while
    /// something plays and from the synthetic fallback otherwise.
    pub fn tick(&mut self, elapsed_seconds: f32, frame: Option<&[u8]>, playing: bool) {
        // A step that is not a number would stay in the trace for good.
        let step = if elapsed_seconds.is_nan() {
            0.0
        } else {
            elapsed_seconds.clamp(0.0, LONGEST_STEP_SECONDS)
        };
        self.seconds += step;
        let target_energy = if playing { 1.0 } else { IDLE_ENERGY };
        self.energy += (target_energy - self.energy) * (step * 4.0).min(1.0);

        match frame.filter(|frame| playing && frame.len() >= WAVE_POINTS) {
            Some(frame) => {
                for (point, byte) in self.wave.iter_mut().zip(frame) {
                    *point = (f32::from(*byte) - 128.0) / 128.0;
                }
            }
            None => {
                for (index, point) in self.wave.iter_mut().enumerate() {
                    let turn = index as f32 / WAVE_POINTS as f32 * TAU;
                    let sample = 0.55 * (turn * 3.0 + self.seconds * 4.0).sin()
                        + 0.28 * (turn * 7.0 + self.seconds * 6.5).sin()
                        + 0.16 * (turn * 13.0 + self.seconds * 9.0).sin();
                    *point = sample * self.energy;
                }
            }
        }
    }

    /// True once the trace has stopped visibly moving: nothing plays and the
    /// fallback has eased down to its resting level. Ticking can stop until
    /// playback resumes.
    pub fn at_rest(&self, playing: bool) -> bool {
        !playing && (self.energy - IDLE_ENERGY).abs() < REST_TOLERANCE
    }

    /// The trace, each point in −1..1.
    #[cfg(test)]
    pub fn wave(&self) -> &[f32; WAVE_POINTS] {
        &self.wave
    }

    /// The trace as SVG path commands filling a `width` × `height` view.
    pub fn path_commands(&self, width: f32, height: f32) -> String {
        let mut commands = String::with_capacity(WAVE_POINTS * 14);
        for (index, sample) in self.wave.iter().enumerate() {
            let x = index as f32 / (WAVE_POINTS - 1) as f32 * width;
            let y = height / 2.0 + sample * height * AMPLITUDE;
            let verb = if index == 0 { 'M' } else { 'L' };
            // Writing to a `String` cannot fail.
            let _ = write!(commands, "{verb}{x:.1} {y:.1} ");
        }
        commands
    }

    /// Where the trace sits vertically at `progress` (0..1) across a view of
    /// `height` — the playhead rides the line.
    pub fn height_at(&self, progress: f32, height: f32) -> f32 {
        let index = (progress.clamp(0.0, 1.0) * (WAVE_POINTS - 1) as f32).floor() as usize;
        height / 2.0 + self.wave[index.min(WAVE_POINTS - 1)] * height * AMPLITUDE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_engine_frame_is_decoded_around_its_silent_midpoint() {
        let mut visualizer = Visualizer::default();
        let mut frame = [128u8; 170];
        frame[0] = 0;
        frame[1] = 192;

        visualizer.tick(0.016, Some(&frame), true);

        assert_eq!(visualizer.wave()[0], -1.0);
        assert_eq!(visualizer.wave()[1], 0.5);
        assert_eq!(visualizer.wave()[2], 0.0);
    }

    #[test]
    fn a_paused_or_absent_source_falls_back_to_a_trace_that_settles_down() {
        let mut visualizer = Visualizer::default();
        let frame = [255u8; 170];
        let peak = |visualizer: &Visualizer| {
            visualizer
                .wave()
                .iter()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
        };

        // Paused: the frame is ignored even though one is available.
        for _ in 0..200 {
            visualizer.tick(0.016, Some(&frame), false);
        }
        let settled = peak(&visualizer);
        assert!(settled > 0.0 && settled < 0.05, "{settled}");

        assert!(visualizer.at_rest(false));
        assert!(!visualizer.at_rest(true));

        // Playing with no frame yet: the fallback swells toward full scale.
        for _ in 0..200 {
            visualizer.tick(0.016, None, true);
        }
        assert!(peak(&visualizer) > 0.5);
    }

    #[test]
    fn the_path_spans_the_view_and_the_playhead_rides_the_line() {
        let mut visualizer = Visualizer::default();
        let mut frame = [128u8; 170];
        frame[159] = 255;
        visualizer.tick(0.016, Some(&frame), true);

        let commands = visualizer.path_commands(318.0, 100.0);

        assert!(commands.starts_with("M0.0 50.0 L2.0 50.0 "));
        assert!(commands.trim_end().ends_with("L318.0 91.7"));
        assert_eq!(visualizer.height_at(0.0, 100.0), 50.0);
        assert!((visualizer.height_at(1.0, 100.0) - 91.67).abs() < 0.01);
        assert_eq!(
            visualizer.height_at(7.0, 100.0),
            visualizer.height_at(1.0, 100.0)
        );
    }

    #[test]
    fn a_time_step_that_is_not_a_number_leaves_the_trace_drawable() {
        let mut visualizer = Visualizer::default();
        visualizer.tick(f32::NAN, None, true);
        for _ in 0..10 {
            visualizer.tick(0.016, None, true);
        }
        let commands = visualizer.path_commands(318.0, 100.0);
        assert!(!commands.contains("NaN"), "{commands}");
    }
}
