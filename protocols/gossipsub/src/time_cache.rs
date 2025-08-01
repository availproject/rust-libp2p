// Copyright 2020 Sigma Prime Pty Ltd.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! This implements a time-based LRU cache for checking gossipsub message duplicates.

use std::{
    collections::{
        hash_map::{
            self,
            Entry::{Occupied, Vacant},
        },
        VecDeque,
    },
    time::Duration,
};

use fnv::FnvHashMap;
use web_time::Instant;

struct ExpiringElement<Element> {
    /// The element that expires
    element: Element,
    /// The expire time.
    expires: Instant,
}

pub(crate) struct TimeCache<Key, Value> {
    /// Mapping a key to its value together with its latest expire time (can be updated through
    /// reinserts).
    map: FnvHashMap<Key, ExpiringElement<Value>>,
    /// An ordered list of keys by expires time.
    list: VecDeque<ExpiringElement<Key>>,
    /// The time elements remain in the cache.
    ttl: Duration,
}

pub(crate) struct OccupiedEntry<'a, K, V> {
    entry: hash_map::OccupiedEntry<'a, K, ExpiringElement<V>>,
}

impl<'a, K, V> OccupiedEntry<'a, K, V>
where
    K: Eq + std::hash::Hash + Clone,
{
    pub(crate) fn into_mut(self) -> &'a mut V {
        &mut self.entry.into_mut().element
    }
}

pub(crate) struct VacantEntry<'a, K, V> {
    expiration: Instant,
    entry: hash_map::VacantEntry<'a, K, ExpiringElement<V>>,
    list: &'a mut VecDeque<ExpiringElement<K>>,
}

impl<'a, K, V> VacantEntry<'a, K, V>
where
    K: Eq + std::hash::Hash + Clone,
{
    pub(crate) fn insert(self, value: V) -> &'a mut V {
        self.list.push_back(ExpiringElement {
            element: self.entry.key().clone(),
            expires: self.expiration,
        });
        &mut self
            .entry
            .insert(ExpiringElement {
                element: value,
                expires: self.expiration,
            })
            .element
    }
}

pub(crate) enum Entry<'a, K: 'a, V: 'a> {
    Occupied(OccupiedEntry<'a, K, V>),
    Vacant(VacantEntry<'a, K, V>),
}

impl<'a, K: 'a, V: 'a> Entry<'a, K, V>
where
    K: Eq + std::hash::Hash + Clone,
{
    pub(crate) fn or_default(self) -> &'a mut V
    where
        V: Default,
    {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(V::default()),
        }
    }
}

impl<Key, Value> TimeCache<Key, Value>
where
    Key: Eq + std::hash::Hash + Clone,
{
    pub(crate) fn new(ttl: Duration) -> Self {
        TimeCache {
            map: FnvHashMap::default(),
            list: VecDeque::new(),
            ttl,
        }
    }

    fn remove_expired_keys(&mut self, now: Instant) {
        while let Some(element) = self.list.pop_front() {
            if element.expires > now {
                self.list.push_front(element);
                break;
            }
            if let Occupied(entry) = self.map.entry(element.element.clone()) {
                if entry.get().expires <= now {
                    entry.remove();
                }
            }
        }
    }

    pub(crate) fn entry(&mut self, key: Key) -> Entry<'_, Key, Value> {
        let now = Instant::now();
        self.remove_expired_keys(now);
        match self.map.entry(key) {
            Occupied(entry) => Entry::Occupied(OccupiedEntry { entry }),
            Vacant(entry) => Entry::Vacant(VacantEntry {
                expiration: now + self.ttl,
                entry,
                list: &mut self.list,
            }),
        }
    }

    /// Empties the entire cache.
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        self.map.clear();
        self.list.clear();
    }

    pub(crate) fn contains_key(&self, key: &Key) -> bool {
        self.map.contains_key(key)
    }
}

pub(crate) struct DuplicateCache<Key>(TimeCache<Key, ()>);

impl<Key> DuplicateCache<Key>
where
    Key: Eq + std::hash::Hash + Clone,
{
    pub(crate) fn new(ttl: Duration) -> Self {
        Self(TimeCache::new(ttl))
    }

    // Inserts new elements and removes any expired elements.
    //
    // If the key was not present this returns `true`. If the value was already present this
    // returns `false`.
    pub(crate) fn insert(&mut self, key: Key) -> bool {
        if let Entry::Vacant(entry) = self.0.entry(key) {
            entry.insert(());
            true
        } else {
            false
        }
    }

    pub(crate) fn contains(&self, key: &Key) -> bool {
        self.0.contains_key(key)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn cache_added_entries_exist() {
        let mut cache = DuplicateCache::new(Duration::from_secs(10));

        cache.insert("t");
        cache.insert("e");

        // Should report that 't' and 't' already exists
        assert!(!cache.insert("t"));
        assert!(!cache.insert("e"));
    }

    #[test]
    fn cache_entries_expire() {
        let mut cache = DuplicateCache::new(Duration::from_millis(100));

        cache.insert("t");
        assert!(!cache.insert("t"));
        cache.insert("e");
        // assert!(!cache.insert("t"));
        assert!(!cache.insert("e"));
        // sleep until cache expiry
        std::thread::sleep(Duration::from_millis(101));
        // add another element to clear previous cache
        cache.insert("s");

        // should be removed from the cache
        assert!(cache.insert("t"));
    }
}
