//! Pluggable rate limit backend trait.

use super::quota::{Quota, QuotaDenied, QuotaMetric};

/// Rate limit counting backend — pluggable implementations.
///
/// Built-in: [`LocalBackend`](super::local::LocalBackend) (in-memory DashMap).
/// Optional: `RedisBackend` (feature-gated behind `redis`).
pub trait RateLimitBackend: Send + Sync {
    /// Check all quotas for a key within the given window.
    ///
    /// - `Requests` quota: atomically increments and checks (pre-check).
    /// - All other quotas: checks current recorded value against limit (post-record).
    ///
    /// Returns `Ok(())` if all quotas pass, or `Err(QuotaDenied)` with the first
    /// quota that was exceeded.
    fn check_quotas(
        &self,
        key: &str,
        window_secs: u64,
        quotas: &[Quota],
    ) -> Result<(), QuotaDenied>;

    /// Record a metric value (called from the response phase).
    ///
    /// Used for post-record metrics like tokens, cost, etc.
    fn record_metric(&self, key: &str, metric: &QuotaMetric, value: u64);

    /// Remove entries whose window has expired.
    fn cleanup(&self, max_window_secs: u64);

    /// Backend name (for logging / diagnostics).
    fn name(&self) -> &str;

    /// Health check — returns false when the backend is degraded.
    fn is_healthy(&self) -> bool {
        true
    }

    /// Peek at the current accumulated value for a metric (read-only, no increment).
    fn peek(&self, _key: &str, _metric: &QuotaMetric) -> u64 {
        0
    }
}
