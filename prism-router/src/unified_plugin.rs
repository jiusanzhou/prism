//! Unified plugin registration framework.
//!
//! Provides a single entry point for registering WASM plugins that can
//! participate in multiple phases: routing, proxy, and analysis.
//!
//! Each phase uses its own ABI:
//! - Proxy: proxy-wasm ABI (compatible with Higress/Envoy)
//! - Route: Prism routing ABI (RoutingPlugin trait)
//! - Analyze: Prism analyzer ABI
//!
//! A single .wasm module can implement multiple ABIs simultaneously.
//! The host auto-detects which exports are present and registers accordingly.

use serde::{Deserialize, Serialize};

#[cfg(feature = "wasm")]
use crate::codec::ProxyPlugin;

/// Which phases a plugin participates in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginPhase {
    /// Proxy layer — intercept/transform HTTP request/response.
    /// Uses proxy-wasm ABI.
    Proxy,
    /// Routing layer — filter/score model candidates.
    /// Uses Prism routing ABI.
    Route {
        /// Score weight (for Score extension point).
        #[serde(default = "default_weight")]
        weight: f32,
        /// Which routing stage(s) this plugin participates in.
        #[serde(default)]
        stages: Vec<RouteStage>,
    },
    /// Analysis layer — extract request features.
    /// Uses Prism analyzer ABI.
    Analyze,
}

fn default_weight() -> f32 {
    1.0
}

/// Routing stages (maps to RoutingPlugin extension points).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteStage {
    PreFilter,
    Filter,
    PostFilter,
    Score,
    Reserve,
}

/// Descriptor for a single plugin — the unit of configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDescriptor {
    /// Plugin name (unique identifier).
    pub name: String,
    /// Where the plugin code comes from.
    pub source: PluginSource,
    /// Which phases this plugin participates in.
    pub phases: Vec<PluginPhase>,
    /// Plugin-specific configuration (passed to on_config).
    #[serde(default)]
    pub config: serde_json::Value,
    /// Whether this plugin is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Execution priority (lower = earlier). Default 100.
    #[serde(default = "default_priority")]
    pub priority: u32,
}

fn default_true() -> bool {
    true
}

fn default_priority() -> u32 {
    100
}

/// Where the plugin code comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginSource {
    /// WASM module file path.
    Wasm(String),
    /// Built-in Rust plugin (referenced by name).
    Builtin(String),
}

/// Result of loading a plugin — tracks which systems it was registered with.
#[derive(Debug)]
pub struct LoadedPlugin {
    pub name: String,
    pub source: PluginSource,
    pub registered_phases: Vec<PluginPhase>,
}

/// Analyzer plugin trait (simplified).
/// Bridges WASM analyzer modules to the unified plugin system.
pub trait AnalyzerPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn analyze(
        &self,
        messages: &[prism_core::analyzer::ChatMessage],
    ) -> Option<prism_core::analyzer::RequestProfile>;
}

/// Result of loading all plugins — grouped by phase for consumption.
pub struct LoadResult {
    /// Proxy plugins ready to add to ProxyPluginChain.
    pub proxy_plugins: Vec<(String, Box<dyn crate::codec::ProxyPlugin>)>,
    /// Route plugin adapters ready to add to core PluginFramework.
    /// Each entry is (name, weight, Box<dyn RoutingPlugin>).
    pub route_plugins: Vec<(String, f32, Box<dyn prism_core::plugin::RoutingPlugin>)>,
    /// Analyzer plugins.
    pub analyzers: Vec<(String, Box<dyn AnalyzerPlugin>)>,
    /// Errors encountered during loading.
    pub errors: Vec<(String, String)>,
}

/// Detected capabilities from WASM module exports.
#[derive(Debug, Clone)]
pub struct DetectedCapabilities {
    pub has_proxy: bool,
    pub has_route: bool,
    pub has_analyze: bool,
}

/// Detect which phases a WASM module supports based on its exports.
pub fn detect_capabilities(module_exports: &[String]) -> DetectedCapabilities {
    let has = |name: &str| module_exports.iter().any(|e| e == name);

    DetectedCapabilities {
        // Proxy phase: proxy-wasm ABI exports
        has_proxy: has("proxy_on_request_headers"),
        // Route phase: prism routing exports
        has_route: has("prism_route_filter") || has("prism_route_score"),
        // Analyze phase: prism analyzer export
        has_analyze: has("analyze"),
    }
}

/// Validate that declared phases are supported by the WASM module's exports.
/// Returns a list of phases that are declared but NOT supported.
pub fn validate_phases(
    declared: &[PluginPhase],
    caps: &DetectedCapabilities,
) -> Vec<PluginPhase> {
    let mut unsupported = Vec::new();
    for phase in declared {
        let supported = match phase {
            PluginPhase::Proxy => caps.has_proxy,
            PluginPhase::Route { .. } => caps.has_route,
            PluginPhase::Analyze => caps.has_analyze,
        };
        if !supported {
            unsupported.push(phase.clone());
        }
    }
    unsupported
}

/// The unified plugin registry — manages all plugins across all phases.
pub struct PluginRegistry {
    /// All registered plugin descriptors.
    descriptors: Vec<PluginDescriptor>,
    /// Successfully loaded plugins.
    loaded: Vec<LoadedPlugin>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            descriptors: Vec::new(),
            loaded: Vec::new(),
        }
    }

    /// Register a plugin descriptor. Does not load the WASM module yet.
    pub fn register(&mut self, descriptor: PluginDescriptor) {
        self.descriptors.push(descriptor);
    }

    /// Get all loaded plugins info.
    pub fn loaded_plugins(&self) -> &[LoadedPlugin] {
        &self.loaded
    }

    /// List all registered descriptors.
    pub fn descriptors(&self) -> &[PluginDescriptor] {
        &self.descriptors
    }

    /// Get enabled descriptors sorted by priority (lower = earlier).
    pub fn enabled_descriptors(&self) -> Vec<&PluginDescriptor> {
        let mut result: Vec<&PluginDescriptor> =
            self.descriptors.iter().filter(|d| d.enabled).collect();
        result.sort_by_key(|d| d.priority);
        result
    }

    /// Enable or disable a plugin by name.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) {
        for desc in &mut self.descriptors {
            if desc.name == name {
                desc.enabled = enabled;
            }
        }
    }

    /// Load all registered plugins and create phase-specific instances.
    ///
    /// For WASM sources, loads the .wasm bytes and inspects exports to verify
    /// declared phases. For builtin sources, records the registration without
    /// creating instances (builtin plugins are created externally).
    ///
    /// Phase 1 implementation:
    /// - Proxy phase: fully supported via WasmProxyPlugin
    /// - Route phase: TODO (needs host function ABI definition)
    /// - Analyze phase: TODO (can integrate with prism-wasm-analyzer)
    pub fn load_all(&mut self) -> LoadResult {
        let mut result = LoadResult {
            proxy_plugins: Vec::new(),
            route_plugins: Vec::new(),
            analyzers: Vec::new(),
            errors: Vec::new(),
        };

        let descriptors: Vec<_> = self.enabled_descriptors().into_iter().cloned().collect();

        for desc in &descriptors {
            match &desc.source {
                PluginSource::Builtin(_builtin_name) => {
                    // Builtin plugins are registered externally.
                    // Just record them as loaded.
                    self.loaded.push(LoadedPlugin {
                        name: desc.name.clone(),
                        source: desc.source.clone(),
                        registered_phases: desc.phases.clone(),
                    });
                }
                PluginSource::Wasm(path) => {
                    self.load_wasm_plugin(&desc, path, &mut result);
                }
            }
        }

        result
    }

    /// Load a single WASM plugin and register it for its declared phases.
    #[allow(unused_variables)]
    fn load_wasm_plugin(
        &mut self,
        desc: &PluginDescriptor,
        path: &str,
        result: &mut LoadResult,
    ) {
        #[cfg(feature = "wasm")]
        {
            // Read WASM bytes
            let wasm_bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    result.errors.push((
                        desc.name.clone(),
                        format!("Failed to read WASM file '{}': {}", path, e),
                    ));
                    return;
                }
            };

            // Detect capabilities by inspecting module exports
            let engine = wasmtime::Engine::default();
            let module = match wasmtime::Module::new(&engine, &wasm_bytes) {
                Ok(m) => m,
                Err(e) => {
                    result.errors.push((
                        desc.name.clone(),
                        format!("Failed to compile WASM module: {}", e),
                    ));
                    return;
                }
            };

            let exports: Vec<String> = module
                .exports()
                .map(|e| e.name().to_string())
                .collect();
            let caps = detect_capabilities(&exports);

            // Validate declared phases
            let unsupported = validate_phases(&desc.phases, &caps);
            if !unsupported.is_empty() {
                let phase_names: Vec<String> = unsupported
                    .iter()
                    .map(|p| format!("{:?}", p))
                    .collect();
                result.errors.push((
                    desc.name.clone(),
                    format!(
                        "WASM module missing exports for declared phases: [{}]",
                        phase_names.join(", ")
                    ),
                ));
                return;
            }

            let mut registered_phases = Vec::new();

            for phase in &desc.phases {
                match phase {
                    PluginPhase::Proxy => {
                        match crate::codec::wasm_host::WasmProxyPlugin::new(
                            &desc.name,
                            &wasm_bytes,
                        ) {
                            Ok(mut plugin) => {
                                // Apply config if present
                                if !desc.config.is_null() {
                                    if let Err(e) = plugin.on_config(&desc.config) {
                                        result.errors.push((
                                            desc.name.clone(),
                                            format!(
                                                "Proxy plugin config error: {}",
                                                e
                                            ),
                                        ));
                                        continue;
                                    }
                                }
                                result
                                    .proxy_plugins
                                    .push((desc.name.clone(), Box::new(plugin)));
                                registered_phases.push(phase.clone());
                            }
                            Err(e) => {
                                result.errors.push((
                                    desc.name.clone(),
                                    format!(
                                        "Failed to create proxy plugin: {}",
                                        e
                                    ),
                                ));
                            }
                        }
                    }
                    PluginPhase::Route { .. } => {
                        // TODO: Route phase WASM adapter requires defining host function ABI
                        // for prism_route_filter / prism_route_score.
                        // For now, record that this phase was requested but not loaded.
                        result.errors.push((
                            desc.name.clone(),
                            "Route phase WASM adapter not yet implemented".to_string(),
                        ));
                    }
                    PluginPhase::Analyze => {
                        // TODO: Integrate with prism-wasm-analyzer ABI.
                        // The analyze export uses a custom ABI (alloc/analyze/dealloc)
                        // which differs from proxy-wasm. Integration planned for Phase 2.
                        result.errors.push((
                            desc.name.clone(),
                            "Analyze phase WASM adapter not yet implemented".to_string(),
                        ));
                    }
                }
            }

            self.loaded.push(LoadedPlugin {
                name: desc.name.clone(),
                source: desc.source.clone(),
                registered_phases,
            });
        }

        #[cfg(not(feature = "wasm"))]
        {
            result.errors.push((
                desc.name.clone(),
                "WASM support not enabled (compile with --features wasm)".to_string(),
            ));
        }
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── PluginPhase serde tests ──

    #[test]
    fn test_phase_proxy_serde() {
        let phase = PluginPhase::Proxy;
        let json = serde_json::to_string(&phase).unwrap();
        assert_eq!(json, r#""proxy""#);
        let parsed: PluginPhase = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, PluginPhase::Proxy);
    }

    #[test]
    fn test_phase_analyze_serde() {
        let phase = PluginPhase::Analyze;
        let json = serde_json::to_string(&phase).unwrap();
        assert_eq!(json, r#""analyze""#);
        let parsed: PluginPhase = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, PluginPhase::Analyze);
    }

    #[test]
    fn test_phase_route_serde_defaults() {
        let json = r#"{"route":{}}"#;
        let parsed: PluginPhase = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed,
            PluginPhase::Route {
                weight: 1.0,
                stages: vec![],
            }
        );
    }

    #[test]
    fn test_phase_route_serde_with_values() {
        let phase = PluginPhase::Route {
            weight: 2.5,
            stages: vec![RouteStage::Filter, RouteStage::Score],
        };
        let json = serde_json::to_string(&phase).unwrap();
        let parsed: PluginPhase = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, phase);
    }

    #[test]
    fn test_phase_route_all_stages() {
        let phase = PluginPhase::Route {
            weight: 1.0,
            stages: vec![
                RouteStage::PreFilter,
                RouteStage::Filter,
                RouteStage::PostFilter,
                RouteStage::Score,
                RouteStage::Reserve,
            ],
        };
        let json = serde_json::to_string(&phase).unwrap();
        let parsed: PluginPhase = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, phase);
    }

    // ── RouteStage serde tests ──

    #[test]
    fn test_route_stage_serde() {
        let cases = vec![
            (RouteStage::PreFilter, r#""pre_filter""#),
            (RouteStage::Filter, r#""filter""#),
            (RouteStage::PostFilter, r#""post_filter""#),
            (RouteStage::Score, r#""score""#),
            (RouteStage::Reserve, r#""reserve""#),
        ];
        for (stage, expected_json) in cases {
            let json = serde_json::to_string(&stage).unwrap();
            assert_eq!(json, expected_json);
            let parsed: RouteStage = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, stage);
        }
    }

    // ── PluginSource serde tests ──

    #[test]
    fn test_source_wasm_serde() {
        let source = PluginSource::Wasm("/path/to/plugin.wasm".to_string());
        let json = serde_json::to_string(&source).unwrap();
        let parsed: PluginSource = serde_json::from_str(&json).unwrap();
        match parsed {
            PluginSource::Wasm(path) => assert_eq!(path, "/path/to/plugin.wasm"),
            _ => panic!("Expected Wasm source"),
        }
    }

    #[test]
    fn test_source_builtin_serde() {
        let source = PluginSource::Builtin("auth-header".to_string());
        let json = serde_json::to_string(&source).unwrap();
        let parsed: PluginSource = serde_json::from_str(&json).unwrap();
        match parsed {
            PluginSource::Builtin(name) => assert_eq!(name, "auth-header"),
            _ => panic!("Expected Builtin source"),
        }
    }

    // ── PluginDescriptor serde tests ──

    #[test]
    fn test_descriptor_full_serde() {
        let desc = PluginDescriptor {
            name: "my-plugin".to_string(),
            source: PluginSource::Wasm("plugins/my-plugin.wasm".to_string()),
            phases: vec![
                PluginPhase::Proxy,
                PluginPhase::Route {
                    weight: 2.0,
                    stages: vec![RouteStage::Filter, RouteStage::Score],
                },
            ],
            config: serde_json::json!({"key": "value", "count": 42}),
            enabled: true,
            priority: 50,
        };
        let json = serde_json::to_string_pretty(&desc).unwrap();
        let parsed: PluginDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "my-plugin");
        assert_eq!(parsed.priority, 50);
        assert_eq!(parsed.phases.len(), 2);
        assert!(parsed.enabled);
    }

    #[test]
    fn test_descriptor_defaults() {
        let json = r#"{
            "name": "minimal",
            "source": {"builtin": "test"},
            "phases": ["proxy"]
        }"#;
        let parsed: PluginDescriptor = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.name, "minimal");
        assert!(parsed.enabled); // default true
        assert_eq!(parsed.priority, 100); // default 100
        assert_eq!(parsed.config, serde_json::Value::Null); // default null
    }

    #[test]
    fn test_descriptor_disabled() {
        let json = r#"{
            "name": "disabled-plugin",
            "source": {"builtin": "test"},
            "phases": ["proxy"],
            "enabled": false
        }"#;
        let parsed: PluginDescriptor = serde_json::from_str(json).unwrap();
        assert!(!parsed.enabled);
    }

    // ── PluginRegistry tests ──

    #[test]
    fn test_registry_new_is_empty() {
        let registry = PluginRegistry::new();
        assert!(registry.descriptors().is_empty());
        assert!(registry.loaded_plugins().is_empty());
    }

    #[test]
    fn test_registry_register() {
        let mut registry = PluginRegistry::new();
        registry.register(PluginDescriptor {
            name: "plugin-a".to_string(),
            source: PluginSource::Builtin("a".to_string()),
            phases: vec![PluginPhase::Proxy],
            config: serde_json::Value::Null,
            enabled: true,
            priority: 100,
        });
        registry.register(PluginDescriptor {
            name: "plugin-b".to_string(),
            source: PluginSource::Builtin("b".to_string()),
            phases: vec![PluginPhase::Analyze],
            config: serde_json::Value::Null,
            enabled: true,
            priority: 50,
        });

        assert_eq!(registry.descriptors().len(), 2);
        assert_eq!(registry.descriptors()[0].name, "plugin-a");
        assert_eq!(registry.descriptors()[1].name, "plugin-b");
    }

    #[test]
    fn test_registry_enabled_descriptors_sorted_by_priority() {
        let mut registry = PluginRegistry::new();
        registry.register(PluginDescriptor {
            name: "low-priority".to_string(),
            source: PluginSource::Builtin("a".to_string()),
            phases: vec![PluginPhase::Proxy],
            config: serde_json::Value::Null,
            enabled: true,
            priority: 200,
        });
        registry.register(PluginDescriptor {
            name: "high-priority".to_string(),
            source: PluginSource::Builtin("b".to_string()),
            phases: vec![PluginPhase::Proxy],
            config: serde_json::Value::Null,
            enabled: true,
            priority: 10,
        });
        registry.register(PluginDescriptor {
            name: "disabled".to_string(),
            source: PluginSource::Builtin("c".to_string()),
            phases: vec![PluginPhase::Proxy],
            config: serde_json::Value::Null,
            enabled: false,
            priority: 1,
        });

        let enabled = registry.enabled_descriptors();
        assert_eq!(enabled.len(), 2);
        assert_eq!(enabled[0].name, "high-priority");
        assert_eq!(enabled[1].name, "low-priority");
    }

    #[test]
    fn test_registry_set_enabled() {
        let mut registry = PluginRegistry::new();
        registry.register(PluginDescriptor {
            name: "toggle-me".to_string(),
            source: PluginSource::Builtin("test".to_string()),
            phases: vec![PluginPhase::Proxy],
            config: serde_json::Value::Null,
            enabled: true,
            priority: 100,
        });

        assert_eq!(registry.enabled_descriptors().len(), 1);

        registry.set_enabled("toggle-me", false);
        assert_eq!(registry.enabled_descriptors().len(), 0);

        registry.set_enabled("toggle-me", true);
        assert_eq!(registry.enabled_descriptors().len(), 1);
    }

    // ── LoadResult tests ──

    #[test]
    fn test_load_empty_registry() {
        let mut registry = PluginRegistry::new();
        let result = registry.load_all();
        assert!(result.proxy_plugins.is_empty());
        assert!(result.route_plugins.is_empty());
        assert!(result.analyzers.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_load_builtin_plugins() {
        let mut registry = PluginRegistry::new();
        registry.register(PluginDescriptor {
            name: "builtin-proxy".to_string(),
            source: PluginSource::Builtin("auth-header".to_string()),
            phases: vec![PluginPhase::Proxy],
            config: serde_json::Value::Null,
            enabled: true,
            priority: 100,
        });

        let result = registry.load_all();
        // Builtin plugins are recorded but don't produce instances
        // (they are created externally and added to their respective systems).
        assert!(result.proxy_plugins.is_empty());
        assert!(result.errors.is_empty());
        assert_eq!(registry.loaded_plugins().len(), 1);
        assert_eq!(registry.loaded_plugins()[0].name, "builtin-proxy");
    }

    #[test]
    fn test_load_disabled_plugins_skipped() {
        let mut registry = PluginRegistry::new();
        registry.register(PluginDescriptor {
            name: "disabled-plugin".to_string(),
            source: PluginSource::Builtin("test".to_string()),
            phases: vec![PluginPhase::Proxy],
            config: serde_json::Value::Null,
            enabled: false,
            priority: 100,
        });

        let result = registry.load_all();
        assert!(result.proxy_plugins.is_empty());
        assert!(result.errors.is_empty());
        assert!(registry.loaded_plugins().is_empty());
    }

    #[test]
    fn test_load_wasm_without_feature() {
        // Without the `wasm` feature, loading a WASM plugin should produce an error.
        #[cfg(not(feature = "wasm"))]
        {
            let mut registry = PluginRegistry::new();
            registry.register(PluginDescriptor {
                name: "wasm-plugin".to_string(),
                source: PluginSource::Wasm("nonexistent.wasm".to_string()),
                phases: vec![PluginPhase::Proxy],
                config: serde_json::Value::Null,
                enabled: true,
                priority: 100,
            });

            let result = registry.load_all();
            assert_eq!(result.errors.len(), 1);
            assert!(result.errors[0].1.contains("WASM support not enabled"));
        }
    }

    // ── detect_capabilities tests ──

    #[test]
    fn test_detect_no_capabilities() {
        let exports: Vec<String> = vec!["memory".to_string(), "alloc".to_string()];
        let caps = detect_capabilities(&exports);
        assert!(!caps.has_proxy);
        assert!(!caps.has_route);
        assert!(!caps.has_analyze);
    }

    #[test]
    fn test_detect_proxy_only() {
        let exports: Vec<String> = vec![
            "memory".to_string(),
            "proxy_on_context_create".to_string(),
            "proxy_on_configure".to_string(),
            "proxy_on_request_headers".to_string(),
            "proxy_on_request_body".to_string(),
            "proxy_on_response_headers".to_string(),
            "proxy_on_response_body".to_string(),
        ];
        let caps = detect_capabilities(&exports);
        assert!(caps.has_proxy);
        assert!(!caps.has_route);
        assert!(!caps.has_analyze);
    }

    #[test]
    fn test_detect_route_filter() {
        let exports: Vec<String> = vec![
            "memory".to_string(),
            "prism_route_filter".to_string(),
        ];
        let caps = detect_capabilities(&exports);
        assert!(!caps.has_proxy);
        assert!(caps.has_route);
        assert!(!caps.has_analyze);
    }

    #[test]
    fn test_detect_route_score() {
        let exports: Vec<String> = vec![
            "memory".to_string(),
            "prism_route_score".to_string(),
        ];
        let caps = detect_capabilities(&exports);
        assert!(!caps.has_proxy);
        assert!(caps.has_route);
        assert!(!caps.has_analyze);
    }

    #[test]
    fn test_detect_analyze_only() {
        let exports: Vec<String> = vec![
            "memory".to_string(),
            "alloc".to_string(),
            "analyze".to_string(),
        ];
        let caps = detect_capabilities(&exports);
        assert!(!caps.has_proxy);
        assert!(!caps.has_route);
        assert!(caps.has_analyze);
    }

    #[test]
    fn test_detect_all_capabilities() {
        let exports: Vec<String> = vec![
            "memory".to_string(),
            "proxy_on_request_headers".to_string(),
            "prism_route_filter".to_string(),
            "prism_route_score".to_string(),
            "analyze".to_string(),
            "alloc".to_string(),
        ];
        let caps = detect_capabilities(&exports);
        assert!(caps.has_proxy);
        assert!(caps.has_route);
        assert!(caps.has_analyze);
    }

    #[test]
    fn test_detect_proxy_and_analyze() {
        let exports: Vec<String> = vec![
            "memory".to_string(),
            "proxy_on_request_headers".to_string(),
            "analyze".to_string(),
        ];
        let caps = detect_capabilities(&exports);
        assert!(caps.has_proxy);
        assert!(!caps.has_route);
        assert!(caps.has_analyze);
    }

    // ── validate_phases tests ──

    #[test]
    fn test_validate_all_supported() {
        let caps = DetectedCapabilities {
            has_proxy: true,
            has_route: true,
            has_analyze: true,
        };
        let phases = vec![PluginPhase::Proxy, PluginPhase::Analyze];
        let unsupported = validate_phases(&phases, &caps);
        assert!(unsupported.is_empty());
    }

    #[test]
    fn test_validate_proxy_unsupported() {
        let caps = DetectedCapabilities {
            has_proxy: false,
            has_route: true,
            has_analyze: true,
        };
        let phases = vec![PluginPhase::Proxy];
        let unsupported = validate_phases(&phases, &caps);
        assert_eq!(unsupported.len(), 1);
        assert_eq!(unsupported[0], PluginPhase::Proxy);
    }

    #[test]
    fn test_validate_route_unsupported() {
        let caps = DetectedCapabilities {
            has_proxy: true,
            has_route: false,
            has_analyze: false,
        };
        let phases = vec![
            PluginPhase::Proxy,
            PluginPhase::Route {
                weight: 1.0,
                stages: vec![RouteStage::Filter],
            },
        ];
        let unsupported = validate_phases(&phases, &caps);
        assert_eq!(unsupported.len(), 1);
        match &unsupported[0] {
            PluginPhase::Route { .. } => {}
            other => panic!("Expected Route, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_empty_phases() {
        let caps = DetectedCapabilities {
            has_proxy: false,
            has_route: false,
            has_analyze: false,
        };
        let phases: Vec<PluginPhase> = vec![];
        let unsupported = validate_phases(&phases, &caps);
        assert!(unsupported.is_empty());
    }

    // ── Full configuration example tests ──

    #[test]
    fn test_full_config_json_array() {
        let json = r#"[
            {
                "name": "auth-injector",
                "source": {"builtin": "auth-header"},
                "phases": ["proxy"],
                "config": {"header": "Authorization", "prefix": "Bearer"},
                "priority": 10
            },
            {
                "name": "openai-compat",
                "source": {"wasm": "plugins/openai-compat.wasm"},
                "phases": ["proxy", "analyze"],
                "config": {},
                "priority": 50
            },
            {
                "name": "quality-router",
                "source": {"wasm": "plugins/quality.wasm"},
                "phases": [
                    {"route": {"weight": 2.0, "stages": ["filter", "score"]}}
                ],
                "priority": 100
            },
            {
                "name": "disabled-plugin",
                "source": {"builtin": "noop"},
                "phases": ["proxy"],
                "enabled": false
            }
        ]"#;

        let descriptors: Vec<PluginDescriptor> = serde_json::from_str(json).unwrap();
        assert_eq!(descriptors.len(), 4);

        // auth-injector
        assert_eq!(descriptors[0].name, "auth-injector");
        assert_eq!(descriptors[0].priority, 10);
        assert!(descriptors[0].enabled);
        match &descriptors[0].source {
            PluginSource::Builtin(name) => assert_eq!(name, "auth-header"),
            _ => panic!("Expected Builtin"),
        }

        // openai-compat — multi-phase plugin
        assert_eq!(descriptors[1].name, "openai-compat");
        assert_eq!(descriptors[1].phases.len(), 2);
        assert_eq!(descriptors[1].phases[0], PluginPhase::Proxy);
        assert_eq!(descriptors[1].phases[1], PluginPhase::Analyze);

        // quality-router — route phase with weight and stages
        assert_eq!(descriptors[2].name, "quality-router");
        assert_eq!(descriptors[2].phases.len(), 1);
        match &descriptors[2].phases[0] {
            PluginPhase::Route { weight, stages } => {
                assert_eq!(*weight, 2.0);
                assert_eq!(stages.len(), 2);
                assert_eq!(stages[0], RouteStage::Filter);
                assert_eq!(stages[1], RouteStage::Score);
            }
            _ => panic!("Expected Route phase"),
        }

        // disabled-plugin
        assert!(!descriptors[3].enabled);
    }

    #[test]
    fn test_registry_with_multiple_plugins() {
        let mut registry = PluginRegistry::new();

        registry.register(PluginDescriptor {
            name: "first".to_string(),
            source: PluginSource::Builtin("a".to_string()),
            phases: vec![PluginPhase::Proxy],
            config: serde_json::Value::Null,
            enabled: true,
            priority: 100,
        });
        registry.register(PluginDescriptor {
            name: "second".to_string(),
            source: PluginSource::Builtin("b".to_string()),
            phases: vec![PluginPhase::Proxy, PluginPhase::Analyze],
            config: serde_json::json!({"key": "value"}),
            enabled: true,
            priority: 50,
        });

        let result = registry.load_all();
        assert!(result.errors.is_empty());
        assert_eq!(registry.loaded_plugins().len(), 2);

        // Check priority ordering: second (50) loads before first (100)
        assert_eq!(registry.loaded_plugins()[0].name, "second");
        assert_eq!(registry.loaded_plugins()[1].name, "first");
    }

    // ── WASM integration tests (requires wasm feature) ──

    #[cfg(feature = "wasm")]
    mod wasm_tests {
        use super::super::*;
        use std::io::Write;

        /// Minimal proxy-wasm WAT module (pass-through).
        const MINIMAL_PROXY_WAT: &str = r#"
            (module
                (func (export "proxy_on_context_create") (param i32 i32))
                (func (export "proxy_on_configure") (param i32 i32) (result i32)
                    (i32.const 1))
                (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
                    (i32.const 0))
                (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
                    (i32.const 0))
                (func (export "proxy_on_response_headers") (param i32 i32 i32) (result i32)
                    (i32.const 0))
                (func (export "proxy_on_response_body") (param i32 i32 i32) (result i32)
                    (i32.const 0))
                (func (export "proxy_on_memory_allocate") (param i32) (result i32)
                    (i32.const 0))
                (memory (export "memory") 1)
            )
        "#;

        /// WAT module with both proxy and analyze exports.
        const MULTI_PHASE_WAT: &str = r#"
            (module
                (func (export "proxy_on_context_create") (param i32 i32))
                (func (export "proxy_on_configure") (param i32 i32) (result i32)
                    (i32.const 1))
                (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
                    (i32.const 0))
                (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
                    (i32.const 0))
                (func (export "proxy_on_response_headers") (param i32 i32 i32) (result i32)
                    (i32.const 0))
                (func (export "proxy_on_response_body") (param i32 i32 i32) (result i32)
                    (i32.const 0))
                (func (export "proxy_on_memory_allocate") (param i32) (result i32)
                    (i32.const 0))
                ;; Analyze export
                (func (export "analyze") (param i32 i32) (result i32)
                    (i32.const 0))
                (func (export "alloc") (param i32) (result i32)
                    (i32.const 0))
                (memory (export "memory") 1)
            )
        "#;

        fn write_wat_to_tempfile(wat: &str) -> (tempfile::NamedTempFile, String) {
            let mut tmp = tempfile::NamedTempFile::new().unwrap();
            // Compile WAT to WASM binary using wasmtime
            let engine = wasmtime::Engine::default();
            let module = wasmtime::Module::new(&engine, wat).unwrap();
            let wasm_bytes = module.serialize().unwrap();
            // Actually, wasmtime::Module::new accepts WAT directly, but
            // we need raw bytes for file loading. Use wat crate or write WAT directly.
            // Since WasmProxyPlugin::new also accepts WAT, just write the WAT.
            tmp.write_all(wat.as_bytes()).unwrap();
            tmp.flush().unwrap();
            let path = tmp.path().to_string_lossy().to_string();
            (tmp, path)
        }

        #[test]
        fn test_wasm_proxy_plugin_loading() {
            let (_tmp, path) = write_wat_to_tempfile(MINIMAL_PROXY_WAT);

            let mut registry = PluginRegistry::new();
            registry.register(PluginDescriptor {
                name: "test-proxy".to_string(),
                source: PluginSource::Wasm(path),
                phases: vec![PluginPhase::Proxy],
                config: serde_json::Value::Null,
                enabled: true,
                priority: 100,
            });

            let result = registry.load_all();
            // Check that proxy plugin was created
            assert_eq!(result.proxy_plugins.len(), 1);
            assert_eq!(result.proxy_plugins[0].0, "test-proxy");
            // No errors for the proxy phase
            let proxy_errors: Vec<_> = result
                .errors
                .iter()
                .filter(|(_, msg)| !msg.contains("not yet implemented"))
                .collect();
            assert!(proxy_errors.is_empty(), "Unexpected errors: {:?}", proxy_errors);
        }

        #[test]
        fn test_wasm_proxy_plugin_functional() {
            let (_tmp, path) = write_wat_to_tempfile(MINIMAL_PROXY_WAT);

            let mut registry = PluginRegistry::new();
            registry.register(PluginDescriptor {
                name: "functional-proxy".to_string(),
                source: PluginSource::Wasm(path),
                phases: vec![PluginPhase::Proxy],
                config: serde_json::Value::Null,
                enabled: true,
                priority: 100,
            });

            let result = registry.load_all();
            assert_eq!(result.proxy_plugins.len(), 1);

            // Use the loaded proxy plugin
            let (name, plugin) = &result.proxy_plugins[0];
            assert_eq!(name, "functional-proxy");

            let mut ctx = crate::codec::ProxyContext::new();
            ctx.request_path = "/v1/chat".to_string();
            let action = plugin.on_request_headers(&mut ctx);
            assert_eq!(action, crate::codec::Action::Continue);
        }

        #[test]
        fn test_wasm_nonexistent_file() {
            let mut registry = PluginRegistry::new();
            registry.register(PluginDescriptor {
                name: "missing".to_string(),
                source: PluginSource::Wasm("/nonexistent/path/plugin.wasm".to_string()),
                phases: vec![PluginPhase::Proxy],
                config: serde_json::Value::Null,
                enabled: true,
                priority: 100,
            });

            let result = registry.load_all();
            assert!(result.proxy_plugins.is_empty());
            assert_eq!(result.errors.len(), 1);
            assert!(result.errors[0].1.contains("Failed to read WASM file"));
        }

        #[test]
        fn test_wasm_invalid_module() {
            let mut tmp = tempfile::NamedTempFile::new().unwrap();
            tmp.write_all(b"not a wasm module").unwrap();
            tmp.flush().unwrap();
            let path = tmp.path().to_string_lossy().to_string();

            let mut registry = PluginRegistry::new();
            registry.register(PluginDescriptor {
                name: "invalid".to_string(),
                source: PluginSource::Wasm(path),
                phases: vec![PluginPhase::Proxy],
                config: serde_json::Value::Null,
                enabled: true,
                priority: 100,
            });

            let result = registry.load_all();
            assert!(result.proxy_plugins.is_empty());
            assert_eq!(result.errors.len(), 1);
            assert!(result.errors[0].1.contains("Failed to compile WASM module"));
        }

        #[test]
        fn test_wasm_multi_phase_plugin() {
            let (_tmp, path) = write_wat_to_tempfile(MULTI_PHASE_WAT);

            let mut registry = PluginRegistry::new();
            registry.register(PluginDescriptor {
                name: "multi-phase".to_string(),
                source: PluginSource::Wasm(path),
                phases: vec![PluginPhase::Proxy, PluginPhase::Analyze],
                config: serde_json::Value::Null,
                enabled: true,
                priority: 100,
            });

            let result = registry.load_all();
            // Proxy should load successfully
            assert_eq!(result.proxy_plugins.len(), 1);
            assert_eq!(result.proxy_plugins[0].0, "multi-phase");
            // Analyze phase not yet implemented, should be in errors
            let analyze_errors: Vec<_> = result
                .errors
                .iter()
                .filter(|(_, msg)| msg.contains("Analyze phase"))
                .collect();
            assert_eq!(analyze_errors.len(), 1);
        }

        #[test]
        fn test_wasm_phase_validation_failure() {
            // A module that only has analyze but declares proxy phase
            let analyze_only_wat = r#"
                (module
                    (func (export "analyze") (param i32 i32) (result i32)
                        (i32.const 0))
                    (func (export "alloc") (param i32) (result i32)
                        (i32.const 0))
                    (memory (export "memory") 1)
                )
            "#;
            let (_tmp, path) = write_wat_to_tempfile(analyze_only_wat);

            let mut registry = PluginRegistry::new();
            registry.register(PluginDescriptor {
                name: "bad-declaration".to_string(),
                source: PluginSource::Wasm(path),
                phases: vec![PluginPhase::Proxy], // declares proxy but doesn't export it
                config: serde_json::Value::Null,
                enabled: true,
                priority: 100,
            });

            let result = registry.load_all();
            assert!(result.proxy_plugins.is_empty());
            assert_eq!(result.errors.len(), 1);
            assert!(result.errors[0].1.contains("missing exports for declared phases"));
        }

        #[test]
        fn test_wasm_capability_detection_from_module() {
            // Test that detect_capabilities works with real module exports
            let engine = wasmtime::Engine::default();
            let module = wasmtime::Module::new(&engine, MULTI_PHASE_WAT).unwrap();
            let exports: Vec<String> =
                module.exports().map(|e| e.name().to_string()).collect();

            let caps = detect_capabilities(&exports);
            assert!(caps.has_proxy);
            assert!(!caps.has_route);
            assert!(caps.has_analyze);
        }
    }
}
