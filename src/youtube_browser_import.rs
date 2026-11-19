//! Optional yt-dlp browser import. No browser database or all-sites jar survives
//! the request; the caller owns the filtered file until account validation commits.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tempfile::NamedTempFile;

use crate::youtube_music::Failure;

const PROCESS_TIMEOUT: Duration = Duration::from_secs(45);
const LARGEST_RAW_FILE_BYTES: u64 = 32 * 1024 * 1024;
const LARGEST_COOKIE_FILE_BYTES: usize = 1024 * 1024;
const COOKIE_PREFIX: &str = "youtube-browser-";
const COOKIE_SUFFIX: &str = ".cookies.txt";
const COOKIE_HEADER: &str = "# Netscape HTTP Cookie File\n";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Browser {
    #[default]
    Firefox,
    Chrome,
    Chromium,
    Brave,
}

impl Browser {
    pub fn from_identifier(identifier: &str) -> Option<Self> {
        match identifier {
            "firefox" => Some(Self::Firefox),
            "chrome" => Some(Self::Chrome),
            "chromium" => Some(Self::Chromium),
            "brave" => Some(Self::Brave),
            _ => None,
        }
    }

    pub fn identifier(self) -> &'static str {
        match self {
            Self::Firefox => "firefox",
            Self::Chrome => "chrome",
            Self::Chromium => "chromium",
            Self::Brave => "brave",
        }
    }
}

/// Uses the listener's yt-dlp on PATH, never installs it or extracts a video.
///
/// yt-dlp's `download_with_info_file` accepts an empty JSON list without
/// invoking an extractor. Its `YoutubeDL.close` then saves the browser cookiejar.
/// A URL does not restrict that jar: filtering is mandatory before returning it.
/// Standard streams are discarded, including diagnostics containing private paths.
/// Dropping this future kills the child and removes its private working directory.
/// Dropping the returned guard removes the filtered file unless the caller keeps it.
pub async fn export(browser: Browser, directory: &Path) -> Result<NamedTempFile, Failure> {
    std::fs::create_dir_all(directory).map_err(|_| Failure::BrowserImport)?;
    // TempDir is owner-only on permission-based systems. Placing every helper
    // temporary file inside it also contains yt-dlp's browser database copies.
    let temporary = private_directory(directory)?;
    let information = temporary.path().join("empty-information.json");
    std::fs::write(&information, b"[]").map_err(|_| Failure::BrowserImport)?;
    let raw_cookies = temporary.path().join("browser.cookies.txt");
    let mut command = Command::new("yt-dlp");
    command
        .args([
            "--ignore-config",
            "--no-plugin-dirs",
            "--no-cache-dir",
            "--simulate",
            "--skip-download",
            "--no-progress",
            "--quiet",
            "--no-warnings",
            "--cookies-from-browser",
            browser.identifier(),
        ])
        .arg("--cookies")
        .arg(&raw_cookies)
        .arg("--load-info-json")
        .arg(&information)
        .env("TMPDIR", temporary.path())
        .env("TMP", temporary.path())
        .env("TEMP", temporary.path())
        .current_dir(temporary.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Declared after the directory so cancellation reaps the child before the
    // directory guard runs, including on systems that cannot unlink open files.
    let mut child = BrowserProcess(command.spawn().map_err(spawn_failure)?);
    let completed = tokio::time::timeout(PROCESS_TIMEOUT, async {
        loop {
            if let Some(status) = child.0.try_wait().map_err(|_| Failure::BrowserImport)? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(Failure::BrowserImport)
                };
            }
            check_raw_file_size(&raw_cookies)?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    if !matches!(completed, Ok(Ok(()))) {
        return Err(Failure::BrowserImport);
    }
    // Child has exited before raw data is read or its directory is destroyed.
    let filtered = filter_cookie_file(&raw_cookies, directory)?;
    temporary.close().map_err(|_| Failure::BrowserImport)?;
    Ok(filtered)
}

struct BrowserProcess(Child);

impl Drop for BrowserProcess {
    fn drop(&mut self) {
        // This service belongs on a worker. Reaping after a kill is deliberately
        // synchronous: no detached task may outlive the raw-cookie directory.
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn private_directory(directory: &Path) -> Result<tempfile::TempDir, Failure> {
    tempfile::Builder::new()
        .prefix("youtube-import-")
        .tempdir_in(directory)
        .map_err(|_| Failure::BrowserImport)
}

fn spawn_failure(error: std::io::Error) -> Failure {
    if error.kind() == std::io::ErrorKind::NotFound {
        Failure::ExtractorUnavailable
    } else {
        Failure::BrowserImport
    }
}

fn check_raw_file_size(path: &Path) -> Result<(), Failure> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.is_file() && metadata.len() <= LARGEST_RAW_FILE_BYTES => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(Failure::BrowserImport),
    }
}

fn filter_cookie_file(source: &Path, directory: &Path) -> Result<NamedTempFile, Failure> {
    check_raw_file_size(source)?;
    let source = File::open(source).map_err(|_| Failure::BrowserImport)?;
    let metadata = source.metadata().map_err(|_| Failure::BrowserImport)?;
    if !metadata.is_file() || metadata.len() > LARGEST_RAW_FILE_BYTES {
        return Err(Failure::BrowserImport);
    }
    let mut raw = String::new();
    source
        .take(LARGEST_RAW_FILE_BYTES + 1)
        .read_to_string(&mut raw)
        .map_err(|_| Failure::InvalidCookieFile)?;
    if raw.len() as u64 > LARGEST_RAW_FILE_BYTES {
        return Err(Failure::BrowserImport);
    }
    let mut filtered = tempfile::Builder::new()
        .prefix(COOKIE_PREFIX)
        .suffix(COOKIE_SUFFIX)
        .tempfile_in(directory)
        .map_err(|_| Failure::BrowserImport)?;
    filter_cookies(&raw, filtered.as_file_mut())?;
    filtered.flush().map_err(|_| Failure::BrowserImport)?;
    Ok(filtered)
}

fn filter_cookies(raw: &str, output: &mut impl Write) -> Result<(), Failure> {
    output
        .write_all(COOKIE_HEADER.as_bytes())
        .map_err(|_| Failure::BrowserImport)?;
    let mut length = COOKIE_HEADER.len();
    let mut count = 0;
    for line in raw.lines() {
        let row = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        if row.starts_with('#') || row.is_empty() {
            continue;
        }
        let mut columns = row.split('\t');
        let fields: [Option<&str>; 7] = std::array::from_fn(|_| columns.next());
        let [Some(domain), Some(subdomains), Some(path), Some(secure), Some(expiry), Some(name), Some(_value)] =
            fields
        else {
            continue;
        };
        if columns.next().is_some()
            || !youtube_domain(domain)
            || !matches!(subdomains, "TRUE" | "FALSE")
            || !path.starts_with('/')
            || !matches!(secure, "TRUE" | "FALSE")
            || expiry.parse::<u64>().is_err()
            || name.is_empty()
            || name.bytes().any(|character| {
                character.is_ascii_whitespace() || matches!(character, b';' | b'=')
            })
            || row
                .chars()
                .any(|character| character.is_control() && character != '\t')
        {
            continue;
        }
        length += line.len() + 1;
        if length > LARGEST_COOKIE_FILE_BYTES {
            return Err(Failure::InvalidCookieFile);
        }
        output
            .write_all(line.as_bytes())
            .and_then(|()| output.write_all(b"\n"))
            .map_err(|_| Failure::BrowserImport)?;
        count += 1;
    }
    if count == 0 {
        return Err(Failure::InvalidCookieFile);
    }
    Ok(())
}

fn youtube_domain(domain: &str) -> bool {
    let domain = domain.strip_prefix('.').unwrap_or(domain);
    if domain.len() > 253
        || !(domain.eq_ignore_ascii_case("youtube.com")
            || domain
                .get(domain.len().saturating_sub(12)..)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".youtube.com")))
    {
        return false;
    }
    domain.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|character| character.is_ascii_alphanumeric() || character == b'-')
    })
}

/// Unlinks only a named managed jar directly inside the supplied directory.
/// Symlinks at the final filename are unlinked, never followed. Manual imports,
/// nested paths, parent traversal and directory symlinks are not accepted.
pub fn remove_managed(path: &Path, directory: &Path) -> Result<(), Failure> {
    let Some(filename) = path.file_name().and_then(|filename| filename.to_str()) else {
        return Ok(());
    };
    if path.parent() != Some(directory)
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        || !filename.starts_with(COOKIE_PREFIX)
        || !filename.ends_with(COOKIE_SUFFIX)
        || filename.len() <= COOKIE_PREFIX.len() + COOKIE_SUFFIX.len()
    {
        return Ok(());
    }
    let metadata = match directory.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(Failure::BrowserImport),
    };
    if !metadata.is_dir() {
        return Err(Failure::BrowserImport);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(Failure::BrowserImport),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(domain: &str) -> String {
        format!("{domain}\tTRUE\t/\tTRUE\t2147483647\tSID\tsynthetic-secret\n")
    }

    #[test]
    fn only_youtube_domains_and_http_only_rows_survive() {
        let raw = [
            "# Netscape HTTP Cookie File\n# unrelated secret in a comment\n".to_owned(),
            cookie(".youtube.com"),
            cookie("music.youtube.com"),
            cookie("#HttpOnly_.youtube.com"),
            cookie(".google.com"),
            cookie("youtube.com.evil.example"),
            cookie("notyoutube.com"),
            cookie("youtube.com@evil.example"),
            cookie(".youtube.com."),
            cookie("..youtube.com"),
            cookie("bad_.youtube.com"),
            cookie("-bad.youtube.com"),
            cookie("bad-.youtube.com"),
        ]
        .concat();
        let mut output = Vec::new();
        filter_cookies(&raw, &mut output).expect("valid YouTube cookies");
        assert_eq!(
            String::from_utf8(output).expect("text"),
            format!(
                "{COOKIE_HEADER}{}{}{}",
                cookie(".youtube.com"),
                cookie("music.youtube.com"),
                cookie("#HttpOnly_.youtube.com")
            )
        );
    }

    #[test]
    fn malformed_rows_cannot_smuggle_extra_fields_or_control_characters() {
        for raw in [
            ".youtube.com\tTRUE\t/\tTRUE\t0\tSID\tvalue\textra\n",
            ".youtube.com\tTRUE\t/\tTRUE\t0\tSID\tvalue\0hidden\n",
            ".youtube.com\tTRUE\t/\tTRUE\tno-expiry\tSID\tvalue\n",
            ".youtube.com\tTRUE\t/\tMAYBE\t0\tSID\tvalue\n",
            ".youtube.com\tTRUE\trelative\tTRUE\t0\tSID\tvalue\n",
            ".youtube.com\tTRUE\t/\tTRUE\t0\t\tvalue\n",
            ".google.com\tTRUE\t/\tTRUE\t0\tSID\tvalue\n",
        ] {
            assert_eq!(
                filter_cookies(raw, &mut Vec::new()),
                Err(Failure::InvalidCookieFile)
            );
        }
    }

    #[test]
    fn filtered_jars_are_bounded_removed_by_their_guards_and_never_retain_invalid_input() {
        let directory = tempfile::tempdir().expect("private directory");
        let source = directory.path().join("source");
        std::fs::write(&source, cookie(".youtube.com")).expect("synthetic source");
        let filtered = filter_cookie_file(&source, directory.path()).expect("filtered jar");
        let path = filtered.path().to_owned();
        assert!(path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with(COOKIE_PREFIX));
        drop(filtered);
        assert!(!path.exists());
        std::fs::write(&source, cookie(".youtube.com").repeat(20_000)).expect("large source");
        assert!(matches!(
            filter_cookie_file(&source, directory.path()),
            Err(Failure::InvalidCookieFile)
        ));
        File::create(&source)
            .unwrap()
            .set_len(LARGEST_RAW_FILE_BYTES + 1)
            .unwrap();
        assert!(matches!(
            filter_cookie_file(&source, directory.path()),
            Err(Failure::BrowserImport)
        ));
        std::fs::write(&source, [0xff]).unwrap();
        assert!(matches!(
            filter_cookie_file(&source, directory.path()),
            Err(Failure::InvalidCookieFile)
        ));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn helper_spawn_errors_are_safe_and_distinguish_missing_installation() {
        assert_eq!(
            spawn_failure(std::io::ErrorKind::NotFound.into()),
            Failure::ExtractorUnavailable
        );
        assert_eq!(
            spawn_failure(std::io::ErrorKind::PermissionDenied.into()),
            Failure::BrowserImport
        );
        assert!(!Failure::BrowserImport.credentials_rejected());
    }

    #[test]
    fn browser_identifiers_do_not_accept_profiles_or_command_arguments() {
        for browser in [
            Browser::Firefox,
            Browser::Chrome,
            Browser::Chromium,
            Browser::Brave,
        ] {
            assert_eq!(
                Browser::from_identifier(browser.identifier()),
                Some(browser)
            );
        }
        assert_eq!(Browser::from_identifier("firefox:/private/profile"), None);
        assert_eq!(Browser::from_identifier("--help"), None);
    }

    #[test]
    fn cleanup_only_unlinks_direct_managed_files() {
        let directory = tempfile::tempdir().unwrap();
        let managed = directory.path().join("youtube-browser-test.cookies.txt");
        let manual = directory.path().join("manual.cookies.txt");
        std::fs::write(&managed, "synthetic").unwrap();
        std::fs::write(&manual, "synthetic").unwrap();
        remove_managed(&manual, directory.path()).unwrap();
        remove_managed(&managed, &directory.path().join("nested")).unwrap();
        assert!(managed.exists());
        assert!(manual.exists());
        remove_managed(&managed, directory.path()).unwrap();
        remove_managed(&managed, directory.path()).unwrap();
        assert!(!managed.exists());
        assert!(manual.exists());
    }
}
