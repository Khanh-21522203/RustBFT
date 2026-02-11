use crate::crypto::hash::sha256;
use crate::types::Hash;
use lru::LruCache;
use std::num::NonZeroUsize;

pub struct Seen {
    cache: LruCache<Hash, ()>,
}

impl Seen {
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity.max(1)).unwrap();
        Self { cache: LruCache::new(cap) }
    }

    // returns true if NOT seen before
    pub fn check_and_mark(&mut self, bytes: &[u8]) -> bool {
        let h = sha256(bytes);
        if self.cache.contains(&h) {
            return false;
        }
        self.cache.put(h, ());
        true
    }
}
