//! The songs the listener starred.
//!
//! A song is a favourite or it is not, by its row in the library; none is
//! until the listener says so. A file's star lives in Janis alone and is
//! saved at once. A song of a media server is starred on the server: the
//! server is where its star lives, every sync brings the server's stars back
//! into `janis.db`, and a star given in Janis shows at once but is saved only
//! once the server has it. Should the server refuse it, or not answer, the
//! song shows the star the server has, in its old place, and the listener is
//! told.
//!
//! Pushes run one after another, in the order given, and their answers come
//! back in that order. While a song's pushes are on their way the state
//! follows the star the server holds: what it had before the first, then
//! what each push it took left it with. The answer to the song's last push
//! settles it. Meanwhile a fresh read of the library, which may have read the
//! server before a push reached it, takes the star the listener gave over
//! what it read.

use std::collections::{HashMap, HashSet};

use crate::effects::Effect;
use crate::jellyfin::Failure;
use crate::library::Track;
use crate::persistence::MediaServer;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FavouriteTracks {
    /// The favourites, the newest first.
    track_ids: Vec<i64>,
    members: HashSet<i64>,
    /// The songs whose stars are on their way to a server.
    pending: HashMap<i64, Pending>,
    /// Numbers the pushes, so the answer to the last one of a song is known.
    sequence: u64,
}

/// A song whose star is on its way to its server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pending {
    /// The star the listener gave last, which the song shows.
    wanted: bool,
    /// The star the server holds, as far as its answers tell.
    server_favourite: bool,
    /// Where the song sat among the favourites before the first push, so a
    /// refused removal puts it back there.
    place: Option<usize>,
    /// The song's last push.
    last_sequence: u64,
}

impl FavouriteTracks {
    /// Takes the favourites as `janis.db` lists them, the newest first: at
    /// boot, and after every read of the library. A star still on its way to
    /// a server wins over what was read.
    pub fn restore(&mut self, track_ids: Vec<i64>) {
        self.track_ids = track_ids;
        self.members = self.track_ids.iter().copied().collect();
        let pending: Vec<(i64, bool)> = self
            .pending
            .iter()
            .map(|(track_id, pending)| (*track_id, pending.wanted))
            .collect();
        for (track_id, favourite) in pending {
            self.set(track_id, favourite, 0);
        }
    }

    pub fn contains(&self, track_id: i64) -> bool {
        self.members.contains(&track_id)
    }

    /// The favourites, the newest first, for the Favourites tab.
    pub fn track_ids(&self) -> &[i64] {
        &self.track_ids
    }

    /// Stars a song, or takes its star away. A file is saved at once; a song
    /// of the signed-in server is sent to it, and saved once it answers.
    pub fn toggle(&mut self, track: &Track, server: Option<&MediaServer>) -> Vec<Effect> {
        let was_favourite = self.contains(track.id);
        let favourite = !was_favourite;
        let place = self.place_of(track.id);
        self.set(track.id, favourite, 0);
        let Some(server) = server.filter(|server| track.server_id == Some(server.id)) else {
            return vec![Effect::PersistFavouriteTrack {
                track_id: track.id,
                favourite,
            }];
        };
        self.sequence += 1;
        let pending = self.pending.entry(track.id).or_insert(Pending {
            wanted: favourite,
            server_favourite: was_favourite,
            place,
            last_sequence: self.sequence,
        });
        pending.wanted = favourite;
        pending.last_sequence = self.sequence;
        vec![Effect::PushServerFavourite {
            server: server.clone(),
            track_id: track.id,
            address: track.path.clone(),
            favourite,
            sequence: self.sequence,
        }]
    }

    /// The server answered the push numbered `sequence`, which carried
    /// `favourite`. Once the song's last push has answered, the star the
    /// server holds is saved; when it is not the one the listener gave last,
    /// the song shows it again and the listener is told — but not of a
    /// refused token, which signing in again answers.
    pub fn push_finished(
        &mut self,
        track_id: i64,
        sequence: u64,
        favourite: bool,
        outcome: Result<(), Failure>,
    ) -> Vec<Effect> {
        let Some(pending) = self.pending.get_mut(&track_id) else {
            return Vec::new();
        };
        match &outcome {
            Ok(()) => pending.server_favourite = favourite,
            Err(failure) => log::warn!("star a song on the server: {failure:?}"),
        }
        if sequence != pending.last_sequence {
            return Vec::new();
        }
        let pending = *pending;
        self.pending.remove(&track_id);
        let mut effects = vec![Effect::PersistFavouriteTrack {
            track_id,
            favourite: pending.server_favourite,
        }];
        if pending.server_favourite == pending.wanted {
            return effects;
        }
        self.set(
            track_id,
            pending.server_favourite,
            pending.place.unwrap_or_default(),
        );
        if !matches!(outcome, Err(Failure::TokenRejected)) {
            effects.push(Effect::NotifyError {
                key: if pending.wanted {
                    "error.favouriteAdd"
                } else {
                    "error.favouriteRemove"
                },
                parameters: Vec::new(),
                dedupe_key: Some("jellyfin-favourite"),
            });
        }
        effects
    }

    fn place_of(&self, track_id: i64) -> Option<usize> {
        self.track_ids.iter().position(|kept| *kept == track_id)
    }

    /// Puts a song among the favourites at `place` (the newest is 0), or
    /// takes it out.
    fn set(&mut self, track_id: i64, favourite: bool, place: usize) {
        if favourite == self.contains(track_id) {
            return;
        }
        if favourite {
            self.members.insert(track_id);
            self.track_ids
                .insert(place.min(self.track_ids.len()), track_id);
        } else {
            self.members.remove(&track_id);
            self.track_ids.retain(|kept| *kept != track_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_support::track;

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

    fn server_song(id: i64) -> Track {
        let mut song = track(id, "Song").server(1, &format!("item{id}"));
        song.path = format!("jellyfin://server/item{id}");
        song
    }

    /// Toggles a server song and returns the push it sent: its sequence and
    /// the star it carries.
    fn push(favourites: &mut FavouriteTracks, id: i64) -> (u64, bool) {
        match favourites
            .toggle(&server_song(id), Some(&server()))
            .as_slice()
        {
            [Effect::PushServerFavourite {
                sequence,
                favourite,
                ..
            }] => (*sequence, *favourite),
            other => panic!("expected one push, got {other:?}"),
        }
    }

    fn saved(track_id: i64, favourite: bool) -> Effect {
        Effect::PersistFavouriteTrack {
            track_id,
            favourite,
        }
    }

    fn unreachable() -> Result<(), Failure> {
        Err(Failure::Unreachable("down".to_owned()))
    }

    #[test]
    fn no_song_is_a_favourite_until_the_listener_stars_it() {
        let favourites = FavouriteTracks::default();
        assert!(!favourites.contains(1));
        assert!(favourites.track_ids().is_empty());
    }

    #[test]
    fn starring_a_file_saves_it_and_puts_it_first_and_starring_again_takes_it_away() {
        let mut favourites = FavouriteTracks::default();
        favourites.restore(vec![2]);

        let effects = favourites.toggle(&track(1, "One"), Some(&server()));
        assert!(favourites.contains(1));
        assert_eq!(favourites.track_ids(), [1, 2]);
        assert_eq!(effects, [saved(1, true)], "a file is starred in Janis only");

        let effects = favourites.toggle(&track(1, "One"), None);
        assert!(!favourites.contains(1));
        assert_eq!(favourites.track_ids(), [2]);
        assert_eq!(effects, [saved(1, false)]);
    }

    #[test]
    fn starring_a_server_song_shows_at_once_and_sends_it_to_its_server_before_saving_it() {
        let mut favourites = FavouriteTracks::default();
        let effects = favourites.toggle(&server_song(5), Some(&server()));
        assert!(favourites.contains(5));
        assert_eq!(
            effects,
            [Effect::PushServerFavourite {
                server: server(),
                track_id: 5,
                address: "jellyfin://server/item5".to_owned(),
                favourite: true,
                sequence: 1,
            }]
        );
    }

    #[test]
    fn a_song_of_another_server_is_starred_in_janis_only() {
        let mut favourites = FavouriteTracks::default();
        let mut elsewhere = server();
        elsewhere.id = 2;
        let effects = favourites.toggle(&server_song(5), Some(&elsewhere));
        assert_eq!(effects, [saved(5, true)]);
        assert!(favourites.contains(5));
    }

    #[test]
    fn a_star_the_server_took_is_saved() {
        let mut favourites = FavouriteTracks::default();
        let (sequence, favourite) = push(&mut favourites, 5);
        let effects = favourites.push_finished(5, sequence, favourite, Ok(()));
        assert!(favourites.contains(5));
        assert_eq!(effects, [saved(5, true)]);
    }

    #[test]
    fn a_star_the_server_refused_is_taken_back_and_the_listener_told() {
        let mut favourites = FavouriteTracks::default();
        let (sequence, favourite) = push(&mut favourites, 5);
        let effects = favourites.push_finished(5, sequence, favourite, unreachable());
        assert!(!favourites.contains(5));
        assert_eq!(
            effects,
            [
                saved(5, false),
                Effect::NotifyError {
                    key: "error.favouriteAdd",
                    parameters: Vec::new(),
                    dedupe_key: Some("jellyfin-favourite"),
                },
            ]
        );
    }

    #[test]
    fn a_removal_the_server_refused_puts_the_star_back_in_its_old_place() {
        let mut favourites = FavouriteTracks::default();
        favourites.restore(vec![7, 5, 3]);
        let (sequence, favourite) = push(&mut favourites, 5);
        assert_eq!(favourites.track_ids(), [7, 3]);

        let effects = favourites.push_finished(5, sequence, favourite, unreachable());
        assert_eq!(favourites.track_ids(), [7, 5, 3]);
        assert!(matches!(
            effects.as_slice(),
            [
                Effect::PersistFavouriteTrack {
                    favourite: true,
                    ..
                },
                Effect::NotifyError {
                    key: "error.favouriteRemove",
                    ..
                },
            ]
        ));
    }

    #[test]
    fn a_refused_token_takes_the_star_back_without_a_toast_of_its_own() {
        let mut favourites = FavouriteTracks::default();
        let (sequence, favourite) = push(&mut favourites, 5);
        let effects = favourites.push_finished(5, sequence, favourite, Err(Failure::TokenRejected));
        assert!(!favourites.contains(5));
        assert_eq!(effects, [saved(5, false)]);
    }

    #[test]
    fn only_the_answer_to_a_songs_last_push_settles_it() {
        let mut favourites = FavouriteTracks::default();
        let first = push(&mut favourites, 5);
        let second = push(&mut favourites, 5);
        assert!(!favourites.contains(5));

        assert!(favourites
            .push_finished(5, first.0, first.1, Ok(()))
            .is_empty());
        assert!(
            !favourites.contains(5),
            "the song shows the last star given"
        );

        // The server took the star, then could not take it away: it still
        // has it, and so does Janis.
        let effects = favourites.push_finished(5, second.0, second.1, unreachable());
        assert!(favourites.contains(5));
        assert_eq!(effects[0], saved(5, true));
    }

    #[test]
    fn two_refused_pushes_leave_the_star_the_server_never_lost() {
        // Not starred on the server, which is down. The listener stars the
        // song, then changes their mind: both pushes fail, and the server
        // still has what the listener asked for last.
        let mut favourites = FavouriteTracks::default();
        let first = push(&mut favourites, 5);
        let second = push(&mut favourites, 5);

        assert!(favourites
            .push_finished(5, first.0, first.1, unreachable())
            .is_empty());
        let effects = favourites.push_finished(5, second.0, second.1, unreachable());
        assert!(!favourites.contains(5));
        assert_eq!(
            effects,
            [saved(5, false)],
            "nothing to take back, nothing to tell"
        );
    }

    #[test]
    fn a_library_read_before_a_push_answers_keeps_the_star_given() {
        let mut favourites = FavouriteTracks::default();
        favourites.restore(vec![7]);
        let (sequence, favourite) = push(&mut favourites, 5);

        // A sync read the server before the push reached it.
        favourites.restore(vec![7]);
        assert_eq!(favourites.track_ids(), [5, 7]);

        favourites.push_finished(5, sequence, favourite, Ok(()));
        favourites.restore(vec![5, 7]);
        assert_eq!(favourites.track_ids(), [5, 7]);
    }

    #[test]
    fn an_answer_for_a_song_with_nothing_on_its_way_is_dropped() {
        let mut favourites = FavouriteTracks::default();
        assert!(favourites.push_finished(5, 1, true, Ok(())).is_empty());
        assert!(!favourites.contains(5));
    }

    #[test]
    fn restoring_replaces_the_stars_already_held() {
        let mut favourites = FavouriteTracks::default();
        favourites.toggle(&track(1, "One"), None);
        favourites.restore(vec![3, 2]);
        assert!(!favourites.contains(1));
        assert_eq!(favourites.track_ids(), [3, 2]);
    }
}
