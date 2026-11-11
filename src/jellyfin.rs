//! The Jellyfin client: signing in, listing the server's songs, and the
//! addresses Janis plays and draws them from.
//!
//! Every call here waits on the network, so it runs on the server worker,
//! never on the interface thread. What the server answers is mapped into the
//! library's own types (`ServerSong`, `Metadata`) by pure functions, which is
//! where the tests are. The password is sent once, to sign in; what is kept is
//! the access token the server gives back.

use std::time::Duration;

use serde::Deserialize;

use crate::audio::{Location, Metadata, RemoteFile, LOSSLESS_EXTENSIONS};
use crate::library::ServerSong;
use crate::persistence::{MediaServer, NewMediaServer};

/// How Janis names itself to the server. No personal detail: the device is
/// told apart by the random identifier `persistence::device_identifier` makes.
const CLIENT_NAME: &str = "Janis";
/// Songs asked for per request while listing the library.
const PAGE_SIZE: usize = 500;
/// How long one request may take before the server counts as unreachable.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The port Jellyfin listens on out of the box.
const DEFAULT_PORT: u16 = 8096;
/// How a server song's library key starts; the rest is `<server>/<item>`.
const SONG_ADDRESS_SCHEME: &str = "jellyfin://";

/// Why a call to the server did not work out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// What was typed is not an address.
    InvalidAddress,
    /// Nothing answered, or not in time.
    Unreachable(String),
    /// Something answered, but not a Jellyfin server.
    NotJellyfin(String),
    /// The server refused the user name and password.
    CredentialsRejected,
    /// The server no longer accepts the kept token: signed out elsewhere, or
    /// revoked by an administrator.
    TokenRejected,
    /// Anything else, with its detail for the log.
    Other(String),
}

/// What a signed-in call needs: where the server is and who is asking.
#[derive(Clone)]
pub struct Session {
    address: String,
    user_identifier: String,
    access_token: String,
    device_identifier: String,
}

impl std::fmt::Debug for Session {
    /// The token stays out of any log.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("address", &self.address)
            .field("user_identifier", &self.user_identifier)
            .finish_non_exhaustive()
    }
}

impl Session {
    pub fn new(server: &MediaServer, device_identifier: &str) -> Self {
        Self {
            address: server.address.clone(),
            user_identifier: server.user_identifier.clone(),
            access_token: server.access_token.clone(),
            device_identifier: device_identifier.to_owned(),
        }
    }

    /// Where the engine fetches a song's original file. The token rides in
    /// the query because the engine sends no headers; the engine keeps it out
    /// of what it logs.
    pub fn stream_location(&self, song_address: &str, format: &str) -> Option<Location> {
        let item = item_of_song_address(song_address)?;
        let mut url = reqwest::Url::parse(&format!("{}/Audio/{item}/stream", self.address)).ok()?;
        url.query_pairs_mut()
            .append_pair("static", "true")
            .append_pair("api_key", &self.access_token);
        let extension = (!format.is_empty()).then(|| format.to_ascii_lowercase());
        Some(Location::Remote(RemoteFile {
            url: url.into(),
            extension,
        }))
    }
}

// ── Pure helpers ────────────────────────────────────────────────────────────

/// Turns what the listener typed into the server's base address: no
/// trailing slash, no query. A bare host (`192.168.1.20`, `jellyfin.local`)
/// gets Jellyfin's own defaults, `http` on port 8096; an address typed with
/// its scheme keeps that scheme's port, as a reverse proxy expects. A path is
/// kept, for a server behind a reverse proxy at `/jellyfin`.
pub fn normalise_server_address(text: &str) -> Result<String, Failure> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains(char::is_whitespace) {
        return Err(Failure::InvalidAddress);
    }
    let typed_scheme = trimmed.contains("://");
    let with_scheme = if typed_scheme {
        trimmed.to_owned()
    } else {
        format!("http://{trimmed}")
    };
    let mut url = reqwest::Url::parse(&with_scheme).map_err(|_| Failure::InvalidAddress)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(Failure::InvalidAddress);
    }
    if !typed_scheme && url.port().is_none() {
        url.set_port(Some(DEFAULT_PORT))
            .map_err(|()| Failure::InvalidAddress)?;
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// The library key of a song on a server.
pub fn song_address(server_identifier: &str, item: &str) -> String {
    format!("{SONG_ADDRESS_SCHEME}{server_identifier}/{item}")
}

/// The server's item behind a song's library key.
pub fn item_of_song_address(address: &str) -> Option<&str> {
    let rest = address.strip_prefix(SONG_ADDRESS_SCHEME)?;
    let (_, item) = rest.split_once('/')?;
    (!item.is_empty()).then_some(item)
}

/// The header that tells Jellyfin which client is calling, and as whom.
fn authorization(device_identifier: &str, token: Option<&str>) -> String {
    let mut header = format!(
        "MediaBrowser Client=\"{CLIENT_NAME}\", Device=\"{CLIENT_NAME}\", DeviceId=\"{device_identifier}\", Version=\"{}\"",
        env!("CARGO_PKG_VERSION")
    );
    if let Some(token) = token {
        header.push_str(&format!(", Token=\"{token}\""));
    }
    header
}

/// One audio item as the server lists it, reduced to what Janis reads.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Item {
    id: String,
    name: Option<String>,
    album: Option<String>,
    album_id: Option<String>,
    album_artist: Option<String>,
    artists: Vec<String>,
    index_number: Option<u32>,
    parent_index_number: Option<u32>,
    production_year: Option<u32>,
    genres: Vec<String>,
    run_time_ticks: Option<i64>,
    container: Option<String>,
    image_tags: ImageTags,
    album_primary_image_tag: Option<String>,
    people: Vec<Person>,
    media_sources: Vec<MediaSource>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct ImageTags {
    primary: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct Person {
    name: String,
    #[serde(rename = "Type")]
    kind: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct MediaSource {
    path: Option<String>,
    container: Option<String>,
    media_streams: Vec<MediaStream>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct MediaStream {
    #[serde(rename = "Type")]
    kind: Option<String>,
    codec: Option<String>,
    sample_rate: Option<u32>,
    bit_depth: Option<u8>,
    channels: Option<u8>,
}

/// Codecs that keep every bit, whatever container carries them.
const LOSSLESS_CODECS: &[&str] = &["flac", "alac", "wav", "aiff", "pcm"];

/// A Jellyfin "ticks" duration (tenths of a microsecond) in seconds.
const TICKS_PER_SECOND: f64 = 10_000_000.0;

/// A listed item as a library song.
pub fn item_song(item: &Item, server_identifier: &str) -> ServerSong {
    let source = item.media_sources.first();
    let stream = source.and_then(|source| {
        source
            .media_streams
            .iter()
            .find(|stream| stream.kind.as_deref() == Some("Audio"))
    });
    // The file's own extension is what the scanner would call its format;
    // the container field is the fallback (it may list several, `mov,mp4,…`).
    let extension = source
        .and_then(|source| source.path.as_deref())
        .and_then(|path| std::path::Path::new(path).extension())
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .or_else(|| {
            source
                .and_then(|source| source.container.as_deref())
                .or(item.container.as_deref())
                .and_then(|container| container.split(',').next())
                .map(|container| container.trim().to_ascii_lowercase())
        })
        .filter(|extension| !extension.is_empty());
    let codec = stream
        .and_then(|stream| stream.codec.as_deref())
        .map(str::to_ascii_lowercase);
    let lossless = extension
        .as_deref()
        .is_some_and(|extension| LOSSLESS_EXTENSIONS.contains(&extension))
        || codec.as_deref().is_some_and(|codec| {
            LOSSLESS_CODECS
                .iter()
                .any(|lossless| codec.starts_with(lossless))
        });

    let non_empty = |text: &Option<String>| text.clone().filter(|text| !text.trim().is_empty());
    let artist = if item.artists.is_empty() {
        non_empty(&item.album_artist)
    } else {
        Some(item.artists.join(", "))
    };
    let composers: Vec<&str> = item
        .people
        .iter()
        .filter(|person| person.kind.as_deref() == Some("Composer"))
        .map(|person| person.name.as_str())
        .collect();

    let artwork_item = if item.image_tags.primary.is_some() {
        Some(item.id.clone())
    } else if item.album_primary_image_tag.is_some() {
        item.album_id.clone()
    } else {
        None
    };

    ServerSong {
        address: song_address(server_identifier, &item.id),
        metadata: Metadata {
            title: non_empty(&item.name).unwrap_or_else(|| item.id.clone()),
            artist,
            album: non_empty(&item.album),
            composer: (!composers.is_empty()).then(|| composers.join(", ")),
            duration_secs: item
                .run_time_ticks
                .map(|ticks| ticks as f64 / TICKS_PER_SECOND)
                .unwrap_or(0.0),
            format: extension
                .as_deref()
                .map(str::to_ascii_uppercase)
                .unwrap_or_default(),
            sample_rate: stream.and_then(|stream| stream.sample_rate),
            bit_depth: stream.and_then(|stream| stream.bit_depth),
            channels: stream.and_then(|stream| stream.channels),
            lossless,
            track_number: item.index_number,
            track_total: None,
            disc_number: item.parent_index_number,
            disc_total: None,
            year: item.production_year,
            genre: item.genres.first().cloned(),
            album_artist: non_empty(&item.album_artist),
            // The engine measures a server song's loudness as it plays, as it
            // does for any untagged file.
            rg_track_gain_db: None,
            rg_track_peak: None,
            rg_album_gain_db: None,
            rg_album_peak: None,
        },
        artwork_item,
    }
}

// ── Calls ───────────────────────────────────────────────────────────────────

fn client() -> Result<reqwest::blocking::Client, Failure> {
    reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| Failure::Other(format!("build the http client: {error}")))
}

/// Sorts a transport error into what the listener is told.
fn transport_failure(error: reqwest::Error) -> Failure {
    // The address is a server's, not a secret, but the query of a stream or
    // picture request would carry the token: it is dropped from the detail.
    let error = error.without_url();
    if error.is_connect() || error.is_timeout() {
        Failure::Unreachable(error.to_string())
    } else {
        Failure::Other(error.to_string())
    }
}

/// Sorts an HTTP status into what the listener is told.
fn status_failure(status: reqwest::StatusCode, signing_in: bool) -> Failure {
    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN if signing_in => {
            Failure::CredentialsRejected
        }
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            Failure::TokenRejected
        }
        other => Failure::Other(format!("the server answered {other}")),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PublicSystemInfo {
    id: String,
    #[serde(default)]
    server_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthenticationResult {
    user: AuthenticatedUser,
    access_token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthenticatedUser {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ItemsPage {
    #[serde(default)]
    items: Vec<Item>,
    #[serde(default)]
    total_record_count: usize,
}

/// Checks the address answers as a Jellyfin server, then signs in. Returns
/// what to keep — the password is not part of it.
pub fn sign_in(
    address_text: &str,
    user_name: &str,
    password: &str,
    device_identifier: &str,
) -> Result<NewMediaServer, Failure> {
    let address = normalise_server_address(address_text)?;
    let client = client()?;

    let response = client
        .get(format!("{address}/System/Info/Public"))
        .send()
        .map_err(transport_failure)?;
    if !response.status().is_success() {
        return Err(Failure::NotJellyfin(format!(
            "the server answered {}",
            response.status()
        )));
    }
    let server: PublicSystemInfo = response
        .json()
        .map_err(|error| Failure::NotJellyfin(error.without_url().to_string()))?;

    let response = client
        .post(format!("{address}/Users/AuthenticateByName"))
        .header("Authorization", authorization(device_identifier, None))
        .json(&serde_json::json!({ "Username": user_name, "Pw": password }))
        .send()
        .map_err(transport_failure)?;
    if !response.status().is_success() {
        return Err(status_failure(response.status(), true));
    }
    let signed_in: AuthenticationResult = response
        .json()
        .map_err(|error| Failure::Other(error.without_url().to_string()))?;

    Ok(NewMediaServer {
        address,
        server_identifier: server.id,
        server_name: server.server_name,
        user_identifier: signed_in.user.id,
        user_name: signed_in.user.name,
        access_token: signed_in.access_token,
    })
}

/// Every song the user can see on the server, read page by page. Either the
/// whole listing or a failure: a partial one would read as deleted songs.
pub fn songs(session: &Session, server_identifier: &str) -> Result<Vec<ServerSong>, Failure> {
    let client = client()?;
    let mut songs = Vec::new();
    loop {
        let mut url = reqwest::Url::parse(&format!("{}/Items", session.address))
            .map_err(|error| Failure::Other(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("userId", &session.user_identifier)
            .append_pair("IncludeItemTypes", "Audio")
            .append_pair("Recursive", "true")
            .append_pair("Fields", "MediaSources,Genres,People")
            .append_pair("SortBy", "SortName")
            .append_pair("StartIndex", &songs.len().to_string())
            .append_pair("Limit", &PAGE_SIZE.to_string());
        let response = client
            .get(url)
            .header(
                "Authorization",
                authorization(&session.device_identifier, Some(&session.access_token)),
            )
            .send()
            .map_err(transport_failure)?;
        if !response.status().is_success() {
            return Err(status_failure(response.status(), false));
        }
        let page: ItemsPage = response
            .json()
            .map_err(|error| Failure::Other(error.without_url().to_string()))?;
        let received = page.items.len();
        songs.extend(
            page.items
                .iter()
                .map(|item| item_song(item, server_identifier)),
        );
        if received == 0 || songs.len() >= page.total_record_count {
            return Ok(songs);
        }
    }
}

/// An item's picture, as the server encodes it, no larger than needed.
pub fn artwork(session: &Session, item: &str, longest_side: u32) -> Result<Vec<u8>, Failure> {
    let mut url = reqwest::Url::parse(&format!("{}/Items/{item}/Images/Primary", session.address))
        .map_err(|error| Failure::Other(error.to_string()))?;
    url.query_pairs_mut()
        .append_pair("maxWidth", &longest_side.to_string())
        .append_pair("maxHeight", &longest_side.to_string());
    let response = client()?
        .get(url)
        .header(
            "Authorization",
            authorization(&session.device_identifier, Some(&session.access_token)),
        )
        .send()
        .map_err(transport_failure)?;
    if !response.status().is_success() {
        return Err(status_failure(response.status(), false));
    }
    response
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(transport_failure)
}

/// Ends the session on the server, so the token stops working. Best effort:
/// signing out in Janis goes ahead whether or not the server is reachable.
pub fn sign_out(session: &Session) {
    let outcome = client().and_then(|client| {
        client
            .post(format!("{}/Sessions/Logout", session.address))
            .header(
                "Authorization",
                authorization(&session.device_identifier, Some(&session.access_token)),
            )
            .send()
            .map_err(transport_failure)
    });
    if let Err(failure) = outcome {
        log::warn!("end the Jellyfin session: {failure:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in Jellyfin server on a local port: public info, a sign-in
    /// that takes one password, and a library of three songs it serves two at
    /// a time, to whoever presents the token.
    fn fake_server() -> String {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let address = format!("http://{}", listener.local_addr().expect("local address"));
        std::thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    continue;
                };
                let mut reader = BufReader::new(connection.try_clone().expect("clone"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut content_length = 0;
                let mut authorization = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(value) = lower.strip_prefix("content-length:") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                    if lower.starts_with("authorization:") {
                        authorization = line.clone();
                    }
                }
                let mut body = vec![0; content_length];
                let _ = reader.read_exact(&mut body);
                let body = String::from_utf8_lossy(&body);
                let target = request_line.split_whitespace().nth(1).unwrap_or_default();
                let signed_in = authorization.contains("Token=\"token\"");
                let (status, answer) = if target == "/System/Info/Public" {
                    (
                        "200 OK",
                        r#"{"Id": "server", "ServerName": "Home"}"#.to_owned(),
                    )
                } else if target == "/Users/AuthenticateByName" {
                    if body.contains(r#""Pw":"right""#) {
                        (
                            "200 OK",
                            r#"{"User": {"Id": "user", "Name": "listener"}, "AccessToken": "token"}"#
                                .to_owned(),
                        )
                    } else {
                        ("401 Unauthorized", String::new())
                    }
                } else if target.starts_with("/Items?") && signed_in {
                    let start: usize = target
                        .split("StartIndex=")
                        .nth(1)
                        .and_then(|rest| rest.split('&').next())
                        .and_then(|number| number.parse().ok())
                        .unwrap_or(0);
                    let items: Vec<String> = (start..3.min(start + 2))
                        .map(|index| format!(r#"{{"Id": "song{index}", "Name": "Song {index}"}}"#))
                        .collect();
                    (
                        "200 OK",
                        format!(
                            r#"{{"Items": [{}], "TotalRecordCount": 3}}"#,
                            items.join(",")
                        ),
                    )
                } else {
                    ("401 Unauthorized", String::new())
                };
                let _ = connection.write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                        answer.len()
                    )
                    .as_bytes(),
                );
            }
        });
        address
    }

    fn signed_in_session(server: NewMediaServer) -> Session {
        Session::new(
            &MediaServer {
                id: 1,
                address: server.address,
                server_identifier: server.server_identifier,
                server_name: server.server_name,
                user_identifier: server.user_identifier,
                user_name: server.user_name,
                access_token: server.access_token,
                last_sync_at: None,
            },
            "device",
        )
    }

    #[test]
    fn signing_in_keeps_the_token_and_the_listing_reads_every_page() {
        let address = fake_server();
        let server = sign_in(&address, "listener", "right", "device").expect("signed in");
        assert_eq!(server.server_identifier, "server");
        assert_eq!(server.server_name, "Home");
        assert_eq!(server.user_identifier, "user");
        assert_eq!(server.access_token, "token");

        let songs = songs(&signed_in_session(server), "server").expect("the listing");
        let addresses: Vec<&str> = songs.iter().map(|song| song.address.as_str()).collect();
        assert_eq!(
            addresses,
            [
                "jellyfin://server/song0",
                "jellyfin://server/song1",
                "jellyfin://server/song2"
            ],
            "the server's pages of two are all read"
        );
    }

    #[test]
    fn a_wrong_password_and_a_revoked_token_are_told_apart() {
        let address = fake_server();
        assert_eq!(
            sign_in(&address, "listener", "wrong", "device"),
            Err(Failure::CredentialsRejected)
        );
        let mut server = sign_in(&address, "listener", "right", "device").expect("signed in");
        server.access_token = "revoked".to_owned();
        assert_eq!(
            songs(&signed_in_session(server), "server").map(|songs| songs.len()),
            Err(Failure::TokenRejected)
        );
    }

    #[test]
    fn a_closed_port_is_unreachable() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free port")
            .port();
        assert!(matches!(
            sign_in(&format!("127.0.0.1:{port}"), "listener", "right", "device"),
            Err(Failure::Unreachable(_))
        ));
    }

    #[test]
    fn an_address_gains_a_scheme_and_loses_its_trailing_slash() {
        assert_eq!(
            normalise_server_address(" jellyfin.local:8096/ "),
            Ok("http://jellyfin.local:8096".to_owned())
        );
        assert_eq!(
            normalise_server_address("https://example.org/jellyfin/?x=1#top"),
            Ok("https://example.org/jellyfin".to_owned())
        );
    }

    #[test]
    fn a_bare_host_gets_jellyfins_port_and_a_typed_scheme_keeps_its_own() {
        assert_eq!(
            normalise_server_address("100.78.170.73"),
            Ok("http://100.78.170.73:8096".to_owned())
        );
        assert_eq!(
            normalise_server_address("jellyfin.local/jellyfin"),
            Ok("http://jellyfin.local:8096/jellyfin".to_owned())
        );
        assert_eq!(
            normalise_server_address("https://media.example.org"),
            Ok("https://media.example.org".to_owned())
        );
        assert_eq!(
            normalise_server_address("http://192.168.1.20"),
            Ok("http://192.168.1.20".to_owned())
        );
    }

    #[test]
    fn what_is_not_a_server_address_is_refused() {
        for typed in ["", "   ", "ftp://host", "http://", "two words"] {
            assert_eq!(
                normalise_server_address(typed),
                Err(Failure::InvalidAddress),
                "{typed:?}"
            );
        }
    }

    #[test]
    fn a_song_address_names_its_item() {
        let address = song_address("server", "0a1b");
        assert_eq!(address, "jellyfin://server/0a1b");
        assert_eq!(item_of_song_address(&address), Some("0a1b"));
        assert_eq!(item_of_song_address("/music/a.flac"), None);
        assert_eq!(item_of_song_address("jellyfin://server/"), None);
    }

    fn session() -> Session {
        Session::new(
            &MediaServer {
                id: 1,
                address: "http://jellyfin.local:8096".to_owned(),
                server_identifier: "server".to_owned(),
                server_name: "Home".to_owned(),
                user_identifier: "user".to_owned(),
                user_name: "listener".to_owned(),
                access_token: "token".to_owned(),
                last_sync_at: None,
            },
            "device",
        )
    }

    #[test]
    fn a_song_streams_its_original_file_with_the_token() {
        let location = session()
            .stream_location("jellyfin://server/item", "FLAC")
            .expect("a song address");
        assert_eq!(
            location,
            Location::Remote(RemoteFile {
                url: "http://jellyfin.local:8096/Audio/item/stream?static=true&api_key=token"
                    .to_owned(),
                extension: Some("flac".to_owned()),
            })
        );
        assert_eq!(session().stream_location("/music/a.flac", "FLAC"), None);
    }

    #[test]
    fn the_session_never_shows_its_token() {
        assert!(!format!("{:?}", session()).contains("token"));
    }

    #[test]
    fn the_authorization_header_names_the_client_and_carries_the_token() {
        let header = authorization("device", Some("token"));
        assert!(header.starts_with("MediaBrowser Client=\"Janis\""));
        assert!(header.contains("DeviceId=\"device\""));
        assert!(header.ends_with("Token=\"token\""));
        assert!(!authorization("device", None).contains("Token"));
    }

    const LISTED_SONG: &str = r#"{
        "Id": "song",
        "Name": "Alive",
        "Album": "Homework",
        "AlbumId": "album",
        "AlbumArtist": "Daft Punk",
        "Artists": ["Daft Punk", "Guest"],
        "IndexNumber": 7,
        "ParentIndexNumber": 1,
        "ProductionYear": 1997,
        "Genres": ["Electronic", "House"],
        "RunTimeTicks": 2650000000,
        "Container": "flac",
        "ImageTags": {},
        "AlbumPrimaryImageTag": "tag",
        "People": [{"Name": "Thomas", "Type": "Composer"}, {"Name": "Singer", "Type": "Artist"}],
        "MediaSources": [{
            "Path": "/media/music/Daft Punk/Homework/07 - Alive.flac",
            "Container": "flac",
            "MediaStreams": [
                {"Type": "EmbeddedImage", "Codec": "mjpeg"},
                {"Type": "Audio", "Codec": "flac", "SampleRate": 96000, "BitDepth": 24, "Channels": 2}
            ]
        }]
    }"#;

    #[test]
    fn a_listed_song_maps_onto_the_library() {
        let item: Item = serde_json::from_str(LISTED_SONG).expect("a listed item parses");
        let song = item_song(&item, "server");
        let metadata = &song.metadata;

        assert_eq!(song.address, "jellyfin://server/song");
        assert_eq!(metadata.title, "Alive");
        assert_eq!(metadata.artist.as_deref(), Some("Daft Punk, Guest"));
        assert_eq!(metadata.album.as_deref(), Some("Homework"));
        assert_eq!(metadata.album_artist.as_deref(), Some("Daft Punk"));
        assert_eq!(metadata.composer.as_deref(), Some("Thomas"));
        assert_eq!(metadata.track_number, Some(7));
        assert_eq!(metadata.disc_number, Some(1));
        assert_eq!(metadata.year, Some(1997));
        assert_eq!(metadata.genre.as_deref(), Some("Electronic"));
        assert!((metadata.duration_secs - 265.0).abs() < 1e-9);
        assert_eq!(metadata.format, "FLAC");
        assert!(metadata.lossless);
        assert_eq!(
            (metadata.sample_rate, metadata.bit_depth, metadata.channels),
            (Some(96_000), Some(24), Some(2))
        );
        assert_eq!(
            song.artwork_item.as_deref(),
            Some("album"),
            "a song without its own picture shows its album's"
        );
    }

    #[test]
    fn a_sparse_song_still_maps() {
        let item: Item = serde_json::from_str(
            r#"{"Id": "bare", "Container": "mov,mp4,m4a", "MediaSources": [{"MediaStreams": [{"Type": "Audio", "Codec": "aac"}]}]}"#,
        )
        .expect("a sparse item parses");
        let song = item_song(&item, "server");
        assert_eq!(
            song.metadata.title, "bare",
            "the item stands in for a title"
        );
        assert_eq!(song.metadata.artist, None);
        assert_eq!(song.metadata.format, "MOV");
        assert!(!song.metadata.lossless);
        assert_eq!(song.metadata.duration_secs, 0.0);
        assert_eq!(song.artwork_item, None);
    }

    #[test]
    fn alac_in_an_mp4_container_is_lossless() {
        let item: Item = serde_json::from_str(
            r#"{"Id": "alac", "ImageTags": {"Primary": "tag"}, "MediaSources": [{"Path": "/a.m4a", "MediaStreams": [{"Type": "Audio", "Codec": "alac"}]}]}"#,
        )
        .expect("an item parses");
        let song = item_song(&item, "server");
        assert_eq!(song.metadata.format, "M4A");
        assert!(song.metadata.lossless);
        assert_eq!(
            song.artwork_item.as_deref(),
            Some("alac"),
            "its own picture first"
        );
    }
}
