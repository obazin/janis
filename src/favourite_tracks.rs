//! The songs the listener starred.
//!
//! A song is a favourite or it is not, by its row in the library; none is
//! until the listener says so. A file's star lives in Janis alone and is
//! saved at once. Jellyfin favourites and YouTube Music likes own their
//! songs' stars: every sync brings those stars back into `janis.db`, and a
//! star given in Janis shows at once but is saved only once its provider has
//! it. Should the provider refuse it, or not answer, the song shows the star
//! the provider has, in its old place, and the listener is
//! told.
//!
//! Each provider's pushes run one after another, in the order given, and answers come
//! back in that order. While a song's pushes are on their way the state
//! follows the star the server holds: what it had before the first, then
//! what each push it took left it with. The answer to the song's last push
//! settles it. Meanwhile a fresh read of the library, which may have read the
//! server before a push reached it, takes the star the listener gave over
//! what it read.

use std::collections::HashSet;

use crate::effects::Effect;
use crate::jellyfin::Failure;
use crate::library::{Track, TrackSource};
use crate::persistence::MediaServer;
use crate::server_pushes::{PendingPushes, Settled};
use crate::youtube_music;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FavouriteTracks {
    /// The favourites, the newest first.
    track_ids: Vec<i64>,
    members: HashSet<i64>,
    /// The songs whose stars are on their way to a server, by track id.
    pending: PendingPushes<i64, bool>,
    youtube_pending: PendingPushes<i64, bool>,
}

impl FavouriteTracks {
    /// Takes the favourites as `janis.db` lists them, the newest first: at
    /// boot, and after every read of the library. A star still on its way to
    /// a server wins over what was read.
    pub fn restore(&mut self, track_ids: Vec<i64>) {
        let previous = std::mem::replace(&mut self.track_ids, track_ids);
        self.members.clear();
        self.members.extend(self.track_ids.iter().copied());
        if self.pending.wanted().next().is_none() && self.youtube_pending.wanted().next().is_none()
        {
            return;
        }
        for track_id in previous.into_iter().rev() {
            let wanted = self
                .pending
                .wanted_for(&track_id)
                .or_else(|| self.youtube_pending.wanted_for(&track_id))
                .copied();
            if wanted == Some(true) {
                self.set(track_id, false, 0);
                self.set(track_id, true, 0);
            }
        }
        self.members.retain(|track_id| {
            self.pending.wanted_for(track_id) != Some(&false)
                && self.youtube_pending.wanted_for(track_id) != Some(&false)
        });
        self.track_ids
            .retain(|track_id| self.members.contains(track_id));
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
        if track.id <= 0 {
            return Vec::new();
        }
        let was_favourite = self.contains(track.id);
        let favourite = !was_favourite;
        let place = self.place_of(track.id);
        self.set(track.id, favourite, 0);
        if track.source == TrackSource::YouTubeMusic {
            let sequence = self
                .youtube_pending
                .push(track.id, favourite, was_favourite, place);
            return vec![Effect::PushYouTubeMusicFavourite {
                track_id: track.id,
                address: track.path.clone(),
                favourite,
                sequence,
            }];
        }
        let Some(server) = server.filter(|server| track.server_id == Some(server.id)) else {
            return vec![Effect::PersistFavouriteTrack {
                track_id: track.id,
                favourite,
            }];
        };
        let sequence = self.pending.push(track.id, favourite, was_favourite, place);
        vec![Effect::PushServerFavourite {
            server: server.clone(),
            track_id: track.id,
            address: track.path.clone(),
            favourite,
            sequence,
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
        if let Err(failure) = &outcome {
            log::warn!("star a song on the server: {failure:?}");
        }
        let Some(settled) = self
            .pending
            .answer(&track_id, sequence, favourite, outcome.is_ok())
        else {
            return Vec::new();
        };
        let persist = Effect::PersistFavouriteTrack {
            track_id,
            favourite: settled.server_value,
        };
        self.settle(
            track_id,
            settled,
            matches!(outcome, Err(Failure::TokenRejected)),
            "jellyfin-favourite",
            persist,
        )
    }

    pub fn youtube_push_finished(
        &mut self,
        track_id: i64,
        address: String,
        sequence: u64,
        favourite: bool,
        outcome: Result<(), youtube_music::Failure>,
    ) -> Vec<Effect> {
        let Some(settled) =
            self.youtube_pending
                .answer(&track_id, sequence, favourite, outcome.is_ok())
        else {
            return Vec::new();
        };
        let persist = Effect::PersistYouTubeMusicFavourite {
            track_id,
            address,
            favourite: settled.server_value,
        };
        self.settle(
            track_id,
            settled,
            outcome.is_err_and(|failure| failure.credentials_rejected()),
            "youtube-music-favourite",
            persist,
        )
    }

    /// A changed connection cannot settle the old account's intentions.
    pub fn cancel_youtube_pushes(&mut self) -> Vec<i64> {
        self.youtube_pending
            .take_settled()
            .map(|(track_id, settled)| {
                self.set(
                    track_id,
                    settled.server_value,
                    settled.place.unwrap_or_default(),
                );
                track_id
            })
            .collect()
    }

    fn settle(
        &mut self,
        track_id: i64,
        settled: Settled<bool>,
        credentials_rejected: bool,
        dedupe_key: &'static str,
        persist: Effect,
    ) -> Vec<Effect> {
        let mut effects = vec![persist];
        if settled.as_wanted() {
            return effects;
        }
        self.set(
            track_id,
            settled.server_value,
            settled.place.unwrap_or_default(),
        );
        if !credentials_rejected {
            effects.push(Effect::NotifyError {
                key: if settled.wanted {
                    "error.favouriteAdd"
                } else {
                    "error.favouriteRemove"
                },
                parameters: Vec::new(),
                dedupe_key: Some(dedupe_key),
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

    fn youtube_song(track_id: i64) -> Track {
        let mut song = track(track_id, "Song");
        song.source = TrackSource::YouTubeMusic;
        song.path = format!("youtube-music://video{track_id:06}");
        song
    }

    fn youtube_push(favourites: &mut FavouriteTracks, track_id: i64) -> (u64, bool) {
        match favourites.toggle(&youtube_song(track_id), None).as_slice() {
            [Effect::PushYouTubeMusicFavourite {
                sequence,
                favourite,
                ..
            }] => (*sequence, *favourite),
            effects => panic!("expected an account like, got {effects:?}"),
        }
    }

    #[test]
    fn youtube_like_waits_for_acknowledgment_and_a_failed_unlike_restores_the_star() {
        let mut favourites = FavouriteTracks::default();
        favourites.restore(vec![9, 5, 2]);
        let (sequence, favourite) = youtube_push(&mut favourites, 5);
        assert!(!favourite);
        assert_eq!(favourites.track_ids(), [9, 2]);
        let effects = favourites.youtube_push_finished(
            5,
            youtube_song(5).path,
            sequence,
            false,
            Err(youtube_music::Failure::Network),
        );
        assert_eq!(favourites.track_ids(), [9, 5, 2]);
        assert_eq!(
            effects,
            [
                Effect::PersistYouTubeMusicFavourite {
                    track_id: 5,
                    address: youtube_song(5).path,
                    favourite: true
                },
                Effect::NotifyError {
                    key: "error.favouriteRemove",
                    parameters: Vec::new(),
                    dedupe_key: Some("youtube-music-favourite")
                },
            ]
        );
    }

    #[test]
    fn pending_youtube_likes_keep_click_order_over_a_stale_library_read() {
        let mut favourites = FavouriteTracks::default();
        favourites.restore(vec![9]);
        youtube_push(&mut favourites, 1);
        youtube_push(&mut favourites, 2);
        favourites.restore(vec![9]);
        assert_eq!(favourites.track_ids(), [2, 1, 9]);
    }

    #[test]
    fn only_the_last_youtube_push_settles_the_server_value() {
        let mut favourites = FavouriteTracks::default();
        let (first, _) = youtube_push(&mut favourites, 5);
        let (last, _) = youtube_push(&mut favourites, 5);
        assert!(favourites
            .youtube_push_finished(5, youtube_song(5).path, first, true, Ok(()))
            .is_empty());
        let effects = favourites.youtube_push_finished(
            5,
            youtube_song(5).path,
            last,
            false,
            Err(youtube_music::Failure::Network),
        );
        assert!(favourites.contains(5));
        assert_eq!(
            effects[0],
            Effect::PersistYouTubeMusicFavourite {
                track_id: 5,
                address: youtube_song(5).path,
                favourite: true,
            }
        );
    }

    #[test]
    fn replacing_youtube_cancels_its_stars_without_losing_server_pushes() {
        let mut favourites = FavouriteTracks::default();
        favourites.restore(vec![9]);
        let (cancelled, _) = youtube_push(&mut favourites, 7);
        let (server_sequence, _) = push(&mut favourites, 8);
        assert_eq!(favourites.cancel_youtube_pushes(), [7]);
        assert_eq!(favourites.track_ids(), [8, 9]);
        assert!(favourites
            .youtube_push_finished(7, youtube_song(7).path, cancelled, true, Ok(()))
            .is_empty());
        assert_eq!(
            favourites.push_finished(8, server_sequence, true, Ok(())),
            [saved(8, true)]
        );
    }

    #[test]
    fn anonymous_youtube_rows_cannot_become_account_favorites() {
        let mut favourites = FavouriteTracks::default();
        assert!(favourites.toggle(&youtube_song(-1), None).is_empty());
        assert!(!favourites.contains(-1));
    }

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
