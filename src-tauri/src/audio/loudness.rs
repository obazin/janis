//! Working out how much to turn each track up or down.
//!
//! Two sources, in order of preference. Most libraries already carry
//! ReplayGain tags written by a tagger, which cost nothing to read. Files
//! without them — and WAV and AIFF can never have them, the formats have
//! nowhere to put them — get measured against EBU R128 while they play.
//!
//! Everything here is deliberately conservative: a wrong gain is worse than no
//! gain, because it is applied to every sample and the listener has no way to
//! tell it is happening.

use ebur128::{EbuR128, Mode};

/// ReplayGain 2.0's reference level. Anything quieter is turned up, anything
/// louder is turned down, so an album mastered in 1985 and one mastered last
/// year sit at the same apparent volume.
pub const TARGET_LUFS: f64 = -18.0;

/// A track needs a lot of headroom before a boost is more likely to be a
/// broken tag than a quiet master.
const MAX_GAIN_DB: f64 = 12.0;
const MIN_GAIN_DB: f64 = -24.0;

/// Leave a little room below full scale, so a gain that would otherwise clip
/// is pulled back instead.
const PEAK_CEILING_DB: f64 = -1.0;

/// Parses a ReplayGain gain value, in dB.
///
/// There is no agreed format. Taggers write `-6.94 dB`, `+3.21 dB`,
/// `-6.94dB`, a bare `-6.94`, and — from anything running in a European
/// locale — `-6,94 dB`. The comma is the one that silently breaks a naive
/// parser on a real library.
pub fn parse_gain_db(raw: &str) -> Option<f64> {
    let cleaned = raw.trim();
    let cleaned = cleaned
        .strip_suffix("dB")
        .or_else(|| cleaned.strip_suffix("DB"))
        .or_else(|| cleaned.strip_suffix("db"))
        .or_else(|| cleaned.strip_suffix("Db"))
        .unwrap_or(cleaned)
        .trim();

    let value: f64 = cleaned.replace(',', ".").parse().ok()?;
    value.is_finite().then_some(value)
}

/// Parses a ReplayGain peak — a linear amplitude where 1.0 is full scale, not
/// a dB value. Can legitimately exceed 1.0 on a clipped master.
pub fn parse_peak(raw: &str) -> Option<f64> {
    let value: f64 = raw.trim().replace(',', ".").parse().ok()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

/// The gain to apply, in dB, from whatever is known about a track.
///
/// A tagged gain wins: someone measured it deliberately, possibly across a
/// whole album. Otherwise the measured loudness gives one. With neither, the
/// track plays untouched rather than guessed at.
pub fn gain_db(tag_gain_db: Option<f64>, lufs: Option<f64>, peak: Option<f64>) -> f64 {
    let raw = match (tag_gain_db, lufs) {
        (Some(tagged), _) if tagged.is_finite() => tagged,
        // Silence and very short files measure as -inf; turning that into a
        // gain would ask for +inf dB.
        (_, Some(measured)) if measured.is_finite() => TARGET_LUFS - measured,
        _ => return 0.0,
    };

    let bounded = raw.clamp(MIN_GAIN_DB, MAX_GAIN_DB);

    // Never push the loudest sample past the ceiling. Without this a quiet-
    // sounding but peak-heavy master would be boosted into clipping.
    match peak {
        Some(peak) if peak > 0.0 => {
            let headroom = PEAK_CEILING_DB - 20.0 * peak.log10();
            bounded.min(headroom)
        }
        _ => bounded,
    }
}

/// Converts dB to the linear factor the mixer multiplies by.
pub fn db_to_linear(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// Where measured loudness is remembered between plays.
///
/// The engine needs nothing else from the library, so it depends on this
/// rather than on an `AppHandle` — which also keeps it constructible in a test
/// without standing up a Tauri app.
pub trait Store: Send + Sync {
    /// Whether the track's loudness is still unknown, and so worth measuring.
    fn needs_measurement(&self, track_id: i64) -> bool;
    /// Records a completed measurement.
    fn record(&self, track_id: i64, measured: Measured);
}

/// A store that remembers nothing. The engine tests play audio without a
/// database behind them.
#[cfg(test)]
pub struct NoStore;

#[cfg(test)]
impl Store for NoStore {
    fn needs_measurement(&self, _track_id: i64) -> bool {
        false
    }
    fn record(&self, _track_id: i64, _measured: Measured) {}
}

/// What a completed measurement found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measured {
    /// Integrated loudness in LUFS.
    pub lufs: f64,
    /// True peak, linear.
    pub peak: f64,
}

/// Measures a track's loudness as it decodes.
///
/// Fed from the same buffers playback uses, so a track listened to end to end
/// costs nothing extra to measure. Not realtime-safe — it belongs on the
/// engine thread, never in the output callback.
pub struct Loudness {
    meter: EbuR128,
    channels: usize,
}

impl Loudness {
    pub fn new(sample_rate: u32, channels: u16) -> Option<Self> {
        if sample_rate == 0 || channels == 0 {
            return None;
        }
        // Integrated loudness and true peak, and nothing else — the other
        // modes cost real time and nothing here reads them.
        let meter = EbuR128::new(u32::from(channels), sample_rate, Mode::I | Mode::TRUE_PEAK)
            .map_err(|e| log::warn!("loudness meter: {}", e))
            .ok()?;
        Some(Self {
            meter,
            channels: usize::from(channels),
        })
    }

    /// Adds interleaved frames. A trailing partial frame is dropped rather
    /// than skewing the channel alignment of everything after it.
    pub fn feed(&mut self, interleaved: &[f32]) {
        let whole = interleaved.len() - interleaved.len() % self.channels;
        if whole == 0 {
            return;
        }
        if let Err(e) = self.meter.add_frames_f32(&interleaved[..whole]) {
            log::warn!("loudness measurement: {}", e);
        }
    }

    /// The result, or `None` when there was not enough audio to judge.
    ///
    /// EBU R128 gates on 400 ms blocks, so anything shorter — and anything
    /// silent — reports `-inf` rather than an error.
    pub fn finish(&self) -> Option<Measured> {
        let lufs = self.meter.loudness_global().ok()?;
        if !lufs.is_finite() {
            return None;
        }
        let peak = (0..self.channels as u32)
            .filter_map(|channel| self.meter.true_peak(channel).ok())
            .fold(0.0f64, f64::max);
        Some(Measured { lufs, peak })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_shapes_taggers_actually_write() {
        assert_eq!(parse_gain_db("-6.94 dB"), Some(-6.94));
        assert_eq!(parse_gain_db("+3.21 dB"), Some(3.21));
        assert_eq!(parse_gain_db("-6.94dB"), Some(-6.94));
        assert_eq!(parse_gain_db("  -6.94  "), Some(-6.94));
        assert_eq!(parse_gain_db("-6.94 DB"), Some(-6.94));
        assert_eq!(parse_gain_db("0"), Some(0.0));
    }

    #[test]
    fn parses_a_comma_decimal_separator() {
        // Written by any tagger running in a European locale, and the single
        // most common way a real library defeats a naive parser.
        assert_eq!(parse_gain_db("-6,94 dB"), Some(-6.94));
        assert_eq!(parse_peak("0,988553"), Some(0.988553));
    }

    #[test]
    fn rejects_what_is_not_a_number() {
        assert_eq!(parse_gain_db(""), None);
        assert_eq!(parse_gain_db("dB"), None);
        assert_eq!(parse_gain_db("loud"), None);
        assert_eq!(parse_peak("-0.5"), None, "a peak cannot be negative");
        assert_eq!(parse_peak("inf"), None);
    }

    #[test]
    fn a_peak_above_full_scale_still_parses() {
        // Legal on a clipped or true-peak-measured master.
        assert_eq!(parse_peak("1.023438"), Some(1.023438));
    }

    #[test]
    fn a_tagged_gain_is_used_as_given() {
        assert_eq!(gain_db(Some(-6.94), None, None), -6.94);
    }

    #[test]
    fn a_tagged_gain_beats_a_measurement() {
        // Someone measured the tag deliberately, possibly across the album.
        assert_eq!(gain_db(Some(-6.0), Some(-23.0), None), -6.0);
    }

    #[test]
    fn a_measurement_is_the_distance_to_the_target() {
        assert_eq!(gain_db(None, Some(TARGET_LUFS - 5.0), None), 5.0);
        assert_eq!(gain_db(None, Some(TARGET_LUFS + 3.0), None), -3.0);
    }

    #[test]
    fn nothing_known_means_nothing_applied() {
        assert_eq!(gain_db(None, None, None), 0.0);
        assert_eq!(gain_db(None, None, Some(0.5)), 0.0);
    }

    #[test]
    fn an_infinite_measurement_never_becomes_a_gain() {
        // Silence and sub-400ms files measure as -inf. Left unguarded this
        // would ask for +inf dB and hand the mixer a full-scale blast.
        assert_eq!(gain_db(None, Some(f64::NEG_INFINITY), None), 0.0);
        assert_eq!(gain_db(Some(f64::NAN), Some(f64::NEG_INFINITY), None), 0.0);
    }

    #[test]
    fn gain_is_bounded_in_both_directions() {
        assert_eq!(gain_db(Some(99.0), None, None), MAX_GAIN_DB);
        assert_eq!(gain_db(Some(-99.0), None, None), MIN_GAIN_DB);
    }

    #[test]
    fn a_boost_is_pulled_back_to_leave_headroom() {
        // Already at full scale: any boost would clip, so the gain goes
        // negative to reach the -1 dBFS ceiling instead.
        let gain = gain_db(Some(6.0), None, Some(1.0));
        assert!((gain - PEAK_CEILING_DB).abs() < 1e-9, "got {gain}");
    }

    #[test]
    fn a_quiet_peak_leaves_the_gain_alone() {
        // Peaking at -20 dBFS leaves 19 dB of headroom, well past the +6 asked
        // for, so the ceiling does not bite.
        let gain = gain_db(Some(6.0), None, Some(0.1));
        assert_eq!(gain, 6.0);
    }

    #[test]
    fn db_converts_to_the_expected_linear_factor() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_linear(-6.0206) - 0.5).abs() < 1e-4);
        assert!((db_to_linear(20.0) - 10.0).abs() < 1e-4);
    }

    /// Measures a full-scale 1 kHz sine at the given channel count.
    fn measure_tone(channels: u16) -> Measured {
        let rate = 48_000u32;
        let mut meter = Loudness::new(rate, channels).expect("meter");
        let mut frame = vec![0.0f32; usize::from(channels)];
        for n in 0..rate * 2 {
            let s = (std::f32::consts::TAU * 1000.0 * n as f32 / rate as f32).sin();
            frame.fill(s);
            meter.feed(&frame);
        }
        meter.finish().expect("two seconds is enough to gate")
    }

    #[test]
    fn a_full_scale_mono_tone_measures_near_minus_four_lufs() {
        // A full-scale sine has an RMS of 1/sqrt(2), i.e. -3 dBFS, and EBU
        // R128 subtracts a further 0.691 in its definition. K-weighting is
        // near flat at 1 kHz, so the answer lands just under -3.7 LUFS.
        let measured = measure_tone(1);
        assert!(
            (measured.lufs - -3.7).abs() < 1.0,
            "expected about -3.7 LUFS, got {}",
            measured.lufs
        );
        assert!(measured.peak > 0.9, "peak was {}", measured.peak);
    }

    #[test]
    fn stereo_reads_three_decibels_louder_than_mono() {
        // R128 sums the channels rather than averaging them, so the same tone
        // on two channels is twice the power. Getting this backwards would
        // make every stereo track quieter than it should be.
        let mono = measure_tone(1).lufs;
        let stereo = measure_tone(2).lufs;
        assert!(
            (stereo - mono - 3.01).abs() < 0.2,
            "expected stereo to be ~3 dB hotter: mono {mono}, stereo {stereo}"
        );
    }

    #[test]
    fn silence_has_no_measurable_loudness() {
        let mut meter = Loudness::new(48_000, 2).expect("meter");
        meter.feed(&vec![0.0; 48_000 * 2]);
        assert_eq!(meter.finish(), None, "silence must not produce a gain");
    }

    #[test]
    fn too_little_audio_has_no_measurable_loudness() {
        // Under one 400 ms gating block.
        let mut meter = Loudness::new(48_000, 2).expect("meter");
        meter.feed(&vec![0.5; 4_800 * 2]);
        assert_eq!(meter.finish(), None);
    }

    /// One second of a stereo sine at the given amplitude.
    fn stereo_tone(amplitude: f32) -> Vec<f32> {
        let rate = 48_000;
        (0..rate * 2)
            .map(|n| {
                let frame = (n / 2) as f32;
                amplitude * (std::f32::consts::TAU * 997.0 * frame / rate as f32).sin()
            })
            .collect()
    }

    #[test]
    fn a_partial_trailing_frame_is_dropped_not_the_whole_buffer() {
        // Two meters hear the same loud-then-quiet signal; one buffer also
        // carries a dangling half frame. The guard must trim just that
        // sample: if the whole odd-length buffer were rejected instead, the
        // ragged meter would miss the loud second entirely and measure a
        // very different loudness.
        let loud = stereo_tone(1.0);
        let quiet = stereo_tone(0.05);

        let mut clean = Loudness::new(48_000, 2).expect("meter");
        clean.feed(&loud);
        clean.feed(&quiet);

        let mut ragged = Loudness::new(48_000, 2).expect("meter");
        let mut loud_with_dangler = loud.clone();
        loud_with_dangler.push(0.7);
        ragged.feed(&loud_with_dangler);
        ragged.feed(&quiet);

        let clean = clean.finish().expect("two seconds gate").lufs;
        let ragged = ragged.finish().expect("two seconds gate").lufs;
        assert!(
            (clean - ragged).abs() < 0.05,
            "one trimmed sample must not move the measurement: clean {clean}, ragged {ragged}"
        );
    }
}
