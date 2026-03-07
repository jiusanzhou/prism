//! Active health checking for upstream endpoints.
//!
//! Periodically probes endpoints to detect availability. Works alongside
//! the circuit breaker (passive) — circuit breaker tracks per-request
//! failures, health checker does background probing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::{debug, info, warn};

/// Health status of an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointHealthStatus {
    Healthy,
    Unhealthy,
    Unknown,
}

/// Health state tracked per endpoint.
#[derive(Debug, Clone)]
struct EndpointState {
    status: EndpointHealthStatus,
    consecutive_failures: u32,
    last_check: Option<Instant>,
    last_success: Option<Instant>,
    last_failure_reason: Option<String>,
}

impl Default for EndpointState {
    fn default() -> Self {
        Self {
            status: EndpointHealthStatus::Unknown,
            consecutive_failures: 0,
            last_check: None,
            last_success: None,
            last_failure_reason: None,
        }
    }
}

/// Health checker configuration.
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    /// Probe interval for healthy endpoints.
    pub interval: Duration,
    /// Probe interval for unhealthy endpoints (faster recovery detection).
    pub unhealthy_interval: Duration,
    /// Consecutive failures before marking unhealthy.
    pub unhealthy_threshold: u32,
    /// Consecutive successes before marking healthy again.
    pub healthy_threshold: u32,
    /// HTTP timeout for health check requests.
    pub timeout: Duration,
    /// Health check path (appended to base_url).
    pub path: String,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            unhealthy_interval: Duration::from_secs(10),
            unhealthy_threshold: 3,
            healthy_threshold: 1,
            timeout: Duration::from_secs(5),
            path: "/v1/models".to_string(),
        }
    }
}

/// Endpoint info needed for health checking.
#[derive(Debug, Clone)]
pub struct EndpointInfo {
    pub id: String,
    pub provider_id: String,
    pub base_url: String,
    pub auth_header: Option<(String, String)>,
}

/// Active health checker — tracks endpoint health and runs background probes.
pub struct HealthChecker {
    states: DashMap<String, EndpointState>,
    config: HealthCheckConfig,
}

impl HealthChecker {
    pub fn new(config: HealthCheckConfig) -> Self {
        Self {
            states: DashMap::new(),
            config,
        }
    }

    /// Check if an endpoint is healthy (fast, lock-free read).
    pub fn is_healthy(&self, endpoint_id: &str) -> bool {
        self.states
            .get(endpoint_id)
            .map(|s| s.status != EndpointHealthStatus::Unhealthy)
            .unwrap_or(true) // unknown = healthy (optimistic)
    }

    /// Record a successful request to an endpoint (passive feedback).
    pub fn record_success(&self, endpoint_id: &str) {
        self.states
            .entry(endpoint_id.to_string())
            .and_modify(|s| {
                s.consecutive_failures = 0;
                s.last_success = Some(Instant::now());
                if s.status == EndpointHealthStatus::Unhealthy {
                    s.status = EndpointHealthStatus::Healthy;
                    info!(endpoint = %endpoint_id, "Endpoint recovered (passive)");
                }
            })
            .or_insert_with(|| {
                let mut s = EndpointState::default();
                s.status = EndpointHealthStatus::Healthy;
                s.last_success = Some(Instant::now());
                s
            });
    }

    /// Record a failed request to an endpoint (passive feedback).
    pub fn record_failure(&self, endpoint_id: &str, reason: &str) {
        let threshold = self.config.unhealthy_threshold;
        self.states
            .entry(endpoint_id.to_string())
            .and_modify(|s| {
                s.consecutive_failures += 1;
                s.last_check = Some(Instant::now());
                s.last_failure_reason = Some(reason.to_string());
                if s.consecutive_failures >= threshold
                    && s.status != EndpointHealthStatus::Unhealthy
                {
                    s.status = EndpointHealthStatus::Unhealthy;
                    warn!(
                        endpoint = %endpoint_id,
                        failures = %s.consecutive_failures,
                        reason = %reason,
                        "Endpoint marked UNHEALTHY"
                    );
                }
            })
            .or_insert_with(|| {
                let mut s = EndpointState::default();
                s.consecutive_failures = 1;
                s.last_check = Some(Instant::now());
                s.last_failure_reason = Some(reason.to_string());
                if 1 >= threshold {
                    s.status = EndpointHealthStatus::Unhealthy;
                    warn!(
                        endpoint = %endpoint_id,
                        reason = %reason,
                        "Endpoint marked UNHEALTHY on first failure"
                    );
                }
                s
            });
    }

    /// Get summary of all endpoint health states.
    pub fn summary(&self) -> Vec<(String, EndpointHealthStatus, u32)> {
        self.states
            .iter()
            .map(|e| (e.key().clone(), e.value().status, e.value().consecutive_failures))
            .collect()
    }

    /// Run one round of active health checks.
    pub async fn check_all(&self, endpoints: &[EndpointInfo]) {
        let client = reqwest::Client::builder()
            .timeout(self.config.timeout)
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap_or_default();

        for ep in endpoints {
            let should_probe = self
                .states
                .get(&ep.id)
                .map(|s| {
                    let interval = if s.status == EndpointHealthStatus::Unhealthy {
                        self.config.unhealthy_interval
                    } else {
                        self.config.interval
                    };
                    s.status == EndpointHealthStatus::Unhealthy
                        || s.status == EndpointHealthStatus::Unknown
                        || s.last_check.map(|t| t.elapsed() > interval).unwrap_or(true)
                })
                .unwrap_or(true);

            if !should_probe {
                continue;
            }

            let url = format!("{}{}", ep.base_url.trim_end_matches('/'), &self.config.path);

            let mut req = client.get(&url);
            if let Some((ref header, ref value)) = ep.auth_header {
                req = req.header(header.as_str(), value.as_str());
            }

            let result: Result<reqwest::Response, reqwest::Error> = req.send().await;
            match result {
                Ok(resp) => {
                    let status_code = resp.status();
                    if status_code.is_success() || status_code.as_u16() == 401 {
                        // 401 = alive (auth issue, not down)
                        self.record_success(&ep.id);
                        debug!(endpoint = %ep.id, status = %status_code, "Health check passed");
                    } else {
                        self.record_failure(&ep.id, &format!("HTTP {}", status_code));
                        debug!(endpoint = %ep.id, status = %status_code, "Health check failed");
                    }
                }
                Err(e) => {
                    let reason = if e.is_timeout() {
                        "timeout".to_string()
                    } else if e.is_connect() {
                        "connection refused".to_string()
                    } else {
                        format!("{}", e)
                    };
                    self.record_failure(&ep.id, &reason);
                    debug!(endpoint = %ep.id, error = %e, "Health check error");
                }
            }
        }
    }

    /// Spawn the background health check loop.
    pub fn spawn_checker(
        self: Arc<Self>,
        endpoints: Vec<EndpointInfo>,
        interval: Duration,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!(
                endpoint_count = endpoints.len(),
                interval_secs = interval.as_secs(),
                "Health checker started"
            );
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {
                        self.check_all(&endpoints).await;
                        let unhealthy: Vec<_> = self.summary().into_iter()
                            .filter(|(_, s, _)| *s == EndpointHealthStatus::Unhealthy)
                            .collect();
                        if !unhealthy.is_empty() {
                            warn!(
                                count = unhealthy.len(),
                                endpoints = ?unhealthy.iter().map(|(id, _, f)| format!("{}({})", id, f)).collect::<Vec<_>>(),
                                "Unhealthy endpoints"
                            );
                        }
                    }
                    _ = shutdown.changed() => {
                        info!("Health checker stopping");
                        break;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_healthy() {
        let hc = HealthChecker::new(HealthCheckConfig::default());
        assert!(hc.is_healthy("unknown-endpoint"));
    }

    #[test]
    fn test_record_failure_threshold() {
        let config = HealthCheckConfig { unhealthy_threshold: 3, ..Default::default() };
        let hc = HealthChecker::new(config);

        hc.record_failure("ep1", "timeout");
        assert!(hc.is_healthy("ep1"));
        hc.record_failure("ep1", "timeout");
        assert!(hc.is_healthy("ep1"));
        hc.record_failure("ep1", "timeout");
        assert!(!hc.is_healthy("ep1"));
    }

    #[test]
    fn test_record_success_recovery() {
        let config = HealthCheckConfig { unhealthy_threshold: 1, ..Default::default() };
        let hc = HealthChecker::new(config);

        hc.record_failure("ep1", "500");
        assert!(!hc.is_healthy("ep1"));
        hc.record_success("ep1");
        assert!(hc.is_healthy("ep1"));
    }

    #[test]
    fn test_failure_count_resets_on_success() {
        let config = HealthCheckConfig { unhealthy_threshold: 3, ..Default::default() };
        let hc = HealthChecker::new(config);

        hc.record_failure("ep1", "500");
        hc.record_failure("ep1", "500");
        hc.record_success("ep1");
        hc.record_failure("ep1", "500");
        hc.record_failure("ep1", "500");
        assert!(hc.is_healthy("ep1")); // only 2 after reset
    }

    #[test]
    fn test_summary() {
        let config = HealthCheckConfig { unhealthy_threshold: 2, ..Default::default() };
        let hc = HealthChecker::new(config);

        hc.record_success("ep1");
        hc.record_failure("ep2", "timeout");
        hc.record_failure("ep2", "timeout");

        let summary = hc.summary();
        assert_eq!(summary.len(), 2);

        let ep1 = summary.iter().find(|(id, _, _)| id == "ep1").unwrap();
        assert_eq!(ep1.1, EndpointHealthStatus::Healthy);

        let ep2 = summary.iter().find(|(id, _, _)| id == "ep2").unwrap();
        assert_eq!(ep2.1, EndpointHealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn test_check_all_with_mock() {
        let app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(|| async { "ok" }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.ok(); });
        tokio::time::sleep(Duration::from_millis(10)).await;

        let hc = HealthChecker::new(HealthCheckConfig {
            timeout: Duration::from_secs(2),
            ..Default::default()
        });

        let endpoints = vec![
            EndpointInfo {
                id: "good-ep".into(),
                provider_id: "test".into(),
                base_url: format!("http://127.0.0.1:{}", port),
                auth_header: None,
            },
            EndpointInfo {
                id: "bad-ep".into(),
                provider_id: "test".into(),
                base_url: "http://127.0.0.1:1".into(),
                auth_header: None,
            },
        ];

        hc.check_all(&endpoints).await;
        assert!(hc.is_healthy("good-ep"));

        let summary = hc.summary();
        let bad = summary.iter().find(|(id, _, _)| id == "bad-ep").unwrap();
        assert_eq!(bad.2, 1); // 1 failure
    }
}
