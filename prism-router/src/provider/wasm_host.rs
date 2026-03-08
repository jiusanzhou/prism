//! WASM host implementation for provider plugins.
//!
//! Loads WASM modules that implement the Prism provider ABI and bridges
//! them to the [`ProviderPlugin`] trait.
//!
//! # Provider WASM ABI
//!
//! ## Guest exports (called by host):
//! - `prism_provider_init()` — called once on load
//! - `prism_provider_id() -> ptr` — returns provider type id (len-prefixed string)
//! - `prism_provider_config_schema() -> ptr` — returns JSON config schema
//! - `prism_provider_build_auth(ctx_ptr, ctx_len) -> ptr` — returns JSON auth result
//! - `prism_provider_rewrite_url(ctx_ptr, ctx_len) -> ptr` — returns JSON url result
//! - `prism_provider_transform_request_body(body_ptr, body_len, ctx_ptr, ctx_len) -> ptr`
//! - `prism_provider_transform_response_body(body_ptr, body_len, ctx_ptr, ctx_len) -> ptr`
//! - `prism_provider_transform_stream_chunk(chunk_ptr, chunk_len, ctx_ptr, ctx_len) -> ptr`
//! - `prism_provider_alloc(size) -> ptr` — guest allocator
//!
//! All pointer returns use the format: first 4 bytes = length (LE u32), then data.
//!
//! ## Host functions (imported by guest):
//! - `prism_log(level, msg_ptr, msg_len)` — log a message

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wasmtime::{Caller, Engine, Linker, Module, Store};

use super::{
    AuthResult, ProviderConfigSchema, ProviderPlugin, ProviderRequestContext, UrlRewriteResult,
};

// ════════════════════════════════════════════════
// Host state
// ════════════════════════════════════════════════

#[derive(Debug, Default)]
struct ProviderHostState {
    log_messages: Vec<(i32, String)>,
}

// ════════════════════════════════════════════════
// WasmProviderPlugin
// ════════════════════════════════════════════════

/// WASM-backed provider plugin.
pub struct WasmProviderPlugin {
    name: String,
    engine: Engine,
    module: Module,
    cached_schema: ProviderConfigSchema,
    cached_id: String,
}

impl WasmProviderPlugin {
    /// Load a WASM provider plugin from raw bytes.
    pub fn new(name: &str, wasm_bytes: &[u8]) -> Result<Self, String> {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm_bytes)
            .map_err(|e| format!("Failed to compile provider WASM: {}", e))?;

        let mut plugin = Self {
            name: name.to_string(),
            engine,
            module,
            cached_schema: ProviderConfigSchema::default(),
            cached_id: name.to_string(),
        };

        plugin.init()?;
        Ok(plugin)
    }

    fn init(&mut self) -> Result<(), String> {
        let state = Arc::new(Mutex::new(ProviderHostState::default()));
        let (mut store, instance) = self.create_instance(state)?;

        if let Ok(init_fn) = instance.get_typed_func::<(), ()>(&mut store, "prism_provider_init") {
            init_fn
                .call(&mut store, ())
                .map_err(|e| format!("prism_provider_init failed: {}", e))?;
        }

        if let Ok(id_fn) = instance.get_typed_func::<(), i32>(&mut store, "prism_provider_id") {
            let ptr = id_fn
                .call(&mut store, ())
                .map_err(|e| format!("prism_provider_id failed: {}", e))?;
            if let Some(id) = Self::read_len_prefixed_string(&store, &instance, ptr) {
                self.cached_id = id;
            }
        }

        if let Ok(schema_fn) =
            instance.get_typed_func::<(), i32>(&mut store, "prism_provider_config_schema")
        {
            let ptr = schema_fn
                .call(&mut store, ())
                .map_err(|e| format!("prism_provider_config_schema failed: {}", e))?;
            if let Some(json) = Self::read_len_prefixed_string(&store, &instance, ptr) {
                if let Ok(schema) = serde_json::from_str(&json) {
                    self.cached_schema = schema;
                }
            }
        }

        Ok(())
    }

    fn create_instance(
        &self,
        state: Arc<Mutex<ProviderHostState>>,
    ) -> Result<(Store<Arc<Mutex<ProviderHostState>>>, wasmtime::Instance), String> {
        let mut store = Store::new(&self.engine, state);
        let mut linker = Linker::new(&self.engine);
        Self::register_host_functions(&mut linker)?;
        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| format!("Failed to instantiate provider WASM: {}", e))?;
        Ok((store, instance))
    }

    fn register_host_functions(
        linker: &mut Linker<Arc<Mutex<ProviderHostState>>>,
    ) -> Result<(), String> {
        linker
            .func_wrap(
                "env",
                "prism_log",
                |mut caller: Caller<'_, Arc<Mutex<ProviderHostState>>>,
                 log_level: i32,
                 msg_ptr: i32,
                 msg_len: i32|
                 -> i32 {
                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return 1,
                    };
                    let data = memory.data(&caller);
                    let ptr = msg_ptr as usize;
                    let len = msg_len as usize;
                    if ptr + len > data.len() {
                        return 1;
                    }
                    let msg = String::from_utf8_lossy(&data[ptr..ptr + len]).to_string();
                    match log_level {
                        0 => tracing::trace!(target: "provider_wasm", "{}", msg),
                        1 => tracing::debug!(target: "provider_wasm", "{}", msg),
                        2 => tracing::info!(target: "provider_wasm", "{}", msg),
                        3 => tracing::warn!(target: "provider_wasm", "{}", msg),
                        _ => tracing::error!(target: "provider_wasm", "{}", msg),
                    }
                    let state = caller.data().clone();
                    if let Ok(mut s) = state.lock() {
                        s.log_messages.push((log_level, msg));
                    }
                    0
                },
            )
            .map_err(|e| format!("Failed to link prism_log: {}", e))?;

        Ok(())
    }

    fn write_to_wasm(
        store: &mut Store<Arc<Mutex<ProviderHostState>>>,
        instance: &wasmtime::Instance,
        data: &[u8],
    ) -> Result<i32, String> {
        let alloc = instance
            .get_typed_func::<i32, i32>(store, "prism_provider_alloc")
            .map_err(|_| "Guest does not export prism_provider_alloc".to_string())?;
        let ptr = alloc
            .call(store, data.len() as i32)
            .map_err(|e| format!("prism_provider_alloc failed: {}", e))?;
        let memory = instance
            .get_memory(store, "memory")
            .ok_or("No memory export")?;
        let dest = memory.data_mut(store);
        let offset = ptr as usize;
        if offset + data.len() > dest.len() {
            return Err("WASM memory overflow".to_string());
        }
        dest[offset..offset + data.len()].copy_from_slice(data);
        Ok(ptr)
    }

    fn read_len_prefixed_string(
        store: &Store<Arc<Mutex<ProviderHostState>>>,
        instance: &wasmtime::Instance,
        ptr: i32,
    ) -> Option<String> {
        let bytes = Self::read_len_prefixed_bytes(store, instance, ptr)?;
        String::from_utf8(bytes).ok()
    }

    fn read_len_prefixed_bytes(
        store: &Store<Arc<Mutex<ProviderHostState>>>,
        instance: &wasmtime::Instance,
        ptr: i32,
    ) -> Option<Vec<u8>> {
        let memory = instance.get_memory(store, "memory")?;
        let data = memory.data(store);
        let offset = ptr as usize;
        if offset + 4 > data.len() {
            return None;
        }
        let len = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        if offset + 4 + len > data.len() {
            return None;
        }
        Some(data[offset + 4..offset + 4 + len].to_vec())
    }

    fn call_with_ctx(
        &self,
        func_name: &str,
        ctx: &ProviderRequestContext,
    ) -> Result<String, String> {
        let ctx_json =
            serde_json::to_vec(ctx).map_err(|e| format!("Failed to serialize ctx: {}", e))?;
        let state = Arc::new(Mutex::new(ProviderHostState::default()));
        let (mut store, instance) = self.create_instance(state)?;
        if let Ok(init_fn) = instance.get_typed_func::<(), ()>(&mut store, "prism_provider_init") {
            let _ = init_fn.call(&mut store, ());
        }
        let func = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, func_name)
            .map_err(|_| format!("Guest does not export {}", func_name))?;
        let ctx_ptr = Self::write_to_wasm(&mut store, &instance, &ctx_json)?;
        let result_ptr = func
            .call(&mut store, (ctx_ptr, ctx_json.len() as i32))
            .map_err(|e| format!("{} failed: {}", func_name, e))?;
        Self::read_len_prefixed_string(&store, &instance, result_ptr)
            .ok_or_else(|| format!("{} returned invalid pointer", func_name))
    }

    fn call_with_body_and_ctx(
        &self,
        func_name: &str,
        body: &[u8],
        ctx: &ProviderRequestContext,
    ) -> Result<Vec<u8>, String> {
        let ctx_json =
            serde_json::to_vec(ctx).map_err(|e| format!("Failed to serialize ctx: {}", e))?;
        let state = Arc::new(Mutex::new(ProviderHostState::default()));
        let (mut store, instance) = self.create_instance(state)?;
        if let Ok(init_fn) = instance.get_typed_func::<(), ()>(&mut store, "prism_provider_init") {
            let _ = init_fn.call(&mut store, ());
        }
        let func = instance
            .get_typed_func::<(i32, i32, i32, i32), i32>(&mut store, func_name)
            .map_err(|_| format!("Guest does not export {}", func_name))?;
        let body_ptr = Self::write_to_wasm(&mut store, &instance, body)?;
        let ctx_ptr = Self::write_to_wasm(&mut store, &instance, &ctx_json)?;
        let result_ptr = func
            .call(
                &mut store,
                (body_ptr, body.len() as i32, ctx_ptr, ctx_json.len() as i32),
            )
            .map_err(|e| format!("{} failed: {}", func_name, e))?;
        Self::read_len_prefixed_bytes(&store, &instance, result_ptr)
            .ok_or_else(|| format!("{} returned invalid pointer", func_name))
    }
}

// ════════════════════════════════════════════════
// ProviderPlugin impl
// ════════════════════════════════════════════════

impl ProviderPlugin for WasmProviderPlugin {
    fn id(&self) -> &str {
        &self.cached_id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn config_schema(&self) -> ProviderConfigSchema {
        self.cached_schema.clone()
    }

    fn validate_config(
        &self,
        _provider_extra: &HashMap<String, serde_json::Value>,
        _account_api_key: &Option<String>,
        _account_headers: &HashMap<String, String>,
    ) -> Result<(), String> {
        Ok(())
    }

    fn build_auth(&self, ctx: &ProviderRequestContext) -> Result<AuthResult, String> {
        match self.call_with_ctx("prism_provider_build_auth", ctx) {
            Ok(json) => {
                let v: serde_json::Value =
                    serde_json::from_str(&json).map_err(|e| format!("Invalid auth JSON: {}", e))?;
                let headers = v
                    .get("headers")
                    .and_then(|h| h.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|pair| {
                                let a = pair.as_array()?;
                                Some((
                                    a.first()?.as_str()?.to_string(),
                                    a.get(1)?.as_str()?.to_string(),
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(AuthResult { headers })
            }
            Err(_) => {
                // Fallback to default Bearer auth
                let mut result = AuthResult::default();
                if let Some(ref key) = ctx.account_api_key {
                    if !key.is_empty() {
                        result
                            .headers
                            .push(("Authorization".to_string(), format!("Bearer {}", key)));
                    }
                }
                Ok(result)
            }
        }
    }

    fn rewrite_url(&self, ctx: &ProviderRequestContext) -> Result<UrlRewriteResult, String> {
        match self.call_with_ctx("prism_provider_rewrite_url", ctx) {
            Ok(json) => {
                let v: serde_json::Value =
                    serde_json::from_str(&json).map_err(|e| format!("Invalid URL JSON: {}", e))?;
                Ok(UrlRewriteResult {
                    path: v
                        .get("path")
                        .and_then(|p| p.as_str())
                        .unwrap_or(&ctx.original_path)
                        .to_string(),
                    host_override: v
                        .get("host_override")
                        .and_then(|h| h.as_str())
                        .map(|s| s.to_string()),
                })
            }
            Err(_) => Ok(UrlRewriteResult {
                path: ctx.original_path.clone(),
                host_override: None,
            }),
        }
    }

    fn transform_request_body(
        &self,
        body: Vec<u8>,
        ctx: &ProviderRequestContext,
    ) -> Result<Vec<u8>, String> {
        match self.call_with_body_and_ctx("prism_provider_transform_request_body", &body, ctx) {
            Ok(result) => Ok(result),
            Err(_) => Ok(body),
        }
    }

    fn transform_response_body(
        &self,
        body: Vec<u8>,
        ctx: &ProviderRequestContext,
    ) -> Result<Vec<u8>, String> {
        match self.call_with_body_and_ctx("prism_provider_transform_response_body", &body, ctx) {
            Ok(result) => Ok(result),
            Err(_) => Ok(body),
        }
    }

    fn transform_stream_chunk(
        &self,
        chunk: &str,
        ctx: &ProviderRequestContext,
    ) -> Result<Option<String>, String> {
        match self.call_with_body_and_ctx(
            "prism_provider_transform_stream_chunk",
            chunk.as_bytes(),
            ctx,
        ) {
            Ok(result) => {
                let s = String::from_utf8(result)
                    .map_err(|e| format!("Invalid UTF-8 in chunk: {}", e))?;
                if s.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(s))
                }
            }
            Err(_) => Ok(Some(chunk.to_string())),
        }
    }
}
