//! Pluggable rate limit backend trait.

/// Rate limit counting backend — pluggable implementations.
///
/// Built-in: [`LocalBackend`](super::local::LocalBackend) (in-memory DashMap).
/// Optional: `RedisBackend` (feature-gated behind `redis`).
pub trait RateLimitBackend: Send + Sync {
    /// Check the rate limit for `key` against the given limits.
    /// On success, increments the request counter and returns `Ok(())`.
    /// On failure, returns `Err(retry_after_secs)`.
    fn check_and_increment(
        &self,
        key: &str,
        window_secs: u64,
        max_requests: u64,
        max_tokens: Option<u64>,
    ) -> Result<(), u64>;

    /// Record token usage for a key (called from the response phase).
    fn record_tokens(&self, key: &str, tokens: u64);

    /// Remove entries whose window has expired.
    fn cleanup(&self, max_window_secs: u64);

    /// Backend name (for logging / diagnostics).
    fn name(&self) -> &str;

    /// Health check — returns false when the backend is degraded.
    fn is_healthy(&self) -> bool {
        true
    }
}
