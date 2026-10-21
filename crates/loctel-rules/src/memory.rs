//! What the rules remember having dropped, so that data arriving later for the
//! same trace or span is dropped too (design §7: "later spans with that trace
//! ID are dropped on arrival, for a bounded time").

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

use crate::set::RuleId;

/// A bounded set of remembered keys, each with the rule that dropped it.
///
/// An entry lives from the moment it is remembered until `ttl` nanoseconds
/// later: it is not renewed when something is dropped because of it, so the
/// memory of a trace that keeps sending spans still ends on time. The set
/// holds at most `max` entries; past that the oldest are forgotten first.
pub(crate) struct Memory<K> {
    entries: HashMap<K, Entry>,
    /// The keys by when they were remembered, oldest first: the key of this
    /// map is `Entry::seq`.
    order: BTreeMap<u64, K>,
    next_seq: u64,
    ttl_nanos: u64,
    max: usize,
}

struct Entry {
    seq: u64,
    /// The first moment at which the entry no longer counts.
    until: u64,
    rule: RuleId,
}

impl<K: Copy + Eq + Hash> Memory<K> {
    pub(crate) fn new(ttl_nanos: u64, max: usize) -> Self {
        Memory {
            entries: HashMap::new(),
            order: BTreeMap::new(),
            next_seq: 0,
            ttl_nanos,
            max,
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remembers `key` as dropped by `rule` as of `now`. A key that is already
    /// remembered (and has not expired) keeps its entry, and its time.
    pub(crate) fn remember(&mut self, key: K, rule: RuleId, now: u64) {
        if self.get(&key, now).is_some() {
            return;
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.entries.insert(
            key,
            Entry {
                seq,
                until: now.saturating_add(self.ttl_nanos),
                rule,
            },
        );
        self.order.insert(seq, key);
        while self.entries.len() > self.max {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    /// The rule that dropped `key`, if it is remembered as of `now`. An entry
    /// found to be out of date is forgotten on the way.
    pub(crate) fn get(&mut self, key: &K, now: u64) -> Option<RuleId> {
        let entry = self.entries.get(key)?;
        if now >= entry.until {
            let seq = entry.seq;
            self.entries.remove(key);
            self.order.remove(&seq);
            return None;
        }
        Some(entry.rule)
    }

    /// Like [`get`](Self::get), but looks only: nothing is forgotten, and an
    /// entry past its time still answers. For crediting a removal to the rule
    /// that asked for it, moments after the entry was made.
    pub(crate) fn peek(&self, key: &K) -> Option<RuleId> {
        self.entries.get(key).map(|entry| entry.rule)
    }

    /// Forgets every entry that is out of date as of `now`, oldest first,
    /// stopping at the first one that is not. (With a clock that only moves
    /// forward that is all of them; if the clock jumped back, a stale entry
    /// behind a fresh one waits until [`get`](Self::get) or a later call
    /// reaches it.)
    pub(crate) fn expire(&mut self, now: u64) {
        while let Some((&seq, key)) = self.order.first_key_value() {
            let Some(entry) = self.entries.get(key) else {
                // Cannot happen: both maps hold the same keys. Drop the stray.
                self.order.remove(&seq);
                continue;
            };
            if now < entry.until {
                break;
            }
            let key = *key;
            self.entries.remove(&key);
            self.order.remove(&seq);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: u64 = 1_000_000_000;

    fn memory(max: usize) -> Memory<u32> {
        Memory::new(10 * SECOND, max)
    }

    #[test]
    fn remembers_for_the_ttl_only() {
        let mut m = memory(10);
        m.remember(1, RuleId(7), 100 * SECOND);

        assert_eq!(m.get(&1, 100 * SECOND), Some(RuleId(7)));
        assert_eq!(m.get(&1, 110 * SECOND - 1), Some(RuleId(7)));
        assert_eq!(m.get(&1, 110 * SECOND), None);
        // Found stale, it is gone for good, even for a clock that went back.
        assert_eq!(m.get(&1, 100 * SECOND), None);
        assert!(m.is_empty());
        assert_eq!(m.get(&2, 100 * SECOND), None);
    }

    #[test]
    fn past_the_maximum_the_oldest_are_forgotten_first() {
        let mut m = memory(3);
        for key in 1..=5 {
            m.remember(key, RuleId(1), 100 * SECOND + u64::from(key));
        }

        assert_eq!(m.len(), 3);
        let now = 101 * SECOND;
        assert_eq!(m.get(&1, now), None);
        assert_eq!(m.get(&2, now), None);
        assert_eq!(m.get(&3, now), Some(RuleId(1)));
        assert_eq!(m.get(&5, now), Some(RuleId(1)));
    }

    #[test]
    fn remembering_again_keeps_the_first_entry() {
        let mut m = memory(10);
        m.remember(1, RuleId(1), 100 * SECOND);

        // Not renewed, and not credited to another rule.
        m.remember(1, RuleId(2), 105 * SECOND);

        assert_eq!(m.len(), 1);
        assert_eq!(m.get(&1, 109 * SECOND), Some(RuleId(1)));
        assert_eq!(m.get(&1, 110 * SECOND), None);
        // Once it has expired it can be remembered afresh.
        m.remember(1, RuleId(2), 120 * SECOND);
        assert_eq!(m.get(&1, 125 * SECOND), Some(RuleId(2)));
    }

    #[test]
    fn expire_forgets_the_stale_front() {
        let mut m = memory(10);
        m.remember(1, RuleId(1), 100 * SECOND);
        m.remember(2, RuleId(1), 105 * SECOND);
        m.remember(3, RuleId(1), 112 * SECOND);

        m.expire(111 * SECOND);

        assert_eq!(m.len(), 2, "only the first is stale");
        m.expire(116 * SECOND);
        assert_eq!(m.len(), 1);
        m.expire(1_000 * SECOND);
        assert!(m.is_empty());
        // And the order index went with the entries.
        assert!(m.order.is_empty());
    }

    #[test]
    fn peek_ignores_the_time() {
        let mut m = memory(10);
        m.remember(1, RuleId(3), 0);

        assert_eq!(m.peek(&1), Some(RuleId(3)));
        assert_eq!(m.peek(&2), None);
    }
}
