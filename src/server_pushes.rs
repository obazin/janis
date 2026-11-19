//! What is on its way to a media server, and what the server holds meanwhile.
//!
//! A change made in Janis to something a server keeps — a song's star, a
//! song's place in a playlist, a playlist's name — shows at once, is sent to
//! the server, and is saved once the server has answered. Pushes run one
//! after another on the server worker, so their answers come back in the
//! order they were sent. While a key's pushes are on their way, this follows
//! the value the server holds: what it had before the first push, then what
//! each push it took left it with. Only the answer to the key's last push
//! settles it: the server's value is what gets saved, and when it is not the
//! one the listener asked for last, the change is taken back.

use std::collections::HashMap;
use std::hash::Hash;

#[derive(Clone, Debug)]
pub struct PendingPushes<Key, Value> {
    pending: HashMap<Key, Pending<Value>>,
    /// Numbers the pushes, so the answer to a key's last one is known.
    sequence: u64,
}

impl<Key, Value> PartialEq for PendingPushes<Key, Value>
where
    Key: Eq + Hash,
    Value: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.pending == other.pending && self.sequence == other.sequence
    }
}

impl<Key, Value> Eq for PendingPushes<Key, Value>
where
    Key: Eq + Hash,
    Value: Eq,
{
}

impl<Key, Value> Default for PendingPushes<Key, Value> {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
            sequence: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Pending<Value> {
    /// What the listener asked for last, which Janis shows.
    wanted: Value,
    /// What the server holds, as far as its answers tell.
    server_value: Value,
    /// Where the key sat in its list before the first push, so a change the
    /// server refused puts it back there.
    place: Option<usize>,
    last_sequence: u64,
}

/// The answer to a key's last push: what the server holds, which is what
/// gets saved, and what the listener had asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settled<Value> {
    pub server_value: Value,
    pub wanted: Value,
    pub place: Option<usize>,
}

impl<Value> Settled<Value>
where
    Value: PartialEq,
{
    /// True when the server holds what the listener asked for.
    pub fn as_wanted(&self) -> bool {
        self.server_value == self.wanted
    }
}

impl<Key, Value> PendingPushes<Key, Value>
where
    Key: Eq + Hash + Clone,
    Value: Clone,
{
    /// Records a push of `wanted` for `key` and returns its number. The
    /// server's value and the place are those before the key's first push
    /// still on its way; a later push keeps them.
    pub fn push(
        &mut self,
        key: Key,
        wanted: Value,
        server_value: Value,
        place: Option<usize>,
    ) -> u64 {
        self.sequence += 1;
        let sequence = self.sequence;
        self.pending
            .entry(key)
            .and_modify(|pending| {
                pending.wanted = wanted.clone();
                pending.last_sequence = sequence;
            })
            .or_insert(Pending {
                wanted,
                server_value,
                place,
                last_sequence: sequence,
            });
        sequence
    }

    /// The server answered the push numbered `sequence`, which carried
    /// `value`: it holds it when `took` is true. Gives the key's settlement
    /// once its last push has answered, and nothing before, nor for a key
    /// with nothing on its way.
    pub fn answer(
        &mut self,
        key: &Key,
        sequence: u64,
        value: Value,
        took: bool,
    ) -> Option<Settled<Value>> {
        let pending = self.pending.get_mut(key)?;
        if took {
            pending.server_value = value;
        }
        if sequence != pending.last_sequence {
            return None;
        }
        let pending = self.pending.remove(key)?;
        Some(Settled {
            server_value: pending.server_value,
            wanted: pending.wanted,
            place: pending.place,
        })
    }

    /// What the listener asked for, for every key still on its way: a fresh
    /// read of what is saved, which may have read the server before a push
    /// reached it, takes these over what it read.
    pub fn wanted(&self) -> impl Iterator<Item = (&Key, &Value)> {
        self.pending
            .iter()
            .map(|(key, pending)| (key, &pending.wanted))
    }

    pub fn wanted_for(&self, key: &Key) -> Option<&Value> {
        self.pending.get(key).map(|pending| &pending.wanted)
    }

    /// Abandons queued intentions without rewinding their sequence numbers.
    pub fn take_settled(
        &mut self,
    ) -> impl Iterator<Item = (Key, Settled<Value>)> + use<Key, Value> {
        std::mem::take(&mut self.pending)
            .into_iter()
            .map(|(key, pending)| {
                (
                    key,
                    Settled {
                        server_value: pending.server_value,
                        wanted: pending.wanted,
                        place: pending.place,
                    },
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_to_a_single_push_settles_it_with_what_the_server_took() {
        let mut pushes = PendingPushes::default();
        let sequence = pushes.push(5, true, false, Some(2));
        let settled = pushes.answer(&5, sequence, true, true);
        assert_eq!(
            settled,
            Some(Settled {
                server_value: true,
                wanted: true,
                place: Some(2),
            })
        );
        assert!(settled.is_some_and(|settled| settled.as_wanted()));
        assert_eq!(
            pushes.answer(&5, sequence, true, true),
            None,
            "a settled key has nothing on its way"
        );
    }

    #[test]
    fn a_refused_push_settles_with_the_value_the_server_had() {
        let mut pushes = PendingPushes::default();
        let sequence = pushes.push(5, true, false, None);
        let settled = pushes.answer(&5, sequence, true, false);
        assert_eq!(
            settled.map(|settled| (settled.server_value, settled.as_wanted())),
            Some((false, false))
        );
    }

    #[test]
    fn only_the_last_push_of_a_key_settles_it_and_earlier_answers_move_the_server_value() {
        let mut pushes = PendingPushes::default();
        let first = pushes.push(5, true, false, Some(1));
        let second = pushes.push(5, false, true, Some(9));
        assert_eq!(pushes.answer(&5, first, true, true), None);
        let settled = pushes.answer(&5, second, false, false);
        assert_eq!(
            settled,
            Some(Settled {
                server_value: true,
                wanted: false,
                place: Some(1),
            }),
            "the first push's place and the value the server took are kept"
        );
    }

    #[test]
    fn an_answer_for_a_key_with_nothing_on_its_way_is_dropped() {
        let mut pushes: PendingPushes<i64, bool> = PendingPushes::default();
        assert_eq!(pushes.answer(&5, 1, true, true), None);
    }

    #[test]
    fn the_wanted_values_are_those_asked_for_last() {
        let mut pushes = PendingPushes::default();
        pushes.push("a", 1, 0, None);
        pushes.push("a", 2, 0, None);
        pushes.push("b", 3, 0, None);
        let mut wanted: Vec<(&str, i32)> =
            pushes.wanted().map(|(key, value)| (*key, *value)).collect();
        wanted.sort();
        assert_eq!(wanted, [("a", 2), ("b", 3)]);
    }
}
