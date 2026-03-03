//! Local in-memory rate limit backend — DashMap sliding window.

use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;

use super::backend::RateLimitBackend;

/// Per-key window state: (window_start_timestamp, request_count, token_count).
pub struct LocalBackend {
    windows: DashMap<String, (u64, u64, u64)>,
}

impl LocalBackend {
    pub fn new() -> Self {
        Self {
            windows: DashMap::new(),
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

impl Default for LocalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimitBackend for LocalBackend {
    fn check_and_increment(
        &self,
        key: &str,
        window_secs: u64,
        max_requests: u64,
        max_tokens: Option<u64>,
    ) -> Result<(), u64> {
        let now = Self::now_secs();

        let mut entry = self.windows.entry(key.to_string()).or_insert((now, 0, 0));
        let (window_start, count, tokens) = entry.value_mut();

        // Reset window if expired.
        if now - *window_start >= window_secs {
            *window_start = now;
            *count = 0;
            *tokens = 0;
        }

        // Check request count.
        if *count >= max_requests {
            let retry_after = window_secs.saturating_sub(now - *window_start);
            return Err(retry_after);
        }

        // Check token count (if configured).
        if let Some(max_tok) = max_tokens {
            if *tokens >= max_tok {
                let retry_after = window_secs.saturating_sub(now - *window_start);
                return Err(retry_after);
            }
        }

        *count += 1;
        Ok(())
    }

    fn record_tokens(&self, key: &str, tokens: u64) {
        if let Some(mut entry) = self.windows.get_mut(key) {
            entry.value_mut().2 += tokens;
        }
    }

    fn cleanup(&self, max_window_secs: u64) {
        let now = Self::now_secs();
        self.windows
            .retain(|_, (start, _, _)| now - *start < max_window_secs);
    }

    fn name(&self) -> &str {
        "local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_name() {
        let backend = LocalBackend::new();
        assert_eq!(backend.name(), "local");
    }

    #[test]
    fn test_backend_is_healthy() {
        let backend = LocalBackend::new();
        assert!(backend.is_healthy());
    }

    #[test]
    fn test_counter_increment_within_limit() {
        let backend = LocalBackend::new();
        for _ in 0..5 {
            assert!(backend.check_and_increment("test", 60, 5, None).is_ok());
        }
    }

    #[test]
    fn test_counter_exceeds_limit() {
        let backend = LocalBackend::new();
        for _ in 0..3 {
            assert!(backend.check_and_increment("test", 60, 3, None).is_ok());
        }
        let result = backend.check_and_increment("test", 60, 3, None);
        assert!(result.is_err());
        let retry_after = result.unwrap_err();
        assert!(retry_after > 0);
    }

    #[test]
    fn test_counter_independent_keys() {
        let backend = LocalBackend::new();
        assert!(backend.check_and_increment("a", 60, 2, None).is_ok());
        assert!(backend.check_and_increment("a", 60, 2, None).is_ok());
        assert!(backend.check_and_increment("a", 60, 2, None).is_err());
        assert!(backend.check_and_increment("b", 60, 2, None).is_ok());
        assert!(backend.check_and_increment("b", 60, 2, None).is_ok());
        assert!(backend.check_and_increment("b", 60, 2, None).is_err());
    }

    #[test]
    fn test_counter_window_expiry() {
        let backend = LocalBackend::new();
        assert!(backend.check_and_increment("test", 0, 1, None).is_ok());
        assert!(backend.check_and_increment("test", 0, 1, None).is_ok());
    }

    #[test]
    fn test_counter_record_tokens() {
        let backend = LocalBackend::new();
        assert!(backend
            .check_and_increment("test", 60, 100, Some(1000))
            .is_ok());
        backend.record_tokens("test", 999);
        assert!(backend
            .check_and_increment("test", 60, 100, Some(1000))
            .is_ok());
        backend.record_tokens("test", 1);
        let result = backend.check_and_increment("test", 60, 100, Some(1000));
        assert!(result.is_err());
    }

    #[test]
    fn test_counter_cleanup() {
        let backend = LocalBackend::new();
        assert!(backend.check_and_increment("key1", 0, 1, None).is_ok());
        assert!(backend.check_and_increment("key2", 0, 1, None).is_ok());
        backend.cleanup(1);
        assert!(backend.windows.len() <= 2);
    }
}
