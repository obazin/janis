//! The codec registry Janis decodes with.
//!
//! `symphonia::default::get_codecs()` returns a `&'static CodecRegistry` that
//! cannot be extended, so adding the libopus-backed Opus decoder means owning
//! the registry. This is that registry: everything Symphonia's feature flags
//! enable, plus [`super::opus::OpusDecoder`].

use std::sync::OnceLock;

use symphonia::core::codecs::registry::CodecRegistry;

use super::opus::OpusDecoder;

static CODECS: OnceLock<CodecRegistry> = OnceLock::new();

/// Like `symphonia::default::get_codecs()`, but knows about Opus.
pub fn get_codecs() -> &'static CodecRegistry {
    CODECS.get_or_init(|| {
        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        registry.register_audio_decoder::<OpusDecoder>();
        registry
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use symphonia::core::codecs::audio::well_known::{CODEC_ID_FLAC, CODEC_ID_OPUS};

    #[test]
    fn opus_is_registered() {
        // The whole point of owning a registry: Symphonia's default has no
        // Opus decoder, so this lookup is what makes `.opus` files playable.
        assert!(
            get_codecs().get_audio_decoder(CODEC_ID_OPUS).is_some(),
            "opus must resolve to a decoder"
        );
    }

    #[test]
    fn the_built_in_codecs_are_still_there() {
        // Building our own registry means re-registering everything the
        // feature flags enable; forgetting that would silently break FLAC.
        assert!(get_codecs().get_audio_decoder(CODEC_ID_FLAC).is_some());
    }
}
