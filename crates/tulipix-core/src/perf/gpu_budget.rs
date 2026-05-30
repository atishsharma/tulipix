//! GPU texture-cache LRU. The compositor uploads thumb textures into a
//! GPU-side cache; this module is the bookkeeping. 200 MB default budget.
//! Reported in Settings → Diagnostics so users on integrated GPUs can lower.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

pub const DEFAULT_BUDGET_BYTES: u64 = 200 * 1024 * 1024;

#[derive(Debug)]
struct Entry { bytes: u64, last_used: Instant }

#[derive(Debug)]
pub struct GpuBudget {
    budget_bytes: u64,
    used_bytes: Mutex<u64>,
    entries: Mutex<HashMap<u64, Entry>>,
    evicted_count: Mutex<u64>,
}

impl GpuBudget {
    pub fn new(budget_bytes: u64) -> Self {
        Self {
            budget_bytes,
            used_bytes: Mutex::new(0),
            entries: Mutex::new(HashMap::new()),
            evicted_count: Mutex::new(0),
        }
    }
    pub fn default_budget() -> Self { Self::new(DEFAULT_BUDGET_BYTES) }

    /// Insert / refresh a texture. Returns the ids evicted to make room.
    pub fn insert(&self, id: u64, bytes: u64) -> Vec<u64> {
        let mut entries = self.entries.lock().unwrap();
        let mut used = self.used_bytes.lock().unwrap();
        if let Some(e) = entries.get_mut(&id) {
            e.last_used = Instant::now();
            return vec![];
        }
        let mut evicted = vec![];
        while *used + bytes > self.budget_bytes {
            let Some((&lru_id, _)) = entries.iter().min_by_key(|(_, e)| e.last_used) else { break; };
            let removed = entries.remove(&lru_id).unwrap();
            *used = used.saturating_sub(removed.bytes);
            evicted.push(lru_id);
            *self.evicted_count.lock().unwrap() += 1;
        }
        entries.insert(id, Entry { bytes, last_used: Instant::now() });
        *used += bytes;
        evicted
    }

    pub fn touch(&self, id: u64) {
        if let Some(e) = self.entries.lock().unwrap().get_mut(&id) { e.last_used = Instant::now(); }
    }

    pub fn at_threshold(&self) -> bool {
        let used = *self.used_bytes.lock().unwrap();
        used as f64 >= 0.90 * self.budget_bytes as f64
    }

    pub fn used_bytes(&self) -> u64 { *self.used_bytes.lock().unwrap() }
    pub fn evicted_count(&self) -> u64 { *self.evicted_count.lock().unwrap() }
    pub fn budget_bytes(&self) -> u64 { self.budget_bytes }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn within_budget_no_eviction() {
        let b = GpuBudget::new(1024);
        assert!(b.insert(1, 256).is_empty());
        assert!(b.insert(2, 256).is_empty());
        assert_eq!(b.used_bytes(), 512);
        assert_eq!(b.evicted_count(), 0);
    }
    #[test] fn over_budget_evicts_lru() {
        let b = GpuBudget::new(512);
        b.insert(1, 256);
        std::thread::sleep(std::time::Duration::from_millis(2));
        b.insert(2, 256);
        std::thread::sleep(std::time::Duration::from_millis(2));
        b.touch(1);
        let evicted = b.insert(3, 256); // forces eviction of LRU (now id 2)
        assert_eq!(evicted, vec![2]);
        assert_eq!(b.used_bytes(), 512);
    }
    #[test] fn threshold_at_90_percent() {
        let b = GpuBudget::new(100);
        b.insert(1, 95);
        assert!(b.at_threshold());
    }
}
