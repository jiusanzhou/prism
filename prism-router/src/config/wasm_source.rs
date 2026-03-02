//! WASM-based configuration source (feature-gated `wasm`).

#[cfg(feature = "wasm")]
mod inner {
    use std::sync::{Arc, Mutex};
    use wasmtime::{Caller, Engine, Linker, Module, Store};

    use crate::config::{ConfigError, ConfigSource, PrismConfig};

    #[allow(dead_code)]
    struct HostState {
        result_json: Option<Vec<u8>>,
        log_messages: Vec<String>,
    }

    #[allow(dead_code)]
    pub struct WasmConfigSource {
        name: String,
        engine: Engine,
        module: Module,
        init_config: Option<String>,
    }

    #[allow(dead_code)]
    impl WasmConfigSource {
        pub fn new(name: &str, wasm_bytes: &[u8]) -> Result<Self, String> {
            let engine = Engine::default();
            let module = Module::new(&engine, wasm_bytes)
                .map_err(|e| format!("Failed to compile WASM module: {}", e))?;
            Ok(Self {
                name: name.to_string(),
                engine,
                module,
                init_config: None,
            })
        }

        pub fn from_file(name: &str, path: &str) -> Result<Self, String> {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("Failed to read WASM file {}: {}", path, e))?;
            Self::new(name, &bytes)
        }

        pub fn with_config(mut self, config: String) -> Self {
            self.init_config = Some(config);
            self
        }
    }

    impl ConfigSource for WasmConfigSource {
        fn name(&self) -> &str {
            &self.name
        }

        fn load(&self) -> Result<PrismConfig, ConfigError> {
            let host_state = Arc::new(Mutex::new(HostState {
                result_json: None,
                log_messages: vec![],
            }));

            let mut store = Store::new(&self.engine, host_state.clone());
            let mut linker = Linker::new(&self.engine);

            // Link proxy_log
            linker
                .func_wrap(
                    "env",
                    "proxy_log",
                    |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                     _level: i32,
                     msg_ptr: i32,
                     msg_len: i32|
                     -> i32 {
                        let mem = caller.get_export("memory").and_then(|e| e.into_memory());
                        if let Some(mem) = mem {
                            let data = mem.data(&caller);
                            if let Some(slice) =
                                data.get(msg_ptr as usize..(msg_ptr + msg_len) as usize)
                            {
                                let msg = String::from_utf8_lossy(slice).to_string();
                                if let Ok(mut state) = caller.data().lock() {
                                    state.log_messages.push(msg);
                                }
                            }
                        }
                        0
                    },
                )
                .map_err(|e| ConfigError::Other(format!("linker error: {}", e)))?;

            // Link prism_config_set_result
            linker
                .func_wrap(
                    "env",
                    "prism_config_set_result",
                    |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                     json_ptr: i32,
                     json_len: i32|
                     -> i32 {
                        let mem = caller.get_export("memory").and_then(|e| e.into_memory());
                        if let Some(mem) = mem {
                            let data = mem.data(&caller);
                            if let Some(slice) =
                                data.get(json_ptr as usize..(json_ptr + json_len) as usize)
                            {
                                let json_bytes = slice.to_vec();
                                if let Ok(mut state) = caller.data().lock() {
                                    state.result_json = Some(json_bytes);
                                }
                                return 0;
                            }
                        }
                        2
                    },
                )
                .map_err(|e| ConfigError::Other(format!("linker error: {}", e)))?;

            // Instantiate
            let instance = linker
                .instantiate(&mut store, &self.module)
                .map_err(|e| ConfigError::Other(format!("instantiation failed: {}", e)))?;

            // Call prism_config_init if config exists
            if let Some(ref config) = self.init_config {
                if let Ok(init_fn) =
                    instance.get_typed_func::<(i32, i32), i32>(&mut store, "prism_config_init")
                {
                    let alloc_fn = instance
                        .get_typed_func::<i32, i32>(&mut store, "proxy_on_memory_allocate")
                        .map_err(|e| {
                            ConfigError::Other(format!("missing proxy_on_memory_allocate: {}", e))
                        })?;
                    let config_bytes = config.as_bytes();
                    let ptr = alloc_fn
                        .call(&mut store, config_bytes.len() as i32)
                        .map_err(|e| ConfigError::Other(format!("alloc failed: {}", e)))?;
                    let mem = instance
                        .get_memory(&mut store, "memory")
                        .ok_or_else(|| ConfigError::Other("no memory export".into()))?;
                    mem.data_mut(&mut store)[ptr as usize..ptr as usize + config_bytes.len()]
                        .copy_from_slice(config_bytes);
                    let result = init_fn
                        .call(&mut store, (ptr, config_bytes.len() as i32))
                        .map_err(|e| ConfigError::Other(format!("init failed: {}", e)))?;
                    if result != 0 {
                        return Err(ConfigError::Other(
                            "WASM config init returned non-zero".into(),
                        ));
                    }
                }
            }

            // Call prism_config_load
            let load_fn = instance
                .get_typed_func::<(), i32>(&mut store, "prism_config_load")
                .map_err(|e| ConfigError::Other(format!("missing prism_config_load: {}", e)))?;
            let status = load_fn
                .call(&mut store, ())
                .map_err(|e| ConfigError::Other(format!("load call failed: {}", e)))?;
            if status != 0 {
                return Err(ConfigError::Other(format!(
                    "load returned status {}",
                    status
                )));
            }

            // Read result
            let result_json = host_state
                .lock()
                .map_err(|_| ConfigError::Other("mutex poisoned".into()))?
                .result_json
                .take()
                .ok_or_else(|| {
                    ConfigError::Other("WASM module did not set config result".into())
                })?;

            serde_json::from_slice(&result_json)
                .map_err(|e| ConfigError::ParseError(format!("invalid config JSON: {}", e)))
        }
    }
}

#[cfg(feature = "wasm")]
pub use inner::WasmConfigSource;

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
#[cfg(feature = "wasm")]
mod tests {
    use super::WasmConfigSource;
    use crate::config::{
        CompositeSource, ConfigPriority, ConfigSource, PrismConfig, ProviderConfig,
    };

    /// Build a WASM module (WAT text) that returns the given PrismConfig as JSON.
    fn make_config_wat(config: &PrismConfig) -> String {
        let json = serde_json::to_vec(config).unwrap();

        // Escape bytes for WAT data segment
        let escaped: String = json.iter().map(|b| format!("\\{:02x}", b)).collect();
        let json_len = json.len();
        let heap_start = json_len + 64;

        format!(
            r#"(module
  (import "env" "proxy_log" (func $log (param i32 i32 i32) (result i32)))
  (import "env" "prism_config_set_result" (func $set_result (param i32 i32) (result i32)))
  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const {heap_start}))
  (data (i32.const 0) "{escaped}")

  (func (export "proxy_on_memory_allocate") (param $size i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $heap))
    (global.set $heap (i32.add (global.get $heap) (local.get $size)))
    (local.get $ptr))

  (func (export "prism_config_init") (param i32 i32) (result i32)
    (i32.const 0))

  (func (export "prism_config_load") (result i32)
    (drop (call $set_result (i32.const 0) (i32.const {json_len})))
    (i32.const 0))
)"#,
            escaped = escaped,
            json_len = json_len,
            heap_start = heap_start,
        )
    }

    // ── Test 1: Basic load ──

    #[test]
    fn test_wasm_config_source_basic() {
        let config = PrismConfig::default();
        let wat = make_config_wat(&config);
        let source = WasmConfigSource::new("test-basic", wat.as_bytes()).unwrap();
        assert_eq!(source.name(), "test-basic");

        let loaded = source.load().unwrap();
        assert_eq!(loaded.server.http_port, config.server.http_port);
        assert_eq!(loaded.server.grpc_port, config.server.grpc_port);
        assert!(loaded.providers.is_empty());
        assert!(loaded.models.is_empty());
    }

    // ── Test 2: With providers ──

    #[test]
    fn test_wasm_config_source_with_providers() {
        let mut config = PrismConfig::default();
        config.server.http_port = 3000;
        config.providers.insert(
            "openai".into(),
            ProviderConfig {
                provider_type: Some("openai".into()),
                api_key: Some("sk-test".into()),
                base_url: "https://api.openai.com/v1".into(),
                timeout_ms: Some(30000),
                protocols: vec!["http".into()],
                headers: std::collections::HashMap::new(),
            },
        );

        let wat = make_config_wat(&config);
        let source = WasmConfigSource::new("with-providers", wat.as_bytes()).unwrap();
        let loaded = source.load().unwrap();

        assert_eq!(loaded.server.http_port, 3000);
        assert_eq!(loaded.providers.len(), 1);
        let p = loaded.providers.get("openai").unwrap();
        assert_eq!(p.base_url, "https://api.openai.com/v1");
        assert_eq!(p.api_key.as_deref(), Some("sk-test"));
    }

    // ── Test 3: With init config ──

    #[test]
    fn test_wasm_config_source_with_init_config() {
        let config = PrismConfig::default();
        let wat = make_config_wat(&config);
        let source = WasmConfigSource::new("with-init", wat.as_bytes())
            .unwrap()
            .with_config(r#"{"key":"value"}"#.to_string());

        // init accepts any config and returns 0, so load should succeed
        let loaded = source.load().unwrap();
        assert_eq!(loaded.server.http_port, config.server.http_port);
    }

    // ── Test 4: Invalid WASM bytes ──

    #[test]
    fn test_wasm_config_source_invalid_wasm() {
        let result = WasmConfigSource::new("bad", b"not-valid-wasm");
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.contains("Failed to compile"));
    }

    // ── Test 5: Missing prism_config_load export ──

    #[test]
    fn test_wasm_config_source_missing_export() {
        let wat = r#"(module
            (import "env" "proxy_log" (func $log (param i32 i32 i32) (result i32)))
            (import "env" "prism_config_set_result" (func $set_result (param i32 i32) (result i32)))
            (memory (export "memory") 1)
        )"#;
        let source = WasmConfigSource::new("missing", wat.as_bytes()).unwrap();
        let err = source.load().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("prism_config_load"),
            "error should mention missing export, got: {}",
            msg
        );
    }

    // ── Test 6: Load succeeds but no set_result called ──

    #[test]
    fn test_wasm_config_source_no_result() {
        let wat = r#"(module
            (import "env" "proxy_log" (func $log (param i32 i32 i32) (result i32)))
            (import "env" "prism_config_set_result" (func $set_result (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (global $heap (mut i32) (i32.const 1024))

            (func (export "proxy_on_memory_allocate") (param $size i32) (result i32)
                (local $ptr i32)
                (local.set $ptr (global.get $heap))
                (global.set $heap (i32.add (global.get $heap) (local.get $size)))
                (local.get $ptr))

            (func (export "prism_config_init") (param i32 i32) (result i32)
                (i32.const 0))

            (func (export "prism_config_load") (result i32)
                (i32.const 0))
        )"#;
        let source = WasmConfigSource::new("no-result", wat.as_bytes()).unwrap();
        let err = source.load().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("did not set config result"),
            "error should mention missing result, got: {}",
            msg
        );
    }

    // ── Test 7: Invalid JSON in set_result ──

    #[test]
    fn test_wasm_config_source_invalid_json() {
        let wat = r#"(module
            (import "env" "proxy_log" (func $log (param i32 i32 i32) (result i32)))
            (import "env" "prism_config_set_result" (func $set_result (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (global $heap (mut i32) (i32.const 1024))
            (data (i32.const 0) "not valid json!!!")

            (func (export "proxy_on_memory_allocate") (param $size i32) (result i32)
                (local $ptr i32)
                (local.set $ptr (global.get $heap))
                (global.set $heap (i32.add (global.get $heap) (local.get $size)))
                (local.get $ptr))

            (func (export "prism_config_init") (param i32 i32) (result i32)
                (i32.const 0))

            (func (export "prism_config_load") (result i32)
                (drop (call $set_result (i32.const 0) (i32.const 17)))
                (i32.const 0))
        )"#;
        let source = WasmConfigSource::new("bad-json", wat.as_bytes()).unwrap();
        let err = source.load().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("parse error") || msg.contains("invalid config JSON"),
            "error should mention parse failure, got: {}",
            msg
        );
    }

    // ── Test 8: CompositeSource integration ──

    #[test]
    fn test_wasm_config_source_composite() {
        let mut config = PrismConfig::default();
        config.server.http_port = 5555;

        let wat = make_config_wat(&config);
        let source = WasmConfigSource::new("wasm-composite", wat.as_bytes()).unwrap();

        let mut composite = CompositeSource::new();
        composite.add_source(ConfigPriority::File, Box::new(source));

        assert_eq!(composite.source_count(), 1);
        let loaded = composite.load().unwrap();
        assert_eq!(loaded.server.http_port, 5555);
    }

    // ── Test 9: from_file with non-existent file ──

    #[test]
    fn test_wasm_config_source_from_file() {
        let result = WasmConfigSource::from_file("missing", "/nonexistent/path/module.wasm");
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(
            err.contains("Failed to read WASM file"),
            "error should mention file read failure, got: {}",
            err
        );
    }
}
