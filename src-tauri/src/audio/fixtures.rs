//! Audio the tests can make for themselves.
//!
//! Building a WAV in memory is a few lines and keeps binary fixtures out of
//! the repository, so several test modules were doing it independently. This
//! is the one copy.

#![cfg(test)]

/// A 16-bit PCM WAV holding `samples`, interleaved if `channels` > 1.
pub fn wav_bytes(sample_rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
    let bits = 16u16;
    let block_align = channels * bits / 8;
    let data_len = (samples.len() * 2) as u32;

    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * block_align as u32).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// A rising ramp, so a test can tell which part of the file it is looking at.
pub fn ramp(channels: u16, frames: usize) -> Vec<i16> {
    let mut samples = Vec::with_capacity(frames * usize::from(channels));
    for frame in 0..frames {
        let value = ((frame as f32 / frames as f32) * 30_000.0) as i16;
        samples.extend(std::iter::repeat_n(value, usize::from(channels)));
    }
    samples
}

/// A sine at `freq`, near full scale.
pub fn tone(sample_rate: u32, channels: u16, frames: usize, freq: f32) -> Vec<i16> {
    let mut samples = Vec::with_capacity(frames * usize::from(channels));
    for n in 0..frames {
        let t = n as f32 / sample_rate as f32;
        let value = ((t * freq * std::f32::consts::TAU).sin() * 12_000.0) as i16;
        samples.extend(std::iter::repeat_n(value, usize::from(channels)));
    }
    samples
}

/// Digital silence.
pub fn silence(channels: u16, frames: usize) -> Vec<i16> {
    vec![0; frames * usize::from(channels)]
}
