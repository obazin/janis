//! The Jellyfin server the listener signed in to, as Settings shows it.
//!
//! One server at a time. Signing in, reading its songs and signing out all
//! wait on the network, so each is an effect carried out on a worker; the
//! answer comes back with the epoch it was asked under, and an answer to a
//! request the listener has since overtaken (a sign-out during a sync, a
//! second sign-in) is dropped.

use crate::effects::{Effect, Secret};
use crate::jellyfin::Failure;
use crate::library::{ScanReport, Track};
use crate::persistence::MediaServer;

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
            Err(Failure::TokenRejected) => {
                self.phase = Phase::SignInAgain;
                self.message_key = Some(failure_key(&Failure::TokenRejected));
                vec![notify("error.jellyfinSignInAgain")]
            }
            Err(failure) => {
                log::warn!("Jellyfin sync: {failure:?}");
                // The songs read before stay; the next sync may well work.
                self.phase = Phase::SignedIn;
                self.message_key = Some(failure_key(&failure));
                vec![notify("error.jellyfinSync")]
            }
        }
    }

    /// The listener pressed Sign out: the server is forgotten at once, and
    /// whatever was in flight for it is dropped when it answers.
    pub fn sign_out(&mut self) -> Vec<Effect> {
        let Some(server) = self.server.take() else {
            return Vec::new();
        };
        self.phase = Phase::SignedOut;
        self.message_key = None;
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
        let effects = state.sign_in(" jellyfin.local ", "listener", "secret");
        assert_eq!(state.phase(), Phase::SigningIn);
        assert!(state.busy());
        assert!(matches!(
            effects.as_slice(),
            [Effect::SignInMediaServer { address, user_name, password, .. }]
                if address == "jellyfin.local" && user_name == "listener" && password.expose() == "secret"
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

    #[test]
    fn signed_out_the_card_has_no_summary() {
        let card = MediaServerState::default().card(&[]);
        assert_eq!(card.summary, None);
        assert!(!card.signed_in);
        assert_eq!(card.status_key, None);
    }
}
