//! The Settings integration: complete account reads, serialized library imports,
//! and source-only removal. Browsing never uses the account's API session.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use slint::ComponentHandle;

use super::{post, Application};
use crate::library::{self, LibrarySnapshot, YouTubeMusicLibrary};
use crate::persistence;
use crate::user_interface::YouTubeMusicIntegration;
use crate::youtube_browser_import::{self, Browser};
use crate::youtube_music::{Failure, Session};
use tempfile::NamedTempFile;

impl Application {
    pub(super) fn connect_youtube_integration(self: &Rc<Self>) {
        let view = self.window.global::<YouTubeMusicIntegration>();
        view.on_browser_selected(
            self.handler(|application, identifier: slint::SharedString| {
                application
                    .youtube_music
                    .integration
                    .borrow_mut()
                    .select_browser(&identifier);
                application.refresh_youtube_integration();
            }),
        );
        view.on_browser_import_clicked(self.action(|application| {
            let effects = application
                .youtube_music
                .integration
                .borrow_mut()
                .import_browser();
            application.run(effects);
            application.refresh_youtube_integration();
        }));
        view.on_sync_clicked(self.action(|application| {
            let effects = application.youtube_music.integration.borrow_mut().sync();
            application.run(effects);
            application.refresh_youtube_integration();
        }));
        view.on_disconnect_clicked(self.action(|application| {
            let effects = application
                .youtube_music
                .integration
                .borrow_mut()
                .disconnect();
            application.run(effects);
            application.refresh_youtube_integration();
        }));
        let weak = Rc::downgrade(self);
        view.on_connect_clicked(move || {
            if weak
                .upgrade()
                .is_none_or(|application| application.youtube_music.integration.borrow().busy)
            {
                return;
            }
            let weak = weak.clone();
            let Some(title) = weak.upgrade().map(|application| {
                application
                    .translations
                    .borrow()
                    .text("youtubeMusic.connect")
            }) else {
                return;
            };
            if let Err(error) = slint::spawn_local(async move {
                let selected = rfd::AsyncFileDialog::new()
                    .set_title(title)
                    .pick_file()
                    .await;
                if let (Some(application), Some(selected)) = (weak.upgrade(), selected) {
                    if application.youtube_music.integration.borrow().busy {
                        return;
                    }
                    let effects = application
                        .youtube_music
                        .integration
                        .borrow_mut()
                        .connect(selected.path().to_path_buf());
                    application.run(effects);
                    application.refresh_youtube_integration();
                }
            }) {
                log::error!("open YouTube cookie picker: {error}");
            }
        });
    }

    fn cancel_youtube_favourites(&self) {
        self.youtube_music
            .favourite_generation
            .fetch_add(1, Ordering::AcqRel);
        let changed = self.favourite_tracks.borrow_mut().cancel_youtube_pushes();
        for track_id in changed {
            self.refresh_favourite(track_id);
        }
        self.refresh_player();
    }

    pub(super) fn push_youtube_favourite(
        &self,
        track_id: i64,
        address: String,
        favourite: bool,
        sequence: u64,
    ) {
        let epoch = self
            .youtube_music
            .favourite_generation
            .load(Ordering::Acquire);
        let state = self.youtube_music.integration.borrow();
        let available = state.connected && !state.replacing_connection();
        let cookie_file = state.cookie_file.clone();
        let replacing = state.replacing_connection();
        drop(state);
        let Some(cookie_file) = cookie_file.filter(|_| available) else {
            let failure = if replacing {
                Failure::Unavailable
            } else {
                Failure::CredentialsRejected
            };
            self.youtube_favourite_finished(
                epoch,
                track_id,
                address,
                favourite,
                sequence,
                Err(failure),
            );
            return;
        };
        let runtime = Arc::clone(&self.youtube_music.runtime);
        let generation = Arc::clone(&self.youtube_music.favourite_generation);
        self.youtube_music.account_worker.run(move || {
            if generation.load(Ordering::Acquire) != epoch {
                return;
            }
            let outcome = runtime.block_on(async {
                tokio::select! {
                    outcome = async {
                        Session::from_cookie_file(&cookie_file).await?
                            .set_favourite(&address, favourite).await
                    } => Some(outcome),
                    () = wait_for_sync_cancellation(&generation, epoch) => None,
                }
            });
            if let Some(outcome) = outcome {
                post(move |application| {
                    application.youtube_favourite_finished(
                        epoch, track_id, address, favourite, sequence, outcome,
                    )
                });
            }
        });
    }

    fn youtube_favourite_finished(
        &self,
        epoch: u64,
        track_id: i64,
        address: String,
        favourite: bool,
        sequence: u64,
        outcome: Result<(), Failure>,
    ) {
        if self
            .youtube_music
            .favourite_generation
            .load(Ordering::Acquire)
            != epoch
        {
            return;
        }
        let rejected = outcome.is_err_and(|failure| failure.credentials_rejected());
        let effects = self
            .favourite_tracks
            .borrow_mut()
            .youtube_push_finished(track_id, address, sequence, favourite, outcome);
        if rejected {
            self.cancel_youtube_favourites();
            let effects = self
                .youtube_music
                .integration
                .borrow_mut()
                .credentials_rejected();
            self.run(effects);
            self.refresh_youtube_integration();
        }
        self.run(effects);
        self.refresh_favourite(track_id);
        self.refresh_player();
    }

    pub(super) fn persist_youtube_favourite(
        &self,
        track_id: i64,
        address: String,
        favourite: bool,
    ) {
        let epoch = self
            .youtube_music
            .favourite_generation
            .load(Ordering::Acquire);
        let generation = Arc::clone(&self.youtube_music.favourite_generation);
        let database = Arc::clone(&self.database);
        self.library_worker.run(move || {
            if generation.load(Ordering::Acquire) != epoch {
                return;
            }
            let outcome = (|| {
                if !library::set_youtube_music_favourite(&database, track_id, &address, favourite)?
                {
                    return Ok(None);
                }
                LibrarySnapshot::read(&database).map(Some)
            })();
            post(move |application| {
                if application
                    .youtube_music
                    .favourite_generation
                    .load(Ordering::Acquire)
                    != epoch
                {
                    return;
                }
                match outcome {
                    Ok(Some(snapshot)) => {
                        application.take_library(snapshot);
                        application.refresh_favourite(track_id);
                        application.refresh_playlists();
                        application.refresh_player();
                    }
                    Ok(None) => {}
                    Err(error) => {
                        log::error!("save YouTube Music favourite: {error}");
                        application.notify_error("error.library", vec![("message", error)], None);
                    }
                }
            });
        });
    }

    pub(super) fn import_youtube_browser(&self, browser: Browser, epoch: u64) {
        self.cancel_youtube_favourites();
        self.youtube_music
            .sync_generation
            .store(epoch, Ordering::Release);
        let generation = Arc::clone(&self.youtube_music.sync_generation);
        let activity = Arc::clone(&self.youtube_music.browser_activity);
        let runtime = Arc::clone(&self.youtube_music.runtime);
        self.youtube_music.account_worker.run(move || {
            let Some(_active) = activity.begin(&generation, epoch) else { return; };
            let outcome = runtime.block_on(async {
                tokio::select! {
                    outcome = async {
                        let directory = managed_cookie_directory()?;
                        let cookies = youtube_browser_import::export(browser, &directory).await?;
                        let songs = Session::from_cookie_file(cookies.path()).await?.saved_library().await?;
                        Ok((cookies, songs))
                    } => outcome,
                    () = wait_for_sync_cancellation(&generation, epoch) => Err(Failure::BrowserImport),
                }
            });
            post(move |application| {
                if !application.youtube_music.integration.borrow().is_current(epoch) {
                    application.library_worker.run(move || drop(outcome));
                    return;
                }
                match outcome {
                    Ok((cookies, songs)) => {
                        let path = cookies.path().to_path_buf();
                        if application.youtube_music.integration.borrow_mut().browser_imported(epoch, path.clone()) {
                            application.youtube_library_read(path, epoch, Ok(songs), Some(cookies));
                        } else {
                            application.library_worker.run(move || drop(cookies));
                        }
                    }
                    Err(failure) => {
                        let effects = application.youtube_music.integration.borrow_mut().sync_finished(epoch, Err(failure));
                        application.run(effects);
                    }
                }
                application.refresh_youtube_integration();
            });
        });
    }

    pub(super) fn sync_youtube_library(&self, cookie_file: PathBuf, epoch: u64) {
        if self
            .youtube_music
            .integration
            .borrow()
            .replacing_connection()
        {
            self.cancel_youtube_favourites();
        }
        self.youtube_music
            .sync_generation
            .store(epoch, Ordering::Release);
        let runtime = Arc::clone(&self.youtube_music.runtime);
        let generation = Arc::clone(&self.youtube_music.sync_generation);
        self.youtube_music.account_worker.run(move || {
            let outcome = runtime.block_on(async {
                tokio::select! {
                    outcome = async {
                        Session::from_cookie_file(&cookie_file)
                            .await?
                            .saved_library()
                            .await
                    } => Some(outcome),
                    () = wait_for_sync_cancellation(&generation, epoch) => None,
                }
            });
            let Some(outcome) = outcome else {
                return;
            };
            post(move |application| {
                application.youtube_library_read(cookie_file, epoch, outcome, None);
            });
        });
    }

    fn youtube_library_read(
        &self,
        cookie_file: PathBuf,
        epoch: u64,
        outcome: Result<YouTubeMusicLibrary, Failure>,
        cookies: Option<NamedTempFile>,
    ) {
        if !self.youtube_music.integration.borrow().is_current(epoch) {
            return;
        }
        let songs = match outcome {
            Ok(songs) => songs,
            Err(failure) => {
                let effects = self
                    .youtube_music
                    .integration
                    .borrow_mut()
                    .sync_finished(epoch, Err(failure));
                self.run(effects);
                self.refresh_youtube_integration();
                return;
            }
        };
        let database = Arc::clone(&self.database);
        let generation = Arc::clone(&self.youtube_music.sync_generation);
        self.library_worker.run(move || {
            // Disconnect can overtake a network read. It cannot overtake a
            // started import: its removal is queued on this same worker.
            if generation.load(Ordering::Acquire) != epoch {
                return;
            }
            let outcome = (|| {
                let managed = if let Some(cookies) = cookies {
                    let (file, path) = cookies
                        .keep()
                        .map_err(|_| "keep imported YouTube cookies".to_owned())?;
                    drop(file);
                    Some(path)
                } else {
                    None
                };
                let result = (|| {
                    let previous = persistence::youtube_music_cookie_file(&database)?;
                    library::sync_youtube_music_library(&database, &songs, &cookie_file)?;
                    if previous.as_deref() != Some(cookie_file.as_path()) {
                        if let Some(previous) = previous {
                            remove_managed_cookie(&previous);
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    if let Some(path) = managed {
                        remove_managed_cookie(&path);
                    }
                    return Err(error);
                }
                let count = library::count_youtube_music_tracks(&database)?;
                let last_sync = persistence::youtube_music_last_sync(&database)?
                    .ok_or_else(|| "YouTube Music sync timestamp is missing".to_owned())?;
                Ok((LibrarySnapshot::read(&database)?, count, last_sync))
            })();
            post(move |application| {
                application.youtube_library_imported(cookie_file, epoch, outcome);
            });
        });
    }

    fn youtube_library_imported(
        &self,
        cookie_file: PathBuf,
        epoch: u64,
        outcome: Result<(LibrarySnapshot, usize, i64), String>,
    ) {
        if !self.youtube_music.integration.borrow().is_current(epoch) {
            return;
        }
        let result = match outcome {
            Ok((snapshot, count, last_sync)) => {
                {
                    let mut current = self
                        .youtube_music
                        .account_cookie_file
                        .write()
                        .expect("YouTube Music cookie path lock poisoned");
                    if current.as_ref() != Some(&cookie_file) {
                        *current = Some(cookie_file);
                        self.youtube_music
                            .generation
                            .fetch_add(1, Ordering::Release);
                    }
                }
                self.take_library(snapshot);
                Ok((count, last_sync))
            }
            Err(error) => {
                log::error!("YouTube Music library sync: {error}");
                self.notify_error("error.library", vec![("message", error)], None);
                Err(Failure::Library)
            }
        };
        let effects = self
            .youtube_music
            .integration
            .borrow_mut()
            .sync_finished(epoch, result);
        self.run(effects);
        self.refresh_everything();
    }

    pub(super) fn remove_youtube_library(&self) {
        self.cancel_youtube_favourites();
        self.youtube_music
            .sync_generation
            .fetch_add(1, Ordering::AcqRel);
        {
            let mut path = self
                .youtube_music
                .account_cookie_file
                .write()
                .expect("YouTube Music cookie path lock poisoned");
            *path = None;
            self.youtube_music
                .generation
                .fetch_add(1, Ordering::Release);
        }
        // Anonymous playback has no dependency on this connection.
        if self
            .player
            .borrow()
            .current()
            .is_some_and(|track| track.id > 0 && track.source == library::TrackSource::YouTubeMusic)
        {
            self.engine.stop();
        }
        let database = Arc::clone(&self.database);
        self.library_worker.run(move || {
            let outcome = (|| {
                let previous = persistence::youtube_music_cookie_file(&database)?;
                library::remove_youtube_music_tracks(&database)?;
                if let Some(previous) = previous {
                    remove_managed_cookie(&previous);
                }
                LibrarySnapshot::read(&database)
            })();
            post(move |application| match outcome {
                Ok(snapshot) => {
                    application.take_library(snapshot);
                    application.refresh_everything();
                }
                Err(error) => {
                    log::error!("YouTube Music removal: {error}");
                    application.notify_error("error.library", vec![("message", error)], None);
                }
            });
        });
    }

    pub(super) fn refresh_youtube_integration(&self) {
        let state = self.youtube_music.integration.borrow();
        let translations = self.translations.borrow();
        let view = self.window.global::<YouTubeMusicIntegration>();
        view.set_browser(state.browser.identifier().into());
        view.set_connected(state.connected);
        view.set_configured(state.cookie_file.is_some());
        view.set_busy(state.busy);
        view.set_status_failed(state.failed);
        view.set_status(
            if state.status_key.is_empty() {
                String::new()
            } else {
                translations.text(state.status_key)
            }
            .into(),
        );
        view.set_summary(
            translations
                .count("youtubeMusic.summary", state.song_count)
                .into(),
        );
    }
}

async fn wait_for_sync_cancellation(generation: &AtomicU64, epoch: u64) {
    while generation.load(Ordering::Acquire) == epoch {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

fn managed_cookie_directory() -> Result<PathBuf, Failure> {
    crate::platform::data_directory()
        .map(|directory| directory.join("youtube-cookies"))
        .map_err(|_| Failure::BrowserImport)
}

fn remove_managed_cookie(path: &std::path::Path) {
    if let Err(failure) = managed_cookie_directory()
        .and_then(|directory| youtube_browser_import::remove_managed(path, &directory))
    {
        log::warn!("remove managed YouTube cookies: {failure}");
    }
}
