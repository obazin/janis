//! The graphic equalizer's state: ten bands, ±12 dB, a set of presets, and
//! the linear-phase quality mode.
//!
//! The filters run in the engine; this module holds what the sliders show and
//! decides what a move means. Applying and persisting are separate effects on
//! purpose: the gains go straight to the engine so a slider is audible on the
//! next buffer, while the database write waits for the drag to settle.

use crate::audio::{CENTER_FREQS, EQ_BAND_COUNT};
use crate::effects::Effect;

/// Symmetric: sliders run from −12 dB to +12 dB.
pub const GAIN_RANGE: f64 = 12.0;

/// The preset name the equalizer reports once a band is moved by hand.
pub const CUSTOM_PRESET: &str = "custom";
const FLAT_PRESET: &str = "flat";

/// The built-in presets (dB per band, low → high), in the order their chips
/// are shown.
pub const PRESETS: [(&str, [f64; EQ_BAND_COUNT]); 8] = [
    (
        FLAT_PRESET,
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ),
    ("rock", [5.0, 4.0, 2.0, 0.0, -1.0, 0.0, 2.0, 3.0, 4.0, 4.0]),
    (
        "pop",
        [-1.0, 1.0, 3.0, 4.0, 3.0, 1.0, 0.0, -1.0, -1.0, -1.0],
    ),
    ("jazz", [4.0, 3.0, 1.0, 2.0, -1.0, -1.0, 0.0, 1.0, 3.0, 4.0]),
    (
        "classical",
        [4.0, 3.0, 2.0, 0.0, 0.0, 0.0, -1.0, -1.0, 2.0, 3.0],
    ),
    (
        "bassboost",
        [7.0, 6.0, 5.0, 3.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ),
    (
        "vocal",
        [-2.0, -1.0, 0.0, 2.0, 4.0, 5.0, 4.0, 2.0, 0.0, -1.0],
    ),
    (
        "electronic",
        [5.0, 4.0, 1.0, 0.0, -2.0, 1.0, 0.0, 2.0, 4.0, 5.0],
    ),
];

/// Translation key of a preset's label, `custom` included.
pub fn preset_label_key(preset: &str) -> String {
    format!("eq.preset.{preset}")
}

/// The label under each slider: the engine's own band centres, so the sliders
/// cannot lie about what they move.
pub fn frequency_labels() -> [String; EQ_BAND_COUNT] {
    CENTER_FREQS.map(|frequency| {
        if frequency >= 1000.0 {
            format!("{}k", (frequency / 1000.0).round())
        } else {
            format!("{}", frequency.round())
        }
    })
}

pub struct Equalizer {
    gains: [f64; EQ_BAND_COUNT],
    preset: String,
    linear_phase: bool,
    /// What the engine reports the linear-phase mode costs — 0 while off.
    /// Read from its report rather than assumed, because the filter length is
    /// fixed in taps and so the delay depends on the device's sample rate.
    latency_seconds: f32,
}

impl Default for Equalizer {
    fn default() -> Self {
        Self {
            gains: [0.0; EQ_BAND_COUNT],
            preset: FLAT_PRESET.to_owned(),
            linear_phase: false,
            latency_seconds: 0.0,
        }
    }
}

impl Equalizer {
    pub fn gains(&self) -> &[f64; EQ_BAND_COUNT] {
        &self.gains
    }

    pub fn preset(&self) -> &str {
        &self.preset
    }

    pub fn linear_phase(&self) -> bool {
        self.linear_phase
    }

    pub fn latency_seconds(&self) -> f32 {
        self.latency_seconds
    }

    /// Boot hydration from the preferences row.
    pub fn restore(&mut self, gains: &[f64], preset: &str, linear_phase: bool) -> Vec<Effect> {
        // A saved curve of the wrong length is flat; a gain the sliders could
        // not have set is brought back within their range.
        self.gains = <[f64; EQ_BAND_COUNT]>::try_from(gains)
            .unwrap_or([0.0; EQ_BAND_COUNT])
            .map(|gain| {
                if gain.is_nan() {
                    0.0
                } else {
                    gain.clamp(-GAIN_RANGE, GAIN_RANGE)
                }
            });
        self.preset = preset.to_owned();
        self.linear_phase = linear_phase;
        vec![
            Effect::SetEqualizerGains(self.gains.to_vec()),
            Effect::SetLinearPhase(linear_phase),
        ]
    }

    /// One band moved by hand — value in dB, snapped to half a decibel; the
    /// preset becomes `custom`.
    pub fn set_band(&mut self, index: usize, value: f64) -> Vec<Effect> {
        let Some(gain) = self.gains.get_mut(index).filter(|_| !value.is_nan()) else {
            return Vec::new();
        };
        // Halves round up.
        *gain = (value.clamp(-GAIN_RANGE, GAIN_RANGE) * 2.0 + 0.5).floor() / 2.0;
        self.preset = CUSTOM_PRESET.to_owned();
        vec![
            Effect::SetEqualizerGains(self.gains.to_vec()),
            Effect::PersistEqualizerDebounced {
                gains: self.gains.to_vec(),
                preset: self.preset.clone(),
            },
        ]
    }

    /// Applies a built-in preset; an unknown name changes nothing.
    pub fn set_preset(&mut self, name: &str) -> Vec<Effect> {
        let Some((_, gains)) = PRESETS.iter().find(|(preset, _)| *preset == name) else {
            return Vec::new();
        };
        self.gains = *gains;
        self.preset = name.to_owned();
        vec![
            Effect::SetEqualizerGains(self.gains.to_vec()),
            Effect::PersistEqualizer {
                gains: self.gains.to_vec(),
                preset: self.preset.clone(),
            },
        ]
    }

    pub fn reset(&mut self) -> Vec<Effect> {
        self.set_preset(FLAT_PRESET)
    }

    /// The quality mode: realtime filters, or the linear-phase mode.
    pub fn set_linear_phase(&mut self, enabled: bool) -> Vec<Effect> {
        self.linear_phase = enabled;
        vec![
            Effect::SetLinearPhase(enabled),
            Effect::PersistLinearPhase(enabled),
        ]
    }

    /// Flips the quality mode.
    pub fn toggle_linear_phase(&mut self) -> Vec<Effect> {
        self.set_linear_phase(!self.linear_phase)
    }

    /// The engine's own account of the mode and its exact latency.
    pub fn apply_engine_report(&mut self, enabled: bool, latency_seconds: f32) {
        self.linear_phase = enabled;
        self.latency_seconds = latency_seconds;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translations::{Language, Translations};

    #[test]
    fn every_preset_stays_within_the_gain_range_and_has_a_label() {
        let translations = Translations::new(Language::English);
        let keys: Vec<&str> = translations.keys().collect();
        for (name, gains) in PRESETS {
            assert!(gains.iter().all(|gain| gain.abs() <= GAIN_RANGE), "{name}");
            assert!(keys.contains(&preset_label_key(name).as_str()), "{name}");
        }
        assert!(keys.contains(&preset_label_key(CUSTOM_PRESET).as_str()));
    }

    #[test]
    fn the_slider_labels_follow_the_engine_bands() {
        assert_eq!(
            frequency_labels(),
            ["32", "64", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"]
        );
    }

    #[test]
    fn a_band_move_snaps_to_half_decibels_clamps_and_turns_the_preset_custom() {
        let mut equalizer = Equalizer::default();

        equalizer.set_band(0, 3.3);
        equalizer.set_band(1, 40.0);
        equalizer.set_band(2, -0.25);
        equalizer.set_band(3, 0.25);
        let effects = equalizer.set_band(4, -12.6);

        assert_eq!(equalizer.gains()[..5], [3.5, 12.0, 0.0, 0.5, -12.0]);
        assert_eq!(equalizer.preset(), CUSTOM_PRESET);
        assert!(matches!(
            effects.as_slice(),
            [
                Effect::SetEqualizerGains(_),
                Effect::PersistEqualizerDebounced { .. }
            ]
        ));
        assert!(equalizer.set_band(EQ_BAND_COUNT, 1.0).is_empty());
    }

    #[test]
    fn a_preset_is_applied_and_saved_at_once_and_reset_returns_to_flat() {
        let mut equalizer = Equalizer::default();

        let effects = equalizer.set_preset("rock");
        assert_eq!(equalizer.preset(), "rock");
        assert_eq!(equalizer.gains()[0], 5.0);
        assert_eq!(
            effects[1],
            Effect::PersistEqualizer {
                gains: equalizer.gains().to_vec(),
                preset: "rock".to_owned(),
            }
        );

        assert!(equalizer.set_preset("no such preset").is_empty());
        equalizer.reset();
        assert_eq!(equalizer.gains(), &[0.0; EQ_BAND_COUNT]);
        assert_eq!(equalizer.preset(), "flat");
    }

    #[test]
    fn restoring_tolerates_a_malformed_gain_list() {
        let mut equalizer = Equalizer::default();
        let effects = equalizer.restore(&[1.0, 2.0], "custom", true);
        assert_eq!(equalizer.gains(), &[0.0; EQ_BAND_COUNT]);
        assert!(equalizer.linear_phase());
        assert_eq!(
            effects,
            [
                Effect::SetEqualizerGains(vec![0.0; EQ_BAND_COUNT]),
                Effect::SetLinearPhase(true),
            ]
        );
    }

    #[test]
    fn the_linear_phase_mode_is_applied_saved_and_corrected_by_the_engine() {
        let mut equalizer = Equalizer::default();
        assert_eq!(
            equalizer.set_linear_phase(true),
            [
                Effect::SetLinearPhase(true),
                Effect::PersistLinearPhase(true)
            ]
        );
        assert_eq!(
            equalizer.toggle_linear_phase(),
            [
                Effect::SetLinearPhase(false),
                Effect::PersistLinearPhase(false)
            ]
        );
        equalizer.apply_engine_report(true, 0.043);
        assert_eq!(equalizer.latency_seconds(), 0.043);
        equalizer.apply_engine_report(false, 0.0);
        assert!(!equalizer.linear_phase());
    }

    #[test]
    fn a_band_value_that_is_not_a_number_is_refused() {
        let mut equalizer = Equalizer::default();
        assert!(equalizer.set_band(0, f64::NAN).is_empty());
        assert_eq!(equalizer.gains()[0], 0.0);
    }

    #[test]
    fn restored_gains_are_held_within_the_gain_range() {
        let mut equalizer = Equalizer::default();
        let mut saved = [0.0; EQ_BAND_COUNT];
        saved[0] = 40.0;
        saved[1] = -40.0;
        saved[2] = f64::NAN;
        saved[3] = 3.5;
        equalizer.restore(&saved, "custom", false);
        assert_eq!(equalizer.gains()[..4], [GAIN_RANGE, -GAIN_RANGE, 0.0, 3.5]);
    }

    proptest::proptest! {
        #[test]
        fn a_band_lands_within_the_range_on_a_half_decibel(
            band in 0..EQ_BAND_COUNT,
            value in proptest::num::f64::ANY,
        ) {
            let mut equalizer = Equalizer::default();
            equalizer.set_band(band, value);
            let gain = equalizer.gains()[band];
            proptest::prop_assert!(gain.is_finite());
            proptest::prop_assert!((-GAIN_RANGE..=GAIN_RANGE).contains(&gain));
            proptest::prop_assert_eq!((gain * 2.0).fract(), 0.0);
        }
    }
}
