//! The play queue and transport rules.
//!
//! Pure state: no IO, no threads, no audio. The engine thread owns one of
//! these and asks it what to play next; everything here is a total function of
//! the queue, the cursor and the two switches, which is what makes the
//! `peek_next` / `advance` agreement testable.
//!
//! That agreement is load-bearing. Gapless works by opening whatever
//! `peek_next` names *before* the current track ends, so if `advance` then
//! picks something else the listener hears the wrong song.

use std::path::PathBuf;

/// One entry in the queue. Carries what playback needs and nothing else — the
/// UI already has the full `Track`.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueEntry {
    pub track_id: i64,
    pub path: PathBuf,
    pub duration_secs: f64,
    /// Normalization gain in dB, resolved when the queue was loaded.
    pub gain_db: f64,
}

pub struct Queue {
    entries: Vec<QueueEntry>,
    /// Playback order as indices into `entries`. Identity when not shuffled,
    /// a permutation when shuffled.
    order: Vec<usize>,
    /// Cursor into `order`, not into `entries`.
    cursor: usize,
    shuffle: bool,
    repeat: bool,
    rng: u64,
}

impl Queue {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            order: Vec::new(),
            cursor: 0,
            shuffle: false,
            repeat: false,
            // Any non-zero seed will do; xorshift is only ever used to pick a
            // listening order, so it needs to be varied, not unpredictable.
            rng: 0x2545_F491_4F6C_DD1D,
        }
    }

    /// Replaces the queue and starts at `index` (an index into `entries`).
    pub fn load(&mut self, entries: Vec<QueueEntry>, index: usize) {
        let start = index.min(entries.len().saturating_sub(1));
        self.entries = entries;
        self.rebuild_order(start);
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Every entry's track id in entry order — the order `index()` refers to,
    /// not the shuffled playback order. What a reloaded webview needs to
    /// rebuild its queue mirror.
    pub fn track_ids(&self) -> Vec<i64> {
        self.entries.iter().map(|e| e.track_id).collect()
    }

    /// The position of the playing track within `entries` — what the UI
    /// highlights.
    pub fn index(&self) -> usize {
        self.order.get(self.cursor).copied().unwrap_or(0)
    }

    pub fn current(&self) -> Option<&QueueEntry> {
        self.entries.get(self.index())
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat(&self) -> bool {
        self.repeat
    }

    /// What [`advance`](Self::advance) will play, without moving the cursor.
    /// `None` means playback stops after the current track.
    pub fn peek_next(&self) -> Option<&QueueEntry> {
        self.next_cursor().and_then(|c| {
            let index = *self.order.get(c)?;
            self.entries.get(index)
        })
    }

    /// Moves to the next track, returning it. `None` means the queue ended.
    pub fn advance(&mut self) -> Option<&QueueEntry> {
        let next = self.next_cursor()?;
        // Wrapping to the top of a shuffled queue deserves a fresh order —
        // otherwise "shuffle" replays the same permutation for ever.
        let wrapped = next == 0 && self.cursor + 1 >= self.order.len();
        self.cursor = next;
        if wrapped && self.shuffle && self.order.len() > 1 {
            let current = self.index();
            self.reshuffle_from(current);
        }
        self.current()
    }

    /// Steps back. Always walks the playback order, and always wraps, which
    /// is what the transport's "previous" button has always done.
    pub fn back(&mut self) -> Option<&QueueEntry> {
        if self.order.is_empty() {
            return None;
        }
        self.cursor = if self.cursor == 0 {
            self.order.len() - 1
        } else {
            self.cursor - 1
        };
        self.current()
    }

    /// Jumps to a position in `entries`.
    pub fn jump_to(&mut self, index: usize) -> Option<&QueueEntry> {
        let cursor = self.order.iter().position(|&i| i == index)?;
        self.cursor = cursor;
        self.current()
    }

    pub fn set_shuffle(&mut self, enabled: bool) {
        if self.shuffle == enabled {
            return;
        }
        self.shuffle = enabled;
        // Rebuild around whatever is playing, so toggling shuffle never
        // interrupts the current track.
        let current = self.index();
        self.rebuild_order(current);
    }

    pub fn set_repeat(&mut self, enabled: bool) {
        self.repeat = enabled;
    }

    /// The cursor `advance` would move to, or `None` at the end of a
    /// non-repeating queue.
    fn next_cursor(&self) -> Option<usize> {
        if self.order.is_empty() {
            return None;
        }
        if self.cursor + 1 < self.order.len() {
            Some(self.cursor + 1)
        } else if self.repeat {
            Some(0)
        } else {
            None
        }
    }

    /// Rebuilds `order` so that `start` (an index into `entries`) plays now.
    fn rebuild_order(&mut self, start: usize) {
        self.order = (0..self.entries.len()).collect();
        self.cursor = 0;
        if self.entries.is_empty() {
            return;
        }
        if self.shuffle {
            self.reshuffle_from(start);
        } else {
            self.cursor = start.min(self.order.len() - 1);
        }
    }

    /// Fisher–Yates over the whole queue, then pulls `start` to the front so
    /// the track that is playing keeps playing.
    ///
    /// A real permutation rather than the "pick a random index each time" the
    /// frontend used: that could replay a track two steps later, and could
    /// never guarantee you heard the whole album.
    fn reshuffle_from(&mut self, start: usize) {
        let len = self.order.len();
        for i in (1..len).rev() {
            let j = (self.next_random() % (i as u64 + 1)) as usize;
            self.order.swap(i, j);
        }
        if let Some(position) = self.order.iter().position(|&i| i == start) {
            self.order.swap(0, position);
        }
        self.cursor = 0;
    }

    /// xorshift64*, so shuffling needs no dependency and tests are
    /// deterministic.
    fn next_random(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(count: usize) -> Vec<QueueEntry> {
        (0..count)
            .map(|i| QueueEntry {
                track_id: i as i64,
                path: PathBuf::from(format!("/music/{i}.flac")),
                duration_secs: 100.0,
                gain_db: 0.0,
            })
            .collect()
    }

    fn queue(count: usize, index: usize) -> Queue {
        let mut queue = Queue::new();
        queue.load(entries(count), index);
        queue
    }

    #[test]
    fn loading_starts_at_the_requested_track() {
        let queue = queue(5, 3);
        assert_eq!(queue.index(), 3);
        assert_eq!(queue.current().unwrap().track_id, 3);
    }

    #[test]
    fn loading_past_the_end_clamps() {
        let queue = queue(3, 99);
        assert_eq!(queue.index(), 2);
    }

    #[test]
    fn advance_walks_forward_then_stops_at_the_end() {
        let mut queue = queue(3, 0);
        assert_eq!(queue.advance().unwrap().track_id, 1);
        assert_eq!(queue.advance().unwrap().track_id, 2);
        assert!(queue.advance().is_none(), "a queue without repeat ends");
    }

    #[test]
    fn advance_wraps_when_repeat_is_on() {
        let mut queue = queue(3, 2);
        queue.set_repeat(true);
        assert_eq!(queue.advance().unwrap().track_id, 0);
    }

    #[test]
    fn back_always_wraps() {
        let mut queue = queue(3, 0);
        assert_eq!(
            queue.back().unwrap().track_id,
            2,
            "previous from the top wraps"
        );
    }

    #[test]
    fn peek_next_agrees_with_advance_in_every_configuration() {
        // The gapless invariant: what we preload must be what we then play.
        for len in 1..=6 {
            for index in 0..len {
                for shuffle in [false, true] {
                    for repeat in [false, true] {
                        let mut queue = Queue::new();
                        queue.set_shuffle(shuffle);
                        queue.set_repeat(repeat);
                        queue.load(entries(len), index);

                        for step in 0..len * 2 {
                            let peeked = queue.peek_next().map(|e| e.track_id);
                            let advanced = queue.advance().map(|e| e.track_id);
                            assert_eq!(
                                peeked, advanced,
                                "len={len} index={index} shuffle={shuffle} \
                                 repeat={repeat} step={step}"
                            );
                            if advanced.is_none() {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn shuffle_is_a_permutation_that_visits_every_track_once() {
        let mut queue = Queue::new();
        queue.set_shuffle(true);
        queue.load(entries(8), 0);

        let mut seen = vec![queue.current().unwrap().track_id];
        while let Some(entry) = queue.advance() {
            seen.push(entry.track_id);
        }

        seen.sort_unstable();
        assert_eq!(seen, (0..8).collect::<Vec<_>>(), "every track exactly once");
    }

    #[test]
    fn shuffle_keeps_the_current_track_playing() {
        let mut queue = queue(10, 7);
        queue.set_shuffle(true);
        assert_eq!(
            queue.index(),
            7,
            "toggling shuffle must not interrupt what is playing"
        );
    }

    #[test]
    fn unshuffling_resumes_linear_order_from_the_current_track() {
        let mut queue = Queue::new();
        queue.set_shuffle(true);
        queue.load(entries(6), 0);
        queue.advance();
        let playing = queue.index();

        queue.set_shuffle(false);
        assert_eq!(queue.index(), playing, "still on the same track");
        let expected = playing + 1;
        if expected < 6 {
            assert_eq!(queue.peek_next().unwrap().track_id, expected as i64);
        }
    }

    #[test]
    fn a_repeating_shuffled_queue_reshuffles_on_wrap() {
        let mut queue = Queue::new();
        queue.set_shuffle(true);
        queue.set_repeat(true);
        queue.load(entries(6), 0);

        let first: Vec<i64> = {
            let mut order = vec![queue.current().unwrap().track_id];
            for _ in 0..5 {
                order.push(queue.advance().unwrap().track_id);
            }
            order
        };
        let second: Vec<i64> = {
            let mut order = vec![queue.advance().unwrap().track_id];
            for _ in 0..5 {
                order.push(queue.advance().unwrap().track_id);
            }
            order
        };

        assert_ne!(first, second, "a second pass should not repeat the first");
        let mut sorted = second.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..6).collect::<Vec<_>>(), "still a permutation");
    }

    #[test]
    fn jump_to_moves_the_cursor_within_the_playback_order() {
        let mut queue = Queue::new();
        queue.set_shuffle(true);
        queue.load(entries(6), 0);

        assert_eq!(queue.jump_to(4).unwrap().track_id, 4);
        assert_eq!(queue.index(), 4);
    }

    #[test]
    fn an_empty_queue_answers_everything_with_none() {
        let mut queue = Queue::new();
        assert!(queue.current().is_none());
        assert!(queue.peek_next().is_none());
        assert!(queue.advance().is_none());
        assert!(queue.back().is_none());
        assert!(queue.is_empty());
    }

    #[test]
    fn a_single_track_queue_does_not_advance_unless_repeating() {
        let mut queue = queue(1, 0);
        assert!(queue.advance().is_none());
        queue.set_repeat(true);
        assert_eq!(queue.advance().unwrap().track_id, 0);
    }
}
