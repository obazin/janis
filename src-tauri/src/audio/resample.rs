//! Sample-rate conversion between the source and the output device.
//!
//! Bypassed entirely when the rates already match, which is the common case
//! and the only one that is bit-transparent — a 44.1 kHz FLAC on a 44.1 kHz
//! device should reach the DAC untouched.
//!
//! rubato is wrapped rather than called directly because its buffer shapes
//! are checked at runtime, not compile time: a wrong length is a
//! `ResampleError`, not a type error. Keeping every call in one place with
//! tests around it means those mistakes surface here rather than as silence
//! in the middle of a track.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler as _};

/// Frames of input consumed per rubato call. 1024 keeps the FFT cheap while
/// staying well under the ring, so a chunk boundary never starves the device.
const CHUNK_FRAMES: usize = 1024;

pub struct Resampler {
    /// `None` when the rates match and samples pass straight through.
    inner: Option<Fft<f32>>,
    from_rate: u32,
    to_rate: u32,
    channels: usize,
    /// Interleaved input staged until a full chunk is available.
    staged: Vec<f32>,
    /// Interleaved scratch for one chunk of rubato output.
    scratch: Vec<f32>,
    scratch_frames: usize,
}

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32, channels: usize) -> Result<Self, String> {
        if channels == 0 {
            return Err("cannot resample zero channels".to_string());
        }
        if from_rate == to_rate {
            return Ok(Self {
                inner: None,
                from_rate,
                to_rate,
                channels,
                staged: Vec::new(),
                scratch: Vec::new(),
                scratch_frames: 0,
            });
        }

        let inner = Fft::<f32>::new(
            from_rate as usize,
            to_rate as usize,
            CHUNK_FRAMES,
            channels,
            // Fixed input: every call consumes exactly CHUNK_FRAMES, which
            // makes the staging buffer a simple fill-and-drain.
            FixedSync::Input,
        )
        .map_err(|e| format!("build resampler {} -> {}: {}", from_rate, to_rate, e))?;

        let scratch_frames = inner.output_frames_max();
        Ok(Self {
            from_rate,
            to_rate,
            channels,
            staged: Vec::with_capacity(CHUNK_FRAMES * channels),
            scratch: vec![0.0; scratch_frames * channels],
            scratch_frames,
            inner: Some(inner),
        })
    }

    pub fn is_passthrough(&self) -> bool {
        self.inner.is_none()
    }

    /// Whether this instance already converts exactly this configuration.
    /// Lets a track join keep a matching resampler — and with it the staged
    /// tail of the outgoing track — instead of rebuilding for nothing.
    pub fn matches(&self, from_rate: u32, to_rate: u32, channels: usize) -> bool {
        self.from_rate == from_rate && self.to_rate == to_rate && self.channels == channels
    }

    /// Converts `input` (interleaved, source rate) and appends the result to
    /// `output` (interleaved, device rate).
    ///
    /// Input that does not fill a whole chunk is held until the next call, so
    /// a caller can feed arbitrary packet sizes.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) -> Result<(), String> {
        let Some(inner) = self.inner.as_mut() else {
            output.extend_from_slice(input);
            return Ok(());
        };

        self.staged.extend_from_slice(input);
        let chunk_samples = CHUNK_FRAMES * self.channels;

        let mut consumed = 0;
        while self.staged.len() - consumed >= chunk_samples {
            let chunk = &self.staged[consumed..consumed + chunk_samples];

            let adapter_in = InterleavedSlice::new(chunk, self.channels, CHUNK_FRAMES)
                .map_err(|e| format!("resampler input shape: {}", e))?;
            let mut adapter_out =
                InterleavedSlice::new_mut(&mut self.scratch, self.channels, self.scratch_frames)
                    .map_err(|e| format!("resampler output shape: {}", e))?;

            let (_, frames_out) = inner
                .process_into_buffer(&adapter_in, &mut adapter_out, None)
                .map_err(|e| format!("resample: {}", e))?;

            output.extend_from_slice(&self.scratch[..frames_out * self.channels]);
            consumed += chunk_samples;
        }

        self.staged.drain(..consumed);
        Ok(())
    }

    /// Flushes the staged remainder and the filter's internal latency into
    /// `output`, padding the last chunk with silence.
    ///
    /// For the moment the configuration has to change mid-stream — a track
    /// join where the source rate differs: whatever is staged is the *end of
    /// the outgoing track* (up to `CHUNK_FRAMES - 1` frames, ~23 ms), and
    /// dropping it with the old resampler would clip its final note. The
    /// trailing silence this appends is inaudible next to that.
    pub fn drain(&mut self, output: &mut Vec<f32>) -> Result<(), String> {
        let Some(inner) = self.inner.as_mut() else {
            return Ok(()); // passthrough stages nothing
        };
        let staged_frames = self.staged.len() / self.channels;
        // Two passes: the first pushes the staged tail through (rubato pads
        // the short chunk with zeros), the second pumps zeros so the frames
        // still inside the filter's latency come out too.
        for partial in [staged_frames, 0] {
            let adapter_in = InterleavedSlice::new(
                &self.staged[..partial * self.channels],
                self.channels,
                partial,
            )
            .map_err(|e| format!("resampler drain input shape: {}", e))?;
            let mut adapter_out =
                InterleavedSlice::new_mut(&mut self.scratch, self.channels, self.scratch_frames)
                    .map_err(|e| format!("resampler drain output shape: {}", e))?;
            let indexing = Indexing::new().partial_len(partial);
            let (_, frames_out) = inner
                .process_into_buffer(&adapter_in, &mut adapter_out, Some(&indexing))
                .map_err(|e| format!("resample drain: {}", e))?;
            output.extend_from_slice(&self.scratch[..frames_out * self.channels]);
        }
        self.staged.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_rates_pass_through_untouched() {
        let mut resampler = Resampler::new(48_000, 48_000, 2).unwrap();
        assert!(resampler.is_passthrough(), "no conversion should be built");

        let input: Vec<f32> = (0..64).map(|n| n as f32 / 64.0).collect();
        let mut output = Vec::new();
        resampler.process(&input, &mut output).unwrap();

        assert_eq!(output, input, "a matching rate must be bit-transparent");
    }

    /// Long-run output/input frame ratio, which is what a wrong rate would
    /// break. Fed enough chunks that the resampler's one-off startup delay
    /// (a few hundred frames of filter latency) is lost in the noise.
    fn measured_ratio(from: u32, to: u32) -> f64 {
        let mut resampler = Resampler::new(from, to, 2).unwrap();
        assert!(!resampler.is_passthrough());

        let frames_in = CHUNK_FRAMES * 200;
        let input = vec![0.0f32; frames_in * 2];
        let mut output = Vec::new();
        resampler.process(&input, &mut output).unwrap();

        (output.len() / 2) as f64 / frames_in as f64
    }

    #[test]
    fn upsampling_tracks_the_rate_ratio() {
        let ratio = measured_ratio(44_100, 48_000);
        let expected = 48_000.0 / 44_100.0;
        assert!(
            (ratio - expected).abs() / expected < 0.01,
            "expected ~{expected:.4}x, got {ratio:.4}x"
        );
    }

    #[test]
    fn downsampling_tracks_the_rate_ratio() {
        let ratio = measured_ratio(96_000, 48_000);
        assert!(
            (ratio - 0.5).abs() / 0.5 < 0.01,
            "expected ~0.5x, got {ratio:.4}x"
        );
    }

    #[test]
    fn output_stays_interleaved_across_channels() {
        // Left silent, right at full scale. If the adapter shapes were wrong
        // the two channels would smear into each other.
        let mut resampler = Resampler::new(44_100, 48_000, 2).unwrap();
        let mut input = Vec::new();
        for _ in 0..CHUNK_FRAMES * 4 {
            input.push(0.0);
            input.push(0.5);
        }
        let mut output = Vec::new();
        resampler.process(&input, &mut output).unwrap();

        // Skip the resampler's startup transient, then check the steady state.
        let frames: Vec<&[f32]> = output.chunks(2).skip(400).collect();
        assert!(!frames.is_empty(), "expected output past the transient");
        for frame in frames {
            assert!(frame[0].abs() < 0.05, "left leaked: {}", frame[0]);
            assert!(frame[1] > 0.4, "right lost level: {}", frame[1]);
        }
    }

    #[test]
    fn partial_input_is_held_until_a_chunk_is_complete() {
        let mut resampler = Resampler::new(44_100, 48_000, 2).unwrap();
        let mut output = Vec::new();

        // Half a chunk: nothing can come out yet.
        resampler
            .process(&vec![0.0f32; CHUNK_FRAMES], &mut output)
            .unwrap();
        assert!(
            output.is_empty(),
            "a partial chunk must be held, not padded"
        );

        // The other half completes it.
        resampler
            .process(&vec![0.0f32; CHUNK_FRAMES], &mut output)
            .unwrap();
        assert!(!output.is_empty(), "a completed chunk should emit");
    }

    #[test]
    fn zero_channels_is_rejected() {
        assert!(Resampler::new(44_100, 48_000, 0).is_err());
    }

    #[test]
    fn matches_compares_the_built_configuration() {
        let converting = Resampler::new(44_100, 48_000, 2).unwrap();
        assert!(converting.matches(44_100, 48_000, 2));
        assert!(
            !converting.matches(48_000, 48_000, 2),
            "different source rate"
        );
        assert!(
            !converting.matches(44_100, 96_000, 2),
            "different device rate"
        );
        assert!(
            !converting.matches(44_100, 48_000, 6),
            "different channel count"
        );

        let passthrough = Resampler::new(48_000, 48_000, 2).unwrap();
        assert!(passthrough.matches(48_000, 48_000, 2));
        assert!(!passthrough.matches(44_100, 48_000, 2));
    }

    #[test]
    fn drain_flushes_the_staged_tail_instead_of_dropping_it() {
        let mut resampler = Resampler::new(44_100, 48_000, 2).unwrap();
        let mut output = Vec::new();

        // One and a half chunks of a steady signal: the trailing half chunk
        // stays staged — it is the end of the outgoing track at a join.
        resampler
            .process(&vec![0.5f32; CHUNK_FRAMES * 3], &mut output)
            .unwrap();
        let before = output.len();

        resampler.drain(&mut output).unwrap();

        let tail = &output[before..];
        assert!(!tail.is_empty(), "the staged tail must come out");
        assert!(
            tail.iter().any(|s| s.abs() > 0.2),
            "the tail carries the staged signal, not just padding silence"
        );
    }

    #[test]
    fn drain_on_a_passthrough_is_a_no_op() {
        let mut resampler = Resampler::new(48_000, 48_000, 2).unwrap();
        let mut output = Vec::new();
        resampler.drain(&mut output).unwrap();
        assert!(output.is_empty());
    }
}
