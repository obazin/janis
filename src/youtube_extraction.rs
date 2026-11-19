//! Resolves one native playback source through the listener's installed yt-dlp.
//!
//! Signed addresses and copied cookies live only for the current request. The
//! subprocess never receives the caller's cookie file because it rewrites jars.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use tempfile::NamedTempFile;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::audio::{BoundedRange, Location, RemoteFile};
use crate::youtube_music::{Failure, ResolvedAudio};

const PROCESS_TIMEOUT: Duration = Duration::from_secs(45);
const COOKIE_COPY_TIMEOUT: Duration = Duration::from_secs(10);
const LARGEST_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const LARGEST_COOKIE_FILE_BYTES: u64 = 1024 * 1024;
const LONGEST_AUDIO_ADDRESS: usize = 16 * 1024;

/// Uses only yt-dlp on PATH; Janis neither installs nor updates an extractor.
/// The child is killed on cancellation, and all explicit exits reap it.
/// The request lease also follows a blocking cookie copy and its unclaimed
/// result, so its owner can join credential cleanup even after a copy timeout.
pub async fn resolve(
    identifier: &str,
    cookie_file: Option<&Path>,
    cancelled: impl Fn() -> bool,
    request_lifetime: Arc<dyn Send + Sync>,
) -> Result<ResolvedAudio, Failure> {
    resolve_with_command(
        identifier,
        cookie_file,
        cancelled,
        Command::new("yt-dlp"),
        request_lifetime,
    )
    .await
}

async fn resolve_with_command(
    identifier: &str,
    cookie_file: Option<&Path>,
    cancelled: impl Fn() -> bool,
    mut command: Command,
    request_lifetime: Arc<dyn Send + Sync>,
) -> Result<ResolvedAudio, Failure> {
    if !is_video_identifier(identifier) {
        return Err(Failure::InvalidInput);
    }
    if cancelled() {
        return Err(Failure::Extraction);
    }

    let temporary_cookies = if let Some(path) = cookie_file {
        let path = path.to_path_buf();
        Some(
            tokio::time::timeout(
                COOKIE_COPY_TIMEOUT,
                tokio::task::spawn_blocking(move || {
                    copy_owned_cookie_file(&path, request_lifetime)
                }),
            )
            .await
            .map_err(|_| Failure::CookieFileUnreadable)?
            .map_err(|_| Failure::CookieFileUnreadable)??,
        )
    } else {
        None
    };

    if cancelled() {
        return Err(Failure::Extraction);
    }
    command
        .args([
            "--ignore-config",
            "--no-cache-dir",
            "--no-playlist",
            "--skip-download",
            "--dump-single-json",
            "--no-progress",
            "--socket-timeout",
            "15",
            "--retries",
            "0",
            "--extractor-retries",
            "0",
            "--fragment-retries",
            "0",
            "--fixup",
            "never",
            "--format",
            "bestaudio[ext=m4a][acodec^=mp4a][vcodec=none][audio_channels<=2]",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(cookies) = &temporary_cookies {
        command.arg("--cookies").arg(cookies.path());
    }
    command
        .arg("--")
        .arg(format!("https://www.youtube.com/watch?v={identifier}"));

    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Failure::ExtractorUnavailable
        } else {
            Failure::Extraction
        }
    })?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err(Failure::Extraction);
    };
    let extracted = tokio::select! {
        outcome = tokio::time::timeout(PROCESS_TIMEOUT, async {
            tokio::try_join!(read_output(stdout), async {
                child.wait().await.map_err(|_| Failure::Extraction)
            })
        }) => outcome.ok(),
        () = wait_for_cancellation(&cancelled) => None,
    };

    let output = match extracted {
        Some(Ok((output, status))) if status.success() => output,
        _ => {
            // kill() waits for the child; wait() also covers an already-exited
            // child and a failed kill. No subprocess diagnostic is retained.
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(Failure::Extraction);
        }
    };
    drop(temporary_cookies);
    parse_audio(&output, identifier, SystemTime::now())
}

/// Fields drop in declaration order: remove the credential before releasing
/// the request lease, including when Tokio discards a detached copy's result.
struct CopiedCookies {
    file: NamedTempFile,
    _request_lifetime: Arc<dyn Send + Sync>,
}

impl CopiedCookies {
    fn path(&self) -> &Path {
        self.file.path()
    }
}

fn copy_owned_cookie_file(
    path: &Path,
    request_lifetime: Arc<dyn Send + Sync>,
) -> Result<CopiedCookies, Failure> {
    Ok(CopiedCookies {
        file: copy_cookie_file(path)?,
        _request_lifetime: request_lifetime,
    })
}

async fn wait_for_cancellation(cancelled: &impl Fn() -> bool) {
    while !cancelled() {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn is_video_identifier(identifier: &str) -> bool {
    identifier.len() == 11
        && identifier
            .bytes()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, b'_' | b'-'))
}

fn copy_cookie_file(path: &Path) -> Result<NamedTempFile, Failure> {
    // Check before opening so a mistakenly selected device or pipe is not read.
    if !path
        .metadata()
        .map_err(|_| Failure::CookieFileUnreadable)?
        .is_file()
    {
        return Err(Failure::CookieFileUnreadable);
    }
    let source = File::open(path).map_err(|_| Failure::CookieFileUnreadable)?;
    if !source
        .metadata()
        .map_err(|_| Failure::CookieFileUnreadable)?
        .is_file()
    {
        return Err(Failure::CookieFileUnreadable);
    }
    // NamedTempFile creates a private, unpredictable file (owner-only on
    // permission-based systems). Its guard also removes yt-dlp's rewritten jar.
    let mut temporary = NamedTempFile::new().map_err(|_| Failure::CookieFileUnreadable)?;
    let copied = std::io::copy(
        &mut source.take(LARGEST_COOKIE_FILE_BYTES + 1),
        temporary.as_file_mut(),
    )
    .map_err(|_| Failure::CookieFileUnreadable)?;
    if copied == 0 || copied > LARGEST_COOKIE_FILE_BYTES {
        return Err(Failure::InvalidCookieFile);
    }
    temporary
        .flush()
        .map_err(|_| Failure::CookieFileUnreadable)?;
    Ok(temporary)
}

async fn read_output(stdout: tokio::process::ChildStdout) -> Result<Vec<u8>, Failure> {
    let mut output = Vec::new();
    stdout
        .take((LARGEST_OUTPUT_BYTES + 1) as u64)
        .read_to_end(&mut output)
        .await
        .map_err(|_| Failure::Extraction)?;
    if output.len() > LARGEST_OUTPUT_BYTES {
        return Err(Failure::Extraction);
    }
    Ok(output)
}

// Do not derive Debug: both the URL and ignored upstream fields can be secrets.
#[derive(Deserialize)]
struct ExtractedAudio {
    id: String,
    #[serde(rename = "_type")]
    item_type: Option<String>,
    url: String,
    ext: String,
    container: Option<String>,
    acodec: String,
    vcodec: String,
    protocol: String,
    audio_channels: u32,
    duration: f64,
    filesize: Option<u64>,
    has_drm: Option<bool>,
    is_live: Option<bool>,
    live_status: String,
    requested_formats: Option<Vec<serde::de::IgnoredAny>>,
}

fn parse_audio(output: &[u8], identifier: &str, now: SystemTime) -> Result<ResolvedAudio, Failure> {
    if output.len() > LARGEST_OUTPUT_BYTES {
        return Err(Failure::Extraction);
    }
    let extracted: ExtractedAudio =
        serde_json::from_slice(output).map_err(|_| Failure::InvalidResponse)?;
    if extracted.id != identifier
        || extracted
            .item_type
            .as_deref()
            .is_some_and(|item_type| item_type != "video")
    {
        return Err(Failure::InvalidResponse);
    }
    if extracted.has_drm == Some(true)
        || extracted.is_live == Some(true)
        || !matches!(extracted.live_status.as_str(), "not_live" | "was_live")
        || extracted.ext != "m4a"
        || extracted
            .container
            .as_deref()
            .is_some_and(|container| !matches!(container, "m4a" | "m4a_dash"))
        || !matches!(
            extracted.acodec.as_str(),
            "mp4a.40.2" | "mp4a.40.5" | "mp4a.40.29"
        )
        || extracted.vcodec != "none"
        || extracted.protocol != "https"
        || !matches!(extracted.audio_channels, 1 | 2)
        || extracted.requested_formats.is_some()
    {
        return Err(Failure::UnsupportedAudio);
    }
    if !extracted.duration.is_finite() || extracted.duration <= 0.0 || extracted.filesize == Some(0)
    {
        return Err(Failure::InvalidResponse);
    }
    let expires_at = audio_expiry(&extracted.url, now)?;
    Ok(ResolvedAudio {
        location: Location::Remote(RemoteFile {
            url: extracted.url,
            extension: Some("m4a".into()),
            bounded_range: Some(BoundedRange {
                content_length: extracted.filesize,
                ..Default::default()
            }),
        }),
        duration_seconds: extracted.duration,
        // yt-dlp has no reliable, documented normalization-gain field here.
        gain_db: 0.0,
        expires_at,
    })
}

fn audio_expiry(address: &str, now: SystemTime) -> Result<SystemTime, Failure> {
    if address.len() > LONGEST_AUDIO_ADDRESS
        || address
            .bytes()
            .any(|character| character.is_ascii_whitespace() || character.is_ascii_control())
    {
        return Err(Failure::InvalidResponse);
    }
    // URL parsers normalize empty userinfo away; reject it before parsing too.
    let authority = address
        .strip_prefix("https://")
        .and_then(|remainder| remainder.split(['/', '?', '#']).next())
        .ok_or(Failure::UnsupportedAudio)?;
    if authority.contains('@') || address.contains('\\') {
        return Err(Failure::UnsupportedAudio);
    }
    let address = reqwest::Url::parse(address).map_err(|_| Failure::InvalidResponse)?;
    if address.scheme() != "https"
        || !address.username().is_empty()
        || address.password().is_some()
        || !address
            .host_str()
            .is_some_and(|host| host == "googlevideo.com" || host.ends_with(".googlevideo.com"))
        || address.port_or_known_default() != Some(443)
        || address.fragment().is_some()
        || address.path() != "/videoplayback"
    {
        return Err(Failure::UnsupportedAudio);
    }
    let mut expiry_parameters = address.query_pairs().filter(|(name, _)| name == "expire");
    let (_, expiry) = expiry_parameters.next().ok_or(Failure::InvalidResponse)?;
    if expiry_parameters.next().is_some()
        || expiry.is_empty()
        || !expiry.bytes().all(|character| character.is_ascii_digit())
    {
        return Err(Failure::InvalidResponse);
    }
    let expiry = expiry
        .parse::<u64>()
        .map_err(|_| Failure::InvalidResponse)?;
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(expiry))
        .filter(|expiry| *expiry > now)
        .ok_or(Failure::InvalidResponse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detached_cookie_copy_keeps_its_request_alive_until_the_unclaimed_jar_is_removed() {
        struct CleanupLease {
            copied_path: Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
            finished: std::sync::mpsc::Sender<bool>,
        }

        impl Drop for CleanupLease {
            fn drop(&mut self) {
                let removed = self
                    .copied_path
                    .lock()
                    .expect("copied path")
                    .as_ref()
                    .is_some_and(|path| !path.exists());
                let _ = self.finished.send(removed);
            }
        }

        let original = NamedTempFile::new().expect("synthetic original");
        std::fs::write(original.path(), "# Netscape HTTP Cookie File\nsynthetic\n")
            .expect("synthetic jar");
        let copied_path = Arc::new(std::sync::Mutex::new(None));
        let (finished, receiver) = std::sync::mpsc::channel();
        let lease: Arc<dyn Send + Sync> = Arc::new(CleanupLease {
            copied_path: Arc::clone(&copied_path),
            finished,
        });
        let (started, copy_started) = std::sync::mpsc::channel();
        let release = Arc::new(std::sync::Barrier::new(2));
        let worker_release = Arc::clone(&release);
        let original_path = original.path().to_path_buf();
        let runtime = tokio::runtime::Runtime::new().expect("copy runtime");
        let task = runtime.spawn_blocking(move || {
            let copied = copy_owned_cookie_file(&original_path, lease).expect("owned copy");
            let path = copied.path().to_path_buf();
            *copied_path.lock().expect("copied path") = Some(path.clone());
            started.send(path).expect("copy observer");
            worker_release.wait();
            copied
        });
        let path = copy_started
            .recv_timeout(Duration::from_secs(2))
            .expect("active copy");
        // A timed-out await drops this handle but cannot stop its blocking copy.
        drop(task);
        let copied_file_exists = path.exists();
        let request_is_active = receiver.try_recv().is_err();
        release.wait();
        assert!(receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("request cleanup"));
        assert!(copied_file_exists);
        assert!(request_is_active);
        assert!(!path.exists());
        assert!(original.path().exists());
    }

    #[cfg(unix)]
    #[test]
    fn obsolete_overview_or_account_extraction_reaps_its_child_and_removes_copied_cookies() {
        for cancellation_reason in ["skip", "stop", "disconnect"] {
            let directory = tempfile::tempdir().expect("isolated extractor");
            let script = directory.path().join("extractor.sh");
            let marker = directory.path().join("started");
            let original = directory.path().join("original.txt");
            std::fs::write(
                &original,
                "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tsynthetic\n",
            )
            .expect("synthetic jar");
            std::fs::write(
                &script,
                format!(
                    "while [ \"$1\" != --cookies ]; do shift; done\nshift\nprintf '%s\\n%s\\n' \"$$\" \"$1\" > '{}'\nexec sleep 60\n",
                    marker.display()
                ),
            ).expect("held extractor script");
            let mut reads = crate::overview_reads::OverviewReads::default();
            let crate::overview_reads::Start::Shown { job: Some(job) } = reads.start(Some(1))
            else {
                panic!("an uncached overview needs a job");
            };
            let generation = std::sync::atomic::AtomicU64::new(0);
            let runtime = tokio::runtime::Runtime::new().expect("extractor runtime");
            runtime.block_on(async {
                let mut command = Command::new("sh");
                command.arg(&script);
                let extraction = resolve_with_command(
                    IDENTIFIER,
                    Some(&original),
                    || {
                        !job.is_current()
                            || generation.load(std::sync::atomic::Ordering::Acquire) != 0
                    },
                    command,
                    Arc::new(()),
                );
                let cancellation = async {
                    tokio::time::timeout(Duration::from_secs(2), async {
                        while std::fs::read_to_string(&marker)
                            .unwrap_or_default()
                            .lines()
                            .count()
                            < 2
                        {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .expect("the child starts");
                    let marker = std::fs::read_to_string(&marker).expect("child marker");
                    let mut lines = marker.lines();
                    let process = lines.next().expect("child process").to_owned();
                    let copied = std::path::PathBuf::from(lines.next().expect("copied jar"));
                    assert!(copied.exists());
                    assert_ne!(copied, original);
                    match cancellation_reason {
                        "skip" => {
                            reads.start(Some(2));
                        }
                        "stop" => reads.stop(),
                        _ => {
                            generation.fetch_add(1, std::sync::atomic::Ordering::Release);
                        }
                    }
                    (process, copied)
                };
                let (outcome, (process, copied)) =
                    tokio::time::timeout(Duration::from_secs(3), async {
                        tokio::join!(extraction, cancellation)
                    })
                    .await
                    .expect("obsolete extraction is promptly cancelled");
                assert_eq!(outcome.err(), Some(Failure::Extraction));
                assert!(!copied.exists());
                assert!(original.exists());
                assert!(!std::process::Command::new("kill")
                    .args(["-0", &process])
                    .stderr(Stdio::null())
                    .status()
                    .expect("probe reaped child")
                    .success());
            });
        }
    }

    const IDENTIFIER: &str = "dQw4w9WgXcQ";

    fn extracted_audio() -> serde_json::Value {
        serde_json::json!({
            "id": IDENTIFIER,
            "_type": "video",
            "url": "https://rr1---example.googlevideo.com/videoplayback?expire=2000000000&signature=secret",
            "ext": "m4a",
            "container": "m4a_dash",
            "acodec": "mp4a.40.2",
            "vcodec": "none",
            "protocol": "https",
            "audio_channels": 2,
            "duration": 212.25,
            "filesize": 1234567,
            "has_drm": false,
            "is_live": false,
            "live_status": "not_live"
        })
    }

    fn parse(value: &serde_json::Value) -> Result<ResolvedAudio, Failure> {
        parse_audio(
            &serde_json::to_vec(value).expect("the fixture is valid JSON"),
            IDENTIFIER,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1900000000),
        )
    }

    #[test]
    fn identifiers_cannot_supply_urls_or_command_options() {
        assert!(is_video_identifier(IDENTIFIER));
        assert!(is_video_identifier("_-012345678"));
        for identifier in [
            "",
            "--cookies",
            "https://youtube.com",
            "abcdefghij/",
            "abcdefghij\n",
        ] {
            assert!(!is_video_identifier(identifier));
        }
    }

    #[test]
    fn supported_audio_retains_exact_size_duration_and_expiry_without_debug_secrets() {
        let resolved = parse(&extracted_audio()).expect("the AAC fixture is supported");
        assert_eq!(resolved.duration_seconds, 212.25);
        assert_eq!(resolved.gain_db, 0.0);
        assert_eq!(
            resolved.expires_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(2000000000)
        );
        let Location::Remote(remote) = &resolved.location else {
            panic!("extracted audio must use native remote playback");
        };
        assert_eq!(remote.extension.as_deref(), Some("m4a"));
        assert_eq!(
            remote
                .bounded_range
                .as_ref()
                .and_then(|range| range.content_length),
            Some(1234567)
        );
        assert!(!format!("{resolved:?}").contains("secret"));
        assert!(!format!("{resolved:?}").contains("googlevideo"));
    }

    #[test]
    fn unknown_file_size_leaves_range_length_discovery_to_the_engine() {
        let mut value = extracted_audio();
        value.as_object_mut().unwrap().remove("filesize");
        let resolved = parse(&value).expect("a direct URL does not require a preannounced size");
        let Location::Remote(remote) = resolved.location else {
            panic!("expected remote audio");
        };
        assert_eq!(remote.bounded_range.unwrap().content_length, None);
    }

    #[test]
    fn encrypted_live_or_non_native_formats_are_rejected() {
        for (field, replacement) in [
            ("has_drm", serde_json::json!(true)),
            ("is_live", serde_json::json!(true)),
            ("live_status", serde_json::json!("is_upcoming")),
            ("live_status", serde_json::json!("post_live")),
            ("ext", serde_json::json!("webm")),
            ("container", serde_json::json!("webm")),
            ("acodec", serde_json::json!("opus")),
            ("acodec", serde_json::json!("mp4a.40.34")),
            ("vcodec", serde_json::json!("avc1")),
            ("protocol", serde_json::json!("m3u8_native")),
            ("audio_channels", serde_json::json!(0)),
            ("audio_channels", serde_json::json!(6)),
            ("requested_formats", serde_json::json!([{}])),
        ] {
            let mut value = extracted_audio();
            value[field] = replacement;
            assert_eq!(
                parse(&value).err(),
                Some(Failure::UnsupportedAudio),
                "{field}"
            );
        }
    }

    #[test]
    fn missing_or_invalid_duration_size_and_identity_are_rejected() {
        for field in ["duration", "audio_channels", "live_status", "url"] {
            let mut value = extracted_audio();
            value
                .as_object_mut()
                .expect("the fixture is an object")
                .remove(field);
            assert_eq!(
                parse(&value).err(),
                Some(Failure::InvalidResponse),
                "{field}"
            );
        }
        for (field, replacement) in [
            ("duration", serde_json::json!(0)),
            ("duration", serde_json::json!(-1)),
            ("filesize", serde_json::json!(0)),
            ("filesize", serde_json::json!(1.5)),
            ("id", serde_json::json!("different00")),
            ("_type", serde_json::json!("playlist")),
        ] {
            let mut value = extracted_audio();
            value[field] = replacement;
            assert_eq!(
                parse(&value).err(),
                Some(Failure::InvalidResponse),
                "{field}"
            );
        }
    }

    #[test]
    fn addresses_cannot_escape_the_https_googlevideo_file_boundary() {
        for address in [
            "http://rr1.googlevideo.com/videoplayback?expire=2000000000",
            "https://googlevideo.com.attacker.test/videoplayback?expire=2000000000",
            "https://attackergooglevideo.com/videoplayback?expire=2000000000",
            "https://secret@rr1.googlevideo.com/videoplayback?expire=2000000000",
            "https://@rr1.googlevideo.com/videoplayback?expire=2000000000",
            "https://:secret@rr1.googlevideo.com/videoplayback?expire=2000000000",
            "https://rr1.googlevideo.com:444/videoplayback?expire=2000000000",
            "https://rr1.googlevideo.com/manifest?expire=2000000000",
            "https://rr1.googlevideo.com/videoplayback?expire=2000000000#secret",
        ] {
            let mut value = extracted_audio();
            value["url"] = address.into();
            assert_eq!(parse(&value).err(), Some(Failure::UnsupportedAudio));
        }
    }

    #[test]
    fn expiry_must_be_unique_numeric_representable_and_in_the_future() {
        for query in [
            "signature=secret",
            "expire=",
            "expire=1900000000",
            "expire=1800000000",
            "expire=-1",
            "expire=+2000000000",
            "expire=2000000000&expire=2100000000",
            "expire=9999999999999999999999999",
        ] {
            let mut value = extracted_audio();
            value["url"] = format!("https://rr1.googlevideo.com/videoplayback?{query}").into();
            assert_eq!(parse(&value).err(), Some(Failure::InvalidResponse));
        }
    }

    #[test]
    fn oversized_or_malformed_output_is_never_accepted() {
        let oversized = vec![b' '; LARGEST_OUTPUT_BYTES + 1];
        assert_eq!(
            parse_audio(&oversized, IDENTIFIER, SystemTime::UNIX_EPOCH).err(),
            Some(Failure::Extraction)
        );
        assert_eq!(
            parse_audio(b"not JSON", IDENTIFIER, SystemTime::UNIX_EPOCH).err(),
            Some(Failure::InvalidResponse)
        );
    }

    #[test]
    fn cookies_are_copied_into_a_separate_ephemeral_jar() {
        let mut original = NamedTempFile::new().expect("the fixture file can be created");
        let contents =
            b"# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tsecret\n";
        original
            .write_all(contents)
            .expect("the fixture can be written");
        let mut copied = copy_cookie_file(original.path()).expect("the fixture can be copied");
        let copied_path = copied.path().to_path_buf();
        assert_ne!(copied_path, original.path());
        copied
            .as_file_mut()
            .set_len(0)
            .expect("the copy can be rewritten");
        copied
            .write_all(b"rewritten")
            .expect("the copy can be written");
        assert_eq!(
            std::fs::read(original.path()).expect("the original is readable"),
            contents
        );
        drop(copied);
        assert!(!copied_path.exists());
    }

    #[test]
    fn empty_or_oversized_cookie_jars_are_rejected() {
        let original = NamedTempFile::new().expect("the fixture file can be created");
        assert_eq!(
            copy_cookie_file(original.path()).err(),
            Some(Failure::InvalidCookieFile)
        );
        original
            .as_file()
            .set_len(LARGEST_COOKIE_FILE_BYTES + 1)
            .expect("the fixture can be resized");
        assert_eq!(
            copy_cookie_file(original.path()).err(),
            Some(Failure::InvalidCookieFile)
        );
    }
}
