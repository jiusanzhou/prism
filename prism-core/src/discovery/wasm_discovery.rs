//! WASM-based discovery plugin (gated behind `wasm` feature).
//!
//! Allows users to supply a compiled WASM module that exports a `discover`
//! function returning JSON-encoded scheduling objects. This enables dynamic
//! discovery from databases, Kubernetes APIs, service meshes, etc.
//!
//! ## WASM Module Contract
//!
//! The WASM module must export:
//!
//! ```text
//! discover() -> *const u8   // pointer to a JSON-encoded DiscoveryResult
//! result_len() -> u32       // length of the JSON result
//! ```
//!
//! The JSON schema matches the serialized form of [`DiscoveryResult`].
//!
//! ## Host Functions
//!
//! The host provides the following imports to the WASM module:
//!
//! ```text
//! env.log(ptr: *const u8, len: u32)           // log a message
//! env.get_config(ptr: *const u8, len: u32) -> i32  // read host config
//! ```

use crate::discovery::{Discovery, DiscoveryError, DiscoveryResult};

/// WASM-based discovery plugin.
///
/// Loads a compiled WASM module and calls its `discover()` export to obtain
/// scheduling objects. The WASM module runs in a sandboxed environment with
/// limited host function access.
pub struct WasmDiscovery {
    /// Display name for this discovery instance.
    name: String,
    /// Path to the compiled WASM module (.wasm file).
    module_path: String,
    /// Optional configuration passed to the WASM module via host functions.
    config: Option<String>,
}

impl WasmDiscovery {
    /// Create a new WASM discovery plugin.
    ///
    /// # Arguments
    /// * `name` — human-readable name for logging
    /// * `module_path` — path to the `.wasm` file
    /// * `config` — optional JSON config string passed to the module
    pub fn new(name: String, module_path: String, config: Option<String>) -> Self {
        Self {
            name,
            module_path,
            config,
        }
    }

    /// Get the configured module path.
    pub fn module_path(&self) -> &str {
        &self.module_path
    }

    /// Get the optional configuration string.
    pub fn config(&self) -> Option<&str> {
        self.config.as_deref()
    }
}

impl Discovery for WasmDiscovery {
    fn name(&self) -> &str {
        &self.name
    }

    fn discover(&self) -> Result<DiscoveryResult, DiscoveryError> {
        // TODO: Full implementation with wasmtime
        //
        // High-level flow:
        // 1. Load the WASM module from `self.module_path`
        // 2. Instantiate with host functions (log, get_config)
        // 3. Call the exported `discover()` function
        // 4. Read the JSON result from WASM memory
        // 5. Deserialize into DiscoveryResult
        //
        // For now, return an error indicating this is not yet implemented.
        Err(DiscoveryError::NotSupported(format!(
            "WASM discovery '{}' is not yet implemented (module: {})",
            self.name, self.module_path
        )))
    }

    fn supports_watch(&self) -> bool {
        // WASM modules could support watch via a `watch()` export + callback,
        // but this is not yet implemented.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_discovery_creation() {
        let d = WasmDiscovery::new(
            "k8s-discovery".to_string(),
            "/opt/plugins/k8s.wasm".to_string(),
            Some(r#"{"namespace":"default"}"#.to_string()),
        );

        assert_eq!(d.name(), "k8s-discovery");
        assert_eq!(d.module_path(), "/opt/plugins/k8s.wasm");
        assert_eq!(d.config(), Some(r#"{"namespace":"default"}"#));
    }

    #[test]
    fn test_wasm_discovery_not_yet_implemented() {
        let d = WasmDiscovery::new(
            "test".to_string(),
            "test.wasm".to_string(),
            None,
        );

        let err = d.discover().unwrap_err();
        match err {
            DiscoveryError::NotSupported(msg) => {
                assert!(msg.contains("not yet implemented"));
                assert!(msg.contains("test.wasm"));
            }
            _ => panic!("expected NotSupported error"),
        }
    }

    #[test]
    fn test_wasm_discovery_no_watch_support() {
        let d = WasmDiscovery::new("test".to_string(), "test.wasm".to_string(), None);
        assert!(!d.supports_watch());
    }
}
