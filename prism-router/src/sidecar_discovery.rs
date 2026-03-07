//! Sidecar Discovery — connects to any discovery sidecar implementing the Prism
//! Sidecar Discovery Protocol (SSE `/events` + HTTP `/prism`).
//!
//! Spawned automatically for providers with `discovery.sidecar_url` config.
//! Works with the Go k8s-discovery sidecar or any compatible implementation.

use std::collections::HashMap;
use std::io::BufRead;
use std::time::Duration;

use serde::Deserialize;
use tracing::{debug, error, info, warn};

use prism_core::discovery::{DiscoveryEvent, DiscoveryResult};
use prism_core::types::*;

use crate::config::types::ProviderDiscoveryConfig;
use crate::config::{build_registry, build_router_from_config, load_config};
use crate::state::AppState;

/// Prism payload from Go sidecar's `/prism` endpoint.
#[derive(Debug, Deserialize)]
struct SidecarPrismPayload {
    #[serde(default)]
    providers: Vec<SidecarProvider>,
    #[serde(default)]
    models: Vec<SidecarModel>,
    #[serde(default)]
    variants: Vec<SidecarVariant>,
    #[serde(default)]
    pools: Vec<SidecarPool>,
}

#[derive(Debug, Deserialize)]
struct SidecarProvider {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    base_url: String,
}

#[derive(Debug, Deserialize)]
struct SidecarModel {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SidecarVariant {
    id: String,
    model_id: String,
    #[allow(dead_code)]
    provider_id: String,
    #[serde(default)]
    weight: Option<f64>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct SidecarPool {
    id: String,
    name: String,
    #[serde(default)]
    variants: Vec<String>,
}

/// SSE event data from `/events`.
/// Go sidecar sends: { type, model?, instance?, timestamp, models? (full_sync only) }
#[derive(Debug, Deserialize)]
struct SSEEvent {
    #[serde(default, rename = "type")]
    event_type: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    instance: Option<SidecarInstance>,
    #[serde(default)]
    timestamp: i64,
    /// full_sync: complete models→instances map.
    #[serde(default)]
    models: HashMap<String, Vec<SidecarInstance>>,
}

#[derive(Debug, Clone, Deserialize)]
struct SidecarInstance {
    address: String,
    #[serde(default)]
    pod_name: Option<String>,
    #[serde(default)]
    labels: HashMap<String, String>,
    #[serde(default)]
    dc: Option<String>,
}

/// Convert sidecar `/prism` payload to a DiscoveryResult.
fn payload_to_discovery_result(payload: &SidecarPrismPayload, provider_id: &str) -> DiscoveryResult {
    let mut result = DiscoveryResult::default();

    // Models
    for m in &payload.models {
        result.models.push(Model {
            id: m.id.clone(),
            display_name: m.display_name.clone().unwrap_or_else(|| m.id.clone()),
            quality_tier: QualityTier::B,
            capabilities: Default::default(),
            pricing: Default::default(),
            specialties: vec![],
            limits: None,
            status: Default::default(),
        });
    }

    // Variants (with base_url — provider-free routing)
    for v in &payload.variants {
        let metadata = v.metadata.clone().unwrap_or_default();
        result.variants.push(ModelVariant {
            id: v.id.clone(),
            model_id: v.model_id.clone(),
            endpoint_id: format!("{}-ep", provider_id),
            account_id: None,
            weight: v.weight.unwrap_or(1.0) as f32,
            performance: Default::default(),
            pricing_override: None,
            base_url: v.base_url.clone(),
            upstream_model_name: None,
            deployment_name: None,
            status: Default::default(),
            metadata,
        });
    }

    // Pools
    for p in &payload.pools {
        result.pools.push(ModelPool {
            id: p.id.clone(),
            name: p.name.clone(),
            variants: p.variants.clone(),
            selection_mode: Default::default(),
            description: None,
        });
    }

    result
}

/// Convert SSE full_sync instance map to a DiscoveryResult directly.
fn instances_to_discovery_result(
    provider_id: &str,
    models: &HashMap<String, Vec<SidecarInstance>>,
) -> DiscoveryResult {
    let mut result = DiscoveryResult::default();

    for (model_name, instances) in models {
        result.models.push(Model {
            id: model_name.clone(),
            display_name: model_name.clone(),
            quality_tier: QualityTier::B,
            capabilities: Default::default(),
            pricing: Default::default(),
            specialties: vec![],
            limits: None,
            status: Default::default(),
        });

        let mut pool_variants = vec![];

        for inst in instances {
            let pod_name = inst.pod_name.clone().unwrap_or_else(|| {
                inst.address.replace('.', "-").replace(':', "-")
            });
            let vid = format!("{}-{}", model_name, pod_name);

            let mut metadata = HashMap::new();
            for key in &[
                "wanqing.inference.instance/role",
                "wanqing.inference.instance.pd/group",
                "apps.kwai.io/duplicateset-instance-id",
                "cluster",
            ] {
                if let Some(val) = inst.labels.get(*key) {
                    metadata.insert(key.to_string(), val.clone());
                }
            }
            if let Some(dc) = &inst.dc {
                metadata.insert("dc".to_string(), dc.clone());
            }

            result.variants.push(ModelVariant {
                id: vid.clone(),
                model_id: model_name.clone(),
                endpoint_id: format!("{}-ep", provider_id),
                account_id: None,
                weight: 1.0,
                performance: Default::default(),
                pricing_override: None,
                base_url: Some(format!("http://{}", inst.address)),
                upstream_model_name: None,
                deployment_name: None,
                status: Default::default(),
                metadata,
            });
            pool_variants.push(vid);
        }

        result.pools.push(ModelPool {
            id: format!("pool-{}", model_name),
            name: model_name.clone(),
            variants: pool_variants,
            selection_mode: Default::default(),
            description: None,
        });
    }

    result
}

/// Fetch /discover — raw models→instances map (used for initial sync + local state seed).
fn fetch_discover(sidecar_url: &str) -> Result<InstanceMap, String> {
    #[derive(Deserialize)]
    struct DiscoverResp {
        #[serde(default)]
        models: HashMap<String, Vec<SidecarInstance>>,
    }
    let url = format!("{}/discover", sidecar_url.trim_end_matches('/'));
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|e| format!("HTTP /discover: {}", e))?;
    let dr: DiscoverResp = resp
        .into_json()
        .map_err(|e| format!("parse /discover: {}", e))?;
    Ok(dr.models)
}

/// Fetch /prism payload from sidecar via HTTP.
fn fetch_prism_payload(sidecar_url: &str) -> Result<SidecarPrismPayload, String> {
    let url = format!("{}/prism", sidecar_url.trim_end_matches('/'));
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|e| format!("HTTP /prism: {}", e))?;
    let payload: SidecarPrismPayload = resp
        .into_json()
        .map_err(|e| format!("parse /prism: {}", e))?;
    Ok(payload)
}

/// Apply a discovery event: merge with static config and swap router.
fn apply_discovery_result(state: &AppState, provider_id: &str, event: DiscoveryEvent) {
    let base_config = match load_config(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            warn!(provider = %provider_id, "Config reload failed: {}", e);
            return;
        }
    };

    let mut registry = build_registry(&base_config);
    registry.apply_event(&event);

    let n_variants = registry.variants.len();
    let new_router = build_router_from_config(registry, &base_config.routing);
    state.swap_router(new_router);

    info!(
        provider = %provider_id,
        variants = n_variants,
        generation = state.generation(),
        "Registry updated from Sidecar discovery"
    );
}

/// Spawn a Sidecar discovery consumer for a provider.
///
/// Connects to the Go sidecar via SSE for real-time updates.
/// Falls back to HTTP polling on error, auto-reconnects.
pub fn spawn_sidecar_discovery(
    state: AppState,
    provider_id: String,
    discovery_config: ProviderDiscoveryConfig,
) -> tokio::task::JoinHandle<()> {
    let sidecar_url = discovery_config
        .sidecar_url
        .clone()
        .unwrap_or_else(|| "http://127.0.0.1:9091".to_string());

    info!(
        provider = %provider_id,
        sidecar = %sidecar_url,
        "Starting Sidecar discovery consumer"
    );

    tokio::spawn(async move {
        loop {
            info!(provider = %provider_id, "Connecting SSE: {}/events", sidecar_url);

            let sse_result = {
                let url = format!("{}/events", sidecar_url.trim_end_matches('/'));
                let state = state.clone();
                let pid = provider_id.clone();
                let surl = sidecar_url.clone();

                tokio::task::spawn_blocking(move || {
                    run_sse_loop(&state, &pid, &url, &surl)
                }).await
            };

            match sse_result {
                Ok(Ok(())) => {
                    warn!(provider = %provider_id, "SSE stream ended cleanly, reconnecting in 5s");
                }
                Ok(Err(e)) => {
                    warn!(provider = %provider_id, "SSE error: {}", e);
                    // Do a poll as fallback
                    let state2 = state.clone();
                    let pid2 = provider_id.clone();
                    let surl2 = sidecar_url.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Err(e) = poll_and_apply(&state2, &pid2, &surl2) {
                            error!(provider = %pid2, "Fallback poll failed: {}", e);
                        }
                    }).await;
                }
                Err(e) => {
                    error!(provider = %provider_id, "spawn_blocking panic: {}", e);
                }
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    })
}

/// Local state maintained from SSE events — the single source of truth for this provider.
type InstanceMap = HashMap<String, Vec<SidecarInstance>>;

/// Blocking SSE event loop with local state management.
///
/// Maintains a local `InstanceMap` that is updated incrementally from SSE events.
/// On each state change, rebuilds the registry from the local map (no HTTP round-trip).
fn run_sse_loop(
    state: &AppState,
    provider_id: &str,
    sse_url: &str,
    sidecar_url: &str,
) -> Result<(), String> {
    let resp = ureq::get(sse_url)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|e| format!("SSE connect: {}", e))?;

    info!(provider = %provider_id, "SSE connected");

    // Initial sync via HTTP /prism (SSE initial full_sync may arrive empty)
    let mut local: InstanceMap = match fetch_discover(sidecar_url) {
        Ok(models) => {
            let n_models = models.len();
            let n_inst: usize = models.values().map(|v| v.len()).sum();
            info!(provider = %provider_id, models = n_models, instances = n_inst, "Initial sync from /discover");
            rebuild_from_local(state, provider_id, &models);
            models
        }
        Err(e) => {
            warn!(provider = %provider_id, "Initial /discover failed: {}", e);
            HashMap::new()
        }
    };

    let reader = std::io::BufReader::new(resp.into_reader());
    let mut event_type = String::new();
    let mut data_buf = String::new();

    for line in reader.lines() {
        let line = line.map_err(|e| format!("SSE read: {}", e))?;

        if line.starts_with("event:") {
            event_type = line[6..].trim().to_string();
        } else if line.starts_with("data:") {
            data_buf = line[5..].trim().to_string();
        } else if line.is_empty() && !data_buf.is_empty() {
            if apply_sse_event(&mut local, state, provider_id, &event_type, &data_buf) {
                rebuild_from_local(state, provider_id, &local);
            }
            event_type.clear();
            data_buf.clear();
        }
    }

    Ok(())
}

/// Apply a single SSE event to local state. Returns true if state changed.
fn apply_sse_event(
    local: &mut InstanceMap,
    _state: &AppState,
    provider_id: &str,
    event_type: &str,
    data: &str,
) -> bool {
    let event: SSEEvent = match serde_json::from_str(data) {
        Ok(e) => e,
        Err(e) => {
            error!(provider = %provider_id, event = event_type, "Parse SSE event: {}", e);
            return false;
        }
    };

    match event_type {
        "full_sync" => {
            if event.models.is_empty() {
                debug!(provider = %provider_id, "SSE full_sync empty — skipping");
                return false;
            }
            let n_models = event.models.len();
            let n_inst: usize = event.models.values().map(|v| v.len()).sum();
            info!(provider = %provider_id, models = n_models, instances = n_inst, "SSE full_sync");
            *local = event.models;
            true
        }
        "instance_add" => {
            let model = match &event.model {
                Some(m) => m.clone(),
                None => { warn!(provider = %provider_id, "instance_add missing model"); return false; }
            };
            let inst = match &event.instance {
                Some(i) => i.clone(),
                None => { warn!(provider = %provider_id, "instance_add missing instance"); return false; }
            };
            info!(provider = %provider_id, model = %model, addr = %inst.address, "SSE instance_add");
            let entries = local.entry(model).or_default();
            // Upsert by address
            if let Some(existing) = entries.iter_mut().find(|e| e.address == inst.address) {
                *existing = inst;
            } else {
                entries.push(inst);
            }
            true
        }
        "instance_del" => {
            let model = match &event.model {
                Some(m) => m.clone(),
                None => { warn!(provider = %provider_id, "instance_del missing model"); return false; }
            };
            let inst = match &event.instance {
                Some(i) => i.clone(),
                None => { warn!(provider = %provider_id, "instance_del missing instance"); return false; }
            };
            info!(provider = %provider_id, model = %model, addr = %inst.address, "SSE instance_del");
            if let Some(entries) = local.get_mut(&model) {
                entries.retain(|e| e.address != inst.address);
                if entries.is_empty() {
                    local.remove(&model);
                }
            }
            true
        }
        "model_remove" => {
            let model = match &event.model {
                Some(m) => m.clone(),
                None => { warn!(provider = %provider_id, "model_remove missing model"); return false; }
            };
            info!(provider = %provider_id, model = %model, "SSE model_remove");
            local.remove(&model);
            true
        }
        _ => {
            debug!(provider = %provider_id, event = event_type, "Unknown SSE event");
            false
        }
    }
}

/// Rebuild registry from local instance map and swap router.
fn rebuild_from_local(state: &AppState, provider_id: &str, local: &InstanceMap) {
    let result = instances_to_discovery_result(provider_id, local);
    apply_discovery_result(state, provider_id, DiscoveryEvent::FullSync(result));
}

/// Fetch /prism and apply as full sync.
fn poll_and_apply(state: &AppState, provider_id: &str, sidecar_url: &str) -> Result<(), String> {
    let payload = fetch_prism_payload(sidecar_url)?;
    let n = payload.variants.len();
    let result = payload_to_discovery_result(&payload, provider_id);
    apply_discovery_result(state, provider_id, DiscoveryEvent::FullSync(result));
    info!(provider = %provider_id, variants = n, "Poll refresh done");
    Ok(())
}
