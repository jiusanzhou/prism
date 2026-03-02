//! WASM-based discovery plugin (gated behind `wasm` feature).
//!
//! Allows users to supply a compiled WASM module that exports discovery
//! functions to find scheduling objects (providers, endpoints, accounts,
//! models, variants, pools) from arbitrary sources.
//!
//! ## WASM Module Contract
//!
//! The WASM module must export:
//!
//! ```text
//! memory                                        // WASM linear memory
//! proxy_on_memory_allocate(size: i32) -> i32    // allocate memory (bump allocator)
//! prism_discovery_init(config_ptr: i32, config_len: i32) -> i32  // init, 0 = ok
//! prism_discovery_discover() -> i32             // run discovery, 0 = ok
//! ```
//!
//! ## Host Functions (imported by WASM)
//!
//! ```text
//! env.proxy_log(level: i32, msg_ptr: i32, msg_len: i32) -> i32
//! env.prism_discovery_set_result(json_ptr: i32, json_len: i32) -> i32
//! ```
//!
//! The WASM module calls `prism_discovery_set_result` during `prism_discovery_discover`
//! to pass a JSON-encoded [`DiscoveryResult`] back to the host.

use std::sync::{Arc, Mutex};

use wasmtime::{Caller, Engine, Extern, Linker, Module, Store};

use crate::discovery::{Discovery, DiscoveryError, DiscoveryResult};

// ════════════════════════════════════════════════
// Host state
// ════════════════════════════════════════════════

/// State shared between host functions during a single WASM execution.
struct HostState {
    /// Discovery result JSON set by the WASM module via `prism_discovery_set_result`.
    result_json: Option<Vec<u8>>,
    /// Log messages captured from `proxy_log` calls.
    log_messages: Vec<(i32, String)>,
}

// ════════════════════════════════════════════════
// WasmDiscovery
// ════════════════════════════════════════════════

/// WASM-based discovery plugin.
///
/// Loads a compiled WASM module and calls its discovery exports to obtain
/// scheduling objects. The WASM module runs in a sandboxed wasmtime environment.
pub struct WasmDiscovery {
    /// Display name for this discovery instance.
    name: String,
    /// Wasmtime engine (shared config).
    engine: Engine,
    /// Compiled WASM module.
    module: Module,
    /// Optional configuration string passed to the WASM module during init.
    config: Option<String>,
}

impl WasmDiscovery {
    /// Create a new WASM discovery plugin from raw WASM bytes (or WAT text).
    ///
    /// # Arguments
    /// * `name` — human-readable name for logging
    /// * `wasm_bytes` — compiled `.wasm` binary or WAT text
    pub fn new(name: &str, wasm_bytes: &[u8]) -> Result<Self, String> {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm_bytes)
            .map_err(|e| format!("Failed to compile WASM module: {}", e))?;
        Ok(Self {
            name: name.to_string(),
            engine,
            module,
            config: None,
        })
    }

    /// Set optional configuration to be passed to the WASM module during init.
    pub fn with_config(mut self, config: String) -> Self {
        self.config = Some(config);
        self
    }

    /// Load a WASM discovery plugin from a file path (convenience).
    pub fn from_file(name: &str, path: &str) -> Result<Self, String> {
        let bytes =
            std::fs::read(path).map_err(|e| format!("Failed to read WASM file {}: {}", path, e))?;
        Self::new(name, &bytes)
    }

    /// Register host functions on the linker.
    fn register_host_functions(linker: &mut Linker<Arc<Mutex<HostState>>>) -> Result<(), String> {
        // ── proxy_log(level, msg_ptr, msg_len) -> status ──
        linker
            .func_wrap(
                "env",
                "proxy_log",
                |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                 level: i32,
                 msg_ptr: i32,
                 msg_len: i32|
                 -> i32 {
                    let memory = match caller.get_export("memory") {
                        Some(Extern::Memory(m)) => m,
                        _ => return 2, // BadArgument
                    };
                    let data = memory.data(&caller);
                    let ptr = msg_ptr as usize;
                    let len = msg_len as usize;
                    if ptr + len > data.len() {
                        return 2;
                    }
                    let msg = String::from_utf8_lossy(&data[ptr..ptr + len]).to_string();

                    match level {
                        0 => tracing::trace!(target: "wasm_discovery", "{}", msg),
                        1 => tracing::debug!(target: "wasm_discovery", "{}", msg),
                        2 => tracing::info!(target: "wasm_discovery", "{}", msg),
                        3 => tracing::warn!(target: "wasm_discovery", "{}", msg),
                        4 | 5 => tracing::error!(target: "wasm_discovery", "{}", msg),
                        _ => tracing::info!(target: "wasm_discovery", "[level={}] {}", level, msg),
                    }

                    let state = caller.data().clone();
                    if let Ok(mut s) = state.lock() {
                        s.log_messages.push((level, msg));
                    }
                    0 // Ok
                },
            )
            .map_err(|e| format!("Failed to link proxy_log: {}", e))?;

        // ── prism_discovery_set_result(json_ptr, json_len) -> status ──
        linker
            .func_wrap(
                "env",
                "prism_discovery_set_result",
                |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                 json_ptr: i32,
                 json_len: i32|
                 -> i32 {
                    let memory = match caller.get_export("memory") {
                        Some(Extern::Memory(m)) => m,
                        _ => return 2,
                    };
                    let data = memory.data(&caller);
                    let ptr = json_ptr as usize;
                    let len = json_len as usize;
                    if ptr + len > data.len() {
                        return 2;
                    }
                    let json_bytes = data[ptr..ptr + len].to_vec();
                    let state = caller.data().clone();
                    if let Ok(mut s) = state.lock() {
                        s.result_json = Some(json_bytes);
                    }
                    0 // Ok
                },
            )
            .map_err(|e| format!("Failed to link prism_discovery_set_result: {}", e))?;

        Ok(())
    }
}

impl Discovery for WasmDiscovery {
    fn name(&self) -> &str {
        &self.name
    }

    fn discover(&self) -> Result<DiscoveryResult, DiscoveryError> {
        // 1. Create store with host state
        let host_state = Arc::new(Mutex::new(HostState {
            result_json: None,
            log_messages: Vec::new(),
        }));

        let mut store = Store::new(&self.engine, host_state.clone());
        let mut linker = Linker::new(&self.engine);

        // 2. Register host functions
        Self::register_host_functions(&mut linker)
            .map_err(|e| DiscoveryError::Other(format!("failed to link host functions: {}", e)))?;

        // 3. Instantiate
        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| DiscoveryError::Other(format!("instantiation failed: {}", e)))?;

        // 4. Call prism_discovery_init if config exists
        if let Some(ref config) = self.config {
            if let Ok(init_fn) =
                instance.get_typed_func::<(i32, i32), i32>(&mut store, "prism_discovery_init")
            {
                // Allocate memory in WASM for config
                let alloc_fn = instance
                    .get_typed_func::<i32, i32>(&mut store, "proxy_on_memory_allocate")
                    .map_err(|e| {
                        DiscoveryError::Other(format!("missing proxy_on_memory_allocate: {}", e))
                    })?;

                let config_bytes = config.as_bytes();
                let ptr = alloc_fn
                    .call(&mut store, config_bytes.len() as i32)
                    .map_err(|e| DiscoveryError::Other(format!("alloc failed: {}", e)))?;

                // Write config to WASM memory
                let mem = instance
                    .get_memory(&mut store, "memory")
                    .ok_or_else(|| DiscoveryError::Other("no memory export".into()))?;
                mem.data_mut(&mut store)[ptr as usize..ptr as usize + config_bytes.len()]
                    .copy_from_slice(config_bytes);

                let result = init_fn
                    .call(&mut store, (ptr, config_bytes.len() as i32))
                    .map_err(|e| DiscoveryError::Other(format!("init call failed: {}", e)))?;
                if result != 0 {
                    return Err(DiscoveryError::Other(format!(
                        "WASM init returned non-zero status: {}",
                        result
                    )));
                }
            }
        }

        // 5. Call prism_discovery_discover
        let discover_fn = instance
            .get_typed_func::<(), i32>(&mut store, "prism_discovery_discover")
            .map_err(|e| {
                DiscoveryError::Other(format!("missing prism_discovery_discover export: {}", e))
            })?;

        let status = discover_fn
            .call(&mut store, ())
            .map_err(|e| DiscoveryError::Other(format!("discover call failed: {}", e)))?;

        if status != 0 {
            return Err(DiscoveryError::Other(format!(
                "discover returned non-zero status: {}",
                status
            )));
        }

        // 6. Read result from host state
        let result_json = host_state
            .lock()
            .map_err(|_| DiscoveryError::Other("mutex poisoned".into()))?
            .result_json
            .take()
            .ok_or_else(|| {
                DiscoveryError::Other("WASM module did not call prism_discovery_set_result".into())
            })?;

        // 7. Deserialize
        serde_json::from_slice(&result_json)
            .map_err(|e| DiscoveryError::ParseError(format!("invalid result JSON: {}", e)))
    }

    fn supports_watch(&self) -> bool {
        false
    }
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::InMemoryRegistry;
    use crate::types::*;

    /// Helper: serialize a DiscoveryResult to JSON so we know the exact format
    /// the WASM module must produce.
    fn expected_json() -> String {
        let result = DiscoveryResult {
            providers: vec![Provider {
                id: "test-provider".to_string(),
                name: "Test".to_string(),
                base_url: "https://test.example.com".to_string(),
                auth: AuthConfig::ApiKey {
                    key: "".to_string(),
                    header: "Authorization".to_string(),
                },
                rate_limit: None,
                status: HealthStatus::Healthy,
                metadata: Default::default(),
                protocols: vec![],
            }],
            endpoints: vec![Endpoint {
                id: "test-ep".to_string(),
                provider_id: "test-provider".to_string(),
                base_url: None,
                region: None,
                auth_override: None,
                rate_limit: None,
                priority: 0,
                status: HealthStatus::Healthy,
            }],
            accounts: vec![Account {
                id: "test-acct".to_string(),
                endpoint_id: "test-ep".to_string(),
                auth: AuthConfig::ApiKey {
                    key: "test-key".to_string(),
                    header: "Authorization".to_string(),
                },
                project_id: None,
                rate_limit: None,
                billing_tag: None,
                weight: 1.0,
                status: HealthStatus::Healthy,
                metadata: Default::default(),
            }],
            models: vec![Model {
                id: "test-model".to_string(),
                display_name: "Test Model".to_string(),
                quality_tier: QualityTier::A,
                capabilities: ModelCapabilities::default(),
                pricing: ModelPricing::default(),
                specialties: vec![],
                limits: None,
                status: ModelStatus::Available,
            }],
            variants: vec![ModelVariant {
                id: "test-variant".to_string(),
                model_id: "test-model".to_string(),
                endpoint_id: "test-ep".to_string(),
                account_id: Some("test-acct".to_string()),
                deployment_name: None,
                weight: 1.0,
                performance: ModelPerformance::default(),
                pricing_override: None,
                status: VariantStatus::Active,
                base_url: None,
                upstream_model_name: None,
            }],
            pools: vec![ModelPool {
                id: "test-pool".to_string(),
                name: "Test Pool".to_string(),
                variants: vec!["test-variant".to_string()],
                selection_mode: SelectionMode::Scored,
                description: None,
            }],
        };
        serde_json::to_string(&result).unwrap()
    }

    /// Build WAT source with embedded JSON result.
    fn build_discovery_wat(json: &str) -> String {
        let json_len = json.len();
        // Escape the JSON for WAT data segment (double-backslash any backslashes,
        // but JSON shouldn't have raw backslashes except in escape sequences which
        // WAT handles fine as-is).
        let escaped_json = json.replace('\\', "\\\\").replace('"', "\\\"");
        format!(
            r#"(module
  ;; Import host functions (must come before other definitions)
  (import "env" "proxy_log" (func $log (param i32 i32 i32) (result i32)))
  (import "env" "prism_discovery_set_result" (func $set_result (param i32 i32) (result i32)))

  ;; Memory
  (memory (export "memory") 4)

  ;; Bump allocator
  (global $heap_ptr (mut i32) (i32.const {heap_start}))

  (func (export "proxy_on_memory_allocate") (param $size i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $heap_ptr))
    (global.set $heap_ptr (i32.add (global.get $heap_ptr) (local.get $size)))
    (local.get $ptr)
  )

  ;; Pre-baked JSON result in data segment at offset 0
  (data (i32.const 0) "{escaped_json}")

  ;; Init — accept config, return 0 (success)
  (func (export "prism_discovery_init") (param i32 i32) (result i32)
    (i32.const 0)
  )

  ;; Discover — call set_result with the pre-baked JSON
  (func (export "prism_discovery_discover") (result i32)
    (drop (call $set_result (i32.const 0) (i32.const {json_len})))
    (i32.const 0)
  )
)"#,
            escaped_json = escaped_json,
            json_len = json_len,
            heap_start = json_len + 64, // start heap after the data segment
        )
    }

    // ── Test 1: Create WasmDiscovery from WAT ──

    #[test]
    fn test_wasm_discovery_new() {
        let json = expected_json();
        let wat = build_discovery_wat(&json);
        let discovery = WasmDiscovery::new("test-wasm", wat.as_bytes()).unwrap();
        assert_eq!(discovery.name(), "test-wasm");
    }

    // ── Test 2: Discover returns DiscoveryResult ──

    #[test]
    fn test_wasm_discovery_discover() {
        let json = expected_json();
        let wat = build_discovery_wat(&json);
        let discovery = WasmDiscovery::new("test-wasm", wat.as_bytes()).unwrap();

        let result = discovery.discover().unwrap();
        assert_eq!(result.providers.len(), 1);
        assert_eq!(result.providers[0].id, "test-provider");
        assert_eq!(result.endpoints.len(), 1);
        assert_eq!(result.accounts.len(), 1);
        assert_eq!(result.models.len(), 1);
        assert_eq!(result.models[0].id, "test-model");
        assert_eq!(result.variants.len(), 1);
        assert_eq!(result.pools.len(), 1);
    }

    // ── Test 3: with_config — init is called ──

    #[test]
    fn test_wasm_discovery_with_config() {
        let json = expected_json();
        let wat = build_discovery_wat(&json);
        let discovery = WasmDiscovery::new("test-wasm", wat.as_bytes())
            .unwrap()
            .with_config(r#"{"key":"value"}"#.to_string());

        // Should succeed — init accepts any config and returns 0
        let result = discovery.discover().unwrap();
        assert!(!result.is_empty());
    }

    // ── Test 4: Error handling — invalid WASM ──

    #[test]
    fn test_wasm_discovery_invalid_wasm() {
        let result = WasmDiscovery::new("bad", b"not-valid-wasm");
        assert!(result.is_err());
        let err_msg = result.err().unwrap();
        assert!(err_msg.contains("Failed to compile"));
    }

    // ── Test 5: Error handling — missing discover export ──

    #[test]
    fn test_wasm_discovery_missing_export() {
        // Module with no exports except memory
        let wat = r#"(module
            (import "env" "proxy_log" (func $log (param i32 i32 i32) (result i32)))
            (import "env" "prism_discovery_set_result" (func $set_result (param i32 i32) (result i32)))
            (memory (export "memory") 1)
        )"#;
        let discovery = WasmDiscovery::new("missing", wat.as_bytes()).unwrap();
        let err = discovery.discover().unwrap_err();
        match err {
            DiscoveryError::Other(msg) => {
                assert!(msg.contains("prism_discovery_discover"));
            }
            _ => panic!("expected Other error, got {:?}", err),
        }
    }

    // ── Test 6: Log capture — proxy_log works ──

    #[test]
    fn test_wasm_discovery_proxy_log() {
        let json = expected_json();
        let json_len = json.len();
        let escaped_json = json.replace('\\', "\\\\").replace('"', "\\\"");
        let log_msg = "hello from wasm";
        let log_msg_len = log_msg.len();
        let log_offset = json_len + 64;

        let wat = format!(
            r#"(module
  (import "env" "proxy_log" (func $log (param i32 i32 i32) (result i32)))
  (import "env" "prism_discovery_set_result" (func $set_result (param i32 i32) (result i32)))
  (memory (export "memory") 4)
  (global $heap_ptr (mut i32) (i32.const {heap_start}))
  (func (export "proxy_on_memory_allocate") (param $size i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $heap_ptr))
    (global.set $heap_ptr (i32.add (global.get $heap_ptr) (local.get $size)))
    (local.get $ptr)
  )
  (data (i32.const 0) "{escaped_json}")
  (data (i32.const {log_offset}) "{log_msg}")
  (func (export "prism_discovery_init") (param i32 i32) (result i32) (i32.const 0))
  (func (export "prism_discovery_discover") (result i32)
    ;; Log a message at INFO level (2)
    (drop (call $log (i32.const 2) (i32.const {log_offset}) (i32.const {log_msg_len})))
    ;; Set result
    (drop (call $set_result (i32.const 0) (i32.const {json_len})))
    (i32.const 0)
  )
)"#,
            escaped_json = escaped_json,
            json_len = json_len,
            log_offset = log_offset,
            log_msg = log_msg,
            log_msg_len = log_msg_len,
            heap_start = log_offset + log_msg_len + 64,
        );

        let discovery = WasmDiscovery::new("log-test", wat.as_bytes()).unwrap();
        let result = discovery.discover().unwrap();
        assert!(!result.is_empty());
        // The proxy_log call should have been invoked without errors.
        // (We can't easily inspect the captured logs from outside since HostState
        // is internal, but the fact that discover() succeeded means proxy_log worked.)
    }

    // ── Test 7: Integration with Registry ──

    #[test]
    fn test_wasm_discovery_registry_integration() {
        let json = expected_json();
        let wat = build_discovery_wat(&json);
        let discovery = WasmDiscovery::new("wasm-source", wat.as_bytes()).unwrap();

        let mut registry = InMemoryRegistry::new();
        let result = discovery.discover().unwrap();
        registry.apply_discovery(&result);

        assert_eq!(registry.providers.len(), 1);
        assert_eq!(registry.models.len(), 1);
        assert_eq!(registry.variants.len(), 1);
        assert_eq!(registry.pools.len(), 1);

        // Verify we can get candidates
        use crate::registry::ModelRegistryTrait;
        let candidates = registry.get_candidates_for_model("test-model");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].variant.id, "test-variant");
        assert_eq!(candidates[0].provider.id, "test-provider");
    }

    // ── Test 8: refresh_from_discoveries with WasmDiscovery ──

    #[test]
    fn test_wasm_discovery_refresh_from_discoveries() {
        let json = expected_json();
        let wat = build_discovery_wat(&json);
        let discovery = WasmDiscovery::new("wasm-source", wat.as_bytes()).unwrap();

        let mut registry = InMemoryRegistry::new();
        let discoveries: Vec<Box<dyn Discovery>> = vec![Box::new(discovery)];
        let errors = registry.refresh_from_discoveries(&discoveries);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);

        assert_eq!(registry.providers.len(), 1);
        assert_eq!(registry.models.len(), 1);
    }

    // ── Test 9: Multiple discover calls (module is re-instantiated each time) ──

    #[test]
    fn test_wasm_discovery_multiple_calls() {
        let json = expected_json();
        let wat = build_discovery_wat(&json);
        let discovery = WasmDiscovery::new("multi", wat.as_bytes()).unwrap();

        for _ in 0..3 {
            let result = discovery.discover().unwrap();
            assert_eq!(result.providers.len(), 1);
        }
    }

    // ── Test 10: Discover without set_result call ──

    #[test]
    fn test_wasm_discovery_no_result_set() {
        let wat = r#"(module
            (import "env" "proxy_log" (func $log (param i32 i32 i32) (result i32)))
            (import "env" "prism_discovery_set_result" (func $set_result (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (global $heap_ptr (mut i32) (i32.const 1024))
            (func (export "proxy_on_memory_allocate") (param $size i32) (result i32)
                (local $ptr i32)
                (local.set $ptr (global.get $heap_ptr))
                (global.set $heap_ptr (i32.add (global.get $heap_ptr) (local.get $size)))
                (local.get $ptr)
            )
            (func (export "prism_discovery_init") (param i32 i32) (result i32) (i32.const 0))
            (func (export "prism_discovery_discover") (result i32)
                ;; Return 0 but never call set_result
                (i32.const 0)
            )
        )"#;
        let discovery = WasmDiscovery::new("no-result", wat.as_bytes()).unwrap();
        let err = discovery.discover().unwrap_err();
        match err {
            DiscoveryError::Other(msg) => {
                assert!(msg.contains("did not call"));
            }
            _ => panic!("expected Other error"),
        }
    }

    // ── Test 11: Discover returns non-zero status ──

    #[test]
    fn test_wasm_discovery_non_zero_status() {
        let wat = r#"(module
            (import "env" "proxy_log" (func $log (param i32 i32 i32) (result i32)))
            (import "env" "prism_discovery_set_result" (func $set_result (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (global $heap_ptr (mut i32) (i32.const 1024))
            (func (export "proxy_on_memory_allocate") (param $size i32) (result i32)
                (local $ptr i32)
                (local.set $ptr (global.get $heap_ptr))
                (global.set $heap_ptr (i32.add (global.get $heap_ptr) (local.get $size)))
                (local.get $ptr)
            )
            (func (export "prism_discovery_init") (param i32 i32) (result i32) (i32.const 0))
            (func (export "prism_discovery_discover") (result i32)
                (i32.const 1)
            )
        )"#;
        let discovery = WasmDiscovery::new("fail", wat.as_bytes()).unwrap();
        let err = discovery.discover().unwrap_err();
        match err {
            DiscoveryError::Other(msg) => {
                assert!(msg.contains("non-zero"));
            }
            _ => panic!("expected Other error"),
        }
    }
}
