//! The Jellyfin server the listener signed in to, as Settings shows it.
//!
//! One server at a time. Signing in, reading its songs and signing out all
//! wait on the network, so each is an effect carried out on a worker; the
//! answer comes back with the epoch it was asked under, and an answer to a
//! request the listener has since overtaken (a sign-out during a sync, a
//! second sign-in) is dropped.
//!
//! Every library of the server is synced; the listener chooses which of them
//! show their songs in Janis, and the state keeps that choice.

use std::collections::HashSet;

use crate::effects::{Effect, Secret};
use crate::jellyfin::Failure;
use crate::library::{ScanReport, Track};
use crate::persistence::{MediaServer, ServerLibrary};

/// The Settings card, as the state has it.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub signed_in: bool,
    pub busy: bool,
    /// The translation key of the line under the card.
    pub status_key: Option<&'static str>,
    pub status_failed: bool,
    pub summary: Option<Summary>,
    /// The address and user name the form starts from.
    pub prefill: Option<(String, String)>,
    /// The server's libraries that hold songs, in its order, each with its
    /// switch.
    pub libraries: Vec<LibraryRow>,
}

/// A library of the server, as the card lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryRow {
    /// Its row in `server_libraries`, which its switch names.
    pub library_id: i64,
    pub name: String,
    /// The songs listed in it, shown or not.
    pub song_count: usize,
    pub shown: bool,
}

/// The server signed in to, as the card sums it up.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub server_name: String,
    pub user_name: String,
    /// That server's songs in the library.
    pub song_count: usize,
}

/// Where the connection stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// No server: the form asks for one.
    #[default]
    SignedOut,
    /// The form's credentials are on their way.
    SigningIn,
    /// Signed in; the songs are in the library.
    SignedIn,
    /// Signed in, and reading the server's songs.
    Syncing,
    /// The server no longer accepts the kept token. Its songs stay in the
    /// library, and the form asks for the password again.
    SignInAgain,
}

#[derive(Debug, Default)]
pub struct MediaServerState {
    phase: Phase,
    server: Option<MediaServer>,
    /// The translation key of the line under the card: progress, or what
    /// went wrong last.
    message_key: Option<&'static str>,
    /// Tags the request in flight, so a late answer is recognised.
    epoch: u64,
    /// Where a sign-in started from, which is where a failed one returns.
    signing_in_from: Phase,
    /// The server's libraries, in its order.
    libraries: Vec<ServerLibrary>,
}

impl MediaServerState {
    /// Boot hydration: a server kept from a previous run is signed in, and
    /// its songs are read again in the background.
    pub fn restore(&mut self, server: Option<MediaServer>) -> Vec<Effect> {
        let Some(server) = server else {
            return Vec::new();
        };
        self.server = Some(server);
        self.phase = Phase::SignedIn;
        self.sync()
    }

    #[cfg(test)]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn server(&self) -> Option<&MediaServer> {
        self.server.as_ref()
    }

    /// Takes the libraries as `janis.db` lists them: at boot, and after every
    /// read of the library. A library already known keeps the switch the
    /// listener last set — that write may have landed after the read — and a
    /// new one comes as read, shown.
    pub fn set_libraries(&mut self, libraries: Vec<ServerLibrary>) {
        let known: Vec<(i64, bool)> = self
            .libraries
            .iter()
            .map(|library| (library.id, library.shown))
            .collect();
        self.libraries = libraries
            .into_iter()
            .map(|mut library| {
                if let Some((_, shown)) = known.iter().find(|(id, _)| *id == library.id) {
                    library.shown = *shown;
                }
                library
            })
            .collect();
    }

    /// The libraries whose songs the listener hid.
    pub fn hidden_libraries(&self) -> HashSet<i64> {
        self.libraries
            .iter()
            .filter(|library| !library.shown)
            .map(|library| library.id)
            .collect()
    }

    /// The listener flipped the switch of a library.
    pub fn toggle_library(&mut self, library_id: i64) -> Vec<Effect> {
        let Some(library) = self
            .libraries
            .iter_mut()
            .find(|library| library.id == library_id)
        else {
            return Vec::new();
        };
        library.shown = !library.shown;
        vec![Effect::PersistServerLibraryShown {
            library_id: library.id,
            shown: library.shown,
        }]
    }

    #[cfg(test)]
    pub fn message_key(&self) -> Option<&'static str> {
        self.message_key
    }

    /// What the Settings card shows, given the library's tracks.
    pub fn card(&self, tracks: &[Track]) -> Card {
        let summary = self.server.as_ref().map(|server| Summary {
            // A server that gave no name is known by its address.
            server_name: if server.server_name.is_empty() {
                server.address.clone()
            } else {
                server.server_name.clone()
            },
            user_name: server.user_name.clone(),
            song_count: tracks
                .iter()
                .filter(|track| track.server_id == Some(server.id))
                .count(),
        });
        Card {
            signed_in: matches!(self.phase, Phase::SignedIn | Phase::Syncing),
            busy: self.busy(),
            status_key: self.message_key,
            // A line that stays once nothing is in flight says what went wrong.
            status_failed: self.message_key.is_some() && !self.busy(),
            summary,
            // Asked to sign in again, the form starts from what was kept.
            prefill: self
                .server
                .as_ref()
                .filter(|_| self.phase == Phase::SignInAgain)
                .map(|server| (server.address.clone(), server.user_name.clone())),
            // A library listing no songs has nothing to show or hide; it is
            // kept, with its switch, for when its songs come back.
            libraries: self
                .libraries
                .iter()
                .map(|library| LibraryRow {
                    library_id: library.id,
                    name: library.name.clone(),
                    song_count: tracks
                        .iter()
                        .filter(|track| track.server_libraries.contains(&library.id))
                        .count(),
                    shown: library.shown,
                })
                .filter(|row| row.song_count > 0)
                .collect(),
        }
    }

    /// Whether a request is in flight, which holds the card's buttons.
    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::SigningIn | Phase::Syncing)
    }

    /// The listener pressed Sign in.
    pub fn sign_in(&mut self, address: &str, user_name: &str, password: &str) -> Vec<Effect> {
        if self.busy() {
            return Vec::new();
        }
        if address.trim().is_empty() {
            self.message_key = Some("jellyfin.status.invalidAddress");
            return Vec::new();
        }
        if user_name.trim().is_empty() {
            self.message_key = Some("jellyfin.status.userName");
            return Vec::new();
        }
        self.signing_in_from = self.phase;
        self.phase = Phase::SigningIn;
        self.message_key = Some("jellyfin.status.signingIn");
        self.epoch += 1;
        vec![Effect::SignInMediaServer {
            address: address.trim().to_owned(),
            user_name: user_name.trim().to_owned(),
            password: Secret::new(password.to_owned()),
            epoch: self.epoch,
        }]
    }

    /// The server answered the sign-in. On success, its songs are read.
    pub fn sign_in_finished(
        &mut self,
        epoch: u64,
        outcome: Result<MediaServer, Failure>,
    ) -> Vec<Effect> {
        if epoch != self.epoch {
            return Vec::new();
        }
        match outcome {
            Ok(server) => {
                // Another server or user replaces the one before, libraries
                // included; the read after the sync lists the new ones. The
                // row's id says nothing here: the replacing row may reuse it.
                let same_account = self.server.as_ref().is_some_and(|current| {
                    current.server_identifier == server.server_identifier
                        && current.user_identifier == server.user_identifier
                });
                if !same_account {
                    self.libraries.clear();
                }
                self.server = Some(server);
                self.phase = Phase::SignedIn;
                self.message_key = None;
                self.sync()
            }
            Err(failure) => {
                log::warn!("Jellyfin sign-in: {failure:?}");
                // A failed attempt to switch server or user leaves the current
                // one signed in: its token still works.
                self.phase = match (self.signing_in_from, &self.server) {
                    (Phase::SignedIn, Some(_)) => Phase::SignedIn,
                    (_, Some(_)) => Phase::SignInAgain,
                    (_, None) => Phase::SignedOut,
                };
                self.message_key = Some(failure_key(&failure));
                Vec::new()
            }
        }
    }

    /// Reads the server's songs again: at boot, after a sign-in, and when the
    /// listener asks. Only while signed in and idle.
    pub fn sync(&mut self) -> Vec<Effect> {
        if self.phase != Phase::SignedIn {
            return Vec::new();
        }
        let Some(server) = self.server.clone() else {
            return Vec::new();
        };
        self.phase = Phase::Syncing;
        self.message_key = Some("jellyfin.status.syncing");
        self.epoch += 1;
        vec![Effect::SyncMediaServer {
            server,
            epoch: self.epoch,
        }]
    }

    /// The songs were read, or could not be. `now` is the Unix time of the
    /// answer, shown as when the library last matched the server.
    pub fn sync_finished(
        &mut self,
        epoch: u64,
        outcome: Result<ScanReport, Failure>,
        now: i64,
    ) -> Vec<Effect> {
        if epoch != self.epoch {
            return Vec::new();
        }
        match outcome {
            Ok(_) => {
                self.phase = Phase::SignedIn;
                self.message_key = None;
                if let Some(server) = self.server.as_mut() {
                    server.last_sync_at = Some(now);
                }
                Vec::new()
            }
            Err(Failure::TokenRejected) => self.sign_in_again(),
            Err(failure) => {
                log::warn!("Jellyfin sync: {failure:?}");
                // The songs read before stay; the next sync may well work.
                self.phase = Phase::SignedIn;
                self.message_key = Some(failure_key(&failure));
                vec![notify("error.jellyfinSync")]
            }
        }
    }

    /// A call made with `server`'s token — a song's star — found the server
    /// no longer accepts it. The card asks for the password again, unless
    /// the listener already signed in again or out since the call left.
    pub fn token_rejected(&mut self, server: &MediaServer) -> Vec<Effect> {
        let current = self
            .server
            .as_ref()
            .is_some_and(|current| current.access_token == server.access_token);
        if !current || !matches!(self.phase, Phase::SignedIn | Phase::Syncing) {
            return Vec::new();
        }
        self.sign_in_again()
    }

    /// The server no longer accepts the kept token: its songs stay, and the
    /// card asks for the password again.
    fn sign_in_again(&mut self) -> Vec<Effect> {
        self.phase = Phase::SignInAgain;
        self.message_key = Some(failure_key(&Failure::TokenRejected));
        vec![notify("error.jellyfinSignInAgain")]
    }

    /// The listener pressed Sign out: the server is forgotten at once, and
    /// whatever was in flight for it is dropped when it answers.
    pub fn sign_out(&mut self) -> Vec<Effect> {
        let Some(server) = self.server.take() else {
            return Vec::new();
        };
        self.phase = Phase::SignedOut;
        self.message_key = None;
        self.libraries.clear();
        self.epoch += 1;
        vec![Effect::SignOutMediaServer { server }]
    }
}

/// What the card says about a failure.
fn failure_key(failure: &Failure) -> &'static str {
    match failure {
        Failure::InvalidAddress => "jellyfin.status.invalidAddress",
        Failure::Unreachable(_) => "jellyfin.status.unreachable",
        Failure::NotJellyfin(_) => "jellyfin.status.notJellyfin",
        Failure::CredentialsRejected => "jellyfin.status.credentialsRejected",
        Failure::TokenRejected => "jellyfin.status.tokenRejected",
        Failure::Other(_) => "jellyfin.status.failed",
    }
}

/// A toast for a failure the listener did not see happen: a sync runs in the
/// background, whatever screen is open. One at a time.
fn notify(key: &'static str) -> Effect {
    Effect::NotifyError {
        key,
        parameters: Vec::new(),
        dedupe_key: Some("jellyfin-sync"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> MediaServer {
        MediaServer {
            id: 1,
            address: "http://jellyfin.local:8096".to_owned(),
            server_identifier: "server".to_owned(),
            server_name: "Home".to_owned(),
            user_identifier: "user".to_owned(),
            user_name: "listener".to_owned(),
            access_token: "token".to_owned(),
            last_sync_at: None,
        }
    }

    fn signed_in() -> MediaServerState {
        let mut state = MediaServerState::default();
        let effects = state.restore(Some(server()));
        let epoch = sync_epoch(&effects);
        state.sync_finished(epoch, Ok(ScanReport::default()), 100);
        state
    }

    fn sync_epoch(effects: &[Effect]) -> u64 {
        match effects {
            [Effect::SyncMediaServer { epoch, .. }] => *epoch,
            other => panic!("expected one sync, got {other:?}"),
        }
    }

    fn sign_in_epoch(effects: &[Effect]) -> u64 {
        match effects {
            [Effect::SignInMediaServer { epoch, .. }] => *epoch,
            other => panic!("expected one sign-in, got {other:?}"),
        }
    }

    #[test]
    fn nothing_happens_at_boot_without_a_server() {
        let mut state = MediaServerState::default();
        assert!(state.restore(None).is_empty());
        assert_eq!(state.phase(), Phase::SignedOut);
    }

    #[test]
    fn a_kept_server_is_read_again_at_boot() {
        let mut state = MediaServerState::default();
        let effects = state.restore(Some(server()));
        assert_eq!(state.phase(), Phase::Syncing);
        assert!(matches!(
            effects.as_slice(),
            [Effect::SyncMediaServer { server: synced, .. }] if *synced == server()
        ));
    }

    #[test]
    fn signing_in_sends_the_credentials_then_reads_the_songs() {
        let mut state = MediaServerState::default();
        let effects = state.sign_in(" music.local ", "listener", "secret");
        assert_eq!(state.phase(), Phase::SigningIn);
        assert!(state.busy());
        assert!(matches!(
            effects.as_slice(),
            [Effect::SignInMediaServer { address, user_name, password, .. }]
                if address == "music.local" && user_name == "listener" && password.expose() == "secret"
        ));

        let effects = state.sign_in_finished(sign_in_epoch(&effects), Ok(server()));
        assert_eq!(state.phase(), Phase::Syncing);
        assert_eq!(state.server(), Some(&server()));
        sync_epoch(&effects);
    }

    #[test]
    fn a_sign_in_without_an_address_or_user_is_not_sent() {
        let mut state = MediaServerState::default();
        assert!(state.sign_in("  ", "listener", "secret").is_empty());
        assert_eq!(state.message_key(), Some("jellyfin.status.invalidAddress"));
        assert!(state.sign_in("host", "", "secret").is_empty());
        assert_eq!(state.message_key(), Some("jellyfin.status.userName"));
        assert_eq!(state.phase(), Phase::SignedOut);
    }

    #[test]
    fn a_refused_sign_in_says_why_and_keeps_the_form() {
        let mut state = MediaServerState::default();
        let effects = state.sign_in("host", "listener", "wrong");
        let effects =
            state.sign_in_finished(sign_in_epoch(&effects), Err(Failure::CredentialsRejected));
        assert!(effects.is_empty());
        assert_eq!(state.phase(), Phase::SignedOut);
        assert_eq!(
            state.message_key(),
            Some("jellyfin.status.credentialsRejected")
        );
    }

    #[test]
    fn a_second_press_while_signing_in_is_ignored() {
        let mut state = MediaServerState::default();
        state.sign_in("host", "listener", "secret");
        assert!(state.sign_in("host", "listener", "secret").is_empty());
    }

    #[test]
    fn a_finished_sync_records_when_it_ran() {
        let state = signed_in();
        assert_eq!(state.phase(), Phase::SignedIn);
        assert_eq!(state.message_key(), None);
        assert_eq!(
            state.server().and_then(|server| server.last_sync_at),
            Some(100)
        );
    }

    #[test]
    fn an_unreachable_server_keeps_its_songs_and_says_so() {
        let mut state = signed_in();
        let epoch = sync_epoch(&state.sync());
        let effects = state.sync_finished(epoch, Err(Failure::Unreachable("refused".into())), 200);
        assert_eq!(state.phase(), Phase::SignedIn, "the next sync may work");
        assert_eq!(state.message_key(), Some("jellyfin.status.unreachable"));
        assert_eq!(
            state.server().and_then(|server| server.last_sync_at),
            Some(100)
        );
        assert!(matches!(
            effects.as_slice(),
            [Effect::NotifyError {
                key: "error.jellyfinSync",
                ..
            }]
        ));
    }

    #[test]
    fn a_revoked_token_asks_for_the_password_again_and_keeps_the_server() {
        let mut state = signed_in();
        let epoch = sync_epoch(&state.sync());
        let effects = state.sync_finished(epoch, Err(Failure::TokenRejected), 200);
        assert_eq!(state.phase(), Phase::SignInAgain);
        assert!(state.server().is_some());
        assert!(matches!(
            effects.as_slice(),
            [Effect::NotifyError {
                key: "error.jellyfinSignInAgain",
                ..
            }]
        ));
        // Nothing to sync with a refused token.
        assert!(state.sync().is_empty());
        // A new sign-in that fails leaves the server known.
        let effects = state.sign_in("host", "listener", "wrong");
        state.sign_in_finished(sign_in_epoch(&effects), Err(Failure::CredentialsRejected));
        assert_eq!(state.phase(), Phase::SignInAgain);
    }

    #[test]
    fn a_sync_answer_after_signing_out_is_dropped() {
        let mut state = signed_in();
        let epoch = sync_epoch(&state.sync());
        let effects = state.sign_out();
        assert!(matches!(
            effects.as_slice(),
            [Effect::SignOutMediaServer { server: forgotten }] if forgotten.id == 1
        ));
        assert_eq!(state.phase(), Phase::SignedOut);

        assert!(state
            .sync_finished(epoch, Ok(ScanReport::default()), 300)
            .is_empty());
        assert_eq!(state.phase(), Phase::SignedOut);
        assert_eq!(state.server(), None);
    }

    #[test]
    fn signing_out_without_a_server_does_nothing() {
        let mut state = MediaServerState::default();
        assert!(state.sign_out().is_empty());
    }

    #[test]
    fn a_password_never_shows_in_a_log() {
        let mut state = MediaServerState::default();
        let effects = state.sign_in("host", "listener", "hunter2");
        assert!(!format!("{effects:?}").contains("hunter2"));
    }

    #[test]
    fn a_failed_sign_in_while_signed_in_keeps_the_current_server() {
        let mut state = signed_in();
        let current = state.server().cloned();
        let epoch = sign_in_epoch(&state.sign_in("http://other.local", "someone", "wrong"));

        assert!(state
            .sign_in_finished(epoch, Err(Failure::CredentialsRejected))
            .is_empty());

        assert_eq!(state.phase(), Phase::SignedIn);
        assert_eq!(state.server().cloned(), current);
        assert_eq!(
            state.message_key(),
            Some(failure_key(&Failure::CredentialsRejected))
        );
    }

    #[test]
    fn signing_in_again_as_another_user_replaces_the_server_and_reads_its_songs() {
        let mut state = signed_in();
        let epoch = sign_in_epoch(&state.sign_in("http://jellyfin.local:8096", "other", "right"));
        let other = MediaServer {
            id: 2,
            user_identifier: "other".to_owned(),
            user_name: "other".to_owned(),
            access_token: "other token".to_owned(),
            ..server()
        };

        let effects = state.sign_in_finished(epoch, Ok(other.clone()));

        assert_eq!(state.server(), Some(&other));
        assert_eq!(state.phase(), Phase::Syncing);
        assert_eq!(
            effects,
            [Effect::SyncMediaServer {
                server: other,
                epoch: epoch + 1,
            }]
        );
    }

    #[test]
    fn the_card_sums_up_the_server_by_name_or_address_with_its_own_songs() {
        let state = signed_in();
        let songs = [
            crate::library::test_support::track(1, "a").server(1, "item"),
            crate::library::test_support::track(2, "b").server(9, "item"),
            crate::library::test_support::track(3, "c"),
        ];
        let card = state.card(&songs);
        assert!(card.signed_in && !card.busy && !card.status_failed);
        assert_eq!(
            card.summary,
            Some(Summary {
                server_name: "Home".to_owned(),
                user_name: "listener".to_owned(),
                song_count: 1,
            })
        );
        assert_eq!(card.prefill, None);

        let mut nameless = MediaServerState::default();
        nameless.restore(Some(MediaServer {
            server_name: String::new(),
            ..server()
        }));
        assert_eq!(
            nameless
                .card(&[])
                .summary
                .map(|summary| summary.server_name),
            Some("http://jellyfin.local:8096".to_owned())
        );
        assert!(nameless.card(&[]).busy, "a sync runs at boot");
        assert!(!nameless.card(&[]).status_failed, "its line is progress");
    }

    #[test]
    fn a_refused_token_shows_a_failed_line_and_the_form_starts_from_what_was_kept() {
        let mut state = signed_in();
        let epoch = sync_epoch(&state.sync());
        state.sync_finished(epoch, Err(Failure::TokenRejected), 200);

        let card = state.card(&[]);

        assert!(!card.signed_in);
        assert!(card.status_failed);
        assert_eq!(
            card.prefill,
            Some((
                "http://jellyfin.local:8096".to_owned(),
                "listener".to_owned()
            ))
        );
    }

    fn library(id: i64, name: &str, shown: bool) -> ServerLibrary {
        ServerLibrary {
            id,
            name: name.to_owned(),
            shown,
        }
    }

    #[test]
    fn the_card_lists_each_library_with_its_songs_and_its_switch() {
        let mut state = signed_in();
        state.set_libraries(vec![
            library(10, "Music", true),
            library(11, "Kids", false),
            library(12, "Films", true),
        ]);
        let songs = [
            crate::library::test_support::track(1, "a")
                .server(1, "a")
                .in_libraries(&[10]),
            crate::library::test_support::track(2, "b")
                .server(1, "b")
                .in_libraries(&[10, 11]),
            crate::library::test_support::track(3, "c"),
        ];
        assert_eq!(
            state.card(&songs).libraries,
            [
                LibraryRow {
                    library_id: 10,
                    name: "Music".to_owned(),
                    song_count: 2,
                    shown: true,
                },
                LibraryRow {
                    library_id: 11,
                    name: "Kids".to_owned(),
                    song_count: 1,
                    shown: false,
                },
            ],
            "a library without songs is not listed"
        );
        assert_eq!(state.hidden_libraries(), HashSet::from([11]));
    }

    #[test]
    fn a_switch_flips_its_library_and_is_saved() {
        let mut state = signed_in();
        state.set_libraries(vec![library(10, "Music", true), library(11, "Kids", true)]);

        assert_eq!(
            state.toggle_library(11),
            [Effect::PersistServerLibraryShown {
                library_id: 11,
                shown: false,
            }]
        );
        assert_eq!(state.hidden_libraries(), HashSet::from([11]));
        assert_eq!(
            state.toggle_library(11),
            [Effect::PersistServerLibraryShown {
                library_id: 11,
                shown: true,
            }]
        );
        assert!(state.hidden_libraries().is_empty());
        assert!(state.toggle_library(99).is_empty(), "no such library");
    }

    #[test]
    fn a_read_from_before_a_switch_does_not_undo_it() {
        let mut state = signed_in();
        state.set_libraries(vec![library(10, "Music", true)]);
        state.toggle_library(10);

        // A sync read the libraries before the switch was saved, and found a
        // new one.
        state.set_libraries(vec![library(10, "Music", true), library(12, "Talk", true)]);

        assert_eq!(state.hidden_libraries(), HashSet::from([10]));
        let songs = [crate::library::test_support::track(1, "a")
            .server(1, "a")
            .in_libraries(&[10, 12])];
        assert_eq!(state.card(&songs).libraries.len(), 2);
    }

    #[test]
    fn signing_out_or_in_elsewhere_forgets_the_libraries() {
        let mut state = signed_in();
        state.set_libraries(vec![library(10, "Music", false)]);
        state.sign_out();
        assert!(state.card(&[]).libraries.is_empty());
        assert!(state.hidden_libraries().is_empty());

        // The replacing row reuses the id of the one it replaces.
        let mut state = signed_in();
        state.set_libraries(vec![library(10, "Music", false)]);
        let epoch = sign_in_epoch(&state.sign_in("http://jellyfin.local:8096", "someone", "x"));
        let someone = MediaServer {
            user_identifier: "someone".to_owned(),
            user_name: "someone".to_owned(),
            ..server()
        };
        state.sign_in_finished(epoch, Ok(someone));
        assert!(state.hidden_libraries().is_empty(), "another user");

        let mut state = signed_in();
        state.set_libraries(vec![library(10, "Music", false)]);
        let epoch = sign_in_epoch(&state.sign_in("http://other.local", "listener", "x"));
        let elsewhere = MediaServer {
            server_identifier: "other".to_owned(),
            ..server()
        };
        state.sign_in_finished(epoch, Ok(elsewhere));
        assert!(state.hidden_libraries().is_empty(), "another server");

        let mut state = signed_in();
        state.set_libraries(vec![library(10, "Music", false)]);
        let epoch = sign_in_epoch(&state.sign_in("http://jellyfin.local:8096", "listener", "x"));
        state.sign_in_finished(epoch, Ok(server()));
        assert_eq!(
            state.hidden_libraries(),
            HashSet::from([10]),
            "signing in again as the same user keeps them"
        );
    }

    #[test]
    fn signed_out_the_card_has_no_summary() {
        let card = MediaServerState::default().card(&[]);
        assert_eq!(card.summary, None);
        assert!(!card.signed_in);
        assert_eq!(card.status_key, None);
    }

    #[test]
    fn a_sign_in_answer_after_signing_out_is_dropped() {
        let mut state = signed_in();
        let epoch = sign_in_epoch(&state.sign_in("http://other.local", "someone", "pass"));
        state.sign_out();

        assert!(state.sign_in_finished(epoch, Ok(server())).is_empty());
        assert_eq!(state.server(), None);
        assert_eq!(state.phase(), Phase::SignedOut);
    }

    #[test]
    fn sync_now_is_ignored_while_a_sync_runs_or_when_signed_out() {
        assert!(MediaServerState::default().sync().is_empty());
        let mut state = signed_in();
        sync_epoch(&state.sync());
        assert!(state.busy());
        assert!(state.sync().is_empty(), "Sync now pressed twice");
        assert!(
            state
                .sign_in("http://other.local", "someone", "pass")
                .is_empty(),
            "a sign-in waits for the sync"
        );
    }

    #[test]
    fn a_failed_sync_raises_one_toast_that_the_next_failure_joins() {
        let mut state = signed_in();
        let epoch = sync_epoch(&state.sync());
        assert_eq!(
            state.sync_finished(epoch, Err(Failure::Unreachable("down".to_owned())), 200),
            [Effect::NotifyError {
                key: "error.jellyfinSync",
                parameters: Vec::new(),
                dedupe_key: Some("jellyfin-sync"),
            }]
        );
        assert_eq!(state.phase(), Phase::SignedIn, "the songs read before stay");
        assert_eq!(state.message_key(), Some("jellyfin.status.unreachable"));
    }

    #[test]
    fn a_successful_sign_in_clears_the_status_line() {
        let mut state = MediaServerState::default();
        let epoch = sign_in_epoch(&state.sign_in("http://jellyfin.local:8096", "listener", "x"));
        assert_eq!(state.message_key(), Some("jellyfin.status.signingIn"));
        let epoch = sync_epoch(&state.sign_in_finished(epoch, Ok(server())));
        assert_eq!(state.message_key(), Some("jellyfin.status.syncing"));
        state.sync_finished(epoch, Ok(ScanReport::default()), 300);
        assert_eq!(state.message_key(), None);
    }

    #[test]
    fn every_failure_names_a_status_line_both_languages_hold() {
        use crate::translations::{Language, Translations};
        for failure in [
            Failure::InvalidAddress,
            Failure::Unreachable(String::new()),
            Failure::NotJellyfin(String::new()),
            Failure::CredentialsRejected,
            Failure::TokenRejected,
            Failure::Other(String::new()),
        ] {
            let key = failure_key(&failure);
            for language in Language::ALL {
                assert_ne!(Translations::new(language).text(key), key, "{failure:?}");
            }
        }
    }

    #[test]
    fn a_star_refused_for_its_token_asks_for_the_password_again() {
        let mut state = signed_in();
        let effects = state.token_rejected(&server());
        assert_eq!(state.phase(), Phase::SignInAgain);
        assert_eq!(state.message_key(), Some("jellyfin.status.tokenRejected"));
        assert!(matches!(
            effects.as_slice(),
            [Effect::NotifyError {
                key: "error.jellyfinSignInAgain",
                ..
            }]
        ));
        assert!(state.token_rejected(&server()).is_empty(), "asked once");
    }

    #[test]
    fn a_refusal_for_a_token_already_replaced_or_while_signing_in_changes_nothing() {
        let mut state = signed_in();
        let mut old = server();
        old.access_token = "old token".to_owned();
        assert!(state.token_rejected(&old).is_empty());
        assert_eq!(state.phase(), Phase::SignedIn);

        state.sign_in("http://jellyfin.local:8096", "listener", "right");
        assert!(state.token_rejected(&server()).is_empty());
        assert_eq!(state.phase(), Phase::SigningIn);

        let mut signed_out = MediaServerState::default();
        assert!(signed_out.token_rejected(&server()).is_empty());
    }
}
