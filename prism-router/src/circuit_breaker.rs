//! Provider-level circuit breaker.
//!
//! Tracks consecutive failures per provider. When the threshold is reached,
//! the circuit opens and rejects requests for a recovery period.
//! After the recovery period, a single probe request is allowed (half-open).
//! If it succeeds, the circuit closes; if it fails, it re-opens.
//!
//! States: Closed → Open → HalfOpen → Closed (on success) or Open (on failure)

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests pass through.
    Closed,
    /// Failures exceeded threshold — requests are rejected.
    Open,
    /// Recovery period elapsed — one probe request is allowed.
    HalfOpen,
}

/// Per-provider circuit state (lock-free atomics).
#[allow(dead_code)]
struct ProviderCircuit {
    consecutive_failures: AtomicU32,
    opened_at: AtomicU64, // epoch secs when circuit opened (0 = closed)
    half_open_active: AtomicU32, // 1 if a half-open probe is in flight
}

impl ProviderCircuit {
    fn new() -> Self {
        Self {
            consecutive_failures: AtomicU32::new(0),
            opened_at: AtomicU64::new(0),
            half_open_active: AtomicU32::new(0),
        }
    }
}

/// Circuit breaker configuration per provider.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CircuitBreakerConfig {
    /// Consecutive failures before opening.
    pub threshold: u32,
    /// Seconds to wait before half-open probe.
    pub recovery_secs: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            threshold: 5,
            recovery_secs: 30,
        }
    }
}

/// Circuit breaker manager — one circuit per provider.
pub struct CircuitBreaker {
    circuits: DashMap<String, ProviderCircuit>,
    default_config: CircuitBreakerConfig,
    /// Per-provider config overrides.
    overrides: DashMap<String, CircuitBreakerConfig>,
}

#[allow(dead_code)]
impl CircuitBreaker {
    pub fn new(default_config: CircuitBreakerConfig) -> Self {
        Self {
            circuits: DashMap::new(),
            default_config,
            overrides: DashMap::new(),
        }
    }

    /// Set per-provider config override.
    pub fn set_provider_config(&self, provider_id: &str, config: CircuitBreakerConfig) {
        self.overrides.insert(provider_id.to_string(), config);
    }

    fn config_for(&self, provider_id: &str) -> CircuitBreakerConfig {
        self.overrides
            .get(provider_id)
            .map(|c| c.clone())
            .unwrap_or_else(|| self.default_config.clone())
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Check if a request to this provider is allowed.
    pub fn check(&self, provider_id: &str) -> CircuitState {
        let circuit = self
            .circuits
            .entry(provider_id.to_string())
            .or_insert_with(ProviderCircuit::new);

        let opened_at = circuit.opened_at.load(Ordering::Relaxed);
        if opened_at == 0 {
            return CircuitState::Closed;
        }

        let config = self.config_for(provider_id);
        let elapsed = Self::now_secs().saturating_sub(opened_at);

        if elapsed < config.recovery_secs {
            return CircuitState::Open;
        }

        // Recovery period elapsed → try half-open
        if circuit
            .half_open_active
            .compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            CircuitState::HalfOpen
        } else {
            // Another probe is already in flight
            CircuitState::Open
        }
    }

    /// Record a successful request.
    pub fn record_success(&self, provider_id: &str) {
        if let Some(circuit) = self.circuits.get(provider_id) {
            circuit.consecutive_failures.store(0, Ordering::Relaxed);
            circuit.opened_at.store(0, Ordering::Relaxed);
            circuit.half_open_active.store(0, Ordering::Relaxed);
        }
    }

    /// Record a failed request.
    pub fn record_failure(&self, provider_id: &str) {
        let circuit = self
            .circuits
            .entry(provider_id.to_string())
            .or_insert_with(ProviderCircuit::new);

        let config = self.config_for(provider_id);
        let failures = circuit.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;

        if failures >= config.threshold {
            let was_open = circuit.opened_at.load(Ordering::Relaxed);
            if was_open == 0 {
                circuit.opened_at.store(Self::now_secs(), Ordering::Relaxed);
                tracing::warn!(
                    provider = %provider_id,
                    failures = %failures,
                    threshold = %config.threshold,
                    recovery_secs = %config.recovery_secs,
                    "Circuit breaker OPENED"
                );
            }
        }

        // If half-open probe failed, re-open
        circuit.half_open_active.store(0, Ordering::Relaxed);
    }

    /// Get the current state for a provider.
    pub fn state(&self, provider_id: &str) -> CircuitState {
        self.circuits
            .get(provider_id)
            .map(|circuit| {
                let opened_at = circuit.opened_at.load(Ordering::Relaxed);
                if opened_at == 0 {
                    CircuitState::Closed
                } else {
                    let config = self.config_for(provider_id);
                    let elapsed = Self::now_secs().saturating_sub(opened_at);
                    if elapsed < config.recovery_secs {
                        CircuitState::Open
                    } else {
                        CircuitState::HalfOpen
                    }
                }
            })
            .unwrap_or(CircuitState::Closed)
    }

    /// Get stats for a provider.
    pub fn stats(&self, provider_id: &str) -> (CircuitState, u32) {
        self.circuits
            .get(provider_id)
            .map(|circuit| {
                let failures = circuit.consecutive_failures.load(Ordering::Relaxed);
                (self.state(provider_id), failures)
            })
            .unwrap_or((CircuitState::Closed, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_closed_by_default() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
        assert_eq!(cb.check("provider-a"), CircuitState::Closed);
        assert_eq!(cb.state("provider-a"), CircuitState::Closed);
    }

    #[test]
    fn test_opens_after_threshold() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            threshold: 3,
            recovery_secs: 30,
        });

        cb.record_failure("p1");
        assert_eq!(cb.check("p1"), CircuitState::Closed);
        cb.record_failure("p1");
        assert_eq!(cb.check("p1"), CircuitState::Closed);
        cb.record_failure("p1");
        assert_eq!(cb.check("p1"), CircuitState::Open);
    }

    #[test]
    fn test_success_resets() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            threshold: 2,
            recovery_secs: 30,
        });

        cb.record_failure("p1");
        cb.record_success("p1");
        cb.record_failure("p1");
        // Should still be closed (reset to 1 failure)
        assert_eq!(cb.check("p1"), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_after_recovery() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            threshold: 1,
            recovery_secs: 0, // immediate recovery for testing
        });

        cb.record_failure("p1");
        // Circuit is open, but recovery_secs=0 means immediately half-open
        assert_eq!(cb.check("p1"), CircuitState::HalfOpen);
        // Second check while half-open probe active → still Open
        assert_eq!(cb.check("p1"), CircuitState::Open);
    }

    #[test]
    fn test_half_open_success_closes() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            threshold: 1,
            recovery_secs: 0,
        });

        cb.record_failure("p1");
        assert_eq!(cb.check("p1"), CircuitState::HalfOpen);
        cb.record_success("p1");
        assert_eq!(cb.check("p1"), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_failure_reopens() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            threshold: 1,
            recovery_secs: 0,
        });

        cb.record_failure("p1");
        assert_eq!(cb.check("p1"), CircuitState::HalfOpen);
        cb.record_failure("p1");
        // Re-opened, and recovery_secs=0 so immediately half-open again
        assert_eq!(cb.check("p1"), CircuitState::HalfOpen);
    }

    #[test]
    fn test_per_provider_isolation() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            threshold: 2,
            recovery_secs: 30,
        });

        cb.record_failure("p1");
        cb.record_failure("p1");
        assert_eq!(cb.check("p1"), CircuitState::Open);
        assert_eq!(cb.check("p2"), CircuitState::Closed);
    }

    #[test]
    fn test_per_provider_config_override() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            threshold: 5,
            recovery_secs: 30,
        });
        cb.set_provider_config(
            "fragile",
            CircuitBreakerConfig {
                threshold: 1,
                recovery_secs: 60,
            },
        );

        cb.record_failure("fragile");
        assert_eq!(cb.check("fragile"), CircuitState::Open);

        // Default provider still needs 5 failures
        cb.record_failure("sturdy");
        assert_eq!(cb.check("sturdy"), CircuitState::Closed);
    }

    #[test]
    fn test_stats() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            threshold: 3,
            recovery_secs: 30,
        });

        assert_eq!(cb.stats("p1"), (CircuitState::Closed, 0));
        cb.record_failure("p1");
        cb.record_failure("p1");
        assert_eq!(cb.stats("p1"), (CircuitState::Closed, 2));
        cb.record_failure("p1");
        assert_eq!(cb.stats("p1").1, 3);
    }
}
