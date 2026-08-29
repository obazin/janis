//! The realtime parameter block.
//!
//! Everything the cpal callback reads while it is running lives here as plain
//! atomics. The callback may not allocate, lock or make a syscall, so a
//! `Mutex` is out; even an `ArcSwap` costs an atomic read-modify-write per
//! read, which is more machinery than ten floats deserve.
//!
//! Writers are the IPC command handlers and the engine thread; the reader is
//! the audio callback. Plain values (volume, track gain, the frame counter)
//! use `Relaxed` — each is read on its own, and one that lands a buffer late
//! is inaudible. Two values are publish flags for *other* memory and use
//! release/acquire pairs instead: `eq_epoch` publishes the ten gain slots,
//! and `flush` orders the engine's ring writes against the callback's drain.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub const EQ_BAND_COUNT: usize = 10;

/// Band centres in Hz, low → high. Mirrors `CENTER_FREQS` on the frontend;
/// the two lists must stay in step or the sliders lie about what they move.
pub const CENTER_FREQS: [f32; EQ_BAND_COUNT] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

/// Shared by every band: wide enough that ten peaking filters cover the
/// spectrum without ripple between them, narrow enough that moving one band
/// is audible on its own.
pub const EQ_Q: f32 = 1.1;

/// Symmetric ±12 dB, matching the slider range the overlay renders.
pub const EQ_GAIN_RANGE_DB: f32 = 12.0;

/// Shared state between the control plane and the realtime callback.
#[derive(Debug)]
pub struct Params {
    eq_gains: [AtomicU32; EQ_BAND_COUNT],
    /// Bumped on every EQ write. The callback rebuilds its coefficient bank
    /// only when this changes, so the steady state costs one atomic load.
    eq_epoch: AtomicU64,
    volume: AtomicU32,
    /// Per-track normalization gain, already clamped against clipping.
    track_gain: AtomicU32,
    /// Set by the engine on seek or track jump; a handshake, not just a flag.
    /// The engine stops feeding the ring while it is up ([`Self::flush_pending`]),
    /// the callback fades out, drops what is left in the ring, and only then
    /// lowers it ([`Self::finish_flush`]) — so audio decoded for the new
    /// position can never be swept away with the stale audio.
    flush: AtomicBool,
    /// Frames the callback has actually handed to the device — the single
    /// source of truth for playback position, so what the UI shows is what
    /// the listener hears rather than what the decoder has run ahead to.
    frames_played: AtomicU64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            eq_gains: std::array::from_fn(|_| AtomicU32::new(0f32.to_bits())),
            eq_epoch: AtomicU64::new(0),
            volume: AtomicU32::new(1.0f32.to_bits()),
            track_gain: AtomicU32::new(1.0f32.to_bits()),
            flush: AtomicBool::new(false),
            frames_played: AtomicU64::new(0),
        }
    }
}

impl Params {
    /// `Acquire` pairs with the `Release` bump in [`Self::set_eq_gains`]: a
    /// callback that observes a new epoch is guaranteed to observe the gains
    /// written before it. With `Relaxed` on a weakly-ordered target the epoch
    /// could arrive first and the callback would latch stale gains for good.
    pub fn eq_epoch(&self) -> u64 {
        self.eq_epoch.load(Ordering::Acquire)
    }

    /// Reads the ten band gains in dB into `out`, avoiding the allocation a
    /// returned `Vec` would cost the callback.
    pub fn eq_gains(&self, out: &mut [f32; EQ_BAND_COUNT]) {
        for (slot, gain) in out.iter_mut().zip(self.eq_gains.iter()) {
            *slot = f32::from_bits(gain.load(Ordering::Relaxed));
        }
    }

    /// Clamps to the slider range so a malformed IPC payload cannot drive the
    /// filters into instability.
    pub fn set_eq_gains(&self, gains: &[f32]) {
        for (slot, gain) in self.eq_gains.iter().zip(gains.iter()) {
            let clamped = gain.clamp(-EQ_GAIN_RANGE_DB, EQ_GAIN_RANGE_DB);
            slot.store(clamped.to_bits(), Ordering::Relaxed);
        }
        // The epoch is the publish flag for the ten stores above.
        self.eq_epoch.fetch_add(1, Ordering::Release);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    pub fn set_volume(&self, volume: f32) {
        self.volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn track_gain(&self) -> f32 {
        f32::from_bits(self.track_gain.load(Ordering::Relaxed))
    }

    pub fn set_track_gain(&self, gain: f32) {
        self.track_gain
            .store(gain.clamp(0.0, 4.0).to_bits(), Ordering::Relaxed);
    }

    pub fn request_flush(&self) {
        self.flush.store(true, Ordering::Release);
    }

    /// Whether a flush is still waiting on the callback. The engine checks
    /// this before every pump and writes nothing into the ring while it is
    /// true — that pause is what makes the callback's drain safe.
    pub fn flush_pending(&self) -> bool {
        self.flush.load(Ordering::Acquire)
    }

    /// Lowers the flag once the ring has been drained. `Release` so an engine
    /// that observes the cleared flag also observes the drain: everything it
    /// pushes afterwards will be played, not discarded.
    pub fn finish_flush(&self) {
        self.flush.store(false, Ordering::Release);
    }

    pub fn frames_played(&self) -> u64 {
        self.frames_played.load(Ordering::Relaxed)
    }

    pub fn advance_frames(&self, frames: u64) {
        self.frames_played.fetch_add(frames, Ordering::Relaxed);
    }

    /// Rebases the position counter after a seek, so the UI jumps straight to
    /// the new spot instead of counting up from where the old one left off.
    pub fn reset_frames(&self, frames: u64) {
        self.frames_played.store(frames, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eq_gains_round_trip_and_clamp_to_the_slider_range() {
        let params = Params::default();
        params.set_eq_gains(&[6.0, -6.0, 99.0, -99.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);

        let mut gains = [0.0f32; EQ_BAND_COUNT];
        params.eq_gains(&mut gains);

        assert_eq!(gains[0], 6.0);
        assert_eq!(gains[1], -6.0);
        assert_eq!(gains[2], EQ_GAIN_RANGE_DB, "boost clamps to +12 dB");
        assert_eq!(gains[3], -EQ_GAIN_RANGE_DB, "cut clamps to -12 dB");
    }

    #[test]
    fn eq_epoch_advances_on_every_write() {
        let params = Params::default();
        let before = params.eq_epoch();
        params.set_eq_gains(&[0.0; EQ_BAND_COUNT]);
        assert!(params.eq_epoch() > before, "callback must see a change");
    }

    #[test]
    fn flush_stays_pending_until_finished() {
        let params = Params::default();
        assert!(!params.flush_pending());
        params.request_flush();
        assert!(
            params.flush_pending(),
            "the engine must see it and hold off"
        );
        assert!(
            params.flush_pending(),
            "observing the flag must not consume it — only the drain does"
        );
        params.finish_flush();
        assert!(!params.flush_pending(), "the engine may pump again");
    }

    #[test]
    fn volume_clamps_to_unit_range() {
        let params = Params::default();
        params.set_volume(2.5);
        assert_eq!(params.volume(), 1.0);
        params.set_volume(-1.0);
        assert_eq!(params.volume(), 0.0);
    }
}
