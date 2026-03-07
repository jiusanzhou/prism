//! HTTP discovery plugin — polls a remote endpoint for candidates.
//!
//! Designed to work with sidecar processes (e.g. k8s-discovery) that expose
//! model/instance data via HTTP. The endpoint returns a JSON payload matching
//! [`HttpDiscoveryPayload`], which is converted to a [`DiscoveryResult`].

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use prism_core::discovery::{Discovery, DiscoveryError, DiscoveryResult};
use prism_core::types::*;

// ════════════════════════════════════════════════
// Configuration
// ════════════════════════════════════════════════

/// Configuration for an HTTP discovery source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpDiscoveryConfig {
    /// HTTP endpoint URL (e.g. http://localhost:9091/prism).
    pub url: String,
    /// Request timeout.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Optional bearer token for authentication.
    #[serde(default)]
    pub bearer_token: Option<String>,
    /// Optional static headers.
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

fn default_timeout_secs() -> u64 {
    10
}

// ════════════════════════════════════════════════
// Payload types (what the HTTP endpoint returns)
// ════════════════════════════════════════════════

/// JSON payload returned by the HTTP discovery endpoint.
///
/// This is a simplified, JSON-friendly representation of Prism's
/// scheduling objects. The plugin converts these into full Prism types.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HttpDiscoveryPayload {
    #[serde(default)]
    pub providers: Vec<HttpProvider>,
    #[serde(default)]
    pub models: Vec<HttpModel>,
    #[serde(default)]
    pub variants: Vec<HttpVariant>,
    #[serde(default)]
    pub pools: Vec<HttpPool>,
    #[serde(default)]
    pub accounts: Vec<HttpAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpProvider {
    pub id: String,
    pub base_url: String,
    #[serde(default)]
    pub auth_type: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpModel {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpVariant {
    pub id: String,
    pub model_id: String,
    pub provider_id: String,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default = "default_weight")]
    pub weight: f64,
    #[serde(default)]
    pub base_url: Option<String>,
}

fn default_weight() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpPool {
    pub id: String,
    pub name: String,
    pub variants: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpAccount {
    pub id: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub endpoint_id: Option<String>,
}

// ════════════════════════════════════════════════
// Conversion
// ════════════════════════════════════════════════

impl HttpDiscoveryPayload {
    /// Convert to a full [`DiscoveryResult`].
    pub fn into_discovery_result(self) -> DiscoveryResult {
        let mut providers = Vec::new();
        let mut endpoints = Vec::new();
        let mut accounts = Vec::new();

        for p in &self.providers {
            let endpoint_id = format!("{}-ep", p.id);

            let auth = if let Some(ref key) = p.api_key {
                AuthConfig::ApiKey {
                    key: key.clone(),
                    header: "Authorization".to_string(),
                }
            } else {
                AuthConfig::default()
            };

            providers.push(Provider {
                id: p.id.clone(),
                name: p.id.clone(),
                auth: auth.clone(),
                base_url: p.base_url.clone(),
                rate_limit: None,
                status: HealthStatus::Healthy,
                metadata: Default::default(),
                protocols: vec!["openai".to_string()],
            });

            endpoints.push(Endpoint {
                id: endpoint_id.clone(),
                provider_id: p.id.clone(),
                base_url: Some(p.base_url.clone()),
                region: None,
                auth_override: None,
                rate_limit: None,
                priority: 0,
                status: HealthStatus::Healthy,
            });

            // Default account for this provider
            accounts.push(Account {
                id: format!("{}-acct", p.id),
                endpoint_id: endpoint_id.clone(),
                auth,
                project_id: None,
                rate_limit: None,
                billing_tag: None,
                weight: 1.0,
                status: HealthStatus::Healthy,
                metadata: Default::default(),
            });
        }

        // Additional explicit accounts
        for a in &self.accounts {
            let ep_id = a.endpoint_id.clone().unwrap_or_else(|| {
                // Guess endpoint from account id
                format!("{}-ep", a.id.trim_end_matches("-acct"))
            });
            let auth = if let Some(ref key) = a.api_key {
                AuthConfig::ApiKey {
                    key: key.clone(),
                    header: "Authorization".to_string(),
                }
            } else {
                AuthConfig::default()
            };
            accounts.push(Account {
                id: a.id.clone(),
                endpoint_id: ep_id,
                auth,
                project_id: None,
                rate_limit: None,
                billing_tag: None,
                weight: 1.0,
                status: HealthStatus::Healthy,
                metadata: Default::default(),
            });
        }

        let models: Vec<Model> = self
            .models
            .iter()
            .map(|m| Model {
                id: m.id.clone(),
                display_name: m.display_name.clone().unwrap_or_else(|| m.id.clone()),
                quality_tier: QualityTier::B,
                capabilities: ModelCapabilities::default(),
                pricing: ModelPricing::default(),
                specialties: vec![],
                limits: None,
                status: ModelStatus::Available,
            })
            .collect();

        let variants: Vec<ModelVariant> = self
            .variants
            .iter()
            .map(|v| {
                let endpoint_id = format!("{}-ep", v.provider_id);
                ModelVariant {
                    id: v.id.clone(),
                    model_id: v.model_id.clone(),
                    endpoint_id,
                    account_id: v.account_id.clone(),
                    deployment_name: None,
                    weight: v.weight as f32,
                    performance: ModelPerformance::default(),
                    pricing_override: None,
                    status: VariantStatus::Active,
                    base_url: v.base_url.clone(),
                    upstream_model_name: None,
                }
            })
            .collect();

        let pools: Vec<ModelPool> = self
            .pools
            .iter()
            .map(|p| ModelPool {
                id: p.id.clone(),
                name: p.name.clone(),
                variants: p.variants.clone(),
                selection_mode: SelectionMode::Scored,
                description: None,
            })
            .collect();

        DiscoveryResult {
            providers,
            endpoints,
            accounts,
            models,
            variants,
            pools,
        }
    }
}

// ════════════════════════════════════════════════
// HttpDiscovery
// ════════════════════════════════════════════════

/// HTTP-based discovery plugin.
///
/// Synchronously fetches JSON from a remote endpoint and converts it
/// to Prism scheduling objects.
pub struct HttpDiscovery {
    config: HttpDiscoveryConfig,
    client: reqwest::blocking::Client,
}

impl HttpDiscovery {
    /// Create a new HTTP discovery plugin.
    pub fn new(config: HttpDiscoveryConfig) -> Result<Self, String> {
        let mut builder = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs));

        if let Some(ref token) = config.bearer_token {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::AUTHORIZATION,
                reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token))
                    .map_err(|e| format!("invalid bearer token: {}", e))?,
            );
            builder = builder.default_headers(headers);
        }

        let client = builder
            .build()
            .map_err(|e| format!("build http client: {}", e))?;

        Ok(Self { config, client })
    }
}

impl Discovery for HttpDiscovery {
    fn name(&self) -> &str {
        "http"
    }

    fn discover(&self) -> Result<DiscoveryResult, DiscoveryError> {
        let mut req = self.client.get(&self.config.url);

        for (k, v) in &self.config.headers {
            req = req.header(k, v);
        }

        let resp = req
            .send()
            .map_err(|e| DiscoveryError::ConnectionFailed(format!("{}: {}", self.config.url, e)))?;

        if !resp.status().is_success() {
            return Err(DiscoveryError::ConnectionFailed(format!(
                "{}: HTTP {}",
                self.config.url,
                resp.status()
            )));
        }

        let payload: HttpDiscoveryPayload = resp
            .json()
            .map_err(|e| DiscoveryError::ParseError(format!("json decode: {}", e)))?;

        Ok(payload.into_discovery_result())
    }
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_payload_to_discovery_result() {
        let payload = HttpDiscoveryPayload::default();
        let result = payload.into_discovery_result();
        assert!(result.is_empty());
    }

    #[test]
    fn test_payload_conversion_single_provider() {
        let payload = HttpDiscoveryPayload {
            providers: vec![HttpProvider {
                id: "vllm-cluster1".into(),
                base_url: "http://10.0.1.5:8000".into(),
                auth_type: None,
                api_key: Some("test-key".into()),
                headers: Default::default(),
            }],
            models: vec![HttpModel {
                id: "llama-3-70b".into(),
                display_name: Some("Llama 3 70B".into()),
            }],
            variants: vec![HttpVariant {
                id: "llama-3-70b-vllm-0".into(),
                model_id: "llama-3-70b".into(),
                provider_id: "vllm-cluster1".into(),
                account_id: None,
                weight: 1.0,
                base_url: Some("http://10.0.1.5:8000".into()),
            }],
            pools: vec![HttpPool {
                id: "pool-llama-3-70b".into(),
                name: "llama-3-70b".into(),
                variants: vec!["llama-3-70b-vllm-0".into()],
            }],
            accounts: vec![],
        };

        let result = payload.into_discovery_result();

        assert_eq!(result.providers.len(), 1);
        assert_eq!(result.providers[0].id, "vllm-cluster1");
        assert_eq!(result.providers[0].base_url, "http://10.0.1.5:8000");

        assert_eq!(result.endpoints.len(), 1);
        assert_eq!(result.endpoints[0].id, "vllm-cluster1-ep");

        assert_eq!(result.accounts.len(), 1);

        assert_eq!(result.models.len(), 1);
        assert_eq!(result.models[0].display_name, "Llama 3 70B");

        assert_eq!(result.variants.len(), 1);
        assert_eq!(result.variants[0].endpoint_id, "vllm-cluster1-ep");
        assert_eq!(
            result.variants[0].base_url,
            Some("http://10.0.1.5:8000".into())
        );

        assert_eq!(result.pools.len(), 1);
        assert_eq!(result.pools[0].variants, vec!["llama-3-70b-vllm-0"]);
    }

    #[test]
    fn test_payload_multi_cluster_multi_model() {
        let payload = HttpDiscoveryPayload {
            providers: vec![
                HttpProvider {
                    id: "k8s-cluster-a".into(),
                    base_url: "http://10.0.1.1:8000".into(),
                    auth_type: None,
                    api_key: None,
                    headers: Default::default(),
                },
                HttpProvider {
                    id: "k8s-cluster-b".into(),
                    base_url: "http://10.0.2.1:8000".into(),
                    auth_type: None,
                    api_key: None,
                    headers: Default::default(),
                },
            ],
            models: vec![
                HttpModel {
                    id: "qwen-72b".into(),
                    display_name: None,
                },
                HttpModel {
                    id: "deepseek-v3".into(),
                    display_name: None,
                },
            ],
            variants: vec![
                HttpVariant {
                    id: "qwen-72b-a-0".into(),
                    model_id: "qwen-72b".into(),
                    provider_id: "k8s-cluster-a".into(),
                    account_id: None,
                    weight: 1.0,
                    base_url: Some("http://10.0.1.10:8000".into()),
                },
                HttpVariant {
                    id: "qwen-72b-b-0".into(),
                    model_id: "qwen-72b".into(),
                    provider_id: "k8s-cluster-b".into(),
                    account_id: None,
                    weight: 1.0,
                    base_url: Some("http://10.0.2.10:8000".into()),
                },
                HttpVariant {
                    id: "deepseek-v3-a-0".into(),
                    model_id: "deepseek-v3".into(),
                    provider_id: "k8s-cluster-a".into(),
                    account_id: None,
                    weight: 1.0,
                    base_url: Some("http://10.0.1.20:8000".into()),
                },
            ],
            pools: vec![
                HttpPool {
                    id: "pool-qwen".into(),
                    name: "qwen-72b".into(),
                    variants: vec!["qwen-72b-a-0".into(), "qwen-72b-b-0".into()],
                },
                HttpPool {
                    id: "pool-deepseek".into(),
                    name: "deepseek-v3".into(),
                    variants: vec!["deepseek-v3-a-0".into()],
                },
            ],
            accounts: vec![],
        };

        let result = payload.into_discovery_result();

        assert_eq!(result.providers.len(), 2);
        assert_eq!(result.endpoints.len(), 2);
        assert_eq!(result.models.len(), 2);
        assert_eq!(result.variants.len(), 3);
        assert_eq!(result.pools.len(), 2);

        // Models without display_name should use id as name
        assert_eq!(result.models[0].display_name, "qwen-72b");
    }

    #[test]
    fn test_payload_with_explicit_accounts() {
        let payload = HttpDiscoveryPayload {
            providers: vec![HttpProvider {
                id: "internal".into(),
                base_url: "http://gateway:8080".into(),
                auth_type: None,
                api_key: None,
                headers: Default::default(),
            }],
            models: vec![],
            variants: vec![],
            pools: vec![],
            accounts: vec![HttpAccount {
                id: "team-a".into(),
                api_key: Some("sk-team-a-xxx".into()),
                endpoint_id: Some("internal-ep".into()),
            }],
        };

        let result = payload.into_discovery_result();

        // 1 default account from provider + 1 explicit
        assert_eq!(result.accounts.len(), 2);
        assert_eq!(result.accounts[1].id, "team-a");
        match &result.accounts[1].auth {
            AuthConfig::ApiKey { key, .. } => assert_eq!(key, "sk-team-a-xxx"),
            _ => panic!("expected ApiKey auth"),
        }
    }

    #[test]
    fn test_provider_auth_passthrough() {
        let payload = HttpDiscoveryPayload {
            providers: vec![
                HttpProvider {
                    id: "with-key".into(),
                    base_url: "http://a".into(),
                    auth_type: None,
                    api_key: Some("secret".into()),
                    headers: Default::default(),
                },
                HttpProvider {
                    id: "no-key".into(),
                    base_url: "http://b".into(),
                    auth_type: None,
                    api_key: None,
                    headers: Default::default(),
                },
            ],
            ..Default::default()
        };

        let result = payload.into_discovery_result();

        match &result.providers[0].auth {
            AuthConfig::ApiKey { key, .. } => assert_eq!(key, "secret"),
            _ => panic!("expected ApiKey"),
        }
        assert!(matches!(result.providers[1].auth, AuthConfig::ApiKey { ref key, .. } if key.is_empty()));
    }

    #[test]
    fn test_default_weight() {
        let v = HttpVariant {
            id: "v1".into(),
            model_id: "m1".into(),
            provider_id: "p1".into(),
            account_id: None,
            weight: default_weight(),
            base_url: None,
        };
        assert!((v.weight - 1.0).abs() < f64::EPSILON);
    }
}
