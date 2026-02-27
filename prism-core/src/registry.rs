use std::collections::HashMap;

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
}

impl InMemoryRegistry {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            endpoints: HashMap::new(),
            accounts: HashMap::new(),
            models: HashMap::new(),
            variants: HashMap::new(),
            pools: HashMap::new(),
        }
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

    /// Assemble a ModelCandidate from a variant by resolving all references.
    fn assemble_candidate(&self, variant: &ModelVariant) -> Option<ModelCandidate> {
        let model = self.models.get(&variant.model_id)?;
        let endpoint = self.endpoints.get(&variant.endpoint_id)?;
        let provider = self.providers.get(&endpoint.provider_id)?;

        let account_id = variant
            .account_id
            .as_deref()
            .unwrap_or(&variant.endpoint_id);
        let account = self.accounts.get(account_id)?;

        let effective_pricing = variant
            .pricing_override
            .clone()
            .unwrap_or_else(|| model.pricing.clone());

        Some(ModelCandidate {
            variant: variant.clone(),
            model: model.clone(),
            account: account.clone(),
            endpoint: endpoint.clone(),
            provider: provider.clone(),
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

    fn update_performance(&self, _variant_id: &str, _perf: ModelPerformance) {
        // In a real implementation this would be atomic / lock-free.
        // For the in-memory stub we skip mutation (fields are not interior-mutable).
    }

    fn get_candidates_for_model(&self, model_id: &str) -> Vec<ModelCandidate> {
        self.variants
            .values()
            .filter(|v| v.model_id == model_id && v.status == VariantStatus::Active)
            .filter_map(|v| self.assemble_candidate(v))
            .collect()
    }
}
