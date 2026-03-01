//! OpenAI Compatibility Layer — Example WASM Proxy Plugin
//!
//! This plugin normalizes non-standard API requests into the OpenAI format:
//! - Rewrites `/api/chat` path to `/v1/chat/completions`
//! - Adds `X-Prism-Converted: true` header
//! - Converts `{"prompt": "..."}` body to `{"messages": [{"role":"user","content":"..."}]}`
//! - Adds `"prism_converted": true` to response body
//!
//! Implements the proxy-wasm ABI directly (no SDK dependency).

#![allow(clippy::missing_safety_doc)]

// ════════════════════════════════════════════════
// Host function imports (proxy-wasm ABI)
// ════════════════════════════════════════════════

#[link(wasm_import_module = "env")]
extern "C" {
    fn proxy_get_header_map_value(
        map_type: i32,
        key_ptr: i32,
        key_size: i32,
        return_value_ptr: i32,
        return_value_size: i32,
    ) -> i32;

    fn proxy_replace_header_map_value(
        map_type: i32,
        key_ptr: i32,
        key_size: i32,
        value_ptr: i32,
        value_size: i32,
    ) -> i32;

    fn proxy_add_header_map_value(
        map_type: i32,
        key_ptr: i32,
        key_size: i32,
        value_ptr: i32,
        value_size: i32,
    ) -> i32;

    fn proxy_get_buffer_bytes(
        buffer_type: i32,
        start: i32,
        length: i32,
        return_data_ptr: i32,
        return_data_size: i32,
    ) -> i32;

    fn proxy_set_buffer_bytes(
        buffer_type: i32,
        start: i32,
        length: i32,
        data_ptr: i32,
        data_size: i32,
    ) -> i32;

    fn proxy_log(log_level: i32, message_ptr: i32, message_size: i32) -> i32;
}

// ════════════════════════════════════════════════
// Constants
// ════════════════════════════════════════════════

const MAP_TYPE_REQUEST_HEADERS: i32 = 0;
const BUFFER_TYPE_REQUEST_BODY: i32 = 0;
const BUFFER_TYPE_RESPONSE_BODY: i32 = 1;
const LOG_LEVEL_INFO: i32 = 2;
const STATUS_OK: i32 = 0;
const HOST_DATA_OFFSET: usize = 4096;

// ════════════════════════════════════════════════
// Static string data
// ════════════════════════════════════════════════

static PATH_KEY: &[u8] = b":path";
static V1_COMPLETIONS_PATH: &[u8] = b"/v1/chat/completions";
static CONVERTED_KEY: &[u8] = b"X-Prism-Converted";
static TRUE_VALUE: &[u8] = b"true";

// Mutable static buffers for host interaction
static mut RETURN_PTR_SLOT: i32 = 0;
static mut RETURN_SIZE_SLOT: i32 = 0;
static mut BODY_WRITE_BUF: [u8; 32768] = [0u8; 32768];
static mut LOG_MSG_BUF: [u8; 256] = [0u8; 256];

// ════════════════════════════════════════════════
// Memory allocator
// ════════════════════════════════════════════════

static mut ALLOC_OFFSET: i32 = 49152;

#[no_mangle]
pub extern "C" fn proxy_on_memory_allocate(size: i32) -> i32 {
    unsafe {
        let ptr = ALLOC_OFFSET;
        ALLOC_OFFSET += size;
        ptr
    }
}

// ════════════════════════════════════════════════
// Lifecycle exports
// ════════════════════════════════════════════════

#[no_mangle]
pub extern "C" fn proxy_on_context_create(_context_id: i32, _parent_context_id: i32) {}

#[no_mangle]
pub extern "C" fn proxy_on_configure(_context_id: i32, _config_size: i32) -> i32 {
    1
}

// ════════════════════════════════════════════════
// Helpers
// ════════════════════════════════════════════════

unsafe fn log_info(msg: &str) {
    let bytes = msg.as_bytes();
    let len = bytes.len().min(LOG_MSG_BUF.len());
    LOG_MSG_BUF[..len].copy_from_slice(&bytes[..len]);
    proxy_log(LOG_LEVEL_INFO, LOG_MSG_BUF.as_ptr() as i32, len as i32);
}

unsafe fn read_host_returned_data() -> Vec<u8> {
    let size = RETURN_SIZE_SLOT as u32 as usize;
    if size == 0 || size > 32768 {
        return Vec::new();
    }
    let src_ptr = HOST_DATA_OFFSET as *const u8;
    let mut result = Vec::with_capacity(size);
    for i in 0..size {
        result.push(core::ptr::read(src_ptr.add(i)));
    }
    result
}

unsafe fn return_slots() -> (i32, i32) {
    let ptr_addr = core::ptr::addr_of_mut!(RETURN_PTR_SLOT) as i32;
    let size_addr = core::ptr::addr_of_mut!(RETURN_SIZE_SLOT) as i32;
    (ptr_addr, size_addr)
}

// ════════════════════════════════════════════════
// Request headers hook
// ════════════════════════════════════════════════

#[no_mangle]
pub extern "C" fn proxy_on_request_headers(
    _context_id: i32,
    _num_headers: i32,
    _end_of_stream: i32,
) -> i32 {
    unsafe {
        log_info("openai-compat: processing request headers");

        let (rp, rs) = return_slots();

        // 1. Get the :path header
        let status = proxy_get_header_map_value(
            MAP_TYPE_REQUEST_HEADERS,
            PATH_KEY.as_ptr() as i32,
            PATH_KEY.len() as i32,
            rp,
            rs,
        );

        if status == STATUS_OK {
            let path_data = read_host_returned_data();
            if let Ok(path) = core::str::from_utf8(&path_data) {
                if path == "/api/chat" {
                    log_info("openai-compat: rewriting /api/chat -> /v1/chat/completions");
                    proxy_replace_header_map_value(
                        MAP_TYPE_REQUEST_HEADERS,
                        PATH_KEY.as_ptr() as i32,
                        PATH_KEY.len() as i32,
                        V1_COMPLETIONS_PATH.as_ptr() as i32,
                        V1_COMPLETIONS_PATH.len() as i32,
                    );
                }
            }
        }

        // 2. Add X-Prism-Converted: true header
        proxy_add_header_map_value(
            MAP_TYPE_REQUEST_HEADERS,
            CONVERTED_KEY.as_ptr() as i32,
            CONVERTED_KEY.len() as i32,
            TRUE_VALUE.as_ptr() as i32,
            TRUE_VALUE.len() as i32,
        );
    }

    0 // Action::Continue
}

// ════════════════════════════════════════════════
// Request body hook
// ════════════════════════════════════════════════

#[no_mangle]
pub extern "C" fn proxy_on_request_body(
    _context_id: i32,
    body_size: i32,
    _end_of_stream: i32,
) -> i32 {
    if body_size <= 0 {
        return 0;
    }

    unsafe {
        let (rp, rs) = return_slots();

        let status = proxy_get_buffer_bytes(
            BUFFER_TYPE_REQUEST_BODY,
            0,
            body_size,
            rp,
            rs,
        );

        if status != STATUS_OK {
            return 0;
        }

        let body_data = read_host_returned_data();
        if body_data.is_empty() {
            return 0;
        }

        if let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&body_data) {
            if let Some(prompt) = parsed.get("prompt").and_then(|v| v.as_str()) {
                let converted = serde_json::json!({
                    "messages": [
                        {
                            "role": "user",
                            "content": prompt
                        }
                    ]
                });

                if let Ok(new_body) = serde_json::to_vec(&converted) {
                    log_info("openai-compat: converted prompt to messages format");

                    let len = new_body.len().min(BODY_WRITE_BUF.len());
                    BODY_WRITE_BUF[..len].copy_from_slice(&new_body[..len]);

                    proxy_set_buffer_bytes(
                        BUFFER_TYPE_REQUEST_BODY,
                        0,
                        body_size,
                        BODY_WRITE_BUF.as_ptr() as i32,
                        len as i32,
                    );
                }
            }
        }
    }

    0 // Action::Continue
}

// ════════════════════════════════════════════════
// Response headers hook
// ════════════════════════════════════════════════

#[no_mangle]
pub extern "C" fn proxy_on_response_headers(
    _context_id: i32,
    _num_headers: i32,
    _end_of_stream: i32,
) -> i32 {
    0 // Action::Continue
}

// ════════════════════════════════════════════════
// Response body hook
// ════════════════════════════════════════════════

#[no_mangle]
pub extern "C" fn proxy_on_response_body(
    _context_id: i32,
    body_size: i32,
    _end_of_stream: i32,
) -> i32 {
    if body_size <= 0 {
        return 0;
    }

    unsafe {
        let (rp, rs) = return_slots();

        let status = proxy_get_buffer_bytes(
            BUFFER_TYPE_RESPONSE_BODY,
            0,
            body_size,
            rp,
            rs,
        );

        if status != STATUS_OK {
            return 0;
        }

        let body_data = read_host_returned_data();
        if body_data.is_empty() {
            return 0;
        }

        if let Ok(mut parsed) = serde_json::from_slice::<serde_json::Value>(&body_data) {
            if let Some(obj) = parsed.as_object_mut() {
                obj.insert(
                    "prism_converted".to_string(),
                    serde_json::Value::Bool(true),
                );

                if let Ok(new_body) = serde_json::to_vec(&parsed) {
                    log_info("openai-compat: added prism_converted to response");

                    let len = new_body.len().min(BODY_WRITE_BUF.len());
                    BODY_WRITE_BUF[..len].copy_from_slice(&new_body[..len]);

                    proxy_set_buffer_bytes(
                        BUFFER_TYPE_RESPONSE_BODY,
                        0,
                        body_size,
                        BODY_WRITE_BUF.as_ptr() as i32,
                        len as i32,
                    );
                }
            }
        }
    }

    0 // Action::Continue
}
