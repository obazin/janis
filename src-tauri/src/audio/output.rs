//! The cpal output stream and the realtime callback.
//!
//! The callback is the only realtime context in the process. It pops frames
//! from the ring the decoder fills, runs the EQ and gain, taps a mono copy
//! for the analyser, and hands the result to the device. It never allocates,
//! locks, or blocks — an underrun is filled with silence rather than waited on.
//!
//! Devices are identified by their display name rather than cpal's opaque
//! `DeviceId`, because that is what the user picks in Settings and what
//! survives being written to `janis.db` and read back next launch.

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use serde::Serialize;

use super::dsp::{Eq, Gain};
use super::params::Params;

/// An output device as offered to the Settings screen.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    /// The device name, which doubles as its stable identifier.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// What the engine actually got when it opened the device — reported to the
/// UI so the Settings screen can stop claiming "System default" at a made-up
/// sample rate.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OutputConfig {
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: u16,
}

/// A live output stream. Owns a `cpal::Stream`, which is `!Send`, so this
/// type never leaves the engine thread.
pub struct Output {
    stream: cpal::Stream,
    pub config: OutputConfig,
}

impl Output {
    pub fn play(&self) -> Result<(), String> {
        self.stream
            .play()
            .map_err(|e| format!("start output stream: {}", e))
    }

    pub fn pause(&self) -> Result<(), String> {
        self.stream
            .pause()
            .map_err(|e| format!("pause output stream: {}", e))
    }
}

/// Enumerates output devices, default first.
pub fn list_devices() -> Result<Vec<AudioDevice>, String> {
    let host = cpal::default_host();
    let default_name = host
        .default_output_device()
        .and_then(|d| d.description().ok())
        .map(|d| d.name().to_string());

    let mut devices = Vec::new();
    for device in host
        .devices()
        .map_err(|e| format!("enumerate audio devices: {}", e))?
    {
        if !device.supports_output() {
            continue;
        }
        let Ok(description) = device.description() else {
            // A device that cannot describe itself cannot be offered to the
            // user or persisted, so skip it rather than fail the whole list.
            continue;
        };
        let name = description.name().to_string();
        let is_default = Some(&name) == default_name.as_ref();
        devices.push(AudioDevice {
            id: name.clone(),
            name,
            is_default,
        });
    }

    devices.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.name.cmp(&b.name)));
    Ok(devices)
}

/// Opens `device_id` (or the system default when `None`), building a stream
/// that drains `source`.
///
/// `analyser` receives a mono copy of everything sent to the device. It is a
/// ring like `source`; when it is full the callback drops the samples rather
/// than stall, because a dropped visualiser frame is invisible and a stalled
/// callback is audible.
pub fn open(
    device_id: Option<&str>,
    params: Arc<Params>,
    source: rtrb::Consumer<f32>,
    analyser: rtrb::Producer<f32>,
) -> Result<Output, String> {
    let host = cpal::default_host();

    let device = match device_id {
        Some(wanted) => host
            .devices()
            .map_err(|e| format!("enumerate audio devices: {}", e))?
            .find(|d| {
                d.supports_output() && d.description().is_ok_and(|desc| desc.name() == wanted)
            })
            // A device named in preferences may simply be unplugged now.
            // Falling back to the default is friendlier than refusing to play.
            .or_else(|| host.default_output_device()),
        None => host.default_output_device(),
    }
    .ok_or_else(|| "no audio output device available".to_string())?;

    let device_name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "Unknown device".to_string());

    let supported = device
        .default_output_config()
        .map_err(|e| format!("default output config for {}: {}", device_name, e))?;

    let sample_format = supported.sample_format();
    // The callback may not allocate, so its scratch is sized up front to the
    // biggest buffer the device may hand over. `Unknown` gets a generous
    // ceiling instead — the resize guard in the callback then exists only for
    // a host that misreports its own maximum.
    let max_frames = match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { max, .. } => *max as usize,
        cpal::SupportedBufferSize::Unknown => 16_384,
    };
    let stream_config = supported.config();
    let config = OutputConfig {
        device_name,
        sample_rate: stream_config.sample_rate,
        channels: stream_config.channels,
    };

    let stream = match sample_format {
        cpal::SampleFormat::F32 => build::<f32>(
            &device,
            &stream_config,
            max_frames,
            params,
            source,
            analyser,
        ),
        cpal::SampleFormat::I16 => build::<i16>(
            &device,
            &stream_config,
            max_frames,
            params,
            source,
            analyser,
        ),
        cpal::SampleFormat::U16 => build::<u16>(
            &device,
            &stream_config,
            max_frames,
            params,
            source,
            analyser,
        ),
        other => Err(format!("unsupported device sample format: {:?}", other)),
    }?;

    Ok(Output { stream, config })
}

/// Builds the stream for one device sample format.
///
/// The whole chain runs in `f32` and converts only on the way out, so the EQ
/// and the analyser always see the same signal regardless of what the device
/// wants.
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    max_frames: usize,
    params: Arc<Params>,
    mut source: rtrb::Consumer<f32>,
    mut analyser: rtrb::Producer<f32>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    // Rejected once here rather than guarded in the callback: a zero-channel
    // output is nonsense, and the callback should carry no branch that can
    // only ever go one way.
    let channels = match config.channels as usize {
        0 => return Err("output device reports zero channels".to_string()),
        n => n,
    };
    let mut eq = Eq::new(config.sample_rate as f32, channels);
    let mut gain = Gain::default();
    // Sized once, outside the callback, to the device's maximum buffer.
    let mut scratch: Vec<f32> = vec![0.0; max_frames.max(1) * channels];

    device
        .build_output_stream::<T, _, _>(
            *config,
            move |out: &mut [T], _| {
                if scratch.len() < out.len() {
                    // Only reachable when the host hands over more than the
                    // maximum it advertised. One allocation on a lying host
                    // beats reading past the block on every one.
                    scratch.resize(out.len(), 0.0);
                }
                let block = &mut scratch[..out.len()];

                let played = process_block(
                    block,
                    channels,
                    &params,
                    &mut eq,
                    &mut gain,
                    &mut source,
                    &mut analyser,
                );

                for (slot, sample) in out.iter_mut().zip(block.iter()) {
                    *slot = T::from_sample(*sample);
                }

                params.advance_frames(played);
            },
            |err| log::error!("audio output stream error: {}", err),
            None,
        )
        .map_err(|e| format!("build output stream: {}", e))
}

/// One callback's worth of work over the shared `f32` block — everything but
/// the final conversion to the device sample format, split out of the stream
/// closure so the realtime rules it enforces are testable without a device.
///
/// Returns the number of frames drained from the ring, which is what the
/// caller charges to `frames_played` — the single source of truth for the
/// playhead. Silence written on underrun is never charged.
fn process_block(
    block: &mut [f32],
    channels: usize,
    params: &Params,
    eq: &mut Eq,
    gain: &mut Gain,
    source: &mut rtrb::Consumer<f32>,
    analyser: &mut rtrb::Producer<f32>,
) -> u64 {
    // A seek or track change invalidated everything already decoded. Fade one
    // buffer of the stale audio down to silence — a hard cut is an audible
    // click — drop the rest, and clear the filter state so the old position
    // cannot ring into the new one. The engine stops feeding the ring while
    // the flag is up and it is lowered only *after* the drain, so audio
    // decoded for the new position can never be swept away here; the fade
    // frames belong to the abandoned timeline and are not charged to the
    // (already rebased) playhead.
    if params.flush_pending() {
        let whole_frames = (source.slots().min(block.len()) / channels) * channels;
        let mut filled = 0;
        if whole_frames > 0 {
            if let Ok(chunk) = source.read_chunk(whole_frames) {
                let (first, second) = chunk.as_slices();
                block[..first.len()].copy_from_slice(first);
                block[first.len()..first.len() + second.len()].copy_from_slice(second);
                filled = first.len() + second.len();
                chunk.commit_all();
            }
        }
        if let Ok(chunk) = source.read_chunk(source.slots()) {
            chunk.commit_all();
        }

        // The stale audio gets the same EQ and gain it would have been played
        // with, so the fade starts at the level the listener was hearing.
        eq.sync(params);
        eq.process(&mut block[..filled]);
        gain.apply(&mut block[..filled], params.volume(), params.track_gain());
        let fade_frames = (filled / channels).max(1) as f32;
        for (i, frame) in block[..filled].chunks_mut(channels).enumerate() {
            let scale = 1.0 - (i as f32 + 1.0) / fade_frames;
            for sample in frame.iter_mut() {
                *sample *= scale;
            }
        }
        block[filled..].fill(0.0);

        eq.reset();
        params.finish_flush();
        return 0;
    }

    let available = source.slots().min(block.len());
    let mut filled = 0;
    if available > 0 {
        if let Ok(chunk) = source.read_chunk(available) {
            let (first, second) = chunk.as_slices();
            block[..first.len()].copy_from_slice(first);
            block[first.len()..first.len() + second.len()].copy_from_slice(second);
            filled = first.len() + second.len();
            chunk.commit_all();
        }
    }
    // Underrun, or simply nothing playing: the tail stays silent.
    block[filled..].fill(0.0);

    eq.sync(params);
    eq.process(block);
    gain.apply(block, params.volume(), params.track_gain());

    // Mono tap for the analyser: fewer samples to move, and a mono signal is
    // what the spectrum and the oscilloscope both want. When the ring is full
    // the extra frames are simply dropped — a missed visualiser frame is
    // invisible, a stalled callback is audible.
    let room = analyser.slots().min(block.len() / channels);
    if room > 0 {
        if let Ok(chunk) = analyser.write_chunk_uninit(room) {
            chunk.fill_from_iter(
                block
                    .chunks(channels)
                    .take(room)
                    .map(|frame| frame.iter().sum::<f32>() / channels as f32),
            );
        }
    }

    (filled / channels) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rings(len: usize) -> (rtrb::Producer<f32>, rtrb::Consumer<f32>) {
        rtrb::RingBuffer::<f32>::new(len)
    }

    fn run_block(
        block_len: usize,
        params: &Params,
        source: &mut rtrb::Consumer<f32>,
    ) -> (Vec<f32>, u64) {
        let channels = 2;
        let mut eq = Eq::new(48_000.0, channels);
        let mut gain = Gain::default();
        let (mut tap_producer, _tap_consumer) = rings(8192);
        let mut block = vec![0.0f32; block_len];
        let played = process_block(
            &mut block,
            channels,
            params,
            &mut eq,
            &mut gain,
            source,
            &mut tap_producer,
        );
        (block, played)
    }

    #[test]
    fn frames_from_the_ring_are_charged_to_the_playhead() {
        let params = Params::default();
        let (mut producer, mut consumer) = rings(1024);
        for n in 0..512 {
            producer.push(n as f32 / 512.0).unwrap();
        }
        let (_, played) = run_block(512, &params, &mut consumer);
        assert_eq!(played, 256, "512 stereo samples are 256 frames");
    }

    #[test]
    fn underrun_silence_is_not_charged_to_the_playhead() {
        // The position must track what the listener heard, so a disk stall's
        // silence cannot count as played audio.
        let params = Params::default();
        let (mut producer, mut consumer) = rings(1024);
        for _ in 0..100 {
            producer.push(0.5).unwrap();
        }
        let (block, played) = run_block(512, &params, &mut consumer);
        assert_eq!(played, 50, "only the 50 real frames count");
        // Not exactly zero: the flat EQ still carries filter state from the
        // real samples, which rings at rounding-error level into the tail.
        assert!(
            block[100..].iter().all(|&s| s.abs() < 1e-3),
            "the unfilled tail must be silence"
        );
    }

    #[test]
    fn a_flush_drains_the_ring_and_completes_the_handshake() {
        let params = Params::default();
        let (mut producer, mut consumer) = rings(4096);
        for _ in 0..4096 {
            producer.push(0.8).unwrap();
        }
        params.request_flush();

        let (_, played) = run_block(512, &params, &mut consumer);

        assert_eq!(played, 0, "stale frames belong to the abandoned timeline");
        assert_eq!(consumer.slots(), 0, "everything stale is gone");
        assert!(
            !params.flush_pending(),
            "the flag drops only after the drain, so the engine may pump again"
        );
    }

    #[test]
    fn a_flush_fades_out_instead_of_cutting_to_silence() {
        // A full-scale signal chopped straight to zero is an audible click;
        // the contract in `params` promises a fade.
        let params = Params::default();
        let (mut producer, mut consumer) = rings(4096);
        for _ in 0..4096 {
            producer.push(1.0).unwrap();
        }
        params.request_flush();

        let (block, _) = run_block(512, &params, &mut consumer);

        assert!(
            block[0].abs() > 0.9,
            "the fade starts near the level being played, got {}",
            block[0]
        );
        let last = block[block.len() - 2]
            .abs()
            .max(block[block.len() - 1].abs());
        assert!(last < 0.01, "the fade must end at silence, got {last}");
        let mono: Vec<f32> = block.chunks(2).map(|f| f[0]).collect();
        assert!(
            mono.windows(2).all(|w| w[1] <= w[0] + 1e-6),
            "the fade must be monotonic"
        );
    }

    #[test]
    fn audio_pushed_after_the_flush_completes_is_played_not_discarded() {
        // The regression the handshake exists to prevent: the engine rebases,
        // refills the ring with the new position's audio, and the callback
        // must play it rather than sweep it away with the stale samples.
        let params = Params::default();
        let (mut producer, mut consumer) = rings(4096);
        for _ in 0..1024 {
            producer.push(0.3).unwrap();
        }
        params.request_flush();
        let (_, _) = run_block(512, &params, &mut consumer);
        assert!(!params.flush_pending());

        // Engine observes the lowered flag and pumps the new position.
        for _ in 0..512 {
            producer.push(0.6).unwrap();
        }
        let (block, played) = run_block(512, &params, &mut consumer);
        assert_eq!(played, 256, "the fresh audio is charged and played");
        assert!(
            block.iter().all(|&s| (s - 0.6).abs() < 1e-3),
            "the fresh audio comes out intact"
        );
    }
}
