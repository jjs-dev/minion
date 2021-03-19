use std::collections::HashSet;

use parking_lot::Mutex;

#[derive(Debug)]
pub(in crate::linux) struct UidAllocator {
    low: u32,
    high: u32,
    used: Mutex<HashSet<u32>>,
}

impl UidAllocator {
    pub(in crate::linux) fn new(low: u32, high: u32) -> Self {
        assert!(low <= high);
        UidAllocator {
            low,
            high,
            used: Mutex::new(HashSet::new()),
        }
    }

    pub(in crate::linux) fn allocate(&self) -> Option<u32> {
        let mut used = self.used.lock();
        for uid in self.low..self.high {
            if used.insert(uid) {
                return Some(uid);
            }
        }
        None
    }

    pub(in crate::linux) fn deallocate(&self, uid: u32) {
        let mut used = self.used.lock();
        assert!(used.remove(&uid));
    }
}
