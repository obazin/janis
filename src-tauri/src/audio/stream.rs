//! Web radio as a Symphonia media source.
//!
//! An Icecast/Shoutcast stream is buffered into something `Read + Seek` and
//! handed to the same [`Decoder`](super::decode::Decoder) a local file uses.
//! From there the path is identical — same resampler, same ring, same EQ,
//! same analyser. That is the whole point: in Rust there is no CORS, so radio
//! finally gets the equalizer and a real visualiser instead of a synthetic
//! animation.
//!
//! Connecting is async because the HTTP fetch is; the engine thread is not.
//! So the command layer opens the stream on Tauri's runtime and hands the
//! finished reader over, which also means the engine never blocks on a
//! network round trip.

use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use icy_metadata::{IcyHeaders, IcyMetadataReader, RequestIcyMetadata};
use stream_download::http::reqwest::Client;
use stream_download::http::HttpStream;
use stream_download::storage::bounded::BoundedStorageProvider;
use stream_download::storage::memory::MemoryStorageProvider;
use stream_download::{Settings, StreamDownload};
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSource;

/// The track title a station is currently announcing, shared with the engine
/// thread. Written from the reader (on the engine thread, during a read) and
/// polled on the engine's tick, so no reentrancy games are needed.
pub type NowPlaying = Arc<Mutex<Option<String>>>;

/// Seconds of audio to buffer before handing the stream over. Enough that a
/// brief network stall is inaudible, short enough that pressing a station
/// still feels immediate.
const PREFETCH_SECONDS: u64 = 5;
/// Fallback when a station does not advertise its bitrate.
const ASSUMED_KBPS: u64 = 128;
/// Cap on the rolling buffer held in memory for one station.
const BUFFER_BYTES: usize = 512 * 1024;

/// Marks a buffered stream as live: Symphonia must not try to seek in it or
/// ask how long it is.
struct RadioSource<R>(R);

impl<R: Read> Read for RadioSource<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl<R: Seek> Seek for RadioSource<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

impl<R: Read + Seek + Send + Sync> MediaSource for RadioSource<R> {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// A connected station, ready to decode.
pub struct RadioStream {
    pub source: Box<dyn MediaSource>,
    /// Steers format detection from the HTTP `Content-Type`, which matters
    /// because a stream URL has no file extension to go on.
    pub hint: Hint,
    pub now_playing: NowPlaying,
}

/// Connects to `url` and buffers enough of it to start decoding.
pub async fn open(url: &str) -> Result<RadioStream, String> {
    let parsed = url
        .parse()
        .map_err(|e| format!("bad station url {}: {}", url, e))?;

    // Asking for in-band metadata is what makes track titles available at all.
    let client = Client::builder()
        .request_icy_metadata()
        .build()
        .map_err(|e| format!("build http client: {}", e))?;

    let stream = HttpStream::new(client, parsed)
        .await
        .map_err(|e| format!("connect to {}: {}", url, e))?;

    let headers = IcyHeaders::parse_from_headers(stream.headers());
    let content_type = stream
        .content_type()
        .as_ref()
        .map(|c| format!("{}/{}", c.r#type, c.subtype));
    let kbps = headers.bitrate().map(u64::from).unwrap_or(ASSUMED_KBPS);
    let prefetch = kbps / 8 * 1024 * PREFETCH_SECONDS;

    // A bounded in-memory buffer: a station plays forever, so the alternative
    // is an unbounded temp file that grows for as long as it is left on.
    let storage = BoundedStorageProvider::new(
        MemoryStorageProvider,
        std::num::NonZeroUsize::new(BUFFER_BYTES).expect("buffer size is non-zero"),
    );

    let reader = StreamDownload::from_stream(
        stream,
        storage,
        Settings::default()
            .prefetch_bytes(prefetch)
            // Switching stations should stop the old download immediately
            // rather than leave it pulling bytes nobody will hear.
            .cancel_on_drop(true),
    )
    .await
    .map_err(|e| format!("buffer stream from {}: {:?}", url, e))?;

    let now_playing: NowPlaying = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&now_playing);
    let reader = IcyMetadataReader::new(reader, headers.metadata_interval(), move |metadata| {
        if let Ok(metadata) = metadata {
            if let Ok(mut guard) = sink.lock() {
                *guard = metadata.stream_title().map(str::to_string);
            }
        }
    });

    let mut hint = Hint::new();
    if let Some(mime) = content_type {
        hint.mime_type(&mime);
    }

    Ok(RadioStream {
        source: Box::new(RadioSource(reader)),
        hint,
        now_playing,
    })
}
