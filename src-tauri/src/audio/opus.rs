//! An Opus decoder for Symphonia, backed by libopus.
//!
//! Symphonia 0.6 demuxes Opus but ships no decoder for it, so `.opus` files
//! probe fine and then fail at `make_audio_decoder`. Everything else is
//! already in place: the OGG reader tags the track [`CODEC_ID_OPUS`], puts the
//! raw `OpusHead` in `extra_data`, and fills in each packet's trim — so this
//! only has to turn packets into samples.
//!
//! Modelled on `symphonia-codec-vorbis`, which is the reference for how a
//! decoder is expected to behave.

use symphonia::core::audio::{
    AsGenericAudioBufferRef, AudioBuffer, AudioMut, AudioSpec, GenericAudioBufferRef,
};
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{
    AudioCodecParameters, AudioDecoder, AudioDecoderOptions, FinalizeResult,
};
use symphonia::core::codecs::registry::{RegisterableAudioDecoder, SupportedAudioCodec};
use symphonia::core::codecs::CodecInfo;
use symphonia::core::errors::{decode_error, unsupported_error, Error, Result};
use symphonia::core::io::BufReader;
use symphonia::core::packet::PacketRef;
use symphonia_common::xiph::audio::opus::OpusHead;

/// Opus always decodes at 48 kHz whatever the source was recorded at.
const OPUS_RATE: u32 = 48_000;

/// The longest an Opus packet can be is 120 ms, which at 48 kHz is this many
/// frames. Sizing for the maximum means the decode path never reallocates.
const MAX_FRAMES_PER_PACKET: usize = 5760;

/// The highest OpusHead version this understands, matching what the OGG
/// mapper accepts.
const MAX_HEAD_VERSION: u8 = 0x0f;

pub struct OpusDecoder {
    opts: AudioDecoderOptions,
    params: AudioCodecParameters,
    inner: opus::Decoder,
    channels: usize,
    /// Interleaved scratch libopus writes into, before deinterleaving.
    scratch: Vec<f32>,
    /// Planar output handed back to Symphonia.
    buf: AudioBuffer<f32>,
}

// SAFETY: `opus::Decoder` is `Send` but not `Sync`, while `AudioDecoder`
// requires both. Every method that touches libopus state — `decode_float`,
// `reset_state`, `set_gain` — takes `&mut self`, so Rust's aliasing rules
// already guarantee exclusive access to the handle; the `&self` methods read
// only Rust-owned fields. Nothing here can be reached from two threads at
// once, which is exactly what `Sync` promises.
unsafe impl Sync for OpusDecoder {}

impl OpusDecoder {
    pub fn try_new(params: &AudioCodecParameters, opts: &AudioDecoderOptions) -> Result<Self> {
        if params.codec != CODEC_ID_OPUS {
            return unsupported_error("opus: invalid codec");
        }

        // The demuxer hands over the whole identification packet, magic
        // included, which is what `OpusHead::read` expects.
        let Some(extra_data) = params.extra_data.as_ref() else {
            return unsupported_error("opus: missing identification header");
        };
        let head = OpusHead::read(&mut BufReader::new(extra_data), MAX_HEAD_VERSION)?;

        let channels = head.channels.count();
        let layout = match channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            // libopus's plain decoder handles mono and stereo only; more
            // needs the multistream decoder and a channel remap, which no
            // file in a music library is likely to want.
            _ => return unsupported_error("opus: only mono and stereo are supported"),
        };

        let mut inner = opus::Decoder::new(OPUS_RATE, layout)
            .map_err(|_| Error::DecodeError("opus: could not create the decoder"))?;

        // The header carries a gain the encoder wants applied on playback.
        // Losing it only costs loudness, so a failure here is not fatal.
        if let Err(e) = inner.set_gain(i32::from(head.gain)) {
            log::warn!("opus: header gain ignored: {}", e);
        }

        let mut params = params.clone();
        params
            .with_sample_rate(OPUS_RATE)
            .with_channels(head.channels.clone())
            .with_max_frames_per_packet(MAX_FRAMES_PER_PACKET as u64);

        Ok(Self {
            opts: *opts,
            params,
            inner,
            channels,
            scratch: vec![0.0; MAX_FRAMES_PER_PACKET * channels],
            buf: AudioBuffer::new(
                AudioSpec::new(OPUS_RATE, head.channels),
                MAX_FRAMES_PER_PACKET,
            ),
        })
    }

    /// The fallible half of `decode_ref`, split out so every error path can
    /// clear the buffer in one place — the trait requires it, and
    /// `last_decoded` depends on it.
    fn decode_inner(&mut self, packet: &PacketRef<'_>) -> Result<()> {
        // A zero-length packet is legal OGG framing (a lacing value of 0, and
        // the mapper forwards it), but handing libopus an empty slice selects
        // its packet-loss *concealment* mode — which fabricates a full
        // buffer's worth (120 ms) of extrapolated audio instead of decoding
        // nothing. Skip it like any other undecodable packet.
        if packet.data.is_empty() {
            return decode_error("opus: empty packet");
        }

        let frames = self
            .inner
            .decode_float(packet.data, &mut self.scratch, false)
            .map_err(|_| Error::DecodeError("opus: undecodeable packet"))?;

        if frames > MAX_FRAMES_PER_PACKET {
            // Cannot happen — libopus is bounded by the buffer it was given —
            // but `render_uninit` panics rather than erroring if it did.
            return decode_error("opus: packet longer than the maximum");
        }

        self.buf.clear();
        self.buf.render_uninit(Some(frames));
        let interleaved = &self.scratch[..frames * self.channels];
        self.buf.copy_from_slice_interleaved(&interleaved);

        // Encoder delay at the head and padding at the tail. The OGG reader
        // has already worked out how much of each packet is real audio.
        if self.opts.gapless {
            self.buf.trim(
                packet.trim_start.get() as usize,
                packet.trim_end.get() as usize,
            );
        }
        Ok(())
    }
}

impl AudioDecoder for OpusDecoder {
    fn reset(&mut self) {
        // Drops the overlap and packet-loss state that belongs to the old
        // position. Called after every seek.
        if let Err(e) = self.inner.reset_state() {
            log::warn!("opus: reset failed: {}", e);
        }
    }

    fn codec_info(&self) -> &CodecInfo {
        &Self::supported_codecs()
            .first()
            .expect("opus: one codec is registered")
            .info
    }

    fn codec_params(&self) -> &AudioCodecParameters {
        &self.params
    }

    fn decode_ref(&mut self, packet: &PacketRef<'_>) -> Result<GenericAudioBufferRef<'_>> {
        match self.decode_inner(packet) {
            Ok(()) => Ok(self.buf.as_generic_audio_buffer_ref()),
            Err(e) => {
                // A caller may read `last_decoded` after a failure and must
                // not be handed the previous packet's audio.
                self.buf.clear();
                Err(e)
            }
        }
    }

    fn finalize(&mut self) -> FinalizeResult {
        // Opus carries no verification checksum.
        Default::default()
    }

    fn last_decoded(&self) -> GenericAudioBufferRef<'_> {
        self.buf.as_generic_audio_buffer_ref()
    }
}

impl RegisterableAudioDecoder for OpusDecoder {
    fn try_registry_new(
        params: &AudioCodecParameters,
        opts: &AudioDecoderOptions,
    ) -> Result<Box<dyn AudioDecoder>>
    where
        Self: Sized,
    {
        Ok(Box::new(OpusDecoder::try_new(params, opts)?))
    }

    fn supported_codecs() -> &'static [SupportedAudioCodec] {
        // Written out rather than using `support_audio_codec!`: that macro
        // expands to an absolute `symphonia_core::` path, and this crate
        // depends on the `symphonia` facade instead.
        &[SupportedAudioCodec {
            id: CODEC_ID_OPUS,
            info: CodecInfo {
                short_name: "opus",
                long_name: "Opus",
                profiles: &[],
            },
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use symphonia::core::codecs::audio::well_known::CODEC_ID_FLAC;
    use symphonia::core::units::{Duration, Timestamp};

    /// A hand-built OpusHead identification header (RFC 7845 §5.1) — what the
    /// OGG demuxer puts in `extra_data`.
    fn opus_head(channels: u8, mapping_family: u8) -> Vec<u8> {
        let mut head = b"OpusHead".to_vec();
        head.push(1); // version
        head.push(channels);
        head.extend_from_slice(&312u16.to_le_bytes()); // pre-skip
        head.extend_from_slice(&48_000u32.to_le_bytes()); // input sample rate
        head.extend_from_slice(&0u16.to_le_bytes()); // output gain
        head.push(mapping_family);
        if mapping_family != 0 {
            head.push(channels); // stream count
            head.push(0); // coupled stream count
            for channel in 0..channels {
                head.push(channel); // channel mapping table
            }
        }
        head
    }

    fn params_for(head: Vec<u8>) -> AudioCodecParameters {
        let mut params = AudioCodecParameters::new();
        params.for_codec(CODEC_ID_OPUS);
        params.with_extra_data(head.into_boxed_slice());
        params
    }

    fn opts() -> AudioDecoderOptions {
        AudioDecoderOptions::default()
    }

    #[test]
    fn a_valid_stereo_head_builds_a_decoder() {
        let decoder = OpusDecoder::try_new(&params_for(opus_head(2, 0)), &opts())
            .expect("stereo OpusHead should build");
        let params = decoder.codec_params();
        assert_eq!(
            params.sample_rate,
            Some(OPUS_RATE),
            "opus always decodes at 48 kHz"
        );
        assert_eq!(params.channels.as_ref().map(|c| c.count()), Some(2));
    }

    #[test]
    fn mono_maps_to_the_mono_layout() {
        let decoder = OpusDecoder::try_new(&params_for(opus_head(1, 0)), &opts())
            .expect("mono OpusHead should build");
        assert_eq!(
            decoder.codec_params().channels.as_ref().map(|c| c.count()),
            Some(1),
            "a mono file must not be widened by the layout mapping"
        );
    }

    #[test]
    fn the_wrong_codec_id_is_rejected() {
        let mut params = params_for(opus_head(2, 0));
        params.for_codec(CODEC_ID_FLAC);
        assert!(OpusDecoder::try_new(&params, &opts()).is_err());
    }

    #[test]
    fn a_missing_identification_header_is_rejected() {
        let mut params = AudioCodecParameters::new();
        params.for_codec(CODEC_ID_OPUS);
        assert!(OpusDecoder::try_new(&params, &opts()).is_err());
    }

    #[test]
    fn more_than_two_channels_is_rejected() {
        // Mapping family 1 with six channels — needs the multistream decoder
        // this deliberately does not carry.
        assert!(OpusDecoder::try_new(&params_for(opus_head(6, 1)), &opts()).is_err());
    }

    #[test]
    fn an_empty_packet_is_an_error_not_fabricated_audio() {
        // The OGG reader emits zero-length packets for a lacing value of 0.
        // libopus would treat the empty slice as a packet-loss-concealment
        // request and synthesise 120 ms of audio; the decoder must refuse
        // instead, so the caller skips the packet.
        let mut decoder =
            OpusDecoder::try_new(&params_for(opus_head(2, 0)), &opts()).expect("decoder");
        let packet = PacketRef::new(0, Timestamp::ZERO, Duration::ZERO, &[]);

        assert!(decoder.decode_ref(&packet).is_err());
        assert_eq!(
            decoder.last_decoded().frames(),
            0,
            "a failed decode must not leave synthesised audio behind"
        );
    }
}
