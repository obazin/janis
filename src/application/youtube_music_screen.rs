//! Anonymous YouTube Music browsing and native playback source resolution.

use std::cell::RefCell;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

use slint::{ComponentHandle, ModelRc, VecModel};

use super::{interface_id, list_place, post, Application};
use crate::background::Worker;
use crate::covers::{self, Pixels};
use crate::effects::YouTubeMusicRequest;
use crate::text;
use crate::user_interface::{self, YouTubeMusic};
use crate::youtube_music::{Failure, Session};
use crate::youtube_music_integration::YouTubeMusicIntegration;
use crate::youtube_music_state::YouTubeMusicState;

pub(super) struct YouTubeMusicController {
    pub(super) state: RefCell<YouTubeMusicState>,
    pub(super) integration: RefCell<YouTubeMusicIntegration>,
    session: Session,
    pub(super) account_cookie_file: Arc<RwLock<Option<PathBuf>>>,
    pub(super) generation: Arc<AtomicU64>,
    pub(super) sync_generation: Arc<AtomicU64>,
    pub(super) favourite_generation: Arc<AtomicU64>,
    pub(super) browser_activity: Arc<BrowserImportActivity>,
    pub(super) extraction_activity: Arc<ExtractionActivity>,
    pub(super) runtime: Arc<tokio::runtime::Runtime>,
    pub(super) account_worker: Worker,
    worker: Worker,
    songs: Rc<VecModel<user_interface::YouTubeMusicSong>>,
}

impl YouTubeMusicController {
    pub(super) fn new(cookie_file: Option<PathBuf>) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("janis youtube network")
            .enable_all()
            .build()
            .map_err(|error| format!("start YouTube Music runtime: {error}"))?;
        Ok(Self {
            state: RefCell::new(YouTubeMusicState::default()),
            integration: RefCell::new(YouTubeMusicIntegration::default()),
            session: Session::anonymous().map_err(|error| error.to_string())?,
            account_cookie_file: Arc::new(RwLock::new(cookie_file)),
            generation: Arc::new(AtomicU64::new(0)),
            sync_generation: Arc::new(AtomicU64::new(0)),
            favourite_generation: Arc::new(AtomicU64::new(0)),
            browser_activity: Arc::new(BrowserImportActivity::default()),
            extraction_activity: Arc::new(ExtractionActivity::default()),
            runtime: Arc::new(runtime),
            account_worker: Worker::new("janis youtube account")?,
            worker: Worker::new("janis youtube browsing")?,
            songs: Rc::new(VecModel::default()),
        })
    }

    pub(super) fn session(&self) -> Session {
        self.session.clone()
    }
}

/// Closing the app cancels and joins an active browser read before process exit,
/// so its child and all-sites cookie workspace cannot outlive the controller.
#[derive(Default)]
pub(super) struct BrowserImportActivity {
    active: Mutex<bool>,
    finished: Condvar,
}

impl BrowserImportActivity {
    pub(super) fn begin(
        self: &Arc<Self>,
        generation: &AtomicU64,
        epoch: u64,
    ) -> Option<BrowserImportGuard> {
        let mut active = self
            .active
            .lock()
            .expect("browser import activity lock poisoned");
        if generation.load(Ordering::Acquire) != epoch {
            return None;
        }
        *active = true;
        Some(BrowserImportGuard(Arc::clone(self)))
    }

    pub(super) fn shut_down(&self, generation: &AtomicU64) {
        let mut active = self
            .active
            .lock()
            .expect("browser import activity lock poisoned");
        generation.fetch_add(1, Ordering::AcqRel);
        while *active {
            active = self
                .finished
                .wait(active)
                .expect("browser import activity lock poisoned");
        }
    }
}

pub(super) struct BrowserImportGuard(Arc<BrowserImportActivity>);

impl Drop for BrowserImportGuard {
    fn drop(&mut self) {
        *self
            .0
            .active
            .lock()
            .expect("browser import activity lock poisoned") = false;
        self.0.finished.notify_all();
    }
}

/// Extraction tasks outlive an abandoned resolver future only long enough to
/// reap their child. Closing registration and joining them precedes runtime exit.
#[derive(Default)]
pub(super) struct ExtractionActivity {
    state: Mutex<(bool, usize)>,
    finished: Condvar,
}

impl ExtractionActivity {
    fn begin(self: &Arc<Self>) -> Option<ExtractionGuard> {
        let mut state = self
            .state
            .lock()
            .expect("extraction activity lock poisoned");
        if state.0 {
            return None;
        }
        state.1 += 1;
        Some(ExtractionGuard(Arc::clone(self)))
    }

    fn closing(&self) -> bool {
        self.state
            .lock()
            .expect("extraction activity lock poisoned")
            .0
    }

    pub(super) fn shut_down(&self) {
        let mut state = self
            .state
            .lock()
            .expect("extraction activity lock poisoned");
        state.0 = true;
        while state.1 != 0 {
            state = self
                .finished
                .wait(state)
                .expect("extraction activity lock poisoned");
        }
    }
}

struct ExtractionGuard(Arc<ExtractionActivity>);

impl Drop for ExtractionGuard {
    fn drop(&mut self) {
        self.0
            .state
            .lock()
            .expect("extraction activity lock poisoned")
            .1 -= 1;
        self.0.finished.notify_all();
    }
}

pub(super) struct PlaybackResolver {
    pub(super) account_cookie_file: Arc<RwLock<Option<PathBuf>>>,
    pub(super) generation: Arc<AtomicU64>,
    pub(super) extraction_activity: Arc<ExtractionActivity>,
}

impl PlaybackResolver {
    pub(super) async fn resolve_source(
        &self,
        identifier: &str,
        overview_epoch: Option<(Arc<AtomicU64>, u64)>,
    ) -> Result<crate::audio::RemoteFile, String> {
        let (video_identifier, overview, account) = [
            ("youtube-music://", false, false),
            ("youtube-music-overview://", true, false),
            ("youtube-music-account://", false, true),
            ("youtube-music-account-overview://", true, true),
        ]
        .into_iter()
        .find_map(|(prefix, overview, account)| {
            identifier
                .strip_prefix(prefix)
                .map(|video| (video, overview, account))
        })
        .ok_or_else(|| Failure::InvalidInput.translation_key().to_owned())?;
        let (cookie_file, generation) = if account {
            let cookie_file = self
                .account_cookie_file
                .read()
                .expect("YouTube Music cookie path lock poisoned");
            let path = cookie_file
                .clone()
                .ok_or_else(|| Failure::CredentialsRejected.translation_key().to_owned())?;
            (Some(path), self.generation.load(Ordering::Acquire))
        } else {
            (None, 0)
        };
        let activity = Arc::clone(&self.extraction_activity);
        let guard = activity
            .begin()
            .ok_or_else(|| Failure::Extraction.translation_key().to_owned())?;
        let account_generation = Arc::clone(&self.generation);
        let video = video_identifier.to_owned();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        // The owning future may be dropped by the engine. A detached task
        // observes its closed receiver and explicitly reaps before finishing.
        tokio::spawn(async move {
            let guard = Arc::new(guard);
            let resolved = crate::youtube_extraction::resolve(
                &video,
                cookie_file.as_deref(),
                || {
                    sender.is_closed()
                        || activity.closing()
                        || (account && account_generation.load(Ordering::Acquire) != generation)
                        || overview_epoch
                            .as_ref()
                            .is_some_and(|(shared, epoch)| shared.load(Ordering::Relaxed) != *epoch)
                },
                guard,
            )
            .await;
            let _ = sender.send(resolved);
        });
        let resolved = receiver
            .await
            .map_err(|_| Failure::Extraction.translation_key().to_owned())?
            .map_err(|error| error.translation_key().to_owned())?;
        if account && self.generation.load(Ordering::Acquire) != generation {
            return Err(Failure::CredentialsRejected.translation_key().to_owned());
        }
        if !overview {
            let path = format!("youtube-music://{video_identifier}");
            let expires_at = resolved.expires_at;
            post(move |application| {
                if !account
                    || application.youtube_music.generation.load(Ordering::Acquire) == generation
                {
                    application
                        .player
                        .borrow_mut()
                        .source_resolved(&path, expires_at);
                }
            });
        }
        match resolved.location {
            crate::audio::Location::Remote(remote) => Ok(remote),
            _ => Err(Failure::UnsupportedAudio.translation_key().to_owned()),
        }
    }
}

impl crate::audio::SourceResolver for PlaybackResolver {
    fn resolve<'a>(
        &'a self,
        identifier: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<crate::audio::RemoteFile, String>> + Send + 'a>> {
        Box::pin(self.resolve_source(identifier, None))
    }
}

impl Application {
    pub(super) fn connect_youtube_music(self: &Rc<Self>) {
        let view = self.window.global::<YouTubeMusic>();
        view.set_songs(ModelRc::from(self.youtube_music.songs.clone()));
        view.on_search_submitted(self.action(|application| {
            let query = application.window.global::<YouTubeMusic>().get_query();
            let effects = application.youtube_music.state.borrow_mut().search(&query);
            application.run(effects);
            application.refresh_youtube_music();
        }));
        view.on_more_clicked(self.action(|application| {
            let effects = application.youtube_music.state.borrow_mut().more();
            application.run(effects);
            application.refresh_youtube_music();
        }));
        view.on_song_clicked(self.handler(|application, index: i32| {
            let tracks = application.youtube_music.state.borrow().queue();
            if let Some(index) = list_place(index).filter(|index| *index < tracks.len()) {
                application.play_queue(tracks, index);
            }
        }));
        self.connect_youtube_integration();
    }

    pub(super) fn read_youtube_music(&self, request: YouTubeMusicRequest, epoch: u64) {
        let session = self.youtube_music.session();
        let runtime = Arc::clone(&self.youtube_music.runtime);
        let next_songs = self.youtube_music.state.borrow().next_songs.clone();
        self.youtube_music.worker.run(move || {
            let append = matches!(request, YouTubeMusicRequest::MoreSongs);
            let outcome = runtime.block_on(async {
                match request {
                    YouTubeMusicRequest::Search(query) => session.search(&query).await,
                    YouTubeMusicRequest::MoreSongs => match next_songs {
                        Some(next) => session.more_songs(next).await,
                        None => Err(Failure::InvalidContinuation),
                    },
                }
            });
            post(move |application| {
                if !application.youtube_music.state.borrow().is_current(epoch) {
                    return;
                }
                if let Err(failure) = &outcome {
                    log::warn!("YouTube Music read: {failure}");
                }
                let effects = application
                    .youtube_music
                    .state
                    .borrow_mut()
                    .songs_finished(epoch, outcome, append);
                application.run(effects);
                application.refresh_youtube_music();
                #[cfg(debug_assertions)]
                if std::env::var_os("JANIS_YOUTUBE_AUTOPLAY").is_some()
                    && application.player.borrow().mode() == crate::audio::Mode::Idle
                {
                    let tracks = application.youtube_music.state.borrow().queue();
                    if !tracks.is_empty() {
                        application.engine.set_volume(0.0);
                        application.play_queue(tracks, 0);
                    }
                }
            });
        });
    }

    pub(super) fn refresh_youtube_music(&self) {
        let state = self.youtube_music.state.borrow();
        let view = self.window.global::<YouTubeMusic>();
        view.set_loading(state.loading);
        view.set_status_failed(state.failed);
        view.set_has_more(state.has_more());
        view.set_status(
            if state.status_key.is_empty() {
                String::new()
            } else {
                self.translations.borrow().text(state.status_key)
            }
            .into(),
        );
        self.youtube_music.songs.set_vec(
            state
                .songs
                .iter()
                .enumerate()
                .map(|(index, row)| user_interface::YouTubeMusicSong {
                    track_id: interface_id(row.track.id),
                    number: (index + 1).to_string().into(),
                    title: row.song.title.clone().into(),
                    artist: row.song.artist.clone().into(),
                    album: row.song.album.clone().unwrap_or_default().into(),
                    duration: text::format_time(f64::from(row.song.duration_seconds)).into(),
                    gradient_index: super::gradient_index(&row.track),
                })
                .collect::<Vec<_>>(),
        );
        self.refresh_youtube_integration();
    }
}

pub(super) fn source_picture(
    session: &Session,
    runtime: &tokio::runtime::Runtime,
    address: Option<&str>,
    longest_side: u32,
) -> Result<Option<Pixels>, String> {
    address
        .map(|address| {
            let bytes = runtime
                .block_on(session.artwork(address, longest_side))
                .map_err(|error| error.to_string())?;
            covers::picture_cover(bytes, longest_side)
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_waits_for_extraction_cleanup_and_rejects_new_tasks() {
        let activity = Arc::new(ExtractionActivity::default());
        let guard = activity.begin().expect("active request");
        let closing = Arc::clone(&activity);
        let (finished, receiver) = std::sync::mpsc::channel();
        let shutdown = std::thread::spawn(move || {
            closing.shut_down();
            finished.send(()).expect("completion receiver");
        });
        let started = std::time::Instant::now();
        while !activity.closing() {
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
            std::thread::yield_now();
        }
        assert!(activity.begin().is_none());
        assert!(receiver.try_recv().is_err());
        drop(guard);
        receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("cleanup unblocks shutdown");
        shutdown.join().expect("shutdown thread");
    }
}
