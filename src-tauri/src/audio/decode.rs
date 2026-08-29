//! Symphonia decoding: one source in, interleaved `f32` out.
//!
//! Deliberately source-agnostic. A local file and a radio stream differ only
//! in the [`MediaSource`] handed to [`Decoder::open`], which is what lets the
//! EQ and the analyser apply to radio — something the webview could never do,
//! because a CORS-less stream routed through `MediaElementSource` is silenced.

use std::fs::File;
use std::path::Path;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase};

/// What the engine and the UI need to know about the stream being played.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub codec: String,
    /// `None` for radio, which has no end.
    pub duration_secs: Option<f64>,
}

pub struct Decoder {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    format: SourceFormat,
    /// Interleaved samples decoded but not yet handed to the caller. A packet
    /// rarely lines up with a device buffer, so the remainder waits here.
    pending: Vec<f32>,
    pending_read: usize,
    position_secs: f64,
    exhausted: bool,
}

impl Decoder {
    pub fn open_file(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("open {}: {}", path.display(), e))?;
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(extension);
        }
        Self::open(Box::new(file), hint)
    }

    /// Builds a decoder over any media source. `hint` only steers format
    /// detection; probing still confirms by reading the actual bytes.
    pub fn open(source: Box<dyn MediaSource>, hint: Hint) -> Result<Self, String> {
        // A live stream cannot be seeked, so probing has to backtrack inside
        // this buffer instead. The 64 KB default is not always enough to
        // recognise ADTS/AAC, and it costs a file nothing.
        let stream = MediaSourceStream::new(
            source,
            MediaSourceStreamOptions {
                buffer_len: 128 * 1024,
            },
        );
        let reader = symphonia::default::get_probe()
            .probe(
                &hint,
                stream,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| format!("probe format: {}", e))?;

        let track = reader
            .default_track(TrackType::Audio)
            .ok_or_else(|| "no audio track in this source".to_string())?;

        let track_id = track.id;
        let time_base = track.time_base;
        let audio_params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| "audio track has no codec parameters".to_string())?;

        let sample_rate = audio_params.sample_rate.unwrap_or(0);
        let channels = audio_params
            .channels
            .as_ref()
            .map(|c| c.count())
            .unwrap_or(0) as u16;
        let duration_secs = track
            .num_frames
            .filter(|_| sample_rate > 0)
            .map(|frames| frames as f64 / sample_rate as f64)
            .or_else(|| {
                let base = time_base?;
                base.calc_duration(track.duration?).map(|t| t.as_secs_f64())
            });

        // `gapless` trims the encoder delay and padding that MP3 and AAC carry
        // at the head and tail of every file — the difference between a
        // continuous-mix album playing seamlessly and clicking at each join.
        // It defaults to true; set it explicitly so the intent is on the page.
        let decoder_options = AudioDecoderOptions::default().gapless(true);
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(audio_params, &decoder_options)
            .map_err(|e| format!("no decoder for this codec: {}", e))?;

        let codec = decoder.codec_info().short_name.to_string();

        Ok(Self {
            reader,
            decoder,
            track_id,
            time_base,
            format: SourceFormat {
                sample_rate,
                channels,
                codec,
                duration_secs,
            },
            pending: Vec::new(),
            pending_read: 0,
            position_secs: 0.0,
            exhausted: false,
        })
    }

    pub fn format(&self) -> &SourceFormat {
        &self.format
    }

    pub fn is_exhausted(&self) -> bool {
        self.exhausted
    }

    /// Fills `out` with interleaved samples at the source's own rate and
    /// channel count. Returns how many were written; a short write means the
    /// source ended.
    pub fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let mut written = 0;
        while written < out.len() {
            if self.pending_read < self.pending.len() {
                let take = (out.len() - written).min(self.pending.len() - self.pending_read);
                out[written..written + take]
                    .copy_from_slice(&self.pending[self.pending_read..self.pending_read + take]);
                self.pending_read += take;
                written += take;
                continue;
            }
            if !self.decode_next_packet()? {
                self.exhausted = true;
                break;
            }
        }
        Ok(written)
    }

    /// Decodes forward until a packet yields samples. `Ok(false)` means the
    /// source is finished.
    fn decode_next_packet(&mut self) -> Result<bool, String> {
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                // End of stream is a value in 0.6, not an error.
                Ok(None) => return Ok(false),
                // The track list changed mid-stream (chained OGG). Rebuilding
                // decoders here would be a bigger change than it is worth;
                // ending the track is honest and cannot corrupt the output.
                Err(SymphoniaError::ResetRequired) => return Ok(false),
                Err(e) => return Err(format!("read packet: {}", e)),
            };

            if packet.track_id != self.track_id {
                continue;
            }

            if let Some(base) = self.time_base {
                if let Some(time) = base.calc_time(packet.pts) {
                    self.position_secs = time.as_secs_f64();
                }
            }

            match self.decoder.decode(&packet) {
                Ok(buffer) => {
                    // A mid-stream format change is legal; keep the reported
                    // format honest so the UI and the resampler both follow.
                    let spec = buffer.spec();
                    self.format.sample_rate = spec.rate();
                    self.format.channels = spec.channels().count() as u16;

                    self.pending.resize(buffer.samples_interleaved(), 0.0);
                    buffer.copy_to_slice_interleaved(&mut self.pending);
                    self.pending_read = 0;
                    if !self.pending.is_empty() {
                        return Ok(true);
                    }
                }
                // A corrupt packet is skippable — decoding continues with the
                // next one rather than ending the track on one bad frame.
                Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::IoError(_)) => continue,
                Err(e) => return Err(format!("decode packet: {}", e)),
            }
        }
    }

    /// Seeks to `secs`, returning the position actually reached.
    pub fn seek(&mut self, secs: f64) -> Result<f64, String> {
        let time = Time::try_from_secs_f64(secs.max(0.0))
            .ok_or_else(|| format!("seek target out of range: {}", secs))?;

        let seeked = self
            .reader
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| format!("seek: {}", e))?;

        // Mandatory after a seek: the decoder's state belongs to the old
        // position and would otherwise ring into the new one.
        self.decoder.reset();
        self.pending.clear();
        self.pending_read = 0;
        self.exhausted = false;

        self.position_secs = self
            .time_base
            .and_then(|base| base.calc_time(seeked.actual_ts))
            .map(|t| t.as_secs_f64())
            .unwrap_or(secs);

        Ok(self.position_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16-bit PCM WAV built in memory, so the decode path can be tested
    /// without shipping a binary fixture.
    fn wav_bytes(sample_rate: u32, channels: u16, frames: usize) -> Vec<u8> {
        let bits = 16u16;
        let block_align = channels * bits / 8;
        let data_len = frames as u32 * block_align as u32;

        let mut out = Vec::new();
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

        for frame in 0..frames {
            // A ramp, so a test can tell which frame it is looking at.
            let value = ((frame as f32 / frames as f32) * 30_000.0) as i16;
            for _ in 0..channels {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        out
    }

    fn decoder_over(bytes: Vec<u8>) -> Decoder {
        let mut hint = Hint::new();
        hint.with_extension("wav");
        Decoder::open(Box::new(std::io::Cursor::new(bytes)), hint).expect("wav should decode")
    }

    #[test]
    fn reports_the_format_of_the_source() {
        let decoder = decoder_over(wav_bytes(44_100, 2, 1000));
        let format = decoder.format();
        assert_eq!(format.sample_rate, 44_100);
        assert_eq!(format.channels, 2);
        let duration = format.duration_secs.expect("a file has a known length");
        assert!(
            (duration - 1000.0 / 44_100.0).abs() < 1e-6,
            "got {duration}"
        );
    }

    #[test]
    fn reads_every_frame_then_reports_exhaustion() {
        let frames = 1000;
        let channels = 2;
        let mut decoder = decoder_over(wav_bytes(44_100, channels, frames));

        let mut total = 0;
        let mut buffer = [0.0f32; 256];
        loop {
            let written = decoder.read(&mut buffer).expect("decode should not fail");
            total += written;
            if written < buffer.len() {
                break;
            }
        }

        assert_eq!(
            total,
            frames * channels as usize,
            "every sample must arrive"
        );
        assert!(decoder.is_exhausted());
    }

    #[test]
    fn a_short_read_only_happens_at_the_end() {
        let mut decoder = decoder_over(wav_bytes(44_100, 2, 1000));
        let mut buffer = [0.0f32; 64];
        // 1000 frames x 2ch = 2000 samples; the first 31 reads are full.
        for _ in 0..31 {
            assert_eq!(decoder.read(&mut buffer).unwrap(), 64);
        }
        assert_eq!(decoder.read(&mut buffer).unwrap(), 16, "the tail is short");
    }

    #[test]
    fn seeking_moves_the_reported_position() {
        let mut decoder = decoder_over(wav_bytes(44_100, 2, 44_100));
        let landed = decoder.seek(0.5).expect("wav is seekable");
        assert!((landed - 0.5).abs() < 0.01, "landed at {landed}");
        assert!((decoder.position_secs - landed).abs() < f64::EPSILON);
    }

    #[test]
    fn seeking_clears_buffered_samples_from_the_old_position() {
        let mut decoder = decoder_over(wav_bytes(44_100, 1, 44_100));
        let mut buffer = [0.0f32; 16];
        decoder.read(&mut buffer).unwrap();
        let before = buffer[0];

        decoder.seek(0.9).unwrap();
        decoder.read(&mut buffer).unwrap();

        // The ramp rises over the file, so a later seek must read higher.
        assert!(buffer[0] > before, "seek should not replay stale samples");
    }

    #[test]
    fn a_source_with_no_audio_track_is_rejected() {
        let hint = Hint::new();
        let result = Decoder::open(Box::new(std::io::Cursor::new(b"not audio".to_vec())), hint);
        assert!(result.is_err(), "garbage must not probe as playable");
    }
}
