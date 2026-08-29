//! The realtime signal chain: ten peaking filters per channel, then gain.
//!
//! The chain is source → 10 × peaking biquad → gain → analyser tap → device:
//! the EQ topology matches the Web Audio graph it replaced, and the analyser
//! taps the final signal — post-EQ *and* post-gain, so the meters show what
//! actually reaches the device and scale with the volume slider.
//!
//! Everything here runs inside the cpal callback. Nothing in `process` may
//! allocate: the filter banks are sized once in `new`, and `sync` rewrites
//! coefficients in place.

use biquad::{Biquad, Coefficients, DirectForm1, ToHertz, Type};

use super::params::{Params, CENTER_FREQS, EQ_BAND_COUNT, EQ_Q};

/// A bank of ten peaking filters per channel, kept in step with `Params`.
///
/// Direct Form 1 rather than the more compact transposed Form 2: the biquad
/// crate documents DF1 as the topology that introduces the fewest artifacts
/// when coefficients are retuned while running, and retuning while running is
/// exactly what an EQ slider does.
pub struct Eq {
    /// One ten-filter chain per channel; coefficients are shared, but each
    /// channel needs its own delay line.
    channels: Vec<[DirectForm1<f32>; EQ_BAND_COUNT]>,
    sample_rate: f32,
    /// The `Params` epoch these coefficients were built from.
    epoch: u64,
}

impl Eq {
    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let identity = identity_coefficients();
        Self {
            channels: (0..channels)
                .map(|_| std::array::from_fn(|_| DirectForm1::new(identity)))
                .collect(),
            sample_rate,
            // `u64::MAX` can never equal a real epoch, so the first `sync`
            // always builds the bank rather than trusting identity filters.
            epoch: u64::MAX,
        }
    }

    /// Rebuilds the coefficient bank if the gains changed since the last call.
    /// Cheap enough for the callback: ten `powf`/`sin`/`cos` per channel, and
    /// only when a slider actually moved.
    pub fn sync(&mut self, params: &Params) {
        let epoch = params.eq_epoch();
        if epoch == self.epoch {
            return;
        }
        self.epoch = epoch;

        let mut gains = [0.0f32; EQ_BAND_COUNT];
        params.eq_gains(&mut gains);

        for band in 0..EQ_BAND_COUNT {
            let coefficients = peaking(self.sample_rate, CENTER_FREQS[band], gains[band]);
            for chain in self.channels.iter_mut() {
                chain[band].update_coefficients(coefficients);
            }
        }
    }

    /// Filters interleaved frames in place.
    pub fn process(&mut self, interleaved: &mut [f32]) {
        let channel_count = self.channels.len();
        if channel_count == 0 {
            return;
        }
        for frame in interleaved.chunks_mut(channel_count) {
            for (channel, sample) in frame.iter_mut().enumerate() {
                let mut value = *sample;
                for filter in self.channels[channel].iter_mut() {
                    value = filter.run(value);
                }
                *sample = value;
            }
        }
    }

    /// Drops the delay lines. Called after a flush so stale samples from
    /// before a seek cannot ring into the new position.
    pub fn reset(&mut self) {
        for chain in self.channels.iter_mut() {
            for filter in chain.iter_mut() {
                filter.reset_state();
            }
        }
    }
}

/// Peaking-EQ coefficients, falling back to a pass-through if the parameters
/// are degenerate. The callback cannot panic, so `from_params` is never
/// unwrapped: a band above Nyquist (16 kHz at a 32 kHz device rate, say)
/// simply does nothing instead of taking the process down.
fn peaking(sample_rate: f32, centre: f32, gain_db: f32) -> Coefficients<f32> {
    if centre >= sample_rate / 2.0 {
        return identity_coefficients();
    }
    Coefficients::<f32>::from_params(
        Type::PeakingEQ(gain_db),
        sample_rate.hz(),
        centre.hz(),
        EQ_Q,
    )
    .unwrap_or_else(|_| identity_coefficients())
}

/// y[n] = x[n]. Used for bands that cannot be realised at the device rate.
fn identity_coefficients() -> Coefficients<f32> {
    Coefficients {
        a1: 0.0,
        a2: 0.0,
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
    }
}

/// Converts an interleaved buffer between channel counts, appending to `out`.
///
/// A file's channel layout rarely matches the device's — mono podcasts on
/// stereo speakers, 5.1 rips on a stereo DAC. The rules are deliberately
/// plain, because anything cleverer (matrix downmix with centre and LFE
/// coefficients) is a surround feature Janis does not otherwise have.
pub fn remap_channels(input: &[f32], from: usize, to: usize, out: &mut Vec<f32>) {
    if from == 0 || to == 0 {
        return;
    }
    if from == to {
        out.extend_from_slice(input);
        return;
    }

    for frame in input.chunks_exact(from) {
        if to == 1 {
            // Fold everything down rather than dropping channels, so a centre
            // vocal does not vanish on a mono output.
            out.push(frame.iter().sum::<f32>() / from as f32);
        } else if from == 1 {
            // Mono to anything: the same signal in every channel.
            out.extend(std::iter::repeat_n(frame[0], to));
        } else {
            // Keep the leading channels (L/R first in every common layout)
            // and leave any extra device channels silent.
            for channel in 0..to {
                out.push(frame.get(channel).copied().unwrap_or(0.0));
            }
        }
    }
}

/// Master volume, per-track normalization, and the output clamp.
///
/// Holds the previous scale so a change can be ramped across the buffer
/// instead of stepping at the boundary — a step is audible as a zipper on a
/// volume drag, and a drag emits dozens of values per second.
#[derive(Debug, Default)]
pub struct Gain {
    previous: Option<f32>,
}

impl Gain {
    /// Applies gain in place and clamps to full scale.
    ///
    /// Volume uses a cubic taper rather than the raw slider fraction: loudness
    /// is roughly logarithmic, so a linear rail otherwise spends most of its
    /// travel in a range that sounds the same. Cubic tracks a dB curve closely
    /// enough while staying exactly 0 at the bottom and 1 at the top.
    ///
    /// The clamp is not optional. Web Audio clamped at the destination node,
    /// but cpal hands whatever it is given straight to the driver, and ten
    /// bands at +12 dB on a loud master will exceed full scale.
    pub fn apply(&mut self, interleaved: &mut [f32], volume: f32, track_gain: f32) {
        let target = volume * volume * volume * track_gain;
        let start = self.previous.unwrap_or(target);
        self.previous = Some(target);

        if interleaved.is_empty() {
            return;
        }

        if (start - target).abs() < f32::EPSILON {
            if (target - 1.0).abs() >= f32::EPSILON {
                for sample in interleaved.iter_mut() {
                    *sample *= target;
                }
            }
        } else {
            let step = (target - start) / interleaved.len() as f32;
            for (i, sample) in interleaved.iter_mut().enumerate() {
                *sample *= start + step * i as f32;
            }
        }

        for sample in interleaved.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Peak amplitude of a sine at `freq` after `eq` has processed it.
    fn response_at(eq: &mut Eq, sample_rate: f32, freq: f32) -> f32 {
        let frames = (sample_rate * 0.5) as usize;
        let mut buffer: Vec<f32> = (0..frames)
            .map(|n| (std::f32::consts::TAU * freq * n as f32 / sample_rate).sin())
            .collect();
        eq.process(&mut buffer);
        // Skip the first half so the filter has settled past its transient.
        buffer[frames / 2..]
            .iter()
            .fold(0.0f32, |peak, s| peak.max(s.abs()))
    }

    #[test]
    fn flat_gains_leave_the_signal_alone() {
        let params = Params::default();
        let mut eq = Eq::new(48_000.0, 1);
        eq.sync(&params);

        let peak = response_at(&mut eq, 48_000.0, 1000.0);
        assert!(
            (peak - 1.0).abs() < 0.01,
            "flat EQ should be unity, got {peak}"
        );
    }

    #[test]
    fn a_boosted_band_lifts_its_own_centre_frequency() {
        let params = Params::default();
        // +12 dB on the 1 kHz band (index 5) only.
        params.set_eq_gains(&[0.0, 0.0, 0.0, 0.0, 0.0, 12.0, 0.0, 0.0, 0.0, 0.0]);
        let mut eq = Eq::new(48_000.0, 1);
        eq.sync(&params);

        let peak = response_at(&mut eq, 48_000.0, 1000.0);
        // +12 dB is a linear factor of ~3.98.
        assert!(peak > 3.5, "expected roughly +12 dB at 1 kHz, got {peak}");
    }

    #[test]
    fn a_boosted_band_leaves_distant_frequencies_alone() {
        let params = Params::default();
        params.set_eq_gains(&[0.0, 0.0, 0.0, 0.0, 0.0, 12.0, 0.0, 0.0, 0.0, 0.0]);
        let mut eq = Eq::new(48_000.0, 1);
        eq.sync(&params);

        let peak = response_at(&mut eq, 48_000.0, 60.0);
        assert!(
            peak < 1.2,
            "60 Hz should be untouched by a 1 kHz band, got {peak}"
        );
    }

    #[test]
    fn a_cut_band_attenuates_its_centre_frequency() {
        let params = Params::default();
        params.set_eq_gains(&[0.0, 0.0, 0.0, 0.0, 0.0, -12.0, 0.0, 0.0, 0.0, 0.0]);
        let mut eq = Eq::new(48_000.0, 1);
        eq.sync(&params);

        let peak = response_at(&mut eq, 48_000.0, 1000.0);
        assert!(peak < 0.35, "expected roughly -12 dB at 1 kHz, got {peak}");
    }

    #[test]
    fn bands_above_nyquist_degrade_to_pass_through() {
        // At 32 kHz the 16 kHz band sits exactly at Nyquist and cannot be
        // realised; it must not blow up or panic.
        let params = Params::default();
        params.set_eq_gains(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 12.0]);
        let mut eq = Eq::new(32_000.0, 2);
        eq.sync(&params);

        let mut buffer = vec![0.5f32; 256];
        eq.process(&mut buffer);
        assert!(buffer.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn channels_are_filtered_independently() {
        let params = Params::default();
        let mut eq = Eq::new(48_000.0, 2);
        eq.sync(&params);

        // Left silent, right at full scale: a shared delay line would bleed
        // the right channel into the left.
        let mut buffer = vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
        eq.process(&mut buffer);

        for frame in buffer.chunks(2) {
            assert_eq!(frame[0], 0.0, "left channel picked up the right");
        }
    }

    #[test]
    fn matching_channel_counts_pass_through() {
        let mut out = Vec::new();
        remap_channels(&[1.0, 2.0, 3.0, 4.0], 2, 2, &mut out);
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn mono_is_replicated_across_the_device_channels() {
        let mut out = Vec::new();
        remap_channels(&[0.5, -0.5], 1, 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5, -0.5, -0.5]);
    }

    #[test]
    fn stereo_folds_down_to_mono_by_averaging() {
        let mut out = Vec::new();
        remap_channels(&[1.0, 0.0, 0.5, 0.5], 2, 1, &mut out);
        assert_eq!(out, vec![0.5, 0.5], "a channel must not simply be dropped");
    }

    #[test]
    fn surround_keeps_the_leading_channels_for_stereo() {
        // 5.1 frame: L R C LFE Ls Rs
        let mut out = Vec::new();
        remap_channels(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 6, 2, &mut out);
        assert_eq!(out, vec![1.0, 2.0]);
    }

    #[test]
    fn extra_device_channels_are_left_silent() {
        let mut out = Vec::new();
        remap_channels(&[1.0, 2.0], 2, 4, &mut out);
        assert_eq!(out, vec![1.0, 2.0, 0.0, 0.0]);
    }

    #[test]
    fn a_partial_trailing_frame_is_ignored() {
        // chunks_exact drops the remainder: half a frame cannot be mapped.
        let mut out = Vec::new();
        remap_channels(&[1.0, 2.0, 3.0], 2, 1, &mut out);
        assert_eq!(out, vec![1.5]);
    }

    #[test]
    fn gain_is_cubic_in_the_volume_fraction() {
        let mut gain = Gain::default();
        let mut buffer = vec![1.0f32; 4];
        gain.apply(&mut buffer, 0.5, 1.0);
        assert!((buffer[0] - 0.125).abs() < f32::EPSILON);
    }

    #[test]
    fn unity_gain_is_a_no_op() {
        let mut gain = Gain::default();
        let mut buffer = vec![0.7f32; 4];
        gain.apply(&mut buffer, 1.0, 1.0);
        assert_eq!(buffer, vec![0.7f32; 4]);
    }

    #[test]
    fn track_gain_scales_on_top_of_volume() {
        let mut gain = Gain::default();
        let mut buffer = vec![1.0f32; 2];
        gain.apply(&mut buffer, 1.0, 0.5);
        assert!((buffer[0] - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn output_is_clamped_to_full_scale() {
        // The device gets whatever we hand it, so a boosted signal must not
        // leave the rails.
        let mut gain = Gain::default();
        let mut buffer = vec![4.0f32, -4.0];
        gain.apply(&mut buffer, 1.0, 1.0);
        assert_eq!(buffer, vec![1.0, -1.0]);
    }

    #[test]
    fn a_volume_change_ramps_across_the_buffer() {
        let mut gain = Gain::default();
        let mut buffer = vec![1.0f32; 4];
        gain.apply(&mut buffer, 1.0, 1.0); // settle at unity
        let mut buffer = vec![1.0f32; 4];
        gain.apply(&mut buffer, 0.0, 1.0); // drop to silence

        assert_eq!(buffer[0], 1.0, "the ramp starts at the old gain");
        assert!(buffer[3] < buffer[0], "and falls toward the new one");
        assert!(
            buffer.windows(2).all(|w| w[1] <= w[0]),
            "a ramp must be monotonic, got {buffer:?}"
        );
    }

    #[test]
    fn a_steady_volume_does_not_ramp() {
        let mut gain = Gain::default();
        let mut buffer = vec![1.0f32; 4];
        gain.apply(&mut buffer, 0.5, 1.0);
        let mut buffer = vec![1.0f32; 4];
        gain.apply(&mut buffer, 0.5, 1.0);
        assert!(
            buffer.iter().all(|&s| (s - 0.125).abs() < f32::EPSILON),
            "an unchanged gain applies flat, got {buffer:?}"
        );
    }
}
