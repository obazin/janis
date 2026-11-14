//! The controller's side of the Jellyfin card in Settings: signing in,
//! reading the server's songs into the library, signing out, and where the
//! engine and the cover loader find a server song.
//!
//! Every call to the server runs on the server worker, one after another, so
//! a sign-out queued behind a sync runs after it and the library ends without
//! the server's songs.

use std::rc::Rc;
use std::sync::Arc;

use slint::ComponentHandle;

use super::library_screens::LibrarySnapshot;
use super::{post, Application};
use crate::audio::Location;
use crate::effects::Secret;
use crate::jellyfin::{self, Failure, Session};
use crate::library::{self, ScanReport, Track};
use crate::persistence::{self, MediaServer};
use crate::user_interface;

impl Application {
    pub(super) fn connect_jellyfin(self: &Rc<Self>) {
        let view = self.window.global::<user_interface::Jellyfin>();
        view.on_sign_in_clicked(self.action(Application::jellyfin_sign_in_clicked));
        view.on_sync_clicked(self.action(|application| {
            let effects = application.media_server.borrow_mut().sync();
            application.run(effects);
            application.refresh_jellyfin();
        }));
        view.on_sign_out_clicked(self.action(|application| {
            let effects = application.media_server.borrow_mut().sign_out();
            application.run(effects);
            application.refresh_jellyfin();
        }));
    }

    fn jellyfin_sign_in_clicked(&self) {
        let view = self.window.global::<user_interface::Jellyfin>();
        let password = view.get_password();
        // The password goes to the server once and is kept nowhere: not in
        // the field, not in the database.
        view.set_password(Default::default());
        let effects = self.media_server.borrow_mut().sign_in(
            &view.get_address(),
            &view.get_user_name(),
            &password,
        );
        self.run(effects);
        self.refresh_jellyfin();
    }

    pub(super) fn sign_in_media_server(
        &self,
        address: String,
        user_name: String,
        password: Secret,
        epoch: u64,
    ) {
        let database = Arc::clone(&self.database);
        let device_identifier = self.device_identifier.clone();
        self.server_worker.run(move || {
            let outcome =
                jellyfin::sign_in(&address, &user_name, password.expose(), &device_identifier)
                    .and_then(|signed_in| {
                        persistence::save_media_server(&database, &signed_in)
                            .map_err(Failure::Other)
                    });
            post(move |application| {
                let effects = application
                    .media_server
                    .borrow_mut()
                    .sign_in_finished(epoch, outcome);
                application.run(effects);
                application.refresh_jellyfin();
            });
        });
    }

    pub(super) fn sync_media_server(&self, server: MediaServer, epoch: u64) {
        let database = Arc::clone(&self.database);
        let session = Session::new(&server, &self.device_identifier);
        self.server_worker.run(move || {
            let outcome = jellyfin::songs(&session, &server.server_identifier)
                .and_then(|songs| {
                    library::sync_server_tracks(&database, server.id, songs).map_err(Failure::Other)
                })
                .and_then(|report| {
                    persistence::set_last_sync(&database, server.id).map_err(Failure::Other)?;
                    let snapshot = LibrarySnapshot::read(&database).map_err(Failure::Other)?;
                    Ok((report, snapshot))
                });
            post(move |application| application.media_server_synced(epoch, outcome));
        });
    }

    fn media_server_synced(
        &self,
        epoch: u64,
        outcome: Result<(ScanReport, LibrarySnapshot), Failure>,
    ) {
        let (report, snapshot) = match outcome {
            Ok((report, snapshot)) => (Ok(report), Some(snapshot)),
            Err(failure) => (Err(failure), None),
        };
        if let Ok(report) = &report {
            log::info!(
                "Jellyfin sync: {} added, {} updated, {} removed, {} skipped",
                report.added,
                report.updated,
                report.removed,
                report.skipped
            );
        }
        let effects = self
            .media_server
            .borrow_mut()
            .sync_finished(epoch, report, unix_now());
        self.run(effects);
        // The database is the truth whichever request this answers: a
        // sign-out queued behind this sync posts its own library after it.
        if let Some(snapshot) = snapshot {
            self.take_library(snapshot);
            self.refresh_everything();
        } else {
            self.refresh_jellyfin();
        }
    }

    pub(super) fn sign_out_media_server(&self, server: MediaServer) {
        let database = Arc::clone(&self.database);
        let session = Session::new(&server, &self.device_identifier);
        self.server_worker.run(move || {
            jellyfin::sign_out(&session);
            let outcome = persistence::remove_media_server(&database, server.id)
                .and_then(|()| LibrarySnapshot::read(&database));
            post(move |application| match outcome {
                Ok(snapshot) => {
                    application.take_library(snapshot);
                    application.refresh_everything();
                }
                Err(error) => {
                    log::error!("Jellyfin sign-out: {error}");
                    application.notify_error("error.library", vec![("message", error)], None);
                }
            });
        });
    }

    /// Where the engine finds a track's audio: its file, or the original file
    /// on its server.
    pub(super) fn track_location(&self, track: &Track) -> Location {
        jellyfin::track_location(
            track,
            self.media_server.borrow().server(),
            &self.device_identifier,
        )
    }

    /// Where a track's overview is read from, in the order to try them: its
    /// file; or, for a song on the signed-in server, the server's small
    /// transcode, then the original file should the server not transcode. A
    /// song on a server Janis is not signed in to has nowhere to be read from.
    pub(super) fn overview_locations(&self, track: &Track) -> Vec<Location> {
        jellyfin::overview_locations(
            track,
            self.media_server.borrow().server(),
            &self.device_identifier,
        )
    }

    /// The signed-in session, for the cover loader.
    pub(super) fn media_session(&self) -> Option<Session> {
        self.media_server
            .borrow()
            .server()
            .map(|server| Session::new(server, &self.device_identifier))
    }

    /// Pushes the Jellyfin card's state to the window.
    pub(super) fn refresh_jellyfin(&self) {
        let card = self.media_server.borrow().card(&self.tracks.borrow());
        let translations = self.translations.borrow();
        let view = self.window.global::<user_interface::Jellyfin>();

        view.set_signed_in(card.signed_in);
        view.set_busy(card.busy);
        view.set_status(
            card.status_key
                .map(|key| translations.text(key))
                .unwrap_or_default()
                .into(),
        );
        view.set_status_failed(card.status_failed);
        view.set_summary(
            card.summary
                .map(|summary| {
                    translations.count_with(
                        "jellyfin.summary",
                        summary.song_count,
                        &[("server", summary.server_name), ("user", summary.user_name)],
                    )
                })
                .unwrap_or_default()
                .into(),
        );
        // What the listener already typed is kept.
        if let Some((address, user_name)) = card.prefill {
            if view.get_address().is_empty() {
                view.set_address(address.into());
            }
            if view.get_user_name().is_empty() {
                view.set_user_name(user_name.into());
            }
        }
    }
}

/// The time now, in Unix seconds.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
