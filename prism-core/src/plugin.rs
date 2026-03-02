//! Plugin Framework for the Prism routing engine.
//!
//! Inspired by the Kubernetes Scheduling Framework, this module defines
//! extension points and a plugin chain that replaces the fixed L1–L5 layers.
//!
//! ## Extension Points
//!
//! 1. **PreFilter** — global pre-check; can reject a request early or cache data.
//! 2. **Filter** — per-candidate elimination (AND across plugins).
//! 3. **PostFilter** — recovery when all candidates are filtered out.
//! 4. **Score** — weighted scoring of surviving candidates.
//! 5. **Reserve / Unreserve** — resource reservation after selection.
//!
//! ## Design
//!
//! Each plugin implements [`RoutingPlugin`] and overrides only the extension
//! points it participates in. Plugins are registered in a [`PluginRegistry`]
//! with optional score weights. The [`PluginFramework`] executes the full
//! routing pipeline.

use std::collections::HashMap;
use std::sync::Arc;

use crate::analyzer::RequestProfile;
use crate::router::{RouteMetadata, RouterError};
use crate::strategy::{FailureType, ModelSelectionStrategy, RecoveryAction};
use crate::types::ModelCandidate;

// ════════════════════════════════════════════════
// RoutingPlugin trait
// ════════════════════════════════════════════════

/// Routing plugin trait — a plugin can implement any combination of extension points.
///
/// All methods have default (no-op) implementations so that a plugin only needs
/// to override the extension points it participates in.
pub trait RoutingPlugin: Send + Sync {
    /// Plugin name, used for logging and configuration references.
    fn name(&self) -> &str;

    // ═══ PreFilter: global pre-check ═══
    /// Called once before filtering begins. Can reject the entire request
    /// by returning `Err`, or cache computation results into `state`.
    fn pre_filter(
        &self,
        _state: &mut PluginState,
        _candidates: &[ModelCandidate],
    ) -> Result<(), RouterError> {
        Ok(())
    }

    // ═══ Filter: per-candidate elimination ═══
    /// Returns `true` if the candidate passes, `false` to eliminate it.
    /// All Filter plugins are AND-combined (any `false` eliminates).
    fn filter(&self, _state: &PluginState, _candidate: &ModelCandidate) -> bool {
        true
    }

    // ═══ PostFilter: recovery when all filtered ═══
    /// Called only when all candidates have been filtered out.
    /// Can relax constraints and return indices of candidates to restore.
    fn post_filter(
        &self,
        _state: &PluginState,
        _candidates: &[ModelCandidate],
    ) -> Result<Vec<usize>, RouterError> {
        Err(RouterError::AllFiltered {
            reason: "no post_filter recovery".into(),
        })
    }

    // ═══ Score: candidate scoring (0.0 – 1.0) ═══
    /// Score a candidate. Multiple Score plugins are weighted by `score_weight`.
    fn score(&self, _state: &PluginState, _candidate: &ModelCandidate) -> f32 {
        0.0
    }

    // ═══ Reserve: resource reservation after selection ═══
    /// Called after a candidate is selected. Can fail to trigger Unreserve.
    fn reserve(&self, _state: &PluginState, _selected: &ModelCandidate) -> Result<(), RouterError> {
        Ok(())
    }

    // ═══ Unreserve: rollback on reserve failure ═══
    /// Called to undo a reservation when a later plugin's Reserve fails.
    fn unreserve(&self, _state: &PluginState, _selected: &ModelCandidate) {}
}

// ════════════════════════════════════════════════
// PluginState — shared cycle state
// ════════════════════════════════════════════════

/// Shared state for a single routing cycle, analogous to k8s CycleState.
///
/// Plugins can store arbitrary data via the typed `set`/`get` API.
pub struct PluginState {
    /// Request analysis results.
    pub profile: RequestProfile,
    /// Request metadata (affinity key, tenant, region, etc.).
    pub metadata: RouteMetadata,
    /// Request-level strategy.
    pub strategy: ModelSelectionStrategy,
    /// Required protocol for this request.
    pub required_protocol: Option<String>,
    /// Plugin-private key-value store (keyed by arbitrary string).
    data: HashMap<String, Box<dyn std::any::Any + Send + Sync>>,
}

impl PluginState {
    pub fn new(
        profile: RequestProfile,
        metadata: RouteMetadata,
        strategy: ModelSelectionStrategy,
        required_protocol: Option<String>,
    ) -> Self {
        Self {
            profile,
            metadata,
            strategy,
            required_protocol,
            data: HashMap::new(),
        }
    }

    /// Store a typed value.
    pub fn set<T: Send + Sync + 'static>(&mut self, key: &str, value: T) {
        self.data.insert(key.to_string(), Box::new(value));
    }

    /// Retrieve a typed value by key.
    pub fn get<T: Send + Sync + 'static>(&self, key: &str) -> Option<&T> {
        self.data.get(key)?.downcast_ref::<T>()
    }
}

// ════════════════════════════════════════════════
// PluginRegistry
// ════════════════════════════════════════════════

/// A registered plugin entry with runtime configuration.
pub struct PluginEntry {
    pub plugin: Arc<dyn RoutingPlugin>,
    /// Weight used during the Score phase.
    pub score_weight: u32,
    /// Whether this plugin is enabled.
    pub enabled: bool,
}

/// Registry of routing plugins.
pub struct PluginRegistry {
    plugins: Vec<PluginEntry>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register a plugin with a specific score weight.
    pub fn register(&mut self, plugin: Arc<dyn RoutingPlugin>, score_weight: u32) {
        self.plugins.push(PluginEntry {
            plugin,
            score_weight,
            enabled: true,
        });
    }

    /// Register a plugin with the default score weight of 1.
    pub fn register_default(&mut self, plugin: Arc<dyn RoutingPlugin>) {
        self.register(plugin, 1);
    }

    /// Enable or disable a plugin by name.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) {
        for entry in &mut self.plugins {
            if entry.plugin.name() == name {
                entry.enabled = enabled;
            }
        }
    }

    /// Return references to all enabled plugin entries.
    pub fn enabled_plugins(&self) -> Vec<&PluginEntry> {
        self.plugins.iter().filter(|e| e.enabled).collect()
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ════════════════════════════════════════════════
// PluginFramework — routing execution engine
// ════════════════════════════════════════════════

/// The routing result produced by the plugin framework.
pub struct RoutingResult {
    /// Index of the selected candidate.
    pub selected_idx: usize,
    /// Final weighted score.
    pub score: f32,
    /// Fallback chain (up to 3 variant IDs).
    pub fallback_chain: Vec<String>,
}

/// Plugin framework — executes the full routing pipeline.
pub struct PluginFramework {
    registry: PluginRegistry,
}

impl PluginFramework {
    pub fn new(registry: PluginRegistry) -> Self {
        Self { registry }
    }

    /// Execute the complete routing flow:
    /// PreFilter → Filter → PostFilter → Score → Reserve/Unreserve.
    pub fn run_routing(
        &self,
        state: &mut PluginState,
        candidates: &[ModelCandidate],
    ) -> Result<RoutingResult, RouterError> {
        if candidates.is_empty() {
            return Err(RouterError::NoCandidates);
        }

        let plugins = self.registry.enabled_plugins();

        // 1. PreFilter
        for entry in &plugins {
            entry.plugin.pre_filter(state, candidates)?;
        }

        // 2. Filter
        let mut surviving: Vec<usize> = (0..candidates.len()).collect();
        for entry in &plugins {
            surviving.retain(|&i| entry.plugin.filter(state, &candidates[i]));
        }

        // 3. PostFilter (only when all candidates filtered out)
        if surviving.is_empty() {
            for entry in &plugins {
                if let Ok(recovered) = entry.plugin.post_filter(state, candidates) {
                    if !recovered.is_empty() {
                        surviving = recovered;
                        break;
                    }
                }
            }
            if surviving.is_empty() {
                return Err(RouterError::AllFiltered {
                    reason: "all candidates filtered, no post_filter recovery".into(),
                });
            }
        }

        // 4. Score + NormalizeScore
        let total_weight: u32 = plugins.iter().map(|e| e.score_weight).sum();
        let total_weight = if total_weight == 0 { 1 } else { total_weight };

        let mut scored: Vec<(usize, f32)> = surviving
            .iter()
            .map(|&idx| {
                let weighted_sum: f32 = plugins
                    .iter()
                    .map(|entry| {
                        let s = entry.plugin.score(state, &candidates[idx]);
                        s * (entry.score_weight as f32)
                    })
                    .sum();
                (idx, weighted_sum / total_weight as f32)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        if scored.is_empty() {
            return Err(RouterError::NoCandidates);
        }

        // 5. Reserve (try top candidates in order)
        for &(idx, score) in &scored {
            let candidate = &candidates[idx];
            let mut reserve_ok = true;
            let mut reserved_plugins: Vec<usize> = Vec::new();

            for (pi, entry) in plugins.iter().enumerate() {
                match entry.plugin.reserve(state, candidate) {
                    Ok(()) => reserved_plugins.push(pi),
                    Err(_) => {
                        // Unreserve already-reserved plugins
                        for &rpi in &reserved_plugins {
                            plugins[rpi].plugin.unreserve(state, candidate);
                        }
                        reserve_ok = false;
                        break;
                    }
                }
            }

            if reserve_ok {
                let fallback_chain: Vec<String> = scored
                    .iter()
                    .filter(|(i, _)| *i != idx)
                    .take(3)
                    .map(|(i, _)| candidates[*i].variant.id.clone())
                    .collect();

                return Ok(RoutingResult {
                    selected_idx: idx,
                    score,
                    fallback_chain,
                });
            }
        }

        Err(RouterError::AllFiltered {
            reason: "all candidates failed reserve phase".into(),
        })
    }

    /// Get a reference to the underlying registry.
    pub fn registry(&self) -> &PluginRegistry {
        &self.registry
    }
}

// ════════════════════════════════════════════════
// ResilienceHandler trait
// ════════════════════════════════════════════════

/// Resilience handler for retry/fallback/hedging — kept separate from the
/// plugin framework because these are post-request concerns, not candidate
/// evaluation concerns.
pub trait ResilienceHandler: Send + Sync {
    fn on_failure(
        &self,
        failed_variant_id: &str,
        error_type: &FailureType,
        attempt: u32,
    ) -> Result<RecoveryAction, RouterError>;

    fn plan_hedging(&self, candidates: &[&ModelCandidate], max: u32) -> Vec<String>;

    fn record_failure(&self, variant_id: &str);

    fn reset_failures(&self, variant_id: &str);
}
