//! Quota metric types for extensible rate limiting.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Rate limiting metric type — what dimension to count/limit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaMetric {
    /// Request count — auto-incremented at request phase.
    Requests,
    /// Total tokens (input + output).
    TotalTokens,
    /// Input / prompt tokens only.
    InputTokens,
    /// Output / completion tokens only.
    OutputTokens,
    /// Cost in micro-dollars (1 = $0.000001) — integer arithmetic, no floats.
    CostMicros,
    /// Custom metric — user-defined string key.
    Custom(String),
}

impl QuotaMetric {
    /// Whether this metric is automatically incremented at request phase.
    /// Only `Requests` is pre-check; all others are post-record.
    pub fn is_pre_check(&self) -> bool {
        matches!(self, QuotaMetric::Requests)
    }

    /// Redis/storage key suffix for this metric.
    pub fn key_suffix(&self) -> String {
        match self {
            QuotaMetric::Requests => "requests".to_string(),
            QuotaMetric::TotalTokens => "total_tokens".to_string(),
            QuotaMetric::InputTokens => "input_tokens".to_string(),
            QuotaMetric::OutputTokens => "output_tokens".to_string(),
            QuotaMetric::CostMicros => "cost_micros".to_string(),
            QuotaMetric::Custom(name) => format!("custom_{}", name),
        }
    }
}

impl fmt::Display for QuotaMetric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QuotaMetric::Requests => write!(f, "requests"),
            QuotaMetric::TotalTokens => write!(f, "total_tokens"),
            QuotaMetric::InputTokens => write!(f, "input_tokens"),
            QuotaMetric::OutputTokens => write!(f, "output_tokens"),
            QuotaMetric::CostMicros => write!(f, "cost_micros"),
            QuotaMetric::Custom(name) => write!(f, "custom:{}", name),
        }
    }
}

/// A single quota entry — (metric, limit) pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quota {
    /// Which metric to limit.
    pub metric: QuotaMetric,
    /// Maximum allowed value within the window.
    pub limit: u64,
}

/// Quota exceeded result — detailed information about which quota was hit.
#[derive(Debug)]
pub struct QuotaDenied {
    /// Which metric triggered the denial.
    pub metric: QuotaMetric,
    /// Current value of the metric.
    pub current: u64,
    /// Configured limit.
    pub limit: u64,
    /// Seconds until the window resets.
    pub retry_after_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quota_metric_serde() {
        let json = r#""requests""#;
        let metric: QuotaMetric = serde_json::from_str(json).unwrap();
        assert_eq!(metric, QuotaMetric::Requests);

        let json = r#""total_tokens""#;
        let metric: QuotaMetric = serde_json::from_str(json).unwrap();
        assert_eq!(metric, QuotaMetric::TotalTokens);

        let json = r#"{"custom":"my_metric"}"#;
        let metric: QuotaMetric = serde_json::from_str(json).unwrap();
        assert_eq!(metric, QuotaMetric::Custom("my_metric".to_string()));
    }

    #[test]
    fn test_quota_metric_roundtrip() {
        let metrics = vec![
            QuotaMetric::Requests,
            QuotaMetric::TotalTokens,
            QuotaMetric::InputTokens,
            QuotaMetric::OutputTokens,
            QuotaMetric::CostMicros,
            QuotaMetric::Custom("foo".to_string()),
        ];
        for m in &metrics {
            let json = serde_json::to_string(m).unwrap();
            let back: QuotaMetric = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, m);
        }
    }

    #[test]
    fn test_quota_metric_is_pre_check() {
        assert!(QuotaMetric::Requests.is_pre_check());
        assert!(!QuotaMetric::TotalTokens.is_pre_check());
        assert!(!QuotaMetric::InputTokens.is_pre_check());
        assert!(!QuotaMetric::CostMicros.is_pre_check());
        assert!(!QuotaMetric::Custom("x".to_string()).is_pre_check());
    }

    #[test]
    fn test_quota_metric_key_suffix() {
        assert_eq!(QuotaMetric::Requests.key_suffix(), "requests");
        assert_eq!(QuotaMetric::TotalTokens.key_suffix(), "total_tokens");
        assert_eq!(
            QuotaMetric::Custom("foo".to_string()).key_suffix(),
            "custom_foo"
        );
    }

    #[test]
    fn test_quota_metric_display() {
        assert_eq!(QuotaMetric::Requests.to_string(), "requests");
        assert_eq!(QuotaMetric::TotalTokens.to_string(), "total_tokens");
        assert_eq!(
            QuotaMetric::Custom("bar".to_string()).to_string(),
            "custom:bar"
        );
    }

    #[test]
    fn test_quota_serde() {
        let json = r#"{"metric":"requests","limit":100}"#;
        let quota: Quota = serde_json::from_str(json).unwrap();
        assert_eq!(quota.metric, QuotaMetric::Requests);
        assert_eq!(quota.limit, 100);
    }

    #[test]
    fn test_quota_list_serde() {
        let json = r#"[
            {"metric":"requests","limit":100},
            {"metric":"total_tokens","limit":50000},
            {"metric":"input_tokens","limit":30000}
        ]"#;
        let quotas: Vec<Quota> = serde_json::from_str(json).unwrap();
        assert_eq!(quotas.len(), 3);
        assert_eq!(quotas[0].metric, QuotaMetric::Requests);
        assert_eq!(quotas[1].limit, 50000);
        assert_eq!(quotas[2].metric, QuotaMetric::InputTokens);
    }
}
