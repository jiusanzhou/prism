//! WASM host implementation for proxy plugins.
//!
//! Loads WASM modules via wasmtime and bridges them to the [`ProxyPlugin`] trait,
//! implementing a subset of the proxy-wasm ABI.
//!
//! # proxy-wasm ABI subset
//!
//! ## Guest exports (called by host):
//! - `proxy_on_context_create(context_id, parent_context_id)`
//! - `proxy_on_configure(context_id, config_size) -> bool`
//! - `proxy_on_request_headers(context_id, num_headers, end_of_stream) -> Action`
//! - `proxy_on_request_body(context_id, body_size, end_of_stream) -> Action`
//! - `proxy_on_response_headers(context_id, num_headers, end_of_stream) -> Action`
//! - `proxy_on_response_body(context_id, body_size, end_of_stream) -> Action`
//! - `proxy_on_log(context_id)`
//! - `proxy_on_done(context_id) -> bool`
//! - `proxy_on_memory_allocate(size) -> ptr`
//!
//! ## Host functions (imported by guest):
//! - `proxy_get_header_map_pairs(map_type, return_data_ptr, return_data_size) -> status`
//! - `proxy_get_header_map_value(map_type, key_ptr, key_size, return_value_ptr, return_value_size) -> status`
//! - `proxy_replace_header_map_value(map_type, key_ptr, key_size, value_ptr, value_size) -> status`
//! - `proxy_add_header_map_value(map_type, key_ptr, key_size, value_ptr, value_size) -> status`
//! - `proxy_remove_header_map_value(map_type, key_ptr, key_size) -> status`
//! - `proxy_get_buffer_bytes(buffer_type, start, length, return_data_ptr, return_data_size) -> status`
//! - `proxy_set_buffer_bytes(buffer_type, start, length, data_ptr, data_size) -> status`
//! - `proxy_get_property(path_ptr, path_size, return_value_ptr, return_value_size) -> status`
//! - `proxy_set_property(path_ptr, path_size, value_ptr, value_size) -> status`
//! - `proxy_log(log_level, message_ptr, message_size) -> status`
//! - `proxy_set_effective_context(context_id) -> status`

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wasmtime::{Caller, Engine, Linker, Module, Store};

use super::{Action, ProxyContext, ProxyPlugin};

// ════════════════════════════════════════════════
// ABI constants
// ════════════════════════════════════════════════

/// proxy-wasm MapType values.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MapType {
    HttpRequestHeaders = 0,
    HttpRequestTrailers = 1,
    HttpResponseHeaders = 2,
    HttpResponseTrailers = 3,
}

impl MapType {
    fn from_i32(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::HttpRequestHeaders),
            1 => Some(Self::HttpRequestTrailers),
            2 => Some(Self::HttpResponseHeaders),
            3 => Some(Self::HttpResponseTrailers),
            _ => None,
        }
    }
}

/// proxy-wasm BufferType values.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferType {
    HttpRequestBody = 0,
    HttpResponseBody = 1,
    // HttpCallResponseBody = 2, // not implemented
    // GrpcReceiveBuffer = 3,    // not implemented
    VmConfiguration = 4,
    PluginConfiguration = 5,
}

impl BufferType {
    fn from_i32(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::HttpRequestBody),
            1 => Some(Self::HttpResponseBody),
            4 => Some(Self::VmConfiguration),
            5 => Some(Self::PluginConfiguration),
            _ => None,
        }
    }
}

/// proxy-wasm Status values.
#[repr(i32)]
#[derive(Debug, Clone, Copy)]
enum Status {
    Ok = 0,
    NotFound = 1,
    BadArgument = 2,
    #[allow(dead_code)]
    Unimplemented = 10,
}

/// Root context ID (constant for simplicity).
const ROOT_CONTEXT_ID: i32 = 1;
/// Stream context ID (incremented per request in a real impl, fixed here for Phase 2).
const STREAM_CONTEXT_ID: i32 = 2;

// ════════════════════════════════════════════════
// HostState — per-invocation state
// ════════════════════════════════════════════════

/// Host-side state held during WASM calls. Contains the current request/response
/// headers, bodies, properties, and config — everything the WASM guest accesses
/// through host functions.
#[derive(Debug, Clone)]
pub struct HostState {
    // ── Header maps (indexed by MapType) ──
    /// Request headers (MapType 0).
    pub request_headers: Vec<(String, String)>,
    /// Request trailers (MapType 1).
    pub request_trailers: Vec<(String, String)>,
    /// Response headers (MapType 2).
    pub response_headers: Vec<(String, String)>,
    /// Response trailers (MapType 3).
    pub response_trailers: Vec<(String, String)>,

    // ── Buffers ──
    /// HTTP request body (BufferType 0).
    pub request_body: Vec<u8>,
    /// HTTP response body (BufferType 1).
    pub response_body: Vec<u8>,
    /// VM configuration (BufferType 4).
    pub vm_config: Vec<u8>,
    /// Plugin configuration (BufferType 5).
    pub plugin_config: Vec<u8>,

    // ── Properties ──
    /// Key-value property store.
    pub properties: HashMap<String, Vec<u8>>,

    // ── Log capture (for testing) ──
    /// Captured log messages: `(level, message)`.
    pub log_messages: Vec<(i32, String)>,

    // ── Context tracking ──
    pub effective_context_id: i32,
}

impl HostState {
    /// Create a new empty host state.
    pub fn new() -> Self {
        Self {
            request_headers: Vec::new(),
            request_trailers: Vec::new(),
            response_headers: Vec::new(),
            response_trailers: Vec::new(),
            request_body: Vec::new(),
            response_body: Vec::new(),
            vm_config: Vec::new(),
            plugin_config: Vec::new(),
            properties: HashMap::new(),
            log_messages: Vec::new(),
            effective_context_id: ROOT_CONTEXT_ID,
        }
    }

    // ── Header map accessors ──

    fn get_header_map(&self, map_type: MapType) -> &Vec<(String, String)> {
        match map_type {
            MapType::HttpRequestHeaders => &self.request_headers,
            MapType::HttpRequestTrailers => &self.request_trailers,
            MapType::HttpResponseHeaders => &self.response_headers,
            MapType::HttpResponseTrailers => &self.response_trailers,
        }
    }

    fn get_header_map_mut(&mut self, map_type: MapType) -> &mut Vec<(String, String)> {
        match map_type {
            MapType::HttpRequestHeaders => &mut self.request_headers,
            MapType::HttpRequestTrailers => &mut self.request_trailers,
            MapType::HttpResponseHeaders => &mut self.response_headers,
            MapType::HttpResponseTrailers => &mut self.response_trailers,
        }
    }

    /// Get a header value by key from the specified map.
    pub fn get_header_value(&self, map_type_raw: i32, key: &str) -> Option<String> {
        let map_type = MapType::from_i32(map_type_raw)?;
        let headers = self.get_header_map(map_type);
        for (k, v) in headers {
            if k == key {
                return Some(v.clone());
            }
        }
        None
    }

    /// Replace (or insert) a header value in the specified map.
    pub fn replace_header_value(&mut self, map_type_raw: i32, key: &str, value: &str) {
        if let Some(map_type) = MapType::from_i32(map_type_raw) {
            let headers = self.get_header_map_mut(map_type);
            for entry in headers.iter_mut() {
                if entry.0 == key {
                    entry.1 = value.to_string();
                    return;
                }
            }
            // Not found — add it.
            headers.push((key.to_string(), value.to_string()));
        }
    }

    /// Add a header value (allows duplicates).
    pub fn add_header_value(&mut self, map_type_raw: i32, key: &str, value: &str) {
        if let Some(map_type) = MapType::from_i32(map_type_raw) {
            let headers = self.get_header_map_mut(map_type);
            headers.push((key.to_string(), value.to_string()));
        }
    }

    /// Remove all header entries with the given key.
    pub fn remove_header_value(&mut self, map_type_raw: i32, key: &str) {
        if let Some(map_type) = MapType::from_i32(map_type_raw) {
            let headers = self.get_header_map_mut(map_type);
            headers.retain(|(k, _)| k != key);
        }
    }

    // ── Buffer accessors ──

    /// Get buffer bytes by type, with start/length range.
    pub fn get_buffer(
        &self,
        buffer_type_raw: i32,
        start: usize,
        length: usize,
    ) -> Option<Vec<u8>> {
        let buf = match BufferType::from_i32(buffer_type_raw)? {
            BufferType::HttpRequestBody => &self.request_body,
            BufferType::HttpResponseBody => &self.response_body,
            BufferType::VmConfiguration => &self.vm_config,
            BufferType::PluginConfiguration => &self.plugin_config,
        };
        let end = std::cmp::min(start + length, buf.len());
        if start > buf.len() {
            return Some(Vec::new());
        }
        Some(buf[start..end].to_vec())
    }

    /// Set buffer bytes by type, replacing the range `[start..start+length]`.
    pub fn set_buffer(
        &mut self,
        buffer_type_raw: i32,
        start: usize,
        length: usize,
        data: &[u8],
    ) -> bool {
        let buf = match BufferType::from_i32(buffer_type_raw) {
            Some(BufferType::HttpRequestBody) => &mut self.request_body,
            Some(BufferType::HttpResponseBody) => &mut self.response_body,
            Some(BufferType::VmConfiguration) => &mut self.vm_config,
            Some(BufferType::PluginConfiguration) => &mut self.plugin_config,
            None => return false,
        };

        // If start == 0 and length == 0 and buffer empty, set directly
        if start == 0 && length == 0 && buf.is_empty() {
            *buf = data.to_vec();
        } else {
            let end = std::cmp::min(start + length, buf.len());
            let mut new_buf = Vec::new();
            if start <= buf.len() {
                new_buf.extend_from_slice(&buf[..start]);
            }
            new_buf.extend_from_slice(data);
            if end <= buf.len() {
                new_buf.extend_from_slice(&buf[end..]);
            }
            *buf = new_buf;
        }
        true
    }
}

impl Default for HostState {
    fn default() -> Self {
        Self::new()
    }
}

// ════════════════════════════════════════════════
// Serialization helpers for proxy-wasm header map pairs
// ════════════════════════════════════════════════

/// Serialize header pairs into proxy-wasm format:
/// `num_headers(4 bytes LE) + [key_size(4) + value_size(4)]* + [key\0 + value\0]*`
fn serialize_header_map(headers: &[(String, String)]) -> Vec<u8> {
    let num = headers.len() as u32;
    let mut buf = Vec::new();
    buf.extend_from_slice(&num.to_le_bytes());
    for (k, v) in headers {
        buf.extend_from_slice(&(k.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(v.len() as u32).to_le_bytes());
    }
    for (k, v) in headers {
        buf.extend_from_slice(k.as_bytes());
        buf.push(0);
        buf.extend_from_slice(v.as_bytes());
        buf.push(0);
    }
    buf
}

/// Deserialize header pairs from proxy-wasm format.
#[allow(dead_code)]
fn deserialize_header_map(data: &[u8]) -> Option<Vec<(String, String)>> {
    if data.len() < 4 {
        return None;
    }
    let num = u32::from_le_bytes(data[0..4].try_into().ok()?) as usize;
    let mut offset = 4;
    let mut sizes = Vec::with_capacity(num);
    for _ in 0..num {
        if offset + 8 > data.len() {
            return None;
        }
        let key_size = u32::from_le_bytes(data[offset..offset + 4].try_into().ok()?) as usize;
        let value_size =
            u32::from_le_bytes(data[offset + 4..offset + 8].try_into().ok()?) as usize;
        sizes.push((key_size, value_size));
        offset += 8;
    }

    let mut pairs = Vec::with_capacity(num);
    for (key_size, value_size) in sizes {
        if offset + key_size > data.len() {
            return None;
        }
        let key = String::from_utf8_lossy(&data[offset..offset + key_size]).to_string();
        offset += key_size + 1; // skip null terminator
        if offset + value_size > data.len() {
            return None;
        }
        let value = String::from_utf8_lossy(&data[offset..offset + value_size]).to_string();
        offset += value_size + 1; // skip null terminator
        pairs.push((key, value));
    }
    Some(pairs)
}

// ════════════════════════════════════════════════
// WasmProxyPlugin
// ════════════════════════════════════════════════

/// A proxy plugin backed by a WASM module implementing the proxy-wasm ABI.
///
/// The module is loaded once; a fresh `Store` + `Instance` is created per call
/// (or can be reused — for Phase 2 we create per-call for simplicity).
pub struct WasmProxyPlugin {
    name: String,
    engine: Engine,
    module: Module,
    /// Plugin configuration bytes (set via `on_config`).
    config_bytes: Vec<u8>,
}

impl WasmProxyPlugin {
    /// Create a new WASM proxy plugin from raw WASM or WAT bytes.
    pub fn new(name: &str, wasm_bytes: &[u8]) -> Result<Self, String> {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm_bytes)
            .map_err(|e| format!("Failed to compile WASM module: {}", e))?;
        Ok(Self {
            name: name.to_string(),
            engine,
            module,
            config_bytes: Vec::new(),
        })
    }

    /// Create a store + instance with all host functions linked.
    fn create_instance(
        &self,
        state: Arc<Mutex<HostState>>,
    ) -> Result<(Store<Arc<Mutex<HostState>>>, wasmtime::Instance), String> {
        let mut store = Store::new(&self.engine, state);
        let mut linker = Linker::new(&self.engine);

        // Register all proxy-wasm host functions in the "env" namespace.
        Self::register_host_functions(&mut linker)?;

        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| format!("Failed to instantiate WASM module: {}", e))?;

        Ok((store, instance))
    }

    /// Register all proxy-wasm host functions on the linker.
    fn register_host_functions(
        linker: &mut Linker<Arc<Mutex<HostState>>>,
    ) -> Result<(), String> {
        // ── proxy_log ──
        linker
            .func_wrap(
                "env",
                "proxy_log",
                |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                 log_level: i32,
                 message_ptr: i32,
                 message_size: i32|
                 -> i32 {
                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return Status::BadArgument as i32,
                    };
                    let data = memory.data(&caller);
                    let ptr = message_ptr as usize;
                    let size = message_size as usize;
                    if ptr + size > data.len() {
                        return Status::BadArgument as i32;
                    }
                    let msg = String::from_utf8_lossy(&data[ptr..ptr + size]).to_string();

                    // Log via tracing
                    match log_level {
                        0 => tracing::trace!(target: "proxy_wasm", "{}", msg),
                        1 => tracing::debug!(target: "proxy_wasm", "{}", msg),
                        2 => tracing::info!(target: "proxy_wasm", "{}", msg),
                        3 => tracing::warn!(target: "proxy_wasm", "{}", msg),
                        4 | 5 => tracing::error!(target: "proxy_wasm", "{}", msg),
                        _ => tracing::info!(target: "proxy_wasm", "[level={}] {}", log_level, msg),
                    }

                    // Also store in HostState for testing
                    let state = caller.data().clone();
                    if let Ok(mut s) = state.lock() {
                        s.log_messages.push((log_level, msg));
                    }
                    Status::Ok as i32
                },
            )
            .map_err(|e| format!("Failed to link proxy_log: {}", e))?;

        // ── proxy_get_header_map_pairs ──
        linker
            .func_wrap(
                "env",
                "proxy_get_header_map_pairs",
                |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                 map_type: i32,
                 return_data_ptr: i32,
                 return_data_size: i32|
                 -> i32 {
                    let state = caller.data().clone();
                    let serialized = {
                        let s = match state.lock() {
                            Ok(s) => s,
                            Err(_) => return Status::BadArgument as i32,
                        };
                        let map_type_enum = match MapType::from_i32(map_type) {
                            Some(mt) => mt,
                            None => return Status::BadArgument as i32,
                        };
                        serialize_header_map(s.get_header_map(map_type_enum))
                    };

                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return Status::BadArgument as i32,
                    };

                    let size = serialized.len() as u32;
                    let mem_data = memory.data_mut(&mut caller);

                    // Write size to return_data_size
                    let size_ptr = return_data_size as usize;
                    if size_ptr + 4 > mem_data.len() {
                        return Status::BadArgument as i32;
                    }
                    mem_data[size_ptr..size_ptr + 4].copy_from_slice(&size.to_le_bytes());

                    // Write data at a known offset, write pointer to return_data_ptr
                    let data_offset = 4096u32;
                    let ptr_ptr = return_data_ptr as usize;
                    if ptr_ptr + 4 > mem_data.len() {
                        return Status::BadArgument as i32;
                    }
                    mem_data[ptr_ptr..ptr_ptr + 4]
                        .copy_from_slice(&data_offset.to_le_bytes());

                    let offset = data_offset as usize;
                    if offset + serialized.len() > mem_data.len() {
                        return Status::BadArgument as i32;
                    }
                    mem_data[offset..offset + serialized.len()]
                        .copy_from_slice(&serialized);

                    Status::Ok as i32
                },
            )
            .map_err(|e| format!("Failed to link proxy_get_header_map_pairs: {}", e))?;

        // ── proxy_get_header_map_value ──
        linker
            .func_wrap(
                "env",
                "proxy_get_header_map_value",
                |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                 map_type: i32,
                 key_ptr: i32,
                 key_size: i32,
                 return_value_ptr: i32,
                 return_value_size: i32|
                 -> i32 {
                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return Status::BadArgument as i32,
                    };

                    let data = memory.data(&caller);
                    let kp = key_ptr as usize;
                    let ks = key_size as usize;
                    if kp + ks > data.len() {
                        return Status::BadArgument as i32;
                    }
                    let key = String::from_utf8_lossy(&data[kp..kp + ks]).to_string();

                    let state = caller.data().clone();
                    let value = {
                        let s = match state.lock() {
                            Ok(s) => s,
                            Err(_) => return Status::BadArgument as i32,
                        };
                        s.get_header_value(map_type, &key)
                    };

                    match value {
                        Some(val) => {
                            let val_bytes = val.as_bytes();
                            let data_offset = 4096u32;
                            let mem_data = memory.data_mut(&mut caller);

                            let offset = data_offset as usize;
                            if offset + val_bytes.len() > mem_data.len() {
                                return Status::BadArgument as i32;
                            }
                            mem_data[offset..offset + val_bytes.len()]
                                .copy_from_slice(val_bytes);

                            let vp = return_value_ptr as usize;
                            if vp + 4 > mem_data.len() {
                                return Status::BadArgument as i32;
                            }
                            mem_data[vp..vp + 4]
                                .copy_from_slice(&data_offset.to_le_bytes());

                            let vs = return_value_size as usize;
                            if vs + 4 > mem_data.len() {
                                return Status::BadArgument as i32;
                            }
                            mem_data[vs..vs + 4]
                                .copy_from_slice(&(val_bytes.len() as u32).to_le_bytes());

                            Status::Ok as i32
                        }
                        None => Status::NotFound as i32,
                    }
                },
            )
            .map_err(|e| format!("Failed to link proxy_get_header_map_value: {}", e))?;

        // ── proxy_replace_header_map_value ──
        linker
            .func_wrap(
                "env",
                "proxy_replace_header_map_value",
                |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                 map_type: i32,
                 key_ptr: i32,
                 key_size: i32,
                 value_ptr: i32,
                 value_size: i32|
                 -> i32 {
                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return Status::BadArgument as i32,
                    };

                    let data = memory.data(&caller);
                    let kp = key_ptr as usize;
                    let ks = key_size as usize;
                    let vp = value_ptr as usize;
                    let vs = value_size as usize;
                    if kp + ks > data.len() || vp + vs > data.len() {
                        return Status::BadArgument as i32;
                    }
                    let key = String::from_utf8_lossy(&data[kp..kp + ks]).to_string();
                    let value = String::from_utf8_lossy(&data[vp..vp + vs]).to_string();

                    let state = caller.data().clone();
                    if let Ok(mut s) = state.lock() {
                        s.replace_header_value(map_type, &key, &value);
                    }
                    Status::Ok as i32
                },
            )
            .map_err(|e| format!("Failed to link proxy_replace_header_map_value: {}", e))?;

        // ── proxy_add_header_map_value ──
        linker
            .func_wrap(
                "env",
                "proxy_add_header_map_value",
                |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                 map_type: i32,
                 key_ptr: i32,
                 key_size: i32,
                 value_ptr: i32,
                 value_size: i32|
                 -> i32 {
                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return Status::BadArgument as i32,
                    };

                    let data = memory.data(&caller);
                    let kp = key_ptr as usize;
                    let ks = key_size as usize;
                    let vp = value_ptr as usize;
                    let vs = value_size as usize;
                    if kp + ks > data.len() || vp + vs > data.len() {
                        return Status::BadArgument as i32;
                    }
                    let key = String::from_utf8_lossy(&data[kp..kp + ks]).to_string();
                    let value = String::from_utf8_lossy(&data[vp..vp + vs]).to_string();

                    let state = caller.data().clone();
                    if let Ok(mut s) = state.lock() {
                        s.add_header_value(map_type, &key, &value);
                    }
                    Status::Ok as i32
                },
            )
            .map_err(|e| format!("Failed to link proxy_add_header_map_value: {}", e))?;

        // ── proxy_remove_header_map_value ──
        linker
            .func_wrap(
                "env",
                "proxy_remove_header_map_value",
                |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                 map_type: i32,
                 key_ptr: i32,
                 key_size: i32|
                 -> i32 {
                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return Status::BadArgument as i32,
                    };

                    let data = memory.data(&caller);
                    let kp = key_ptr as usize;
                    let ks = key_size as usize;
                    if kp + ks > data.len() {
                        return Status::BadArgument as i32;
                    }
                    let key = String::from_utf8_lossy(&data[kp..kp + ks]).to_string();

                    let state = caller.data().clone();
                    if let Ok(mut s) = state.lock() {
                        s.remove_header_value(map_type, &key);
                    }
                    Status::Ok as i32
                },
            )
            .map_err(|e| format!("Failed to link proxy_remove_header_map_value: {}", e))?;

        // ── proxy_get_buffer_bytes ──
        linker
            .func_wrap(
                "env",
                "proxy_get_buffer_bytes",
                |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                 buffer_type: i32,
                 start: i32,
                 length: i32,
                 return_data_ptr: i32,
                 return_data_size: i32|
                 -> i32 {
                    let state = caller.data().clone();
                    let buf_data = {
                        let s = match state.lock() {
                            Ok(s) => s,
                            Err(_) => return Status::BadArgument as i32,
                        };
                        match s.get_buffer(buffer_type, start as usize, length as usize) {
                            Some(d) => d,
                            None => return Status::BadArgument as i32,
                        }
                    };

                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return Status::BadArgument as i32,
                    };

                    let data_offset = 4096u32;
                    let mem_data = memory.data_mut(&mut caller);

                    let offset = data_offset as usize;
                    if offset + buf_data.len() > mem_data.len() {
                        return Status::BadArgument as i32;
                    }
                    mem_data[offset..offset + buf_data.len()].copy_from_slice(&buf_data);

                    let pp = return_data_ptr as usize;
                    if pp + 4 > mem_data.len() {
                        return Status::BadArgument as i32;
                    }
                    mem_data[pp..pp + 4].copy_from_slice(&data_offset.to_le_bytes());

                    let sp = return_data_size as usize;
                    if sp + 4 > mem_data.len() {
                        return Status::BadArgument as i32;
                    }
                    mem_data[sp..sp + 4]
                        .copy_from_slice(&(buf_data.len() as u32).to_le_bytes());

                    Status::Ok as i32
                },
            )
            .map_err(|e| format!("Failed to link proxy_get_buffer_bytes: {}", e))?;

        // ── proxy_set_buffer_bytes ──
        linker
            .func_wrap(
                "env",
                "proxy_set_buffer_bytes",
                |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                 buffer_type: i32,
                 start: i32,
                 length: i32,
                 data_ptr: i32,
                 data_size: i32|
                 -> i32 {
                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return Status::BadArgument as i32,
                    };

                    let dp = data_ptr as usize;
                    let ds = data_size as usize;
                    let mem_data = memory.data(&caller);
                    if dp + ds > mem_data.len() {
                        return Status::BadArgument as i32;
                    }
                    let data = mem_data[dp..dp + ds].to_vec();

                    let state = caller.data().clone();
                    let result = if let Ok(mut s) = state.lock() {
                        if s.set_buffer(
                            buffer_type,
                            start as usize,
                            length as usize,
                            &data,
                        ) {
                            Status::Ok as i32
                        } else {
                            Status::BadArgument as i32
                        }
                    } else {
                        Status::BadArgument as i32
                    };
                    result
                },
            )
            .map_err(|e| format!("Failed to link proxy_set_buffer_bytes: {}", e))?;

        // ── proxy_get_property ──
        linker
            .func_wrap(
                "env",
                "proxy_get_property",
                |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                 path_ptr: i32,
                 path_size: i32,
                 return_value_ptr: i32,
                 return_value_size: i32|
                 -> i32 {
                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return Status::BadArgument as i32,
                    };

                    let data = memory.data(&caller);
                    let pp = path_ptr as usize;
                    let ps = path_size as usize;
                    if pp + ps > data.len() {
                        return Status::BadArgument as i32;
                    }
                    let path = String::from_utf8_lossy(&data[pp..pp + ps]).to_string();

                    let state = caller.data().clone();
                    let value = {
                        let s = match state.lock() {
                            Ok(s) => s,
                            Err(_) => return Status::BadArgument as i32,
                        };
                        s.properties.get(&path).cloned()
                    };

                    match value {
                        Some(val) => {
                            let data_offset = 4096u32;
                            let mem_data = memory.data_mut(&mut caller);

                            let offset = data_offset as usize;
                            if offset + val.len() > mem_data.len() {
                                return Status::BadArgument as i32;
                            }
                            mem_data[offset..offset + val.len()].copy_from_slice(&val);

                            let vp = return_value_ptr as usize;
                            if vp + 4 > mem_data.len() {
                                return Status::BadArgument as i32;
                            }
                            mem_data[vp..vp + 4]
                                .copy_from_slice(&data_offset.to_le_bytes());

                            let vs = return_value_size as usize;
                            if vs + 4 > mem_data.len() {
                                return Status::BadArgument as i32;
                            }
                            mem_data[vs..vs + 4]
                                .copy_from_slice(&(val.len() as u32).to_le_bytes());

                            Status::Ok as i32
                        }
                        None => Status::NotFound as i32,
                    }
                },
            )
            .map_err(|e| format!("Failed to link proxy_get_property: {}", e))?;

        // ── proxy_set_property ──
        linker
            .func_wrap(
                "env",
                "proxy_set_property",
                |mut caller: Caller<'_, Arc<Mutex<HostState>>>,
                 path_ptr: i32,
                 path_size: i32,
                 value_ptr: i32,
                 value_size: i32|
                 -> i32 {
                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return Status::BadArgument as i32,
                    };

                    let data = memory.data(&caller);
                    let pp = path_ptr as usize;
                    let ps = path_size as usize;
                    let vp = value_ptr as usize;
                    let vs = value_size as usize;
                    if pp + ps > data.len() || vp + vs > data.len() {
                        return Status::BadArgument as i32;
                    }
                    let path = String::from_utf8_lossy(&data[pp..pp + ps]).to_string();
                    let value = data[vp..vp + vs].to_vec();

                    let state = caller.data().clone();
                    if let Ok(mut s) = state.lock() {
                        s.properties.insert(path, value);
                    }
                    Status::Ok as i32
                },
            )
            .map_err(|e| format!("Failed to link proxy_set_property: {}", e))?;

        // ── proxy_set_effective_context ──
        linker
            .func_wrap(
                "env",
                "proxy_set_effective_context",
                |caller: Caller<'_, Arc<Mutex<HostState>>>, context_id: i32| -> i32 {
                    let state = caller.data().clone();
                    if let Ok(mut s) = state.lock() {
                        s.effective_context_id = context_id;
                    }
                    Status::Ok as i32
                },
            )
            .map_err(|e| format!("Failed to link proxy_set_effective_context: {}", e))?;

        Ok(())
    }

    /// Synchronize headers from ProxyContext into HostState (for request phase).
    fn sync_request_headers_to_host(ctx: &ProxyContext, state: &mut HostState) {
        state.request_headers = ctx
            .request_headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // Add pseudo-headers
        if !ctx.request_path.is_empty() {
            state
                .request_headers
                .push((":path".to_string(), ctx.request_path.clone()));
        }
        if !ctx.request_method.is_empty() {
            state
                .request_headers
                .push((":method".to_string(), ctx.request_method.clone()));
        }
    }

    /// Synchronize headers from HostState back to ProxyContext (after request phase).
    fn sync_request_headers_from_host(ctx: &mut ProxyContext, state: &HostState) {
        ctx.request_headers.clear();
        for (k, v) in &state.request_headers {
            if k == ":path" {
                ctx.request_path = v.clone();
            } else if k == ":method" {
                ctx.request_method = v.clone();
            } else {
                ctx.request_headers.insert(k.clone(), v.clone());
            }
        }
    }

    /// Synchronize response headers from ProxyContext into HostState.
    fn sync_response_headers_to_host(ctx: &ProxyContext, state: &mut HostState) {
        state.response_headers = ctx
            .response_headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if let Some(status) = ctx.response_status {
            state
                .response_headers
                .push((":status".to_string(), status.to_string()));
        }
    }

    /// Synchronize response headers from HostState back to ProxyContext.
    fn sync_response_headers_from_host(ctx: &mut ProxyContext, state: &HostState) {
        ctx.response_headers.clear();
        for (k, v) in &state.response_headers {
            if k == ":status" {
                if let Ok(status) = v.parse::<u16>() {
                    ctx.response_status = Some(status);
                }
            } else {
                ctx.response_headers.insert(k.clone(), v.clone());
            }
        }
    }

    /// Convert proxy-wasm i32 action to our Action enum.
    fn action_from_wasm(val: i32) -> Action {
        match val {
            0 => Action::Continue,
            1 => Action::Stop, // proxy-wasm "Pause" maps to our "Stop"
            _ => Action::Continue,
        }
    }
}

impl ProxyPlugin for WasmProxyPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_config(&mut self, config: &serde_json::Value) -> Result<(), String> {
        self.config_bytes = serde_json::to_vec(config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        // Create a temporary instance to call proxy_on_configure
        let state = Arc::new(Mutex::new(HostState::new()));
        {
            let mut s = state.lock().unwrap();
            s.plugin_config = self.config_bytes.clone();
        }

        let (mut store, instance) = self.create_instance(state.clone())?;

        // Create root context
        if let Ok(create_ctx) =
            instance.get_typed_func::<(i32, i32), ()>(&mut store, "proxy_on_context_create")
        {
            create_ctx
                .call(&mut store, (ROOT_CONTEXT_ID, 0))
                .map_err(|e| format!("proxy_on_context_create failed: {}", e))?;
        }

        // Call proxy_on_configure
        if let Ok(configure) =
            instance.get_typed_func::<(i32, i32), i32>(&mut store, "proxy_on_configure")
        {
            let result = configure
                .call(
                    &mut store,
                    (ROOT_CONTEXT_ID, self.config_bytes.len() as i32),
                )
                .map_err(|e| format!("proxy_on_configure failed: {}", e))?;
            if result == 0 {
                return Err("WASM plugin rejected configuration".to_string());
            }
        }

        Ok(())
    }

    fn on_request_headers(&self, ctx: &mut ProxyContext) -> Action {
        let state = Arc::new(Mutex::new(HostState::new()));
        {
            let mut s = state.lock().unwrap();
            s.plugin_config = self.config_bytes.clone();
            Self::sync_request_headers_to_host(ctx, &mut s);
        }

        let (mut store, instance) = match self.create_instance(state.clone()) {
            Ok(v) => v,
            Err(_) => return Action::Continue,
        };

        // Create root context + stream context
        if let Ok(create_ctx) =
            instance.get_typed_func::<(i32, i32), ()>(&mut store, "proxy_on_context_create")
        {
            let _ = create_ctx.call(&mut store, (ROOT_CONTEXT_ID, 0));
            let _ = create_ctx.call(&mut store, (STREAM_CONTEXT_ID, ROOT_CONTEXT_ID));
        }

        // Call proxy_on_request_headers
        let action = if let Ok(on_headers) = instance
            .get_typed_func::<(i32, i32, i32), i32>(&mut store, "proxy_on_request_headers")
        {
            let num_headers = {
                let s = state.lock().unwrap();
                s.request_headers.len() as i32
            };
            match on_headers.call(&mut store, (STREAM_CONTEXT_ID, num_headers, 1)) {
                Ok(a) => Self::action_from_wasm(a),
                Err(_) => Action::Continue,
            }
        } else {
            Action::Continue
        };

        // Sync headers back
        {
            let s = state.lock().unwrap();
            Self::sync_request_headers_from_host(ctx, &s);
        }

        action
    }

    fn on_request_body(
        &self,
        ctx: &mut ProxyContext,
        body: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        let state = Arc::new(Mutex::new(HostState::new()));
        {
            let mut s = state.lock().unwrap();
            s.plugin_config = self.config_bytes.clone();
            s.request_body = body;
            Self::sync_request_headers_to_host(ctx, &mut s);
        }

        let (mut store, instance) = self.create_instance(state.clone())?;

        // Create contexts
        if let Ok(create_ctx) =
            instance.get_typed_func::<(i32, i32), ()>(&mut store, "proxy_on_context_create")
        {
            let _ = create_ctx.call(&mut store, (ROOT_CONTEXT_ID, 0));
            let _ = create_ctx.call(&mut store, (STREAM_CONTEXT_ID, ROOT_CONTEXT_ID));
        }

        // Call proxy_on_request_body
        let body_size = {
            let s = state.lock().unwrap();
            s.request_body.len() as i32
        };

        if let Ok(on_body) = instance
            .get_typed_func::<(i32, i32, i32), i32>(&mut store, "proxy_on_request_body")
        {
            on_body
                .call(&mut store, (STREAM_CONTEXT_ID, body_size, 1))
                .map_err(|e| format!("proxy_on_request_body failed: {}", e))?;
        }

        // Return potentially modified body
        let result = {
            let s = state.lock().unwrap();
            s.request_body.clone()
        };
        Ok(result)
    }

    fn on_response_headers(&self, ctx: &mut ProxyContext) -> Action {
        let state = Arc::new(Mutex::new(HostState::new()));
        {
            let mut s = state.lock().unwrap();
            s.plugin_config = self.config_bytes.clone();
            Self::sync_response_headers_to_host(ctx, &mut s);
        }

        let (mut store, instance) = match self.create_instance(state.clone()) {
            Ok(v) => v,
            Err(_) => return Action::Continue,
        };

        // Create contexts
        if let Ok(create_ctx) =
            instance.get_typed_func::<(i32, i32), ()>(&mut store, "proxy_on_context_create")
        {
            let _ = create_ctx.call(&mut store, (ROOT_CONTEXT_ID, 0));
            let _ = create_ctx.call(&mut store, (STREAM_CONTEXT_ID, ROOT_CONTEXT_ID));
        }

        // Call proxy_on_response_headers
        let action = if let Ok(on_headers) = instance
            .get_typed_func::<(i32, i32, i32), i32>(
                &mut store,
                "proxy_on_response_headers",
            )
        {
            let num_headers = {
                let s = state.lock().unwrap();
                s.response_headers.len() as i32
            };
            match on_headers.call(&mut store, (STREAM_CONTEXT_ID, num_headers, 1)) {
                Ok(a) => Self::action_from_wasm(a),
                Err(_) => Action::Continue,
            }
        } else {
            Action::Continue
        };

        // Sync headers back
        {
            let s = state.lock().unwrap();
            Self::sync_response_headers_from_host(ctx, &s);
        }

        action
    }

    fn on_response_body(
        &self,
        ctx: &mut ProxyContext,
        body: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        let state = Arc::new(Mutex::new(HostState::new()));
        {
            let mut s = state.lock().unwrap();
            s.plugin_config = self.config_bytes.clone();
            s.response_body = body;
            Self::sync_response_headers_to_host(ctx, &mut s);
        }

        let (mut store, instance) = self.create_instance(state.clone())?;

        // Create contexts
        if let Ok(create_ctx) =
            instance.get_typed_func::<(i32, i32), ()>(&mut store, "proxy_on_context_create")
        {
            let _ = create_ctx.call(&mut store, (ROOT_CONTEXT_ID, 0));
            let _ = create_ctx.call(&mut store, (STREAM_CONTEXT_ID, ROOT_CONTEXT_ID));
        }

        // Call proxy_on_response_body
        let body_size = {
            let s = state.lock().unwrap();
            s.response_body.len() as i32
        };

        if let Ok(on_body) = instance
            .get_typed_func::<(i32, i32, i32), i32>(&mut store, "proxy_on_response_body")
        {
            on_body
                .call(&mut store, (STREAM_CONTEXT_ID, body_size, 1))
                .map_err(|e| format!("proxy_on_response_body failed: {}", e))?;
        }

        // Return potentially modified body
        let result = {
            let s = state.lock().unwrap();
            s.response_body.clone()
        };
        Ok(result)
    }
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Minimal WAT module ──

    /// A minimal proxy-wasm module that does nothing (all pass-through).
    const MINIMAL_WAT: &str = r#"
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

    /// A WAT module that calls proxy_log during proxy_on_request_headers.
    const LOGGING_WAT: &str = r#"
        (module
            ;; Import proxy_log from host
            (import "env" "proxy_log"
                (func $proxy_log (param i32 i32 i32) (result i32)))

            ;; Memory with a message pre-stored at offset 0
            (memory (export "memory") 1)
            (data (i32.const 0) "hello from wasm")

            (func (export "proxy_on_context_create") (param i32 i32))
            (func (export "proxy_on_configure") (param i32 i32) (result i32)
                (i32.const 1))
            (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
                ;; Call proxy_log(level=2(Info), msg_ptr=0, msg_size=15)
                (drop (call $proxy_log (i32.const 2) (i32.const 0) (i32.const 15)))
                (i32.const 0))
            (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
                (i32.const 0))
            (func (export "proxy_on_response_headers") (param i32 i32 i32) (result i32)
                (i32.const 0))
            (func (export "proxy_on_response_body") (param i32 i32 i32) (result i32)
                (i32.const 0))
            (func (export "proxy_on_memory_allocate") (param i32) (result i32)
                (i32.const 0))
        )
    "#;

    /// A WAT module that returns Action::Pause (1) from proxy_on_request_headers.
    const PAUSE_WAT: &str = r#"
        (module
            (func (export "proxy_on_context_create") (param i32 i32))
            (func (export "proxy_on_configure") (param i32 i32) (result i32)
                (i32.const 1))
            (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
                (i32.const 1))
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

    /// A WAT module that modifies request headers by calling host functions.
    const HEADER_MODIFIER_WAT: &str = r#"
        (module
            ;; Import host functions
            (import "env" "proxy_replace_header_map_value"
                (func $proxy_replace (param i32 i32 i32 i32 i32) (result i32)))
            (import "env" "proxy_add_header_map_value"
                (func $proxy_add (param i32 i32 i32 i32 i32) (result i32)))

            ;; Memory with pre-stored strings
            (memory (export "memory") 1)
            ;; "x-wasm-processed" at offset 0 (16 bytes)
            (data (i32.const 0) "x-wasm-processed")
            ;; "true" at offset 16 (4 bytes)
            (data (i32.const 16) "true")
            ;; "x-added" at offset 20 (7 bytes)
            (data (i32.const 20) "x-added")
            ;; "yes" at offset 27 (3 bytes)
            (data (i32.const 27) "yes")

            (func (export "proxy_on_context_create") (param i32 i32))
            (func (export "proxy_on_configure") (param i32 i32) (result i32)
                (i32.const 1))
            (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
                ;; Replace: map_type=0, key="x-wasm-processed"(0,16), value="true"(16,4)
                (drop (call $proxy_replace
                    (i32.const 0)
                    (i32.const 0) (i32.const 16)
                    (i32.const 16) (i32.const 4)))
                ;; Add: map_type=0, key="x-added"(20,7), value="yes"(27,3)
                (drop (call $proxy_add
                    (i32.const 0)
                    (i32.const 20) (i32.const 7)
                    (i32.const 27) (i32.const 3)))
                (i32.const 0))
            (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
                (i32.const 0))
            (func (export "proxy_on_response_headers") (param i32 i32 i32) (result i32)
                (i32.const 0))
            (func (export "proxy_on_response_body") (param i32 i32 i32) (result i32)
                (i32.const 0))
            (func (export "proxy_on_memory_allocate") (param i32) (result i32)
                (i32.const 0))
        )
    "#;

    /// A WAT module that replaces the request body via proxy_set_buffer_bytes.
    const BODY_MODIFIER_WAT: &str = r#"
        (module
            ;; Import host functions
            (import "env" "proxy_set_buffer_bytes"
                (func $proxy_set_buf (param i32 i32 i32 i32 i32) (result i32)))

            ;; Memory with pre-stored data
            (memory (export "memory") 1)
            ;; "modified" at offset 0 (8 bytes)
            (data (i32.const 0) "modified")

            (func (export "proxy_on_context_create") (param i32 i32))
            (func (export "proxy_on_configure") (param i32 i32) (result i32)
                (i32.const 1))
            (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
                (i32.const 0))
            (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
                ;; Replace entire body: buffer_type=0, start=0, length=body_size, data="modified"(0,8)
                (drop (call $proxy_set_buf
                    (i32.const 0)
                    (i32.const 0) (local.get 1)
                    (i32.const 0) (i32.const 8)))
                (i32.const 0))
            (func (export "proxy_on_response_headers") (param i32 i32 i32) (result i32)
                (i32.const 0))
            (func (export "proxy_on_response_body") (param i32 i32 i32) (result i32)
                (i32.const 0))
            (func (export "proxy_on_memory_allocate") (param i32) (result i32)
                (i32.const 0))
        )
    "#;

    // ════════════════════════════════════
    // HostState unit tests
    // ════════════════════════════════════

    #[test]
    fn test_host_state_header_get_set() {
        let mut state = HostState::new();
        state
            .request_headers
            .push(("content-type".to_string(), "application/json".to_string()));
        state
            .request_headers
            .push(("authorization".to_string(), "Bearer token".to_string()));

        assert_eq!(
            state.get_header_value(0, "content-type"),
            Some("application/json".to_string())
        );
        assert_eq!(
            state.get_header_value(0, "authorization"),
            Some("Bearer token".to_string())
        );
        assert_eq!(state.get_header_value(0, "missing"), None);
        // Wrong map type
        assert_eq!(state.get_header_value(2, "content-type"), None);
    }

    #[test]
    fn test_host_state_header_replace() {
        let mut state = HostState::new();
        state
            .request_headers
            .push(("content-type".to_string(), "text/plain".to_string()));

        state.replace_header_value(0, "content-type", "application/json");
        assert_eq!(
            state.get_header_value(0, "content-type"),
            Some("application/json".to_string())
        );

        // Replace non-existent → inserts
        state.replace_header_value(0, "x-new", "value");
        assert_eq!(
            state.get_header_value(0, "x-new"),
            Some("value".to_string())
        );
    }

    #[test]
    fn test_host_state_header_add() {
        let mut state = HostState::new();
        state.add_header_value(0, "x-multi", "val1");
        state.add_header_value(0, "x-multi", "val2");

        // get_header_value returns first match
        assert_eq!(
            state.get_header_value(0, "x-multi"),
            Some("val1".to_string())
        );
        assert_eq!(state.request_headers.len(), 2);
    }

    #[test]
    fn test_host_state_header_remove() {
        let mut state = HostState::new();
        state.add_header_value(0, "x-remove", "val1");
        state.add_header_value(0, "x-remove", "val2");
        state.add_header_value(0, "x-keep", "keep");

        state.remove_header_value(0, "x-remove");
        assert_eq!(state.get_header_value(0, "x-remove"), None);
        assert_eq!(
            state.get_header_value(0, "x-keep"),
            Some("keep".to_string())
        );
        assert_eq!(state.request_headers.len(), 1);
    }

    #[test]
    fn test_host_state_header_response_map() {
        let mut state = HostState::new();
        state
            .response_headers
            .push(("content-type".to_string(), "text/html".to_string()));

        assert_eq!(
            state.get_header_value(2, "content-type"),
            Some("text/html".to_string())
        );
        state.replace_header_value(2, "content-type", "application/json");
        assert_eq!(
            state.get_header_value(2, "content-type"),
            Some("application/json".to_string())
        );
    }

    #[test]
    fn test_host_state_buffer_get_set() {
        let mut state = HostState::new();
        state.request_body = b"hello world".to_vec();

        // Full read
        assert_eq!(
            state.get_buffer(0, 0, 11),
            Some(b"hello world".to_vec())
        );
        // Partial read
        assert_eq!(state.get_buffer(0, 6, 5), Some(b"world".to_vec()));
        // Out of range read (clamped)
        assert_eq!(state.get_buffer(0, 6, 100), Some(b"world".to_vec()));

        // Set buffer (replace range)
        state.set_buffer(0, 0, 5, b"HELLO");
        assert_eq!(state.request_body, b"HELLO world");

        // Set buffer (replace all)
        state.set_buffer(0, 0, 11, b"new body");
        assert_eq!(state.request_body, b"new body");
    }

    #[test]
    fn test_host_state_buffer_plugin_config() {
        let mut state = HostState::new();
        state.plugin_config = b"{\"key\":\"value\"}".to_vec();

        assert_eq!(
            state.get_buffer(5, 0, 15),
            Some(b"{\"key\":\"value\"}".to_vec())
        );
    }

    #[test]
    fn test_host_state_buffer_invalid_type() {
        let state = HostState::new();
        assert_eq!(state.get_buffer(99, 0, 10), None);
    }

    #[test]
    fn test_host_state_property_operations() {
        let mut state = HostState::new();

        state
            .properties
            .insert("plugin_name".to_string(), b"test-plugin".to_vec());
        assert_eq!(
            state.properties.get("plugin_name"),
            Some(&b"test-plugin".to_vec())
        );

        assert_eq!(state.properties.get("missing"), None);

        state
            .properties
            .insert("plugin_name".to_string(), b"new-name".to_vec());
        assert_eq!(
            state.properties.get("plugin_name"),
            Some(&b"new-name".to_vec())
        );
    }

    // ════════════════════════════════════
    // Serialization tests
    // ════════════════════════════════════

    #[test]
    fn test_serialize_deserialize_header_map() {
        let headers = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("authorization".to_string(), "Bearer token".to_string()),
        ];
        let serialized = serialize_header_map(&headers);
        let deserialized = deserialize_header_map(&serialized).unwrap();
        assert_eq!(deserialized, headers);
    }

    #[test]
    fn test_serialize_empty_header_map() {
        let headers: Vec<(String, String)> = vec![];
        let serialized = serialize_header_map(&headers);
        let deserialized = deserialize_header_map(&serialized).unwrap();
        assert_eq!(deserialized, headers);
    }

    // ════════════════════════════════════
    // WasmProxyPlugin::new tests
    // ════════════════════════════════════

    #[test]
    fn test_wasm_plugin_new_from_wat() {
        let plugin = WasmProxyPlugin::new("test-minimal", MINIMAL_WAT.as_bytes());
        assert!(plugin.is_ok());
        assert_eq!(plugin.unwrap().name(), "test-minimal");
    }

    #[test]
    fn test_wasm_plugin_new_invalid_wasm() {
        let result = WasmProxyPlugin::new("bad", b"not valid wasm");
        assert!(result.is_err());
    }

    #[test]
    fn test_wasm_plugin_on_config() {
        let mut plugin =
            WasmProxyPlugin::new("test-config", MINIMAL_WAT.as_bytes()).unwrap();
        let config = serde_json::json!({"model": "gpt-4", "temperature": 0.7});
        let result = plugin.on_config(&config);
        assert!(result.is_ok());
        assert!(!plugin.config_bytes.is_empty());
    }

    // ════════════════════════════════════
    // Request headers flow tests
    // ════════════════════════════════════

    #[test]
    fn test_wasm_on_request_headers_passthrough() {
        let plugin =
            WasmProxyPlugin::new("passthrough", MINIMAL_WAT.as_bytes()).unwrap();
        let mut ctx = ProxyContext::new();
        ctx.request_headers
            .insert("content-type".to_string(), "application/json".to_string());
        ctx.request_path = "/v1/chat/completions".to_string();
        ctx.request_method = "POST".to_string();

        let action = plugin.on_request_headers(&mut ctx);
        assert_eq!(action, Action::Continue);
        assert_eq!(
            ctx.request_headers.get("content-type"),
            Some(&"application/json".to_string())
        );
        assert_eq!(ctx.request_path, "/v1/chat/completions");
        assert_eq!(ctx.request_method, "POST");
    }

    #[test]
    fn test_wasm_on_request_headers_pause_action() {
        let plugin = WasmProxyPlugin::new("pauser", PAUSE_WAT.as_bytes()).unwrap();
        let mut ctx = ProxyContext::new();

        let action = plugin.on_request_headers(&mut ctx);
        assert_eq!(action, Action::Stop);
    }

    #[test]
    fn test_wasm_on_request_headers_modifier() {
        let plugin =
            WasmProxyPlugin::new("modifier", HEADER_MODIFIER_WAT.as_bytes()).unwrap();
        let mut ctx = ProxyContext::new();
        ctx.request_headers
            .insert("content-type".to_string(), "application/json".to_string());

        let action = plugin.on_request_headers(&mut ctx);
        assert_eq!(action, Action::Continue);

        assert_eq!(
            ctx.request_headers.get("x-wasm-processed"),
            Some(&"true".to_string())
        );
        assert_eq!(
            ctx.request_headers.get("x-added"),
            Some(&"yes".to_string())
        );
        assert_eq!(
            ctx.request_headers.get("content-type"),
            Some(&"application/json".to_string())
        );
    }

    // ════════════════════════════════════
    // Request body flow tests
    // ════════════════════════════════════

    #[test]
    fn test_wasm_on_request_body_passthrough() {
        let plugin =
            WasmProxyPlugin::new("passthrough", MINIMAL_WAT.as_bytes()).unwrap();
        let mut ctx = ProxyContext::new();
        let body = b"original body".to_vec();

        let result = plugin.on_request_body(&mut ctx, body.clone());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), body);
    }

    #[test]
    fn test_wasm_on_request_body_modifier() {
        let plugin =
            WasmProxyPlugin::new("body-mod", BODY_MODIFIER_WAT.as_bytes()).unwrap();
        let mut ctx = ProxyContext::new();
        let body = b"original body data".to_vec();

        let result = plugin.on_request_body(&mut ctx, body);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), b"modified");
    }

    // ════════════════════════════════════
    // Response flow tests
    // ════════════════════════════════════

    #[test]
    fn test_wasm_on_response_headers_passthrough() {
        let plugin =
            WasmProxyPlugin::new("passthrough", MINIMAL_WAT.as_bytes()).unwrap();
        let mut ctx = ProxyContext::new();
        ctx.response_headers
            .insert("content-type".to_string(), "text/html".to_string());
        ctx.response_status = Some(200);

        let action = plugin.on_response_headers(&mut ctx);
        assert_eq!(action, Action::Continue);
        assert_eq!(
            ctx.response_headers.get("content-type"),
            Some(&"text/html".to_string())
        );
        assert_eq!(ctx.response_status, Some(200));
    }

    #[test]
    fn test_wasm_on_response_body_passthrough() {
        let plugin =
            WasmProxyPlugin::new("passthrough", MINIMAL_WAT.as_bytes()).unwrap();
        let mut ctx = ProxyContext::new();
        let body = b"response body".to_vec();

        let result = plugin.on_response_body(&mut ctx, body.clone());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), body);
    }

    // ════════════════════════════════════
    // proxy_log host function tests
    // ════════════════════════════════════

    #[test]
    fn test_wasm_proxy_log() {
        let plugin =
            WasmProxyPlugin::new("logger", LOGGING_WAT.as_bytes()).unwrap();
        let mut ctx = ProxyContext::new();

        let action = plugin.on_request_headers(&mut ctx);
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn test_wasm_proxy_log_captures_messages() {
        let plugin =
            WasmProxyPlugin::new("logger", LOGGING_WAT.as_bytes()).unwrap();
        let state = Arc::new(Mutex::new(HostState::new()));

        let (mut store, instance) = plugin.create_instance(state.clone()).unwrap();

        // Create context
        if let Ok(create_ctx) = instance
            .get_typed_func::<(i32, i32), ()>(&mut store, "proxy_on_context_create")
        {
            let _ = create_ctx.call(&mut store, (ROOT_CONTEXT_ID, 0));
            let _ =
                create_ctx.call(&mut store, (STREAM_CONTEXT_ID, ROOT_CONTEXT_ID));
        }

        // Call proxy_on_request_headers which triggers proxy_log
        let on_headers = instance
            .get_typed_func::<(i32, i32, i32), i32>(
                &mut store,
                "proxy_on_request_headers",
            )
            .unwrap();
        let action = on_headers
            .call(&mut store, (STREAM_CONTEXT_ID, 0, 1))
            .unwrap();
        assert_eq!(action, 0); // Continue

        // Check captured log messages
        let s = state.lock().unwrap();
        assert_eq!(s.log_messages.len(), 1);
        assert_eq!(s.log_messages[0].0, 2); // Info level
        assert_eq!(s.log_messages[0].1, "hello from wasm");
    }

    // ════════════════════════════════════
    // Integration: WasmProxyPlugin in ProxyPluginChain
    // ════════════════════════════════════

    #[test]
    fn test_wasm_plugin_in_chain() {
        use super::super::chain::ProxyPluginChain;

        let plugin =
            WasmProxyPlugin::new("wasm-test", MINIMAL_WAT.as_bytes()).unwrap();
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(plugin));

        let mut ctx = ProxyContext::new();
        ctx.request_headers
            .insert("host".to_string(), "example.com".to_string());

        let action = chain.process_request_headers(&mut ctx);
        assert_eq!(action, Action::Continue);

        let body = b"test body".to_vec();
        let result = chain
            .process_request_body(&mut ctx, body.clone())
            .unwrap();
        assert_eq!(result, body);
    }

    #[test]
    fn test_wasm_header_modifier_in_chain() {
        use super::super::chain::ProxyPluginChain;

        let plugin = WasmProxyPlugin::new(
            "wasm-modifier",
            HEADER_MODIFIER_WAT.as_bytes(),
        )
        .unwrap();
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(plugin));

        let mut ctx = ProxyContext::new();
        ctx.request_headers
            .insert("content-type".to_string(), "application/json".to_string());

        let action = chain.process_request_headers(&mut ctx);
        assert_eq!(action, Action::Continue);

        assert_eq!(
            ctx.request_headers.get("x-wasm-processed"),
            Some(&"true".to_string())
        );
        assert_eq!(
            ctx.request_headers.get("x-added"),
            Some(&"yes".to_string())
        );
    }

    // ════════════════════════════════════
    // OpenAI Compat WASM plugin — E2E integration tests
    // ════════════════════════════════════

    /// Load the compiled OpenAI compat WASM plugin.
    fn load_openai_compat_wasm() -> Vec<u8> {
        let wasm_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("wasm_fixtures")
            .join("prism_openai_compat.wasm");
        std::fs::read(&wasm_path).unwrap_or_else(|e| {
            panic!(
                "Failed to load OpenAI compat WASM plugin from {:?}: {}. \
                 Build it first with: cd examples/wasm-plugins/openai-compat && \
                 cargo build --target wasm32-wasip1 --release",
                wasm_path, e
            )
        })
    }

    #[test]
    fn test_openai_compat_wasm_loads() {
        let wasm_bytes = load_openai_compat_wasm();
        let plugin = WasmProxyPlugin::new("openai-compat", &wasm_bytes);
        assert!(plugin.is_ok(), "Failed to load OpenAI compat WASM plugin: {:?}", plugin.err());
    }

    #[test]
    fn test_openai_compat_wasm_request_headers_rewrite() {
        let wasm_bytes = load_openai_compat_wasm();
        let plugin = WasmProxyPlugin::new("openai-compat", &wasm_bytes).unwrap();

        let mut ctx = ProxyContext::new();
        ctx.request_path = "/api/chat".to_string();
        ctx.request_method = "POST".to_string();
        ctx.request_headers
            .insert("content-type".to_string(), "application/json".to_string());

        let action = plugin.on_request_headers(&mut ctx);
        assert_eq!(action, Action::Continue);

        // Path should be rewritten
        assert_eq!(ctx.request_path, "/v1/chat/completions");

        // X-Prism-Converted header should be added
        assert_eq!(
            ctx.request_headers.get("X-Prism-Converted"),
            Some(&"true".to_string())
        );
    }

    #[test]
    fn test_openai_compat_wasm_request_headers_no_rewrite() {
        let wasm_bytes = load_openai_compat_wasm();
        let plugin = WasmProxyPlugin::new("openai-compat", &wasm_bytes).unwrap();

        let mut ctx = ProxyContext::new();
        ctx.request_path = "/v1/chat/completions".to_string();
        ctx.request_method = "POST".to_string();

        let action = plugin.on_request_headers(&mut ctx);
        assert_eq!(action, Action::Continue);

        // Path should NOT be rewritten
        assert_eq!(ctx.request_path, "/v1/chat/completions");

        // X-Prism-Converted should still be added
        assert_eq!(
            ctx.request_headers.get("X-Prism-Converted"),
            Some(&"true".to_string())
        );
    }

    #[test]
    fn test_openai_compat_wasm_request_body_prompt_to_messages() {
        let wasm_bytes = load_openai_compat_wasm();
        let plugin = WasmProxyPlugin::new("openai-compat", &wasm_bytes).unwrap();

        let mut ctx = ProxyContext::new();
        let body = br#"{"prompt":"Hello world"}"#.to_vec();

        let result = plugin.on_request_body(&mut ctx, body).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();

        assert!(parsed["messages"].is_array());
        let messages = parsed["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Hello world");
    }

    #[test]
    fn test_openai_compat_wasm_request_body_passthrough() {
        let wasm_bytes = load_openai_compat_wasm();
        let plugin = WasmProxyPlugin::new("openai-compat", &wasm_bytes).unwrap();

        let mut ctx = ProxyContext::new();
        // Body that already has "messages" — should pass through unchanged
        let body = br#"{"messages":[{"role":"user","content":"Hi"}]}"#.to_vec();

        let result = plugin.on_request_body(&mut ctx, body.clone()).unwrap();
        // Should be unchanged since there's no "prompt" field
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert!(parsed["messages"].is_array());
    }

    #[test]
    fn test_openai_compat_wasm_response_body_adds_converted() {
        let wasm_bytes = load_openai_compat_wasm();
        let plugin = WasmProxyPlugin::new("openai-compat", &wasm_bytes).unwrap();

        let mut ctx = ProxyContext::new();
        let body = br#"{"id":"chatcmpl-1","choices":[]}"#.to_vec();

        let result = plugin.on_response_body(&mut ctx, body).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();

        assert_eq!(parsed["prism_converted"], true);
        assert_eq!(parsed["id"], "chatcmpl-1");
        assert!(parsed["choices"].is_array());
    }

    #[test]
    fn test_openai_compat_wasm_e2e_full_pipeline() {
        use super::super::chain::ProxyPluginChain;

        let wasm_bytes = load_openai_compat_wasm();
        let plugin = WasmProxyPlugin::new("openai-compat", &wasm_bytes).unwrap();

        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(plugin));

        // ── Request phase ──

        let mut ctx = ProxyContext::new();
        ctx.request_path = "/api/chat".to_string();
        ctx.request_method = "POST".to_string();
        ctx.request_headers
            .insert("content-type".to_string(), "application/json".to_string());

        // Process request headers
        let action = chain.process_request_headers(&mut ctx);
        assert_eq!(action, Action::Continue);
        assert_eq!(ctx.request_path, "/v1/chat/completions");
        assert_eq!(
            ctx.request_headers.get("X-Prism-Converted"),
            Some(&"true".to_string())
        );

        // Process request body
        let req_body = br#"{"prompt":"Write a quicksort in Rust"}"#.to_vec();
        let transformed_body = chain.process_request_body(&mut ctx, req_body).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_slice(&transformed_body).unwrap();
        assert!(parsed["messages"].is_array());
        let messages = parsed["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Write a quicksort in Rust");

        // ── Response phase ──

        // Process response headers
        ctx.response_status = Some(200);
        ctx.response_headers
            .insert("content-type".to_string(), "application/json".to_string());
        let action = chain.process_response_headers(&mut ctx);
        assert_eq!(action, Action::Continue);

        // Process response body
        let resp_body =
            br#"{"id":"chatcmpl-abc","choices":[{"message":{"content":"fn quicksort..."}}]}"#
                .to_vec();
        let transformed_resp = chain.process_response_body(&mut ctx, resp_body).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_slice(&transformed_resp).unwrap();
        assert_eq!(parsed["prism_converted"], true);
        assert_eq!(parsed["id"], "chatcmpl-abc");
        assert!(parsed["choices"].is_array());
    }

    #[test]
    fn test_openai_compat_wasm_config() {
        let wasm_bytes = load_openai_compat_wasm();
        let mut plugin = WasmProxyPlugin::new("openai-compat", &wasm_bytes).unwrap();

        // Plugin should accept any config
        let config = serde_json::json!({"some_setting": true});
        let result = plugin.on_config(&config);
        assert!(result.is_ok());
    }
}
