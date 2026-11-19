//! The Settings connection owns a persistent shared-library source, not catalog search.
//! Reads and imports finish under one epoch; failure never removes cached songs.

use std::path::PathBuf;

use crate::effects::Effect;
use crate::youtube_browser_import::Browser;
use crate::youtube_music::Failure;

pub struct YouTubeMusicIntegration {
    pub browser: Browser,
    pub connected: bool,
    pub busy: bool,
    pub failed: bool,
    pub status_key: &'static str,
    pub song_count: usize,
    pub cookie_file: Option<PathBuf>,
    pub last_sync: Option<i64>,
    epoch: u64,
    pending: Option<PendingSync>,
}

struct PendingSync {
    cookie_file: Option<PathBuf>,
    replacing_connection: bool,
}

impl Default for YouTubeMusicIntegration {
    fn default() -> Self {
        Self {
            browser: Browser::Firefox,
            connected: false,
            busy: false,
            failed: false,
            status_key: "youtubeMusic.status.disconnected",
            song_count: 0,
            cookie_file: None,
            last_sync: None,
            epoch: 0,
            pending: None,
        }
    }
}

impl YouTubeMusicIntegration {
    /// Cached songs and credentials are usable while their background refresh runs.
    pub fn restore(
        &mut self,
        cookie_file: Option<PathBuf>,
        song_count: usize,
        last_sync: Option<i64>,
    ) -> Vec<Effect> {
        self.epoch += 1;
        self.connected = cookie_file.is_some();
        self.cookie_file = cookie_file;
        self.song_count = song_count;
        self.last_sync = last_sync;
        self.busy = false;
        self.failed = false;
        self.pending = None;
        self.status_key = if self.connected {
            "youtubeMusic.status.connected"
        } else {
            "youtubeMusic.status.disconnected"
        };
        self.sync()
    }

    /// A replacement becomes the kept connection only after its whole import succeeds.
    /// Path encoding is checked here; reading the imported file belongs to the worker.
    pub fn connect(&mut self, cookie_file: PathBuf) -> Vec<Effect> {
        self.begin(cookie_file, true)
    }

    pub fn select_browser(&mut self, identifier: &str) {
        if !self.busy {
            if let Some(browser) = Browser::from_identifier(identifier) {
                self.browser = browser;
            }
        }
    }

    pub fn import_browser(&mut self) -> Vec<Effect> {
        if self.busy {
            return Vec::new();
        }
        self.epoch += 1;
        self.busy = true;
        self.failed = false;
        self.status_key = "youtubeMusic.status.importingBrowser";
        self.pending = Some(PendingSync {
            cookie_file: None,
            replacing_connection: true,
        });
        vec![Effect::ImportYouTubeMusicBrowser {
            browser: self.browser,
            epoch: self.epoch,
        }]
    }

    /// The controller has exported and validated a private provider-only jar.
    pub fn browser_imported(&mut self, epoch: u64, cookie_file: PathBuf) -> bool {
        if !self.is_current(epoch) || !self.busy {
            return false;
        }
        let Some(pending) = self
            .pending
            .as_mut()
            .filter(|pending| pending.cookie_file.is_none())
        else {
            return false;
        };
        pending.cookie_file = Some(cookie_file);
        self.status_key = "youtubeMusic.status.connecting";
        true
    }

    pub fn sync(&mut self) -> Vec<Effect> {
        if !self.connected || self.busy {
            return Vec::new();
        }
        let Some(cookie_file) = self.cookie_file.clone() else {
            return Vec::new();
        };
        self.begin(cookie_file, false)
    }

    fn begin(&mut self, cookie_file: PathBuf, replacing_connection: bool) -> Vec<Effect> {
        self.epoch += 1;
        self.pending = None;
        self.busy = false;
        if cookie_file.to_str().is_none() {
            return self.failure(Failure::InvalidInput);
        }
        self.busy = true;
        self.failed = false;
        self.status_key = if replacing_connection {
            "youtubeMusic.status.connecting"
        } else {
            "youtubeMusic.status.syncing"
        };
        self.pending = Some(PendingSync {
            cookie_file: Some(cookie_file.clone()),
            replacing_connection,
        });
        vec![Effect::SyncYouTubeMusicLibrary {
            cookie_file,
            epoch: self.epoch,
        }]
    }

    /// Invalidates network reads and queued imports before the controller removes rows.
    pub fn disconnect(&mut self) -> Vec<Effect> {
        self.epoch += 1;
        self.pending = None;
        self.connected = false;
        self.busy = false;
        self.failed = false;
        self.status_key = "youtubeMusic.status.disconnected";
        self.cookie_file = None;
        self.song_count = 0;
        self.last_sync = None;
        vec![Effect::RemoveYouTubeMusicLibrary]
    }

    pub fn is_current(&self, epoch: u64) -> bool {
        self.epoch == epoch
    }

    pub fn replacing_connection(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.replacing_connection)
    }

    pub fn credentials_rejected(&mut self) -> Vec<Effect> {
        if self.cookie_file.is_none() {
            return Vec::new();
        }
        self.connected = false;
        self.failure(Failure::CredentialsRejected)
    }

    /// Completes a sync attempt. Success requires the full listing to have been
    /// imported and the library reloaded; a failed read or import reports its error.
    pub fn sync_finished(
        &mut self,
        epoch: u64,
        outcome: Result<(usize, i64), Failure>,
    ) -> Vec<Effect> {
        if !self.is_current(epoch) || !self.busy {
            return Vec::new();
        }
        let Some(pending) = self.pending.take() else {
            return Vec::new();
        };
        self.busy = false;
        match outcome {
            Ok((song_count, last_sync)) => {
                let Some(cookie_file) = pending.cookie_file else {
                    return self.failure(Failure::InvalidInput);
                };
                self.connected = true;
                self.failed = false;
                self.status_key = "youtubeMusic.status.connected";
                self.cookie_file = Some(cookie_file);
                self.song_count = song_count;
                self.last_sync = Some(last_sync);
                Vec::new()
            }
            Err(failure) => {
                // A rejected replacement says nothing about the kept session.
                if !pending.replacing_connection && failure.credentials_rejected() {
                    self.connected = false;
                }
                self.failure(failure)
            }
        }
    }

    fn failure(&mut self, failure: Failure) -> Vec<Effect> {
        self.failed = true;
        self.status_key = failure.translation_key();
        vec![Effect::NotifyError {
            key: failure.translation_key(),
            parameters: Vec::new(),
            dedupe_key: Some("youtube-music-library"),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_like_keeps_cached_account_data_and_requires_reconnection() {
        let mut state = YouTubeMusicIntegration::default();
        let request = epoch(
            &state.restore(Some(PathBuf::from("cookies.txt")), 12, Some(100)),
            "cookies.txt",
        );
        state.sync_finished(request, Ok((12, 100)));
        assert_eq!(
            state.credentials_rejected(),
            notification(Failure::CredentialsRejected)
        );
        assert!(!state.connected);
        assert!(state.failed);
        assert_eq!(state.cookie_file, Some(PathBuf::from("cookies.txt")));
        assert_eq!(state.song_count, 12);
        assert_eq!(state.last_sync, Some(100));
        assert!(state.sync().is_empty());
    }

    fn epoch(effects: &[Effect], expected_path: &str) -> u64 {
        match effects {
            [Effect::SyncYouTubeMusicLibrary { cookie_file, epoch }] => {
                assert_eq!(cookie_file, &PathBuf::from(expected_path));
                *epoch
            }
            _ => panic!("a library read emits exactly one sync effect"),
        }
    }

    fn notification(failure: Failure) -> Vec<Effect> {
        vec![Effect::NotifyError {
            key: failure.translation_key(),
            parameters: Vec::new(),
            dedupe_key: Some("youtube-music-library"),
        }]
    }

    fn connected() -> YouTubeMusicIntegration {
        let mut state = YouTubeMusicIntegration::default();
        let current = epoch(&state.connect(PathBuf::from("kept.txt")), "kept.txt");
        state.sync_finished(current, Ok((12, 100)));
        state
    }

    #[test]
    fn browser_replacement_keeps_the_existing_connection_until_library_commit() {
        let mut state = connected();
        state.select_browser("chromium");
        let current = match state.import_browser().as_slice() {
            [Effect::ImportYouTubeMusicBrowser {
                browser: Browser::Chromium,
                epoch,
            }] => *epoch,
            _ => panic!("selected browser import"),
        };
        assert!(state.busy);
        assert_eq!(state.cookie_file, Some(PathBuf::from("kept.txt")));
        state.select_browser("brave");
        assert_eq!(state.browser, Browser::Chromium);
        assert!(state.import_browser().is_empty());
        assert!(state.browser_imported(current, PathBuf::from("managed.txt")));
        assert_eq!(state.cookie_file, Some(PathBuf::from("kept.txt")));
        assert_eq!(state.song_count, 12);
        state.sync_finished(current, Ok((24, 200)));
        assert_eq!(state.cookie_file, Some(PathBuf::from("managed.txt")));
        assert_eq!(state.song_count, 24);
        assert!(!state.busy);
    }

    #[test]
    fn a_failed_browser_export_keeps_the_saved_account_and_library() {
        let mut state = connected();
        let current = match state.import_browser().as_slice() {
            [Effect::ImportYouTubeMusicBrowser { epoch, .. }] => *epoch,
            _ => panic!("browser import"),
        };
        state.sync_finished(current, Err(Failure::BrowserImport));
        assert!(state.connected && state.failed && !state.busy);
        assert_eq!(state.cookie_file, Some(PathBuf::from("kept.txt")));
        assert_eq!(state.song_count, 12);
        assert_eq!(state.last_sync, Some(100));
    }

    #[test]
    fn cancel_during_initial_browser_import_rejects_its_export_and_completion() {
        let mut state = YouTubeMusicIntegration::default();
        let current = match state.import_browser().as_slice() {
            [Effect::ImportYouTubeMusicBrowser { epoch, .. }] => *epoch,
            _ => panic!("browser import"),
        };
        assert_eq!(state.disconnect(), vec![Effect::RemoveYouTubeMusicLibrary]);
        assert!(!state.browser_imported(current, PathBuf::from("discarded.txt")));
        assert!(state.sync_finished(current, Ok((24, 200))).is_empty());
        assert!(!state.connected && !state.busy);
        assert_eq!(state.cookie_file, None);
        assert_eq!(state.song_count, 0);
    }

    #[test]
    fn restoring_a_kept_connection_exposes_cached_songs_while_refreshing() {
        let mut state = YouTubeMusicIntegration::default();
        let current = epoch(
            &state.restore(Some(PathBuf::from("kept.txt")), 12, Some(100)),
            "kept.txt",
        );
        assert!(state.is_current(current));
        assert!(state.connected);
        assert!(state.busy);
        assert!(!state.failed);
        assert_eq!(state.status_key, "youtubeMusic.status.syncing");
        assert_eq!(state.song_count, 12);
        assert_eq!(state.last_sync, Some(100));
        assert_eq!(state.cookie_file, Some(PathBuf::from("kept.txt")));
        assert!(state.sync().is_empty());
        assert!(state.sync_finished(current, Ok((15, 200))).is_empty());
        assert_eq!(state.song_count, 15);
        assert_eq!(state.last_sync, Some(200));
        assert!(!state.busy);
    }

    #[test]
    fn restoring_without_a_connection_does_not_read_or_persist_credentials() {
        let mut state = YouTubeMusicIntegration::default();
        assert!(state.restore(None, 0, None).is_empty());
        assert!(!state.connected);
        assert!(!state.busy);
        assert_eq!(state.status_key, "youtubeMusic.status.disconnected");
        assert!(state.sync().is_empty());
    }

    #[test]
    fn a_failed_startup_refresh_preserves_the_restored_library_and_credentials() {
        for failure in [Failure::Network, Failure::CredentialsRejected] {
            let mut state = YouTubeMusicIntegration::default();
            let current = epoch(
                &state.restore(Some(PathBuf::from("kept.txt")), 12, Some(100)),
                "kept.txt",
            );
            assert_eq!(
                state.sync_finished(current, Err(failure)),
                notification(failure)
            );
            assert_eq!(state.connected, !failure.credentials_rejected());
            assert!(!state.busy);
            assert!(state.failed);
            assert_eq!(state.cookie_file, Some(PathBuf::from("kept.txt")));
            assert_eq!(state.song_count, 12);
            assert_eq!(state.last_sync, Some(100));
        }
    }

    #[test]
    fn connecting_requests_a_read_without_touching_the_file_or_persisting_it() {
        let mut state = YouTubeMusicIntegration::default();
        let current = epoch(
            &state.connect(PathBuf::from("missing-export.txt")),
            "missing-export.txt",
        );
        assert!(!state.connected);
        assert!(state.busy);
        assert_eq!(state.cookie_file, None);
        assert_eq!(state.status_key, "youtubeMusic.status.connecting");
        assert!(state.sync_finished(current, Ok((3, 200))).is_empty());
        assert!(state.connected);
        assert_eq!(state.song_count, 3);
        assert_eq!(state.last_sync, Some(200));
    }

    #[test]
    fn a_failed_initial_connection_keeps_no_unvalidated_cookie_path() {
        let mut state = YouTubeMusicIntegration::default();
        let current = epoch(&state.connect(PathBuf::from("invalid.txt")), "invalid.txt");
        assert_eq!(
            state.sync_finished(current, Err(Failure::InvalidCookieFile)),
            notification(Failure::InvalidCookieFile)
        );
        assert!(!state.connected);
        assert!(!state.busy);
        assert!(state.failed);
        assert_eq!(state.cookie_file, None);
        assert_eq!(state.song_count, 0);
        assert_eq!(state.last_sync, None);
        assert!(state.sync().is_empty());
    }

    #[test]
    fn successful_sync_updates_the_summary_and_a_failed_sync_keeps_cached_songs() {
        let mut state = connected();
        let first = epoch(&state.sync(), "kept.txt");
        assert!(state.connected);
        assert!(state.sync().is_empty());
        state.sync_finished(first, Ok((18, 200)));
        assert_eq!(state.song_count, 18);
        assert_eq!(state.last_sync, Some(200));
        let second = epoch(&state.sync(), "kept.txt");
        assert_ne!(first, second);
        assert_eq!(
            state.sync_finished(second, Err(Failure::Network)),
            notification(Failure::Network)
        );
        assert!(state.connected);
        assert!(state.failed);
        assert!(!state.busy);
        assert_eq!(state.status_key, "youtubeMusic.error.network");
        assert_eq!(state.song_count, 18);
        assert_eq!(state.last_sync, Some(200));
        assert_eq!(state.cookie_file, Some(PathBuf::from("kept.txt")));
    }

    #[test]
    fn failed_replacement_preserves_the_kept_library_and_connection() {
        for failure in [
            Failure::CredentialsRejected,
            Failure::InvalidCookieFile,
            Failure::CookieFileUnreadable,
            Failure::Network,
            Failure::Library,
        ] {
            let mut state = connected();
            let current = epoch(
                &state.connect(PathBuf::from("replacement.txt")),
                "replacement.txt",
            );
            assert!(state.connected);
            assert_eq!(state.cookie_file, Some(PathBuf::from("kept.txt")));
            assert_eq!(
                state.sync_finished(current, Err(failure)),
                notification(failure)
            );
            assert!(state.connected);
            assert!(state.failed);
            assert_eq!(state.cookie_file, Some(PathBuf::from("kept.txt")));
            assert_eq!(state.song_count, 12);
            assert_eq!(state.last_sync, Some(100));
            epoch(&state.sync(), "kept.txt");
        }
    }

    #[test]
    fn replacement_credentials_are_kept_only_after_the_import_succeeds() {
        let mut state = connected();
        let current = epoch(
            &state.connect(PathBuf::from("replacement.txt")),
            "replacement.txt",
        );
        assert_eq!(state.song_count, 12);
        assert_eq!(state.last_sync, Some(100));
        assert!(state.sync_finished(current, Ok((4, 300))).is_empty());
        assert_eq!(state.cookie_file, Some(PathBuf::from("replacement.txt")));
        assert_eq!(state.song_count, 4);
        assert_eq!(state.last_sync, Some(300));
        assert!(!state.failed);
    }

    #[test]
    fn rejected_kept_credentials_request_reconnection_without_deleting_cache() {
        let mut state = connected();
        let current = epoch(&state.sync(), "kept.txt");
        assert_eq!(
            state.sync_finished(current, Err(Failure::CredentialsRejected)),
            notification(Failure::CredentialsRejected)
        );
        assert!(!state.connected);
        assert!(!state.busy);
        assert!(state.failed);
        assert_eq!(state.status_key, "youtubeMusic.status.credentialsRejected");
        assert_eq!(state.cookie_file, Some(PathBuf::from("kept.txt")));
        assert_eq!(state.song_count, 12);
        assert_eq!(state.last_sync, Some(100));
        assert!(state.sync().is_empty());
        assert_eq!(state.disconnect(), vec![Effect::RemoveYouTubeMusicLibrary]);
        assert_eq!(state.song_count, 0);
        assert_eq!(state.cookie_file, None);
    }

    #[test]
    fn disconnect_invalidates_inflight_work_and_clears_only_integration_state() {
        let mut state = connected();
        let pending = epoch(&state.sync(), "kept.txt");
        assert_eq!(state.disconnect(), vec![Effect::RemoveYouTubeMusicLibrary]);
        assert!(!state.is_current(pending));
        assert!(!state.connected);
        assert!(!state.busy);
        assert!(!state.failed);
        assert_eq!(state.cookie_file, None);
        assert_eq!(state.song_count, 0);
        assert_eq!(state.last_sync, None);
        assert!(state.sync_finished(pending, Ok((99, 400))).is_empty());
        assert!(state
            .sync_finished(pending, Err(Failure::Network))
            .is_empty());
        assert_eq!(state.status_key, "youtubeMusic.status.disconnected");
        assert_eq!(state.cookie_file, None);
    }

    #[test]
    fn superseded_reads_and_duplicate_completions_cannot_change_the_kept_connection() {
        let mut state = connected();
        let old = epoch(&state.connect(PathBuf::from("first.txt")), "first.txt");
        let current = epoch(&state.connect(PathBuf::from("second.txt")), "second.txt");
        assert!(!state.is_current(old));
        assert!(state
            .sync_finished(old, Err(Failure::CredentialsRejected))
            .is_empty());
        assert!(state.sync_finished(old, Ok((99, 400))).is_empty());
        assert!(state.busy);
        assert!(!state.failed);
        assert_eq!(state.cookie_file, Some(PathBuf::from("kept.txt")));
        state.sync_finished(current, Ok((8, 500)));
        assert!(state
            .sync_finished(current, Err(Failure::Network))
            .is_empty());
        assert!(state.sync_finished(current, Ok((99, 600))).is_empty());
        assert_eq!(state.cookie_file, Some(PathBuf::from("second.txt")));
        assert_eq!(state.song_count, 8);
        assert_eq!(state.last_sync, Some(500));
        assert!(!state.failed);
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_replacement_paths_fail_before_reading_and_preserve_the_kept_connection() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let mut state = connected();
        let pending = epoch(&state.sync(), "kept.txt");
        let path = PathBuf::from(OsString::from_vec(vec![0xff]));
        assert_eq!(state.connect(path), notification(Failure::InvalidInput));
        assert!(!state.is_current(pending));
        assert!(state.connected);
        assert!(!state.busy);
        assert!(state.failed);
        assert_eq!(state.cookie_file, Some(PathBuf::from("kept.txt")));
        assert_eq!(state.song_count, 12);
        assert_eq!(state.last_sync, Some(100));
        assert!(state.sync_finished(pending, Ok((99, 600))).is_empty());
    }
}
