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
    let stream_config = supported.config();
    let config = OutputConfig {
        device_name,
        sample_rate: stream_config.sample_rate,
        channels: stream_config.channels,
    };

    let stream = match sample_format {
        cpal::SampleFormat::F32 => build::<f32>(&device, &stream_config, params, source, analyser),
        cpal::SampleFormat::I16 => build::<i16>(&device, &stream_config, params, source, analyser),
        cpal::SampleFormat::U16 => build::<u16>(&device, &stream_config, params, source, analyser),
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
    // Scratch for one callback's worth of f32, grown on the first call and
    // reused after — resizing inside the callback would allocate.
    let mut scratch: Vec<f32> = Vec::new();

    device
        .build_output_stream::<T, _, _>(
            *config,
            move |out: &mut [T], _| {
                if scratch.len() < out.len() {
                    scratch.resize(out.len(), 0.0);
                }
                let block = &mut scratch[..out.len()];

                // A seek or track jump invalidates everything already
                // decoded. Drop it, clear the filter delay lines so the old
                // position cannot ring into the new one, and emit one buffer
                // of silence while the decoder refills.
                if params.take_flush() {
                    let stale = source.slots();
                    if let Ok(chunk) = source.read_chunk(stale) {
                        chunk.commit_all();
                    }
                    eq.reset();
                    out.fill(T::from_sample(0.0));
                    return;
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

                eq.sync(&params);
                eq.process(block);
                gain.apply(block, params.volume(), params.track_gain());

                // Mono tap for the analyser: fewer samples to move, and a mono
                // signal is what the spectrum and the oscilloscope both want.
                // When the ring is full the extra frames are simply dropped —
                // a missed visualiser frame is invisible, a stalled callback
                // is audible.
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

                for (slot, sample) in out.iter_mut().zip(block.iter()) {
                    *slot = T::from_sample(*sample);
                }

                params.advance_frames((filled / channels) as u64);
            },
            |err| log::error!("audio output stream error: {}", err),
            None,
        )
        .map_err(|e| format!("build output stream: {}", e))
}
