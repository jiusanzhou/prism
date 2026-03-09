use std::collections::HashMap;
use std::sync::Arc;

use crate::data::{DataLayer, MetricKind, MetricValue, PerformanceDataSource};
use crate::discovery::{Discovery, DiscoveryResult};
use crate::types::*;

/// Trait for managing all scheduling objects and their runtime state.
pub trait ModelRegistryTrait: Send + Sync {
    /// Get all available candidates from specified pools (filtered by health).
    fn get_candidates(&self, pool_ids: &[String]) -> Vec<ModelCandidate>;

    /// Get all available candidates across all pools.
    fn get_all_candidates(&self) -> Vec<ModelCandidate>;

    /// Filter candidates by required capabilities.
    fn filter_by_capabilities(
        &self,
        candidates: &[ModelCandidate],
        required: &RequiredCapabilities,
    ) -> Vec<ModelCandidate>;

    /// Look up a specific model by id.
    fn get_model(&self, model_id: &str) -> Option<Model>;

    /// Look up a specific variant by id.
    fn get_variant(&self, variant_id: &str) -> Option<ModelVariant>;

    /// Update real-time performance data for a variant.
    fn update_performance(&self, variant_id: &str, perf: ModelPerformance);

    /// Get candidates for a specific model id (across all providers/accounts).
    fn get_candidates_for_model(&self, model_id: &str) -> Vec<ModelCandidate>;
}

/// Required capabilities filter (used in L1).
#[derive(Debug, Clone, Default)]
pub struct RequiredCapabilities {
    pub min_context_window: Option<u32>,
    pub function_calling: Option<bool>,
    pub vision: Option<bool>,
    pub json_mode: Option<bool>,
    pub reasoning: Option<bool>,
}

impl RequiredCapabilities {
    pub fn matches(&self, caps: &ModelCapabilities) -> bool {
        if let Some(min_ctx) = self.min_context_window {
            if caps.max_context_window < min_ctx {
                return false;
            }
        }
        if let Some(fc) = self.function_calling {
            if fc && !caps.supports_function_calling {
                return false;
            }
        }
        if let Some(v) = self.vision {
            if v && !caps.supports_vision {
                return false;
            }
        }
        if let Some(jm) = self.json_mode {
            if jm && !caps.supports_json_mode {
                return false;
            }
        }
        if let Some(r) = self.reasoning {
            if r && !caps.supports_reasoning {
                return false;
            }
        }
        true
    }
}

// ──────────────────────────────────────────────
// In-memory registry implementation
// ──────────────────────────────────────────────

/// A simple in-memory registry for testing and development.
pub struct InMemoryRegistry {
    pub providers: HashMap<String, Provider>,
    pub endpoints: HashMap<String, Endpoint>,
    pub accounts: HashMap<String, Account>,
    pub models: HashMap<String, Model>,
    pub variants: HashMap<String, ModelVariant>,
    pub pools: HashMap<String, ModelPool>,
    /// Unified data layer for all runtime metrics.
    data_layer: Arc<DataLayer>,
}

impl InMemoryRegistry {
    pub fn new() -> Self {
        let data_layer = Arc::new(DataLayer::new());
        // Register the default in-memory performance data source.
        data_layer.register(Arc::new(PerformanceDataSource::new()));

        Self {
            providers: HashMap::new(),
            endpoints: HashMap::new(),
            accounts: HashMap::new(),
            models: HashMap::new(),
            variants: HashMap::new(),
            pools: HashMap::new(),
            data_layer,
        }
    }

    /// Create a registry with a custom [`DataLayer`].
    pub fn with_data_layer(data_layer: Arc<DataLayer>) -> Self {
        Self {
            providers: HashMap::new(),
            endpoints: HashMap::new(),
            accounts: HashMap::new(),
            models: HashMap::new(),
            variants: HashMap::new(),
            pools: HashMap::new(),
            data_layer,
        }
    }

    /// Get a reference to the data layer.
    pub fn data_layer(&self) -> &Arc<DataLayer> {
        &self.data_layer
    }

    pub fn add_provider(&mut self, p: Provider) {
        self.providers.insert(p.id.clone(), p);
    }

    pub fn add_endpoint(&mut self, e: Endpoint) {
        self.endpoints.insert(e.id.clone(), e);
    }

    pub fn add_account(&mut self, a: Account) {
        self.accounts.insert(a.id.clone(), a);
    }

    pub fn add_model(&mut self, m: Model) {
        self.models.insert(m.id.clone(), m);
    }

    pub fn add_variant(&mut self, v: ModelVariant) {
        self.variants.insert(v.id.clone(), v);
    }

    pub fn add_pool(&mut self, p: ModelPool) {
        self.pools.insert(p.id.clone(), p);
    }

    /// Apply a [`DiscoveryResult`] to this registry, merging all objects.
    ///
    /// Objects with the same id are overwritten (last-write wins).
    pub fn apply_discovery(&mut self, result: &DiscoveryResult) {
        for p in &result.providers {
            self.providers.insert(p.id.clone(), p.clone());
        }
        for e in &result.endpoints {
            self.endpoints.insert(e.id.clone(), e.clone());
        }
        for a in &result.accounts {
            self.accounts.insert(a.id.clone(), a.clone());
        }
        for m in &result.models {
            self.models.insert(m.id.clone(), m.clone());
        }
        for v in &result.variants {
            self.variants.insert(v.id.clone(), v.clone());
        }
        for p in &result.pools {
            self.pools.insert(p.id.clone(), p.clone());
        }
    }

    /// Apply a single discovery event (incremental update).
    pub fn apply_event(&mut self, event: &crate::discovery::DiscoveryEvent) {
        use crate::discovery::DiscoveryEvent;
        match event {
            DiscoveryEvent::FullSync(result) => {
                // Clear and replace
                self.providers.clear();
                self.endpoints.clear();
                self.accounts.clear();
                self.models.clear();
                self.variants.clear();
                self.pools.clear();
                self.apply_discovery(result);
            }
            DiscoveryEvent::IncrementalUpsert(result) => {
                self.apply_discovery(result);
            }
            DiscoveryEvent::Remove {
                provider_ids,
                endpoint_ids,
                account_ids,
                model_ids,
                variant_ids,
                pool_ids,
            } => {
                for id in provider_ids {
                    self.providers.remove(id);
                }
                for id in endpoint_ids {
                    self.endpoints.remove(id);
                }
                for id in account_ids {
                    self.accounts.remove(id);
                }
                for id in model_ids {
                    self.models.remove(id);
                }
                for id in variant_ids {
                    self.variants.remove(id);
                }
                for id in pool_ids {
                    self.pools.remove(id);
                }
            }
            DiscoveryEvent::VariantUpsert(variant) => {
                self.variants.insert(variant.id.clone(), variant.clone());
            }
            DiscoveryEvent::VariantRemove(id) => {
                self.variants.remove(id);
            }
        }
    }

    /// Refresh the registry from multiple discovery sources.
    ///
    /// Clears all existing data and re-populates from the given discoveries.
    /// Discovery errors are logged (via the returned error list) but do not
    /// prevent other sources from being applied.
    pub fn refresh_from_discoveries(
        &mut self,
        discoveries: &[Box<dyn Discovery>],
    ) -> Vec<(String, crate::discovery::DiscoveryError)> {
        // Clear existing data
        self.providers.clear();
        self.endpoints.clear();
        self.accounts.clear();
        self.models.clear();
        self.variants.clear();
        self.pools.clear();

        let mut errors = Vec::new();

        for discovery in discoveries {
            match discovery.discover() {
                Ok(result) => self.apply_discovery(&result),
                Err(e) => errors.push((discovery.name().to_string(), e)),
            }
        }

        errors
    }

    /// Assemble a ModelCandidate from a variant by resolving all references.
    fn assemble_candidate(&self, variant: &ModelVariant) -> Option<ModelCandidate> {
        let model = self.models.get(&variant.model_id)?;

        // Try full chain: endpoint -> provider -> account
        let endpoint = self.endpoints.get(&variant.endpoint_id);
        let provider = endpoint.and_then(|ep| self.providers.get(&ep.provider_id));
        let account_id = variant
            .account_id
            .as_deref()
            .unwrap_or(&variant.endpoint_id);
        let account = self.accounts.get(account_id);

        // If variant has its own base_url, it can work without provider/endpoint/account
        // (typical for K8s-discovered instances with per-pod IP:port).
        let (endpoint, provider, account) = match (endpoint, provider, account) {
            (Some(ep), Some(prov), Some(acct)) => (ep.clone(), prov.clone(), acct.clone()),
            _ if variant.base_url.is_some() => {
                let synth_provider = provider.cloned().unwrap_or_else(|| Provider {
                    id: format!("{}-auto", variant.id),
                    name: String::new(),
                    auth: AuthConfig::default(),
                    base_url: variant.base_url.clone().unwrap_or_default(),
                    rate_limit: None,
                    protocols: vec![],
                    status: Default::default(),
                    metadata: Default::default(),
                });
                let synth_endpoint = endpoint.cloned().unwrap_or_else(|| Endpoint {
                    id: format!("{}-ep", variant.id),
                    provider_id: synth_provider.id.clone(),
                    base_url: variant.base_url.clone(),
                    region: None,
                    auth_override: None,
                    rate_limit: None,
                    priority: 0,
                    status: Default::default(),
                });
                let synth_account = account.cloned().unwrap_or_else(|| Account {
                    id: format!("{}-acct", variant.id),
                    endpoint_id: synth_endpoint.id.clone(),
                    auth: AuthConfig::default(),
                    project_id: None,
                    rate_limit: None,
                    billing_tag: None,
                    weight: 1.0,
                    status: Default::default(),
                    metadata: Default::default(),
                });
                (synth_endpoint, synth_provider, synth_account)
            }
            _ => return None, // no base_url and missing chain -- skip
        };

        // Overlay real-time performance data from the data layer (if available).
        let mut variant_clone = variant.clone();
        if let Some(MetricValue::Performance(perf)) =
            self.data_layer.query(MetricKind::Performance, &variant.id)
        {
            variant_clone.performance = perf;
        }

        // Overlay dynamic pricing from the data layer (if available).
        let effective_pricing = if let Some(MetricValue::Pricing(p)) =
            self.data_layer.query(MetricKind::Pricing, &variant.id)
        {
            p
        } else {
            variant
                .pricing_override
                .clone()
                .unwrap_or_else(|| model.pricing.clone())
        };

        Some(ModelCandidate {
            variant: variant_clone,
            model: model.clone(),
            account,
            endpoint,
            provider,
            effective_pricing,
        })
    }
}

impl Default for InMemoryRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelRegistryTrait for InMemoryRegistry {
    fn get_candidates(&self, pool_ids: &[String]) -> Vec<ModelCandidate> {
        let mut candidates = Vec::new();
        for pool_id in pool_ids {
            if let Some(pool) = self.pools.get(pool_id) {
                for variant_id in &pool.variants {
                    if let Some(variant) = self.variants.get(variant_id) {
                        if variant.status != VariantStatus::Active {
                            continue;
                        }
                        if let Some(c) = self.assemble_candidate(variant) {
                            candidates.push(c);
                        }
                    }
                }
            }
        }
        candidates
    }

    fn get_all_candidates(&self) -> Vec<ModelCandidate> {
        self.variants
            .values()
            .filter(|v| v.status == VariantStatus::Active)
            .filter_map(|v| self.assemble_candidate(v))
            .collect()
    }

    fn filter_by_capabilities(
        &self,
        candidates: &[ModelCandidate],
        required: &RequiredCapabilities,
    ) -> Vec<ModelCandidate> {
        candidates
            .iter()
            .filter(|c| required.matches(&c.model.capabilities))
            .cloned()
            .collect()
    }

    fn get_model(&self, model_id: &str) -> Option<Model> {
        self.models.get(model_id).cloned()
    }

    fn get_variant(&self, variant_id: &str) -> Option<ModelVariant> {
        self.variants.get(variant_id).cloned()
    }

    fn update_performance(&self, variant_id: &str, perf: ModelPerformance) {
        self.data_layer.update(
            MetricKind::Performance,
            variant_id,
            MetricValue::Performance(perf),
        );
    }

    fn get_candidates_for_model(&self, model_id: &str) -> Vec<ModelCandidate> {
        self.variants
            .values()
            .filter(|v| v.model_id == model_id && v.status == VariantStatus::Active)
            .filter_map(|v| self.assemble_candidate(v))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{DiscoveryEvent, DiscoveryResult};

    // ── helpers ──

    fn make_provider(id: &str) -> Provider {
        Provider {
            id: id.into(),
            name: format!("Provider {}", id),
            auth: AuthConfig::default(),
            base_url: format!("https://{}.example.com", id),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
            protocols: vec![],
        }
    }

    fn make_endpoint(id: &str, provider_id: &str) -> Endpoint {
        Endpoint {
            id: id.into(),
            provider_id: provider_id.into(),
            base_url: None,
            region: Some("us-east-1".into()),
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        }
    }

    fn make_account(id: &str, endpoint_id: &str) -> Account {
        Account {
            id: id.into(),
            endpoint_id: endpoint_id.into(),
            auth: AuthConfig::default(),
            project_id: None,
            rate_limit: None,
            billing_tag: None,
            weight: 1.0,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
        }
    }

    fn make_model(id: &str) -> Model {
        Model {
            id: id.into(),
            display_name: format!("Model {}", id),
            quality_tier: QualityTier::A,
            capabilities: ModelCapabilities {
                max_context_window: 128000,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_function_calling: true,
                supports_vision: false,
                supports_json_mode: true,
                supports_reasoning: false,
            },
            pricing: ModelPricing::default(),
            specialties: vec![],
            limits: None,
            status: ModelStatus::Available,
        }
    }

    fn make_variant(id: &str, model_id: &str, endpoint_id: &str, account_id: &str) -> ModelVariant {
        ModelVariant {
            id: id.into(),
            model_id: model_id.into(),
            endpoint_id: endpoint_id.into(),
            account_id: Some(account_id.into()),
            deployment_name: None,
            weight: 1.0,
            performance: ModelPerformance::default(),
            pricing_override: None,
            status: VariantStatus::Active,
            base_url: None,
            upstream_model_name: None,
            metadata: HashMap::new(),
        }
    }

    fn make_pool(id: &str, variant_ids: &[&str]) -> ModelPool {
        ModelPool {
            id: id.into(),
            name: format!("Pool {}", id),
            variants: variant_ids.iter().map(|s| s.to_string()).collect(),
            selection_mode: SelectionMode::Scored,
            description: None,
        }
    }

    /// Build a minimal registry with one complete chain.
    fn build_basic_registry() -> InMemoryRegistry {
        let mut reg = InMemoryRegistry::new();
        reg.add_provider(make_provider("p1"));
        reg.add_endpoint(make_endpoint("ep1", "p1"));
        reg.add_account(make_account("acct1", "ep1"));
        reg.add_model(make_model("m1"));
        reg.add_variant(make_variant("v1", "m1", "ep1", "acct1"));
        reg.add_pool(make_pool("pool1", &["v1"]));
        reg
    }

    // ── Creation ──

    #[test]
    fn new_registry_is_empty() {
        let reg = InMemoryRegistry::new();
        assert!(reg.providers.is_empty());
        assert!(reg.models.is_empty());
        assert!(reg.variants.is_empty());
        assert!(reg.pools.is_empty());
    }

    #[test]
    fn default_registry_is_empty() {
        let reg = InMemoryRegistry::default();
        assert!(reg.providers.is_empty());
    }

    // ── get_model / get_variant lookups ──

    #[test]
    fn get_model_found() {
        let reg = build_basic_registry();
        let m = reg.get_model("m1").unwrap();
        assert_eq!(m.id, "m1");
        assert_eq!(m.quality_tier, QualityTier::A);
    }

    #[test]
    fn get_model_not_found() {
        let reg = build_basic_registry();
        assert!(reg.get_model("nonexistent").is_none());
    }

    #[test]
    fn get_variant_found() {
        let reg = build_basic_registry();
        let v = reg.get_variant("v1").unwrap();
        assert_eq!(v.model_id, "m1");
    }

    #[test]
    fn get_variant_not_found() {
        let reg = build_basic_registry();
        assert!(reg.get_variant("nope").is_none());
    }

    // ── get_candidates with pool filtering ──

    #[test]
    fn get_candidates_from_pool() {
        let reg = build_basic_registry();
        let candidates = reg.get_candidates(&["pool1".into()]);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].variant.id, "v1");
    }

    #[test]
    fn get_candidates_unknown_pool() {
        let reg = build_basic_registry();
        let candidates = reg.get_candidates(&["nonexistent".into()]);
        assert!(candidates.is_empty());
    }

    #[test]
    fn get_candidates_multiple_pools() {
        let mut reg = build_basic_registry();
        // Add second variant + pool
        reg.add_variant(make_variant("v2", "m1", "ep1", "acct1"));
        reg.add_pool(make_pool("pool2", &["v2"]));

        let candidates = reg.get_candidates(&["pool1".into(), "pool2".into()]);
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn get_candidates_skips_inactive_variants() {
        let mut reg = build_basic_registry();
        let mut disabled = make_variant("v2", "m1", "ep1", "acct1");
        disabled.status = VariantStatus::Disabled;
        reg.add_variant(disabled);
        reg.pools
            .get_mut("pool1")
            .unwrap()
            .variants
            .push("v2".into());

        let candidates = reg.get_candidates(&["pool1".into()]);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].variant.id, "v1");
    }

    #[test]
    fn get_candidates_skips_draining_variants() {
        let mut reg = build_basic_registry();
        let mut draining = make_variant("v2", "m1", "ep1", "acct1");
        draining.status = VariantStatus::Draining;
        reg.add_variant(draining);
        reg.pools
            .get_mut("pool1")
            .unwrap()
            .variants
            .push("v2".into());

        let candidates = reg.get_candidates(&["pool1".into()]);
        assert_eq!(candidates.len(), 1);
    }

    // ── get_all_candidates ──

    #[test]
    fn get_all_candidates_returns_active_only() {
        let mut reg = build_basic_registry();
        let mut disabled = make_variant("v2", "m1", "ep1", "acct1");
        disabled.status = VariantStatus::Disabled;
        reg.add_variant(disabled);

        let candidates = reg.get_all_candidates();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].variant.id, "v1");
    }

    // ── filter_by_capabilities ──

    #[test]
    fn filter_capabilities_no_requirements() {
        let reg = build_basic_registry();
        let all = reg.get_all_candidates();
        let filtered = reg.filter_by_capabilities(&all, &RequiredCapabilities::default());
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn filter_capabilities_vision_required() {
        let reg = build_basic_registry();
        let all = reg.get_all_candidates();
        let required = RequiredCapabilities {
            vision: Some(true),
            ..Default::default()
        };
        let filtered = reg.filter_by_capabilities(&all, &required);
        assert!(filtered.is_empty()); // base model doesn't support vision
    }

    #[test]
    fn filter_capabilities_context_window() {
        let reg = build_basic_registry();
        let all = reg.get_all_candidates();

        // Require 200k context — model only has 128k
        let required = RequiredCapabilities {
            min_context_window: Some(200_000),
            ..Default::default()
        };
        assert!(reg.filter_by_capabilities(&all, &required).is_empty());

        // Require 64k context — model has 128k, should pass
        let required = RequiredCapabilities {
            min_context_window: Some(64_000),
            ..Default::default()
        };
        assert_eq!(reg.filter_by_capabilities(&all, &required).len(), 1);
    }

    #[test]
    fn filter_capabilities_reasoning() {
        let reg = build_basic_registry();
        let all = reg.get_all_candidates();
        let required = RequiredCapabilities {
            reasoning: Some(true),
            ..Default::default()
        };
        assert!(reg.filter_by_capabilities(&all, &required).is_empty());
    }

    #[test]
    fn filter_capabilities_function_calling() {
        let reg = build_basic_registry();
        let all = reg.get_all_candidates();
        let required = RequiredCapabilities {
            function_calling: Some(true),
            ..Default::default()
        };
        // Our test model supports function calling
        assert_eq!(reg.filter_by_capabilities(&all, &required).len(), 1);
    }

    #[test]
    fn filter_capabilities_json_mode() {
        let reg = build_basic_registry();
        let all = reg.get_all_candidates();
        let required = RequiredCapabilities {
            json_mode: Some(true),
            ..Default::default()
        };
        assert_eq!(reg.filter_by_capabilities(&all, &required).len(), 1);
    }

    // ── RequiredCapabilities::matches ──

    #[test]
    fn required_caps_matches_all_none() {
        let caps = ModelCapabilities::default();
        assert!(RequiredCapabilities::default().matches(&caps));
    }

    #[test]
    fn required_caps_false_requirement_passes() {
        let caps = ModelCapabilities {
            supports_vision: false,
            ..Default::default()
        };
        // Requiring vision=false should still pass (not enforced)
        let req = RequiredCapabilities {
            vision: Some(false),
            ..Default::default()
        };
        assert!(req.matches(&caps));
    }

    // ── update_performance ──

    #[test]
    fn update_performance_reflected_in_candidates() {
        let reg = build_basic_registry();
        let perf = ModelPerformance {
            p50_latency_ms: 42,
            ..Default::default()
        };
        reg.update_performance("v1", perf);

        let candidates = reg.get_candidates(&["pool1".into()]);
        assert_eq!(candidates[0].variant.performance.p50_latency_ms, 42);
    }

    // ── get_candidates_for_model ──

    #[test]
    fn get_candidates_for_model_basic() {
        let reg = build_basic_registry();
        let candidates = reg.get_candidates_for_model("m1");
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn get_candidates_for_model_unknown() {
        let reg = build_basic_registry();
        assert!(reg.get_candidates_for_model("unknown").is_empty());
    }

    // ── apply_event ──

    #[test]
    fn apply_event_full_sync_clears_and_replaces() {
        let mut reg = build_basic_registry();
        assert_eq!(reg.models.len(), 1);

        let result = DiscoveryResult {
            providers: vec![make_provider("p2")],
            endpoints: vec![make_endpoint("ep2", "p2")],
            accounts: vec![make_account("acct2", "ep2")],
            models: vec![make_model("m2")],
            variants: vec![make_variant("v2", "m2", "ep2", "acct2")],
            pools: vec![make_pool("pool2", &["v2"])],
        };
        reg.apply_event(&DiscoveryEvent::FullSync(result));

        // Old data should be gone
        assert!(reg.get_model("m1").is_none());
        assert!(reg.get_variant("v1").is_none());
        // New data should be present
        assert!(reg.get_model("m2").is_some());
        assert!(reg.get_variant("v2").is_some());
    }

    #[test]
    fn apply_event_incremental_upsert() {
        let mut reg = build_basic_registry();
        let result = DiscoveryResult {
            models: vec![make_model("m2")],
            ..DiscoveryResult::default()
        };
        reg.apply_event(&DiscoveryEvent::IncrementalUpsert(result));

        // Both old and new data present
        assert!(reg.get_model("m1").is_some());
        assert!(reg.get_model("m2").is_some());
    }

    #[test]
    fn apply_event_remove() {
        let mut reg = build_basic_registry();
        reg.apply_event(&DiscoveryEvent::Remove {
            provider_ids: vec![],
            endpoint_ids: vec![],
            account_ids: vec![],
            model_ids: vec!["m1".into()],
            variant_ids: vec!["v1".into()],
            pool_ids: vec!["pool1".into()],
        });

        assert!(reg.get_model("m1").is_none());
        assert!(reg.get_variant("v1").is_none());
        assert!(!reg.pools.contains_key("pool1"));
    }

    #[test]
    fn apply_event_variant_upsert() {
        let mut reg = build_basic_registry();
        let v = make_variant("v_new", "m1", "ep1", "acct1");
        reg.apply_event(&DiscoveryEvent::VariantUpsert(v));
        assert!(reg.get_variant("v_new").is_some());
    }

    #[test]
    fn apply_event_variant_remove() {
        let mut reg = build_basic_registry();
        reg.apply_event(&DiscoveryEvent::VariantRemove("v1".into()));
        assert!(reg.get_variant("v1").is_none());
    }

    // ── Variant with base_url (synthetic provider/endpoint) ──

    #[test]
    fn variant_with_base_url_assembles_without_provider_chain() {
        let mut reg = InMemoryRegistry::new();
        reg.add_model(make_model("m1"));
        let mut v = make_variant("v1", "m1", "missing-ep", "missing-acct");
        v.base_url = Some("http://localhost:8080".into());
        reg.add_variant(v);
        reg.add_pool(make_pool("pool1", &["v1"]));

        let candidates = reg.get_candidates(&["pool1".into()]);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].provider.id.contains("auto"));
    }

    #[test]
    fn variant_without_base_url_and_missing_chain_skipped() {
        let mut reg = InMemoryRegistry::new();
        reg.add_model(make_model("m1"));
        // No provider/endpoint/account, no base_url
        let v = make_variant("v1", "m1", "missing-ep", "missing-acct");
        reg.add_variant(v);
        reg.add_pool(make_pool("pool1", &["v1"]));

        let candidates = reg.get_candidates(&["pool1".into()]);
        assert!(candidates.is_empty());
    }

    // ── Pricing overlay ──

    #[test]
    fn effective_pricing_uses_model_default() {
        let reg = build_basic_registry();
        let candidates = reg.get_all_candidates();
        // No pricing override → model default (0.0)
        assert_eq!(candidates[0].effective_pricing.input_per_1m_tokens, 0.0);
    }

    #[test]
    fn effective_pricing_uses_variant_override() {
        let mut reg = build_basic_registry();
        let mut v = reg.variants.get("v1").unwrap().clone();
        v.pricing_override = Some(ModelPricing {
            input_per_1m_tokens: 5.0,
            output_per_1m_tokens: 15.0,
            currency: "USD".into(),
        });
        reg.variants.insert("v1".into(), v);

        let candidates = reg.get_all_candidates();
        assert_eq!(candidates[0].effective_pricing.input_per_1m_tokens, 5.0);
    }
}
