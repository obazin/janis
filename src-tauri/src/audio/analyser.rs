//! Spectrum and waveform frames for the visualiser.
//!
//! This replaces the webview's `AnalyserNode`, and deliberately reproduces its
//! numbers rather than inventing better ones: a 2048-point FFT, magnitudes
//! mapped from −100…−30 dB onto 0…255, and 0.8 temporal smoothing. The
//! canvases on the other side keep drawing exactly what they drew before.
//!
//! Unlike the browser's analyser, this one runs off a tap taken *after* the
//! EQ and gain, on its own thread — so it costs the realtime callback nothing
//! but a mono copy, and radio streams get real bars for the first time.

use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

use super::params::EQ_BAND_COUNT;

/// Matches the `fftSize` the Web Audio graph used.
const FFT_SIZE: usize = 2048;
/// `frequencyBinCount` — the half-spectrum the browser exposed.
const BIN_COUNT: usize = FFT_SIZE / 2;
/// Points in the oscilloscope trace, matching `WAVE_POINTS` on the frontend.
const WAVE_POINTS: usize = 160;
/// `AnalyserNode`'s default `smoothingTimeConstant`.
const SMOOTHING: f32 = 0.8;
/// `AnalyserNode`'s default `minDecibels` / `maxDecibels`.
const MIN_DB: f32 = -100.0;
const MAX_DB: f32 = -30.0;

/// Wire size of one frame: the trace, then one byte per EQ band.
pub const FRAME_BYTES: usize = WAVE_POINTS + EQ_BAND_COUNT;

/// Consumes the mono tap and turns it into visualiser frames.
pub struct Analyser {
    tap: rtrb::Consumer<f32>,
    /// The most recent `FFT_SIZE` samples, oldest first.
    history: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    scratch: Vec<f32>,
    spectrum: Vec<realfft::num_complex::Complex<f32>>,
    /// Smoothed magnitudes in dB, one per bin, carried between frames.
    smoothed: Vec<f32>,
    window: Vec<f32>,
}

impl Analyser {
    pub fn new(tap: rtrb::Consumer<f32>) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        Self {
            tap,
            history: vec![0.0; FFT_SIZE],
            scratch: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            fft,
            smoothed: vec![MIN_DB; BIN_COUNT],
            // Hann window: without it, the rectangular window's spectral
            // leakage smears every tone across neighbouring bands.
            window: (0..FFT_SIZE)
                .map(|n| {
                    let phase = std::f32::consts::TAU * n as f32 / FFT_SIZE as f32;
                    0.5 - 0.5 * phase.cos()
                })
                .collect(),
        }
    }

    /// Drains the tap into the rolling history. Returns how many samples
    /// arrived, so a caller can tell a silent engine from a stalled one.
    pub fn drain(&mut self) -> usize {
        let available = self.tap.slots();
        if available == 0 {
            return 0;
        }
        let Ok(chunk) = self.tap.read_chunk(available) else {
            return 0;
        };
        let (first, second) = chunk.as_slices();
        for part in [first, second] {
            if part.is_empty() {
                continue;
            }
            // Only the newest FFT_SIZE samples can matter; if the reader fell
            // far behind, jump straight to the tail instead of shifting the
            // whole backlog through the history one window at a time.
            let tail = if part.len() > FFT_SIZE {
                &part[part.len() - FFT_SIZE..]
            } else {
                part
            };
            self.history.rotate_left(tail.len());
            let start = FFT_SIZE - tail.len();
            self.history[start..].copy_from_slice(tail);
        }
        chunk.commit_all();
        available
    }

    /// Builds one frame: 160 waveform bytes then 10 band magnitudes.
    pub fn frame(&mut self) -> [u8; FRAME_BYTES] {
        let mut out = [128u8; FRAME_BYTES];

        // Time domain — the same mapping as `getByteTimeDomainData`, where
        // 128 is silence and full scale spans the byte range.
        for (i, slot) in out[..WAVE_POINTS].iter_mut().enumerate() {
            let index = i * FFT_SIZE / WAVE_POINTS;
            let value = self.history[index];
            *slot = ((value * 128.0) + 128.0).clamp(0.0, 255.0) as u8;
        }

        // Frequency domain.
        for (slot, (sample, window)) in self
            .scratch
            .iter_mut()
            .zip(self.history.iter().zip(self.window.iter()))
        {
            *slot = sample * window;
        }
        if self
            .fft
            .process(&mut self.scratch, &mut self.spectrum)
            .is_err()
        {
            // Wrong buffer length is the only failure mode and it cannot
            // happen here, but a visualiser must never take the app down.
            out[WAVE_POINTS..].fill(0);
            return out;
        }

        let scale = 2.0 / FFT_SIZE as f32;
        for (bin, value) in self.spectrum.iter().take(BIN_COUNT).enumerate() {
            let magnitude = value.norm() * scale;
            let db = if magnitude > 0.0 {
                20.0 * magnitude.log10()
            } else {
                MIN_DB
            };
            // Exponential smoothing, matching AnalyserNode's behaviour.
            self.smoothed[bin] = SMOOTHING * self.smoothed[bin] + (1.0 - SMOOTHING) * db;
        }

        for (band, slot) in out[WAVE_POINTS..].iter_mut().enumerate() {
            let (lo, hi) = band_range(band);
            let mut sum = 0.0;
            let mut count = 0;
            for bin in lo..hi.min(BIN_COUNT) {
                sum += byte_from_db(self.smoothed[bin]) as f32;
                count += 1;
            }
            *slot = if count > 0 {
                (sum / count as f32) as u8
            } else {
                0
            };
        }

        out
    }
}

/// The bin range folded into `band`.
///
/// `BIN_COUNT^(i/10)` walks the bins in octaves — bins 1–2, 2–4, 4–8 and so
/// on — which at a 2048-point FFT lands each band close to the EQ centre
/// frequency it sits under. This is the fold the frontend used to do; keeping
/// it means the bars look the same after the move.
fn band_range(band: usize) -> (usize, usize) {
    let n = BIN_COUNT as f32;
    let lo = n.powf(band as f32 / EQ_BAND_COUNT as f32).floor() as usize;
    let hi = n.powf((band + 1) as f32 / EQ_BAND_COUNT as f32).floor() as usize;
    (lo, hi.max(lo + 1))
}

/// Maps dB onto 0…255 the way `getByteFrequencyData` does.
fn byte_from_db(db: f32) -> u8 {
    let normalised = (db - MIN_DB) / (MAX_DB - MIN_DB);
    (normalised * 255.0).clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyser_fed_with(signal: impl Fn(usize) -> f32, samples: usize) -> Analyser {
        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(samples.max(1));
        for n in 0..samples {
            producer.push(signal(n)).expect("ring sized for the signal");
        }
        let mut analyser = Analyser::new(consumer);
        analyser.drain();
        analyser
    }

    #[test]
    fn frame_is_the_documented_wire_size() {
        assert_eq!(FRAME_BYTES, 170, "160 wave points + 10 bands");
    }

    #[test]
    fn silence_reads_as_the_zero_line() {
        let mut analyser = analyser_fed_with(|_| 0.0, FFT_SIZE);
        let frame = analyser.frame();
        assert!(
            frame[..WAVE_POINTS].iter().all(|&b| b == 128),
            "silence must sit on the 128 midline"
        );
    }

    #[test]
    fn full_scale_signal_spans_the_byte_range() {
        // Alternating ±1 exercises both rails of the trace mapping.
        let mut analyser = analyser_fed_with(|n| if n % 2 == 0 { 1.0 } else { -1.0 }, FFT_SIZE);
        let frame = analyser.frame();
        let max = frame[..WAVE_POINTS].iter().copied().max().unwrap();
        let min = frame[..WAVE_POINTS].iter().copied().min().unwrap();
        assert!(max >= 254, "positive peak should reach the top, got {max}");
        assert!(min <= 1, "negative peak should reach the bottom, got {min}");
    }

    #[test]
    fn bands_cover_every_bin_without_gaps_or_overlap() {
        let mut previous_hi = band_range(0).0;
        for band in 0..EQ_BAND_COUNT {
            let (lo, hi) = band_range(band);
            assert_eq!(
                lo,
                previous_hi,
                "band {band} must start where {} ended",
                band.saturating_sub(1)
            );
            assert!(hi > lo, "band {band} must be non-empty");
            previous_hi = hi;
        }
        assert_eq!(previous_hi, BIN_COUNT, "the last band must reach Nyquist");
    }

    #[test]
    fn a_low_tone_lights_a_low_band_and_not_a_high_one() {
        // 100 Hz at 48 kHz lands in the third band (bins 4-8 ≈ 94-187 Hz).
        let rate = 48_000.0;
        let mut analyser = analyser_fed_with(
            |n| (std::f32::consts::TAU * 100.0 * n as f32 / rate).sin(),
            FFT_SIZE,
        );
        // Smoothing needs several frames to converge on a steady tone.
        let mut frame = analyser.frame();
        for _ in 0..64 {
            frame = analyser.frame();
        }
        let bands = &frame[WAVE_POINTS..];
        let low = bands[2];
        let high = bands[9];
        assert!(
            low > high,
            "100 Hz should favour a low band: {low} vs {high}"
        );
    }

    #[test]
    fn db_mapping_matches_the_analyser_node_range() {
        assert_eq!(byte_from_db(MIN_DB), 0);
        assert_eq!(byte_from_db(MAX_DB), 255);
        assert_eq!(byte_from_db(-200.0), 0, "below the floor clamps");
        assert_eq!(byte_from_db(0.0), 255, "above the ceiling clamps");
    }

    #[test]
    fn draining_more_than_a_window_keeps_the_newest_samples() {
        // Push three windows of a ramp; the history must end on the last one.
        let total = FFT_SIZE * 3;
        let analyser = analyser_fed_with(|n| n as f32 / total as f32, total);
        let last = *analyser.history.last().unwrap();
        let expected = (total - 1) as f32 / total as f32;
        assert!(
            (last - expected).abs() < 1e-6,
            "history should end at the newest sample, got {last} want {expected}"
        );
    }

    #[test]
    fn a_wrapped_ring_still_ends_history_on_the_newest_sample() {
        // The steady-state path in production: the tap ring wraps
        // continuously, so `read_chunk` hands back two slices. The other
        // tests fill a fresh ring once and never see a second slice; here
        // the bursts straddle the ring's end, and processing the slices in
        // the wrong order would end the history on stale audio.
        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(100);
        let mut analyser = Analyser::new(consumer);

        let mut newest = 0.0f32;
        for _ in 0..7 {
            for _ in 0..60 {
                newest += 1.0;
                producer.push(newest).expect("burst fits the ring");
            }
            assert_eq!(analyser.drain(), 60);
        }

        let last = *analyser.history.last().unwrap();
        assert_eq!(
            last, newest,
            "history must end at the newest sample — ending anywhere else \
             means the wrapped chunk's halves were applied out of order"
        );
    }
}
