//! Station "now playing" APIs.
//!
//! ICY metadata is one free-form string and some stations send none at all —
//! every Radio France webradio, for instance. Where a station's operator
//! publishes a proper endpoint, this asks that instead and gets artist, title,
//! album and often cover art as separate fields.
//!
//! A station either has a provider or it does not. When it does the provider
//! is the only source, because merging two feeds that disagree about what is
//! playing produces worse answers than either alone. Everything else falls
//! back to the ICY parser in [`super::icy`].
//!
//! Cover art is downloaded and re-encoded here rather than linked, because the
//! webview's CSP allows no remote images — the same reason local cover art
//! crosses IPC as base64.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use crossbeam_channel::Sender;
use serde::Deserialize;
use stream_download::http::reqwest::{redirect, Client, Response, Url};

use super::engine::EngineCommand;
use super::icy::TrackInfo;

/// Where a station publishes what it is playing. Supplied by the frontend,
/// which owns the station list.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "provider", content = "key")]
pub enum Source {
    /// SomaFM channel slug, e.g. `groovesalad`.
    Somafm(String),
    /// Radio France numeric station id, e.g. `7` for FIP.
    Radiofrance(String),
    /// Radio Paradise channel number, e.g. `0` for the Main Mix.
    Radioparadise(String),
}

/// One answer from a provider.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Update {
    pub info: Option<TrackInfo>,
    /// Cover art as a `data:` URL, ready for an `<img src>`.
    pub cover: Option<String>,
}

/// Fallback cadence for providers that do not say when the track ends.
const DEFAULT_POLL: Duration = Duration::from_secs(15);
/// Backoff after a failed request, so a station being down is not hammered.
const ERROR_POLL: Duration = Duration::from_secs(30);
/// Bounds on a provider-supplied wait, in case a timestamp is nonsense.
const MIN_POLL: Duration = Duration::from_secs(5);
const MAX_POLL: Duration = Duration::from_secs(300);
/// Cover art larger than this is skipped rather than pushed through IPC.
/// Enforced while downloading, not after — the cap must bound what reaches
/// RAM, and the body size is chosen by the remote end.
const MAX_COVER_BYTES: usize = 2 * 1024 * 1024;
/// A now-playing JSON answer is a few KB; anything past this is not one.
const MAX_JSON_BYTES: usize = 512 * 1024;

/// Polls `source` until `epoch` changes, reporting each change to the engine.
///
/// `epoch` is bumped by the engine whenever what is playing changes, which is
/// how a poller for a station the listener has left stops on its own.
pub fn spawn(commands: Sender<EngineCommand>, epoch: Arc<AtomicU64>, mine: u64, source: Source) {
    tauri::async_runtime::spawn(async move {
        // No redirects: every URL this client fetches is either one we build
        // against a provider's https API, or a cover URL out of third-party
        // JSON that has already passed the host allowlist — and a redirect
        // would walk it straight back off that list.
        let client = match Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(redirect::Policy::none())
            .build()
        {
            Ok(client) => client,
            Err(e) => {
                log::warn!("now-playing client: {}", e);
                return;
            }
        };
        let mut last = Update::default();

        loop {
            if epoch.load(Ordering::Relaxed) != mine {
                return;
            }
            let wait = match fetch(&client, &source).await {
                Ok((update, wait)) => {
                    // Still the same station? The await above may have
                    // outlived the listener's interest in it.
                    if epoch.load(Ordering::Relaxed) != mine {
                        return;
                    }
                    if update != last {
                        last = update.clone();
                        if commands
                            .send(EngineCommand::StationMetadata {
                                epoch: mine,
                                update,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    wait
                }
                Err(message) => {
                    log::warn!("now playing for {:?}: {}", source, message);
                    ERROR_POLL
                }
            };
            tokio::time::sleep(wait).await;
        }
    });
}

/// Asks one provider what is playing, and how long to wait before asking again.
async fn fetch(client: &Client, source: &Source) -> Result<(Update, Duration), String> {
    match source {
        Source::Somafm(channel) => somafm(client, channel).await,
        Source::Radiofrance(id) => radiofrance(client, id).await,
        Source::Radioparadise(channel) => radioparadise(client, channel).await,
    }
}

async fn get_json<T: for<'de> Deserialize<'de>>(client: &Client, url: &str) -> Result<T, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("request {}: {}", url, e))?;
    if !response.status().is_success() {
        return Err(format!("{} returned {}", url, response.status()));
    }
    let body = read_bounded(response, MAX_JSON_BYTES)
        .await
        .map_err(|e| format!("read {}: {}", url, e))?;
    serde_json::from_slice(&body).map_err(|e| format!("decode {}: {}", url, e))
}

/// Reads a response body with a hard byte budget, chunk by chunk, so the cap
/// bounds the allocation rather than being checked after the whole body has
/// already landed in memory.
async fn read_bounded(mut response: Response, budget: usize) -> Result<Vec<u8>, String> {
    if let Some(length) = response.content_length() {
        if length > budget as u64 {
            return Err(format!("declared {} bytes, budget is {}", length, budget));
        }
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("read body: {}", e))?
    {
        if body.len() + chunk.len() > budget {
            return Err(format!("body exceeds the {} byte budget", budget));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

// ── SomaFM ──────────────────────────────────────────────────────────────
// https://somafm.com/songs/{channel}.json — newest first. No cover art.

#[derive(Deserialize)]
struct SomaResponse {
    songs: Vec<SomaSong>,
}

#[derive(Deserialize)]
struct SomaSong {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
}

async fn somafm(client: &Client, channel: &str) -> Result<(Update, Duration), String> {
    let url = format!("https://somafm.com/songs/{}.json", channel);
    let body: SomaResponse = get_json(client, &url).await?;
    let current = body.songs.into_iter().next();
    Ok((
        Update {
            info: current.and_then(|s| track(s.title, s.artist, s.album)),
            cover: None,
        },
        DEFAULT_POLL,
    ))
}

// ── Radio France ────────────────────────────────────────────────────────
// https://api.radiofrance.fr/livemeta/pull/{id} — a map of programme steps,
// each with start/end. The one covering now is what is playing, and it also
// carries the album art the other providers mostly lack.

#[derive(Deserialize)]
struct RadioFranceResponse {
    steps: std::collections::HashMap<String, RadioFranceStep>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RadioFranceStep {
    title: Option<String>,
    authors: Option<String>,
    #[serde(rename = "titreAlbum")]
    album: Option<String>,
    visual: Option<String>,
    start: Option<i64>,
    end: Option<i64>,
}

/// Hosts Radio France cover art may come from; everything else in `visual`
/// is refused.
const RADIOFRANCE_COVER_HOSTS: &[&str] = &["radiofrance.fr", "radiofrance-podcast.net"];

async fn radiofrance(client: &Client, id: &str) -> Result<(Update, Duration), String> {
    let url = format!("https://api.radiofrance.fr/livemeta/pull/{}", id);
    let body: RadioFranceResponse = get_json(client, &url).await?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let Some(step) = pick_step(&body.steps, now) else {
        return Ok((Update::default(), DEFAULT_POLL));
    };

    let wait = radiofrance_wait(step.end, now);

    let cover = match step.visual.as_deref() {
        Some(url) => cover_data_url(client, url, RADIOFRANCE_COVER_HOSTS).await,
        None => None,
    };

    Ok((
        Update {
            info: track(step.title.clone(), step.authors.clone(), step.album.clone()),
            cover,
        },
        wait,
    ))
}

/// The step spanning `now`, else the most recent one that has begun.
fn pick_step(
    steps: &std::collections::HashMap<String, RadioFranceStep>,
    now: i64,
) -> Option<&RadioFranceStep> {
    steps
        .values()
        .filter(|s| s.title.is_some())
        .filter(|s| s.start.is_none_or(|start| start <= now))
        .max_by_key(|s| s.start.unwrap_or(0))
}

/// The station says when the track ends, so the next poll can land just after
/// it rather than guessing — bounded in case the timestamp is nonsense.
fn radiofrance_wait(end: Option<i64>, now: i64) -> Duration {
    end.map(|end| Duration::from_secs((end - now).max(1) as u64 + 1))
        .unwrap_or(DEFAULT_POLL)
        .clamp(MIN_POLL, MAX_POLL)
}

// ── Radio Paradise ──────────────────────────────────────────────────────
// https://api.radioparadise.com/api/now_playing?chan=N

#[derive(Deserialize)]
struct RadioParadiseResponse {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    cover: Option<String>,
}

const RADIOPARADISE_COVER_HOSTS: &[&str] = &["radioparadise.com"];

async fn radioparadise(client: &Client, channel: &str) -> Result<(Update, Duration), String> {
    let url = format!(
        "https://api.radioparadise.com/api/now_playing?chan={}",
        channel
    );
    let body: RadioParadiseResponse = get_json(client, &url).await?;
    let cover = match body.cover.as_deref() {
        Some(url) => cover_data_url(client, url, RADIOPARADISE_COVER_HOSTS).await,
        None => None,
    };
    Ok((
        Update {
            info: track(body.title, body.artist, body.album),
            cover,
        },
        DEFAULT_POLL,
    ))
}

// ── shared ──────────────────────────────────────────────────────────────

/// Builds a `TrackInfo`, treating blank strings as absent. A provider with no
/// title has told us nothing worth showing.
fn track(
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
) -> Option<TrackInfo> {
    let clean = |v: Option<String>| {
        v.map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "None")
    };
    Some(TrackInfo {
        title: clean(title)?,
        artist: clean(artist),
        album: clean(album),
    })
}

/// Whether a provider-supplied cover URL points at one of the provider's own
/// hosts, over https.
///
/// The URL comes straight out of third-party JSON, and this process fetches
/// from inside the user's network: unchecked, a hostile or compromised feed
/// could aim GETs at the LAN (a router's admin page answers requests it
/// trusts) or name any host as a per-listener tracking beacon. Only the
/// provider's own domains are ever fetched.
fn cover_url_allowed(url: &Url, allowed_hosts: &[&str]) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    allowed_hosts
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

/// Downloads cover art and re-encodes it as a `data:` URL.
///
/// Best-effort: art is decoration, so a failure logs and yields `None` rather
/// than failing the whole update.
async fn cover_data_url(client: &Client, url: &str, allowed_hosts: &[&str]) -> Option<String> {
    let parsed = url.parse::<Url>().ok()?;
    if !cover_url_allowed(&parsed, allowed_hosts) {
        log::warn!("cover art url refused: {}", url);
        return None;
    }
    let response = client.get(parsed).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mime = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .split(';')
        .next()
        .unwrap_or("image/jpeg")
        .to_string();
    if !mime.starts_with("image/") {
        return None;
    }
    let bytes = match read_bounded(response, MAX_COVER_BYTES).await {
        Ok(bytes) => bytes,
        Err(e) => {
            log::warn!("cover art at {}: {}", url, e);
            return None;
        }
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(format!("data:{};base64,{}", mime, encoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_deserialise_from_the_frontend_shape() {
        let parse = |s: &str| serde_json::from_str::<Source>(s).unwrap();
        assert_eq!(
            parse(r#"{"provider":"somafm","key":"groovesalad"}"#),
            Source::Somafm("groovesalad".into())
        );
        assert_eq!(
            parse(r#"{"provider":"radiofrance","key":"7"}"#),
            Source::Radiofrance("7".into())
        );
        assert_eq!(
            parse(r#"{"provider":"radioparadise","key":"0"}"#),
            Source::Radioparadise("0".into())
        );
    }

    #[test]
    fn blank_and_placeholder_fields_are_dropped() {
        assert_eq!(
            track(None, None, None),
            None,
            "no title means nothing to show"
        );
        assert_eq!(track(Some("  ".into()), None, None), None);
        // Radio France sends the literal string "None" for absent albums.
        let info = track(
            Some("Cry".into()),
            Some("Jon Batiste".into()),
            Some("None".into()),
        )
        .expect("a title is enough");
        assert_eq!(info.album, None);
        assert_eq!(info.artist.as_deref(), Some("Jon Batiste"));
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let info = track(
            Some(" Vibe 05 ".into()),
            Some(" Alex Cortiz ".into()),
            Some(" The Chill Out Room ".into()),
        )
        .unwrap();
        assert_eq!(info.title, "Vibe 05");
        assert_eq!(info.artist.as_deref(), Some("Alex Cortiz"));
        assert_eq!(info.album.as_deref(), Some("The Chill Out Room"));
    }

    #[test]
    fn the_step_spanning_now_is_picked() {
        // A captured-shape Radio France body, so the serde renames
        // (`titreAlbum`, camelCase) are pinned by a test that runs offline.
        let body: RadioFranceResponse = serde_json::from_str(
            r#"{"steps":{
                "a":{"title":"Earlier","authors":"A","titreAlbum":"Old LP","visual":null,"start":100,"end":200},
                "b":{"title":"Now Playing","authors":"B","titreAlbum":"New LP","visual":null,"start":200,"end":300},
                "c":{"title":"Later","authors":"C","titreAlbum":null,"visual":null,"start":300,"end":400},
                "d":{"authors":"No title","titreAlbum":null,"visual":null,"start":250,"end":260}
            }}"#,
        )
        .expect("the captured shape must deserialise");

        let step = pick_step(&body.steps, 250).expect("a step covers 250");
        assert_eq!(step.title.as_deref(), Some("Now Playing"));
        assert_eq!(
            step.album.as_deref(),
            Some("New LP"),
            "titreAlbum maps to album"
        );
        assert_eq!(step.end, Some(300));

        assert!(
            pick_step(&body.steps, 50).is_none(),
            "nothing has begun yet at 50"
        );
    }

    #[test]
    fn the_poll_lands_just_after_the_announced_end() {
        assert_eq!(radiofrance_wait(Some(250), 200), Duration::from_secs(51));
        assert_eq!(radiofrance_wait(None, 200), DEFAULT_POLL);
        assert_eq!(
            radiofrance_wait(Some(100), 200),
            MIN_POLL,
            "an end already past still waits the minimum"
        );
        assert_eq!(
            radiofrance_wait(Some(1_000_000), 0),
            MAX_POLL,
            "a nonsense timestamp is clamped"
        );
    }

    #[test]
    fn cover_fetches_stay_on_the_provider_hosts() {
        let allowed = |url: &str| cover_url_allowed(&url.parse().unwrap(), RADIOFRANCE_COVER_HOSTS);

        assert!(allowed("https://www.radiofrance.fr/img/cover.jpg"));
        assert!(allowed("https://cdn.radiofrance-podcast.net/x.jpg"));
        assert!(
            !allowed("http://www.radiofrance.fr/img/cover.jpg"),
            "plain http is refused even on an allowed host"
        );
        assert!(
            !allowed("https://192.168.1.1/setup.cgi?reboot=1"),
            "a feed must not be able to aim requests into the LAN"
        );
        assert!(
            !allowed("https://evilradiofrance.fr/x.jpg"),
            "suffix matching must not accept a lookalike registration"
        );
        assert!(!allowed("https://example.com/radiofrance.fr"));
    }

    #[test]
    fn poll_bounds_are_sane() {
        assert!(MIN_POLL <= DEFAULT_POLL && DEFAULT_POLL <= MAX_POLL);
        assert!(
            MIN_POLL.as_secs() >= 5,
            "polling a public API faster than this would be rude"
        );
    }

    /// Asks each provider for real. Ignored by default because it needs the
    /// network and depends on third-party services staying up; run it when
    /// changing a parser or when a station stops showing titles:
    /// `cargo test -- --ignored every_provider_answers --nocapture`
    #[test]
    #[ignore = "requires network access to third-party station APIs"]
    fn every_provider_answers() {
        let sources = [
            Source::Somafm("groovesalad".into()),
            Source::Radiofrance("7".into()),
            Source::Radioparadise("0".into()),
        ];

        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("http client");

        for source in sources {
            let (update, wait) = tauri::async_runtime::block_on(fetch(&client, &source))
                .unwrap_or_else(|e| panic!("{source:?} failed: {e}"));

            let info = update
                .info
                .unwrap_or_else(|| panic!("{source:?} returned no track"));
            assert!(!info.title.is_empty(), "{source:?} returned a blank title");
            assert!(
                wait >= MIN_POLL && wait <= MAX_POLL,
                "{source:?} asked for a {wait:?} wait"
            );
            println!(
                "{source:?}\n    {} — {} [{}] cover={} next in {:?}",
                info.artist.as_deref().unwrap_or("(no artist)"),
                info.title,
                info.album.as_deref().unwrap_or("(no album)"),
                update
                    .cover
                    .map_or("no", |c| if c.starts_with("data:image/") {
                        "yes"
                    } else {
                        "MALFORMED"
                    }),
                wait,
            );
        }
    }
}
