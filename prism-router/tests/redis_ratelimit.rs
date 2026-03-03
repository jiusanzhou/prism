//! Integration tests for RedisBackend rate limiting.
//!
//! Requires a running Redis instance. Set REDIS_URL env var or defaults to
//! redis://:myredissecret@127.0.0.1:6379
//!
//! Run with: cargo test --features redis --test redis_ratelimit

#![cfg(feature = "redis")]

use std::sync::Arc;

use prism_router::ratelimit::{
    DegradationPolicy, LocalBackend, RateLimitBackend, RateLimitRule, RateLimiter, RedisBackend,
    RedisConfig,
};

fn redis_url() -> String {
    std::env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://:myredissecret@127.0.0.1:6379".to_string())
}

fn unique_prefix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("prism:test:{}", ts)
}

async fn make_backend(policy: DegradationPolicy) -> RedisBackend {
    let config = RedisConfig {
        url: redis_url(),
        key_prefix: unique_prefix(),
        degradation_policy: policy,
    };
    RedisBackend::new(&config)
        .await
        .expect("Failed to connect to Redis — is it running?")
}

fn make_rule(
    name: &str,
    key_headers: Vec<&str>,
    window_secs: u64,
    max_requests: u64,
) -> RateLimitRule {
    RateLimitRule {
        name: name.to_string(),
        key_headers: key_headers.into_iter().map(|s| s.to_string()).collect(),
        window_secs,
        max_requests,
        max_tokens: None,
        overrides: vec![],
        enabled: true,
        status_code: 429,
        message: "Rate limit exceeded".to_string(),
    }
}

fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// ════════════════════════════════════════════════
// RedisBackend trait tests
// ════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_redis_backend_name() {
    let backend = make_backend(DegradationPolicy::Local).await;
    assert_eq!(backend.name(), "redis");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_redis_backend_healthy() {
    let backend = make_backend(DegradationPolicy::Local).await;
    assert!(backend.is_healthy());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_redis_check_within_limit() {
    let backend = make_backend(DegradationPolicy::Local).await;
    for i in 0..5 {
        let result = backend.check_and_increment("test_within", 60, 5, None);
        assert!(result.is_ok(), "request {} should pass", i);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_redis_check_exceeds_limit() {
    let backend = make_backend(DegradationPolicy::Local).await;
    for _ in 0..3 {
        assert!(backend
            .check_and_increment("test_exceed", 60, 3, None)
            .is_ok());
    }
    let result = backend.check_and_increment("test_exceed", 60, 3, None);
    assert!(result.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_redis_independent_keys() {
    let backend = make_backend(DegradationPolicy::Local).await;

    assert!(backend.check_and_increment("indep_a", 60, 2, None).is_ok());
    assert!(backend.check_and_increment("indep_a", 60, 2, None).is_ok());
    assert!(backend.check_and_increment("indep_a", 60, 2, None).is_err());

    assert!(backend.check_and_increment("indep_b", 60, 2, None).is_ok());
    assert!(backend.check_and_increment("indep_b", 60, 2, None).is_ok());
    assert!(backend.check_and_increment("indep_b", 60, 2, None).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_redis_token_recording() {
    let backend = make_backend(DegradationPolicy::Local).await;

    assert!(backend
        .check_and_increment("tok_test", 60, 100, Some(1000))
        .is_ok());

    backend.record_tokens("tok_test", 999);

    assert!(backend
        .check_and_increment("tok_test", 60, 100, Some(1000))
        .is_ok());

    backend.record_tokens("tok_test", 1);

    let result = backend.check_and_increment("tok_test", 60, 100, Some(1000));
    assert!(result.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_redis_cleanup_no_panic() {
    let backend = make_backend(DegradationPolicy::Local).await;
    assert!(backend.check_and_increment("cleanup", 60, 10, None).is_ok());
    backend.cleanup(60);
}

// ════════════════════════════════════════════════
// RateLimiter + RedisBackend integration
// ════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_limiter_with_redis_backend() {
    let backend = Arc::new(make_backend(DegradationPolicy::Local).await);
    let limiter = RateLimiter::with_backend(
        vec![make_rule("redis_global", vec![], 60, 3)],
        backend,
    );

    assert_eq!(limiter.backend_name(), "redis");
    assert!(limiter.is_backend_healthy());

    assert!(limiter.check(&headers(&[])).is_ok());
    assert!(limiter.check(&headers(&[])).is_ok());
    assert!(limiter.check(&headers(&[])).is_ok());
    let denied = limiter.check(&headers(&[])).unwrap_err();
    assert_eq!(denied.rule_name, "redis_global");
    assert_eq!(denied.status_code, 429);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_limiter_redis_per_model() {
    let backend = Arc::new(make_backend(DegradationPolicy::Local).await);
    let limiter = RateLimiter::with_backend(
        vec![make_rule("redis_model", vec!["x-model"], 60, 2)],
        backend,
    );

    let h_gpt4 = headers(&[("x-model", "gpt-4")]);
    let h_claude = headers(&[("x-model", "claude-3")]);

    assert!(limiter.check(&h_gpt4).is_ok());
    assert!(limiter.check(&h_gpt4).is_ok());
    assert!(limiter.check(&h_gpt4).is_err());

    assert!(limiter.check(&h_claude).is_ok());
    assert!(limiter.check(&h_claude).is_ok());
    assert!(limiter.check(&h_claude).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_limiter_redis_token_limit() {
    let backend = Arc::new(make_backend(DegradationPolicy::Local).await);
    let mut rule = make_rule("redis_tok", vec!["x-app-id"], 60, 100);
    rule.max_tokens = Some(500);

    let limiter = RateLimiter::with_backend(vec![rule], backend);
    let h = headers(&[("x-app-id", "app-1")]);

    assert!(limiter.check(&h).is_ok());
    limiter.record_tokens("redis_tok", &h, 499);
    assert!(limiter.check(&h).is_ok());
    limiter.record_tokens("redis_tok", &h, 1);
    assert!(limiter.check(&h).is_err());
}

// ════════════════════════════════════════════════
// Degradation policy tests
// ════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_degradation_bad_url_fails_to_connect() {
    // Use a definitely-invalid URL (not a hanging TCP — parse error)
    let config = RedisConfig {
        url: "redis://this-host-does-not-exist.invalid:6379".to_string(),
        key_prefix: unique_prefix(),
        degradation_policy: DegradationPolicy::Local,
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        RedisBackend::new(&config),
    )
    .await;

    // Either connection fails or times out — both are acceptable
    match result {
        Ok(Err(_)) => {} // Connection error — expected
        Err(_) => {}     // Timeout — also acceptable
        Ok(Ok(_)) => panic!("Should not connect to invalid host"),
    }
}

// ════════════════════════════════════════════════
// Behavioral parity with LocalBackend
// ════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_redis_parity_with_local() {
    let redis = Arc::new(make_backend(DegradationPolicy::Local).await);
    let local = Arc::new(LocalBackend::new());

    let rules = vec![make_rule("parity", vec!["x-key"], 60, 5)];

    let redis_limiter = RateLimiter::with_backend(rules.clone(), redis);
    let local_limiter = RateLimiter::with_backend(rules, local);

    let h = headers(&[("x-key", "test")]);

    for _ in 0..5 {
        assert!(redis_limiter.check(&h).is_ok());
        assert!(local_limiter.check(&h).is_ok());
    }

    assert!(redis_limiter.check(&h).is_err());
    assert!(local_limiter.check(&h).is_err());
}

// ════════════════════════════════════════════════
// Multi-instance simulation
// ════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_redis_shared_counter_across_instances() {
    // Simulate two Prism instances sharing the same Redis
    let prefix = unique_prefix();

    let config1 = RedisConfig {
        url: redis_url(),
        key_prefix: prefix.clone(),
        degradation_policy: DegradationPolicy::Local,
    };
    let config2 = RedisConfig {
        url: redis_url(),
        key_prefix: prefix,
        degradation_policy: DegradationPolicy::Local,
    };

    let backend1 = Arc::new(RedisBackend::new(&config1).await.unwrap());
    let backend2 = Arc::new(RedisBackend::new(&config2).await.unwrap());

    let rules = vec![make_rule("shared", vec![], 60, 4)];

    let limiter1 = RateLimiter::with_backend(rules.clone(), backend1);
    let limiter2 = RateLimiter::with_backend(rules, backend2);

    let h = headers(&[]);

    // Instance 1 uses 2 slots
    assert!(limiter1.check(&h).is_ok());
    assert!(limiter1.check(&h).is_ok());

    // Instance 2 uses 2 slots — total now 4
    assert!(limiter2.check(&h).is_ok());
    assert!(limiter2.check(&h).is_ok());

    // Both should be denied — shared counter reached limit
    assert!(limiter1.check(&h).is_err());
    assert!(limiter2.check(&h).is_err());
}
