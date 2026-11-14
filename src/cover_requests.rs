//! Which list and grid covers to load, and when.
//!
//! A tile asks for its cover each time it is drawn after a change of
//! `Covers.revision`. A fast scroll draws hundreds of tiles for a frame or
//! two each, and loading every cover they asked for would keep the loaders
//! busy with tiles long gone while the ones in view wait. So a request waits:
//! at every check the controller bumps the revision and only the tiles still
//! on screen ask again. A cover starts loading once its tile has asked on
//! both sides of a check, at most `parallel_limit` at once; a request nobody
//! repeated by the next check is forgotten.
//!
//! Once a list has rested, the covers just beyond what it shows are
//! prefetched, so a short scroll lands on covers already there. A prefetch
//! gives way to the tiles in view: it starts only while none waits, and takes
//! at most `prefetch_limit` loaders. Pure: the controller owns the clock, the
//! loaders, the lists and the window.

use std::collections::HashMap;
use std::hash::Hash;

pub struct CoverRequests<Key> {
    waiting: HashMap<Key, Waiting>,
    loading: HashMap<Key, Loading>,
    /// The covers near the list's window, nearest first, not yet started.
    prefetching: Vec<Key>,
    /// How many checks have run.
    check: u64,
    /// Counts every ask, so the covers start in the order the tiles were drawn.
    sequence: u64,
    parallel_limit: usize,
    prefetch_limit: usize,
    /// Moves on when the requests are cleared, so a load started before is
    /// recognised on arrival and dropped.
    epoch: u64,
}

struct Waiting {
    /// The check count when the tile first asked.
    first_check: u64,
    /// The check count when the tile last asked.
    last_check: u64,
    last_sequence: u64,
}

struct Loading {
    prefetched: bool,
    /// Whether a tile in view asked for it, so the window must be told when
    /// it lands.
    shown: bool,
}

/// Which way a list last moved, so the covers it is heading for come first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    #[default]
    Down,
    Up,
}

impl<Key: Clone + Eq + Hash> CoverRequests<Key> {
    pub fn new(parallel_limit: usize, prefetch_limit: usize) -> Self {
        Self {
            waiting: HashMap::new(),
            loading: HashMap::new(),
            prefetching: Vec::new(),
            check: 0,
            sequence: 0,
            parallel_limit,
            prefetch_limit,
            epoch: 0,
        }
    }

    /// The epoch the loads started now run under.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// A tile in view wants this cover. One already loading is only marked
    /// as shown.
    pub fn ask(&mut self, key: Key) {
        if let Some(loading) = self.loading.get_mut(&key) {
            loading.shown = true;
            return;
        }
        self.sequence += 1;
        let (check, sequence) = (self.check, self.sequence);
        self.waiting
            .entry(key)
            .and_modify(|waiting| {
                waiting.last_check = check;
                waiting.last_sequence = sequence;
            })
            .or_insert(Waiting {
                first_check: check,
                last_check: check,
                last_sequence: sequence,
            });
    }

    /// The covers near a list that rested, nearest first, uncached ones
    /// only. They replace those of the list's previous resting place.
    pub fn prefetch(&mut self, keys: Vec<Key>) {
        self.prefetching = keys;
    }

    /// The periodic check: forgets the requests no tile repeated since the
    /// previous one — their tiles scrolled away — and returns the covers to
    /// load now. The controller then asks the tiles in view to ask again.
    pub fn check(&mut self) -> Vec<Key> {
        let check = self.check;
        self.waiting
            .retain(|_, waiting| waiting.last_check == check);
        self.check += 1;
        self.start()
    }

    /// The covers to load now, as many as free loaders allow: first those
    /// asked for on both sides of a check, in the order their tiles were
    /// drawn; then, while no tile waits, the nearest prefetches. They count
    /// as loading until `finished`.
    pub fn start(&mut self) -> Vec<Key> {
        let mut free = self.parallel_limit.saturating_sub(self.loading.len());
        let mut confirmed: Vec<(u64, Key)> = self
            .waiting
            .iter()
            .filter(|(_, waiting)| waiting.last_check > waiting.first_check)
            .map(|(key, waiting)| (waiting.last_sequence, key.clone()))
            .collect();
        confirmed.sort_unstable_by_key(|(sequence, _)| *sequence);
        let mut started: Vec<Key> = confirmed
            .into_iter()
            .take(free)
            .map(|(_, key)| key)
            .collect();
        for key in &started {
            self.waiting.remove(key);
            self.loading.insert(
                key.clone(),
                Loading {
                    prefetched: false,
                    shown: true,
                },
            );
        }
        free -= started.len();
        if !self.waiting.is_empty() {
            return started;
        }
        let prefetching = self
            .loading
            .values()
            .filter(|loading| loading.prefetched)
            .count();
        let mut room = free.min(self.prefetch_limit.saturating_sub(prefetching));
        while room > 0 && !self.prefetching.is_empty() {
            let key = self.prefetching.remove(0);
            if self.loading.contains_key(&key) {
                continue;
            }
            self.loading.insert(
                key.clone(),
                Loading {
                    prefetched: true,
                    shown: false,
                },
            );
            started.push(key);
            room -= 1;
        }
        started
    }

    /// A cover's load, started under `epoch`, ended, whatever its outcome:
    /// its loader is free. `None` when the requests were cleared since, and
    /// the cover is to be dropped; otherwise whether a tile in view asked for
    /// it, so the window must look again.
    pub fn finished(&mut self, key: &Key, epoch: u64) -> Option<bool> {
        if epoch != self.epoch {
            return None;
        }
        Some(
            self.loading
                .remove(key)
                .is_some_and(|loading| loading.shown),
        )
    }

    /// Whether requests wait, so the checks must go on.
    pub fn has_waiting(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// Forgets every request, as when the library is replaced. Loads already
    /// running finish on their own and are dropped on arrival.
    pub fn clear(&mut self) {
        self.waiting.clear();
        self.loading.clear();
        self.prefetching.clear();
        self.epoch += 1;
    }
}

/// The places just beyond a list's window of `count` places from `first`,
/// in a list of `length`: as many on each side as the window holds, the side
/// the list is heading for first, nearest first on each side.
pub fn nearby(first: usize, count: usize, length: usize, direction: Direction) -> Vec<usize> {
    let end = first.saturating_add(count).min(length);
    let after = end..end.saturating_add(count).min(length);
    let before = (first.saturating_sub(count)..first.min(length)).rev();
    match direction {
        Direction::Down => after.chain(before).collect(),
        Direction::Up => before.chain(after).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cover_loads_once_its_tile_asked_on_both_sides_of_a_check() {
        let mut requests = CoverRequests::new(4, 2);
        requests.ask(1);

        assert!(requests.start().is_empty(), "a first ask is not enough");
        assert!(requests.check().is_empty());
        assert!(requests.has_waiting());

        requests.ask(1);
        assert_eq!(requests.check(), [1]);
        assert!(!requests.has_waiting());
    }

    #[test]
    fn a_tile_that_scrolled_away_before_a_check_is_forgotten() {
        let mut requests = CoverRequests::new(4, 2);
        for key in 0..100 {
            requests.ask(key);
        }
        // Only the tile where the scroll stopped is drawn after the check.
        requests.check();
        requests.ask(99);

        assert_eq!(requests.check(), [99]);
        assert!(!requests.has_waiting(), "the other 99 requests are gone");
    }

    #[test]
    fn no_more_covers_load_at_once_than_the_limit() {
        let mut requests = CoverRequests::new(2, 2);
        for key in 1..=5 {
            requests.ask(key);
        }
        requests.check();
        for key in 1..=5 {
            requests.ask(key);
        }

        assert_eq!(
            requests.check(),
            [1, 2],
            "in the order the tiles were drawn"
        );
        assert!(requests.start().is_empty(), "no loader is free");

        requests.finished(&1, 0);
        assert_eq!(requests.start(), [3]);
    }

    #[test]
    fn a_request_waiting_for_a_loader_must_still_be_repeated() {
        let mut requests = CoverRequests::new(1, 1);
        requests.ask(1);
        requests.ask(2);
        requests.check();
        requests.ask(1);
        requests.ask(2);
        assert_eq!(requests.check(), [1]);

        // Tile 2 scrolled away while cover 1 loaded.
        requests.check();
        requests.finished(&1, 0);
        assert!(requests.start().is_empty());
        assert!(!requests.has_waiting());
    }

    #[test]
    fn asking_for_a_cover_already_loading_starts_it_only_once() {
        let mut requests = CoverRequests::new(4, 2);
        requests.ask(1);
        requests.check();
        requests.ask(1);
        assert_eq!(requests.check(), [1]);

        requests.ask(1);
        requests.check();
        requests.ask(1);
        assert!(requests.check().is_empty());
    }

    #[test]
    fn clearing_forgets_waiting_and_loading_covers() {
        let mut requests = CoverRequests::new(1, 1);
        requests.ask(1);
        requests.ask(2);
        requests.check();
        requests.ask(1);
        requests.ask(2);
        assert_eq!(requests.check(), [1]);

        requests.clear();
        assert!(!requests.has_waiting());
        requests.ask(1);
        requests.check();
        requests.ask(1);
        assert_eq!(requests.check(), [1], "the loader counts as free again");
    }

    /// Confirms `keys` as a tile in view would: asked on both sides of a
    /// check.
    fn confirm(requests: &mut CoverRequests<i32>, keys: &[i32]) -> Vec<i32> {
        for key in keys {
            requests.ask(*key);
        }
        requests.check();
        for key in keys {
            requests.ask(*key);
        }
        requests.check()
    }

    #[test]
    fn prefetches_start_nearest_first_within_their_share_of_loaders() {
        let mut requests = CoverRequests::new(4, 2);
        requests.prefetch(vec![10, 11, 12]);

        assert_eq!(requests.start(), [10, 11]);
        assert!(
            requests.start().is_empty(),
            "two loaders stay for the tiles"
        );

        assert_eq!(
            requests.finished(&10, 0),
            Some(false),
            "no tile shows a prefetched cover"
        );
        assert_eq!(requests.start(), [12]);
    }

    #[test]
    fn a_tile_in_view_goes_before_the_prefetches() {
        let mut requests = CoverRequests::new(1, 1);
        requests.prefetch(vec![10]);
        requests.ask(1);

        assert!(requests.start().is_empty(), "a tile waits to be confirmed");
        requests.check();
        requests.ask(1);
        assert_eq!(requests.check(), [1]);

        assert_eq!(requests.finished(&1, 0), Some(true), "the tile is told");
        assert_eq!(requests.start(), [10]);
    }

    #[test]
    fn a_new_resting_place_replaces_the_prefetches_not_started() {
        let mut requests = CoverRequests::new(1, 1);
        requests.prefetch(vec![10, 11]);
        assert_eq!(requests.start(), [10]);

        requests.prefetch(vec![20]);
        requests.finished(&10, 0);
        assert_eq!(requests.start(), [20]);
    }

    #[test]
    fn a_prefetched_cover_a_tile_asks_for_while_it_loads_is_shown() {
        let mut requests = CoverRequests::new(4, 2);
        requests.prefetch(vec![10]);
        assert_eq!(requests.start(), [10]);

        requests.ask(10);
        assert!(confirm(&mut requests, &[10]).is_empty(), "not loaded twice");
        assert_eq!(requests.finished(&10, 0), Some(true));
    }

    #[test]
    fn the_places_the_list_heads_for_come_first() {
        assert_eq!(nearby(10, 3, 100, Direction::Down), [13, 14, 15, 9, 8, 7]);
        assert_eq!(nearby(10, 3, 100, Direction::Up), [9, 8, 7, 13, 14, 15]);
    }

    #[test]
    fn the_places_nearby_stay_within_the_list() {
        assert_eq!(nearby(0, 3, 5, Direction::Down), [3, 4]);
        assert_eq!(nearby(3, 3, 5, Direction::Down), [2, 1, 0]);
        assert!(nearby(0, 3, 0, Direction::Down).is_empty());
    }

    #[test]
    fn a_load_started_before_the_requests_were_cleared_is_dropped() {
        let mut requests = CoverRequests::new(4, 2);
        requests.ask(1);
        requests.check();
        requests.ask(1);
        assert_eq!(requests.check(), [1]);
        let epoch = requests.epoch();

        requests.clear();

        assert_eq!(requests.finished(&1, epoch), None);
        assert_eq!(requests.finished(&1, requests.epoch()), Some(false));
    }
}
