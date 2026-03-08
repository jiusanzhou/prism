//! In-flight request tracker for load-aware scheduling.
//!
//! Tracks the number of concurrent requests per variant. Updated by the proxy
//! layer (increment on dispatch, decrement on response/error) and read by
//! routing plugins to make load-aware decisions.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::RwLock;

/// Thread-safe in-flight request counter per variant.
///
/// Designed for high-frequency concurrent access:
/// - Writes (increment/decrement) use atomic operations — zero contention.
/// - Reads (load snapshot) acquire a read lock only to iterate keys.
/// - New variants are lazily created on first increment (write lock, rare).
#[derive(Debug)]
pub struct InFlightTracker {
    counters: RwLock<HashMap<String, AtomicU32>>,
}

impl InFlightTracker {
    pub fn new() -> Self {
        Self {
            counters: RwLock::new(HashMap::new()),
        }
    }

    /// Increment in-flight count for a variant. Called when a request is dispatched.
    pub fn increment(&self, variant_id: &str) {
        // Fast path: read lock + atomic increment
        {
            let counters = self.counters.read().unwrap();
            if let Some(counter) = counters.get(variant_id) {
                counter.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        // Slow path: variant not seen before — write lock to insert
        {
            let mut counters = self.counters.write().unwrap();
            let counter = counters
                .entry(variant_id.to_string())
                .or_insert_with(|| AtomicU32::new(0));
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Decrement in-flight count for a variant. Called when a request completes.
    pub fn decrement(&self, variant_id: &str) {
        let counters = self.counters.read().unwrap();
        if let Some(counter) = counters.get(variant_id) {
            // Saturating: don't underflow on double-decrement
            let prev = counter.load(Ordering::Relaxed);
            if prev > 0 {
                counter.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    /// Get the in-flight count for a single variant.
    pub fn get(&self, variant_id: &str) -> u32 {
        let counters = self.counters.read().unwrap();
        counters
            .get(variant_id)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Snapshot all variant loads. Used by routing plugins.
    pub fn snapshot(&self) -> HashMap<String, u32> {
        let counters = self.counters.read().unwrap();
        counters
            .iter()
            .map(|(k, v)| (k.clone(), v.load(Ordering::Relaxed)))
            .collect()
    }
}

impl Default for InFlightTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_increment_decrement() {
        let tracker = InFlightTracker::new();
        assert_eq!(tracker.get("v1"), 0);
        tracker.increment("v1");
        assert_eq!(tracker.get("v1"), 1);
        tracker.increment("v1");
        assert_eq!(tracker.get("v1"), 2);
        tracker.decrement("v1");
        assert_eq!(tracker.get("v1"), 1);
        tracker.decrement("v1");
        assert_eq!(tracker.get("v1"), 0);
    }

    #[test]
    fn decrement_saturates_at_zero() {
        let tracker = InFlightTracker::new();
        tracker.increment("v1");
        tracker.decrement("v1");
        tracker.decrement("v1");
        assert_eq!(tracker.get("v1"), 0);
    }

    #[test]
    fn snapshot_returns_all() {
        let tracker = InFlightTracker::new();
        tracker.increment("v1");
        tracker.increment("v1");
        tracker.increment("v2");
        let snap = tracker.snapshot();
        assert_eq!(snap.get("v1"), Some(&2));
        assert_eq!(snap.get("v2"), Some(&1));
    }

    #[test]
    fn concurrent_access() {
        use std::sync::Arc;
        use std::thread;
        let tracker = Arc::new(InFlightTracker::new());
        let mut handles = vec![];
        for _ in 0..10 {
            let t = Arc::clone(&tracker);
            handles.push(thread::spawn(move || {
                for _ in 0..1000 { t.increment("v1"); }
            }));
        }
        for h in handles { h.join().unwrap(); }
        assert_eq!(tracker.get("v1"), 10_000);
    }
}
