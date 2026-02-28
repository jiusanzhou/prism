//! Pluggable Discovery Layer for candidate sources.
//!
//! This module defines the [`Discovery`] trait — an abstraction for discovering
//! scheduling objects (providers, endpoints, accounts, models, variants, pools)
//! from various sources. The default implementation is [`StaticDiscovery`],
//! which loads candidates from a static configuration.
//!
//! ## Extension Points
//!
//! - **StaticDiscovery** — loads from in-memory config structs (default).
//! - **WASM Discovery** — user-supplied WASM modules (gated behind `wasm` feature).
//! - Custom implementations via the [`Discovery`] trait.

pub mod static_discovery;
#[cfg(feature = "wasm")]
pub mod wasm_discovery;

pub use static_discovery::StaticDiscovery;
#[cfg(feature = "wasm")]
pub use wasm_discovery::WasmDiscovery;

use std::fmt;

use crate::types::*;

// ════════════════════════════════════════════════
// DiscoveryResult
// ════════════════════════════════════════════════

/// The complete result set returned by a [`Discovery`] plugin.
///
/// Contains all scheduling objects discovered from a single source.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryResult {
    pub providers: Vec<Provider>,
    pub endpoints: Vec<Endpoint>,
    pub accounts: Vec<Account>,
    pub models: Vec<Model>,
    pub variants: Vec<ModelVariant>,
    pub pools: Vec<ModelPool>,
}

impl DiscoveryResult {
    /// Create an empty result.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Total number of objects across all categories.
    pub fn total_objects(&self) -> usize {
        self.providers.len()
            + self.endpoints.len()
            + self.accounts.len()
            + self.models.len()
            + self.variants.len()
            + self.pools.len()
    }

    /// Whether this result contains no objects at all.
    pub fn is_empty(&self) -> bool {
        self.total_objects() == 0
    }
}

// ════════════════════════════════════════════════
// DiscoveryError
// ════════════════════════════════════════════════

/// Errors that can occur during discovery.
#[derive(Debug, Clone)]
pub enum DiscoveryError {
    /// The requested operation is not supported by this plugin.
    NotSupported(String),
    /// Failed to connect to the discovery source.
    ConnectionFailed(String),
    /// Failed to parse discovery data.
    ParseError(String),
    /// Any other error.
    Other(String),
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiscoveryError::NotSupported(msg) => write!(f, "not supported: {}", msg),
            DiscoveryError::ConnectionFailed(msg) => write!(f, "connection failed: {}", msg),
            DiscoveryError::ParseError(msg) => write!(f, "parse error: {}", msg),
            DiscoveryError::Other(msg) => write!(f, "discovery error: {}", msg),
        }
    }
}

impl std::error::Error for DiscoveryError {}

// ════════════════════════════════════════════════
// DiscoveryEvent
// ════════════════════════════════════════════════

/// Events emitted by discovery plugins that support incremental updates.
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    /// Full replacement of all discovered objects.
    FullSync(DiscoveryResult),
    /// A variant was added or updated.
    VariantUpsert(ModelVariant),
    /// A variant was removed (by id).
    VariantRemove(String),
    // Future extensions: ProviderUpsert, ModelUpsert, EndpointUpsert, etc.
}

// ════════════════════════════════════════════════
// Discovery trait
// ════════════════════════════════════════════════

/// Candidate discovery plugin trait.
///
/// Implementations provide a way to discover scheduling objects from various
/// sources — static config files, databases, Kubernetes, WASM modules, etc.
///
/// All methods have sensible defaults except [`Discovery::name`] and
/// [`Discovery::discover`], which must be implemented.
pub trait Discovery: Send + Sync {
    /// Plugin name, used for logging and diagnostics.
    fn name(&self) -> &str;

    /// Execute discovery and return all scheduling objects from this source.
    fn discover(&self) -> Result<DiscoveryResult, DiscoveryError>;

    /// Whether this plugin supports incremental watch updates.
    ///
    /// When `true`, the framework may call [`Discovery::watch`] instead of
    /// polling [`Discovery::discover`] repeatedly.
    fn supports_watch(&self) -> bool {
        false
    }

    /// Start watching for incremental updates.
    ///
    /// The callback is invoked whenever the discovery source detects changes.
    /// Only called when [`Discovery::supports_watch`] returns `true`.
    fn watch(
        &self,
        _callback: Box<dyn Fn(DiscoveryEvent) + Send + Sync>,
    ) -> Result<(), DiscoveryError> {
        Err(DiscoveryError::NotSupported(
            "watch not supported".into(),
        ))
    }
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──

    fn make_provider(id: &str) -> Provider {
        Provider {
            id: id.to_string(),
            name: id.to_string(),
            auth: AuthConfig::default(),
            base_url: format!("https://{}.example.com", id),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: Default::default(),
            protocols: vec![],
        }
    }

    fn make_endpoint(id: &str, provider_id: &str) -> Endpoint {
        Endpoint {
            id: id.to_string(),
            provider_id: provider_id.to_string(),
            base_url: None,
            region: None,
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        }
    }

    fn make_account(id: &str, endpoint_id: &str) -> Account {
        Account {
            id: id.to_string(),
            endpoint_id: endpoint_id.to_string(),
            auth: AuthConfig::default(),
            project_id: None,
            rate_limit: None,
            billing_tag: None,
            weight: 1.0,
            status: HealthStatus::Healthy,
            metadata: Default::default(),
        }
    }

    fn make_model(id: &str, tier: QualityTier) -> Model {
        Model {
            id: id.to_string(),
            display_name: id.to_string(),
            quality_tier: tier,
            capabilities: ModelCapabilities::default(),
            pricing: ModelPricing::default(),
            specialties: vec![],
            limits: None,
            status: ModelStatus::Available,
        }
    }

    fn make_variant(id: &str, model_id: &str, endpoint_id: &str) -> ModelVariant {
        ModelVariant {
            id: id.to_string(),
            model_id: model_id.to_string(),
            endpoint_id: endpoint_id.to_string(),
            account_id: None,
            deployment_name: None,
            weight: 1.0,
            performance: ModelPerformance::default(),
            pricing_override: None,
            status: VariantStatus::Active,
        }
    }

    fn make_pool(id: &str, variant_ids: Vec<&str>) -> ModelPool {
        ModelPool {
            id: id.to_string(),
            name: id.to_string(),
            variants: variant_ids.into_iter().map(String::from).collect(),
            selection_mode: SelectionMode::Scored,
            description: None,
        }
    }

    // ── A minimal mock discovery for testing ──

    struct MockDiscovery {
        name: String,
        result: DiscoveryResult,
    }

    impl Discovery for MockDiscovery {
        fn name(&self) -> &str {
            &self.name
        }

        fn discover(&self) -> Result<DiscoveryResult, DiscoveryError> {
            Ok(self.result.clone())
        }
    }

    struct FailingDiscovery;

    impl Discovery for FailingDiscovery {
        fn name(&self) -> &str {
            "failing"
        }

        fn discover(&self) -> Result<DiscoveryResult, DiscoveryError> {
            Err(DiscoveryError::ConnectionFailed("connection refused".into()))
        }
    }

    // ── tests ──

    #[test]
    fn test_discovery_result_empty() {
        let result = DiscoveryResult::empty();
        assert!(result.is_empty());
        assert_eq!(result.total_objects(), 0);
    }

    #[test]
    fn test_discovery_result_total_objects() {
        let result = DiscoveryResult {
            providers: vec![make_provider("p1")],
            endpoints: vec![make_endpoint("e1", "p1")],
            accounts: vec![make_account("a1", "e1")],
            models: vec![make_model("m1", QualityTier::A)],
            variants: vec![make_variant("v1", "m1", "e1")],
            pools: vec![make_pool("pool1", vec!["v1"])],
        };
        assert_eq!(result.total_objects(), 6);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_discovery_error_display() {
        let e = DiscoveryError::NotSupported("watch".into());
        assert_eq!(format!("{}", e), "not supported: watch");

        let e = DiscoveryError::ConnectionFailed("timeout".into());
        assert_eq!(format!("{}", e), "connection failed: timeout");

        let e = DiscoveryError::ParseError("invalid json".into());
        assert_eq!(format!("{}", e), "parse error: invalid json");

        let e = DiscoveryError::Other("something".into());
        assert_eq!(format!("{}", e), "discovery error: something");
    }

    #[test]
    fn test_mock_discovery_returns_result() {
        let discovery = MockDiscovery {
            name: "mock".to_string(),
            result: DiscoveryResult {
                providers: vec![make_provider("p1")],
                models: vec![make_model("m1", QualityTier::S)],
                ..Default::default()
            },
        };

        assert_eq!(discovery.name(), "mock");
        let result = discovery.discover().unwrap();
        assert_eq!(result.providers.len(), 1);
        assert_eq!(result.models.len(), 1);
        assert_eq!(result.providers[0].id, "p1");
    }

    #[test]
    fn test_failing_discovery() {
        let discovery = FailingDiscovery;
        assert_eq!(discovery.name(), "failing");
        let err = discovery.discover().unwrap_err();
        match err {
            DiscoveryError::ConnectionFailed(msg) => {
                assert_eq!(msg, "connection refused");
            }
            _ => panic!("expected ConnectionFailed"),
        }
    }

    #[test]
    fn test_discovery_default_watch_not_supported() {
        let discovery = MockDiscovery {
            name: "no-watch".to_string(),
            result: DiscoveryResult::empty(),
        };
        assert!(!discovery.supports_watch());
        let err = discovery.watch(Box::new(|_| {})).unwrap_err();
        match err {
            DiscoveryError::NotSupported(_) => {}
            _ => panic!("expected NotSupported"),
        }
    }

    #[test]
    fn test_discovery_event_full_sync() {
        let result = DiscoveryResult {
            providers: vec![make_provider("p1")],
            ..Default::default()
        };
        let event = DiscoveryEvent::FullSync(result);
        match event {
            DiscoveryEvent::FullSync(r) => assert_eq!(r.providers.len(), 1),
            _ => panic!("expected FullSync"),
        }
    }

    #[test]
    fn test_discovery_event_variant_upsert() {
        let variant = make_variant("v1", "m1", "e1");
        let event = DiscoveryEvent::VariantUpsert(variant);
        match event {
            DiscoveryEvent::VariantUpsert(v) => assert_eq!(v.id, "v1"),
            _ => panic!("expected VariantUpsert"),
        }
    }

    #[test]
    fn test_discovery_event_variant_remove() {
        let event = DiscoveryEvent::VariantRemove("v1".to_string());
        match event {
            DiscoveryEvent::VariantRemove(id) => assert_eq!(id, "v1"),
            _ => panic!("expected VariantRemove"),
        }
    }

    #[test]
    fn test_multiple_discoveries_merge() {
        let d1 = MockDiscovery {
            name: "source-a".to_string(),
            result: DiscoveryResult {
                providers: vec![make_provider("openai")],
                endpoints: vec![make_endpoint("openai-ep", "openai")],
                accounts: vec![make_account("openai-acct", "openai-ep")],
                models: vec![make_model("gpt-4o", QualityTier::S)],
                variants: vec![make_variant("gpt-4o@openai", "gpt-4o", "openai-ep")],
                pools: vec![make_pool("pool-a", vec!["gpt-4o@openai"])],
            },
        };

        let d2 = MockDiscovery {
            name: "source-b".to_string(),
            result: DiscoveryResult {
                providers: vec![make_provider("anthropic")],
                endpoints: vec![make_endpoint("anthropic-ep", "anthropic")],
                accounts: vec![make_account("anthropic-acct", "anthropic-ep")],
                models: vec![make_model("claude-3", QualityTier::S)],
                variants: vec![make_variant("claude-3@anthropic", "claude-3", "anthropic-ep")],
                pools: vec![make_pool("pool-b", vec!["claude-3@anthropic"])],
            },
        };

        let discoveries: Vec<Box<dyn Discovery>> = vec![Box::new(d1), Box::new(d2)];

        // Simulate merging
        let mut merged = DiscoveryResult::empty();
        for d in &discoveries {
            let r = d.discover().unwrap();
            merged.providers.extend(r.providers);
            merged.endpoints.extend(r.endpoints);
            merged.accounts.extend(r.accounts);
            merged.models.extend(r.models);
            merged.variants.extend(r.variants);
            merged.pools.extend(r.pools);
        }

        assert_eq!(merged.providers.len(), 2);
        assert_eq!(merged.models.len(), 2);
        assert_eq!(merged.variants.len(), 2);
        assert_eq!(merged.pools.len(), 2);
        assert_eq!(merged.total_objects(), 12);
    }
}
