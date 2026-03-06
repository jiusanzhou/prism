//! Azure OpenAI Provider Plugin (WASM)
//!
//! Implements the Prism provider WASM ABI for Azure OpenAI:
//! - Auth: `api-key: <key>` header (not Bearer)
//! - URL: `/openai/deployments/{deployment}/chat/completions?api-version={version}`
//! - Body: pass-through (Azure speaks OpenAI format)
//!
//! Required provider_extra fields:
//! - `api_version` (default: "2025-04-01-preview")
//!
//! The deployment name comes from `ctx.deployment_name` or falls back to `ctx.model_id`.

#![allow(clippy::missing_safety_doc)]

use serde::{Deserialize, Serialize};

// ════════════════════════════════════════════════
// Host imports
// ════════════════════════════════════════════════

#[link(wasm_import_module = "env")]
extern "C" {
    fn prism_log(level: i32, msg_ptr: i32, msg_len: i32) -> i32;
}

fn log_info(msg: &str) {
    unsafe {
        prism_log(2, msg.as_ptr() as i32, msg.len() as i32);
    }
}

// ════════════════════════════════════════════════
// Guest allocator
// ════════════════════════════════════════════════

/// Simple bump allocator for returning data to the host.
static mut ALLOC_OFFSET: usize = 0;
static mut ALLOC_BUF: [u8; 65536] = [0u8; 65536];

#[no_mangle]
pub extern "C" fn prism_provider_alloc(size: i32) -> i32 {
    unsafe {
        let offset = ALLOC_OFFSET;
        ALLOC_OFFSET += size as usize;
        if ALLOC_OFFSET > ALLOC_BUF.len() {
            return 0; // OOM
        }
        ALLOC_BUF.as_ptr().add(offset) as i32
    }
}

/// Write a length-prefixed result into the alloc buffer.
fn write_result(data: &[u8]) -> i32 {
    unsafe {
        let total = 4 + data.len();
        let offset = ALLOC_OFFSET;
        ALLOC_OFFSET += total;
        if ALLOC_OFFSET > ALLOC_BUF.len() {
            return 0;
        }
        let buf = &mut ALLOC_BUF[offset..offset + total];
        buf[0..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
        buf[4..].copy_from_slice(data);
        ALLOC_BUF.as_ptr().add(offset) as i32
    }
}

fn reset_alloc() {
    unsafe {
        ALLOC_OFFSET = 0;
    }
}

// ════════════════════════════════════════════════
// Provider context (deserialized from host JSON)
// ════════════════════════════════════════════════

#[derive(Deserialize)]
struct ProviderCtx {
    provider_id: String,
    provider_type: String,
    base_url: String,
    #[serde(default)]
    provider_extra: std::collections::HashMap<String, serde_json::Value>,
    account_id: String,
    account_api_key: Option<String>,
    #[serde(default)]
    account_headers: std::collections::HashMap<String, String>,
    variant_id: String,
    model_id: String,
    upstream_model_name: Option<String>,
    deployment_name: Option<String>,
    original_path: String,
    method: String,
    is_streaming: bool,
}

// ════════════════════════════════════════════════
// Config schema
// ════════════════════════════════════════════════

#[derive(Serialize)]
struct ConfigSchema {
    provider_fields: Vec<ConfigField>,
    account_fields: Vec<ConfigField>,
}

#[derive(Serialize)]
struct ConfigField {
    name: String,
    label: String,
    field_type: String,
    required: bool,
    default: Option<String>,
    options: Vec<String>,
    description: String,
}

// ════════════════════════════════════════════════
// ABI exports
// ════════════════════════════════════════════════

#[no_mangle]
pub extern "C" fn prism_provider_init() {
    log_info("Azure OpenAI provider plugin initialized");
}

#[no_mangle]
pub extern "C" fn prism_provider_id() -> i32 {
    reset_alloc();
    write_result(b"azure")
}

#[no_mangle]
pub extern "C" fn prism_provider_config_schema() -> i32 {
    reset_alloc();
    let schema = ConfigSchema {
        provider_fields: vec![
            ConfigField {
                name: "api_version".to_string(),
                label: "API Version".to_string(),
                field_type: "string".to_string(),
                required: false,
                default: Some("2025-04-01-preview".to_string()),
                options: vec![],
                description: "Azure OpenAI API version".to_string(),
            },
        ],
        account_fields: vec![
            ConfigField {
                name: "api_key".to_string(),
                label: "API Key".to_string(),
                field_type: "secret".to_string(),
                required: true,
                default: None,
                options: vec![],
                description: "Azure API key (sent as api-key header)".to_string(),
            },
        ],
    };
    let json = serde_json::to_vec(&schema).unwrap_or_default();
    write_result(&json)
}

/// Build auth: Azure uses `api-key` header instead of Bearer.
#[no_mangle]
pub extern "C" fn prism_provider_build_auth(ctx_ptr: i32, ctx_len: i32) -> i32 {
    reset_alloc();

    let ctx_bytes = unsafe {
        std::slice::from_raw_parts(ctx_ptr as *const u8, ctx_len as usize)
    };
    let ctx: ProviderCtx = match serde_json::from_slice(ctx_bytes) {
        Ok(c) => c,
        Err(_) => return write_result(b"{}"),
    };

    let key = ctx.account_api_key.unwrap_or_default();
    if key.is_empty() {
        return write_result(b"{\"headers\":[]}");
    }

    // Azure uses api-key header
    let result = serde_json::json!({
        "headers": [
            ["api-key", key]
        ]
    });
    let json = serde_json::to_vec(&result).unwrap_or_default();
    write_result(&json)
}

/// Rewrite URL: /v1/chat/completions → /openai/deployments/{name}/chat/completions?api-version=...
#[no_mangle]
pub extern "C" fn prism_provider_rewrite_url(ctx_ptr: i32, ctx_len: i32) -> i32 {
    reset_alloc();

    let ctx_bytes = unsafe {
        std::slice::from_raw_parts(ctx_ptr as *const u8, ctx_len as usize)
    };
    let ctx: ProviderCtx = match serde_json::from_slice(ctx_bytes) {
        Ok(c) => c,
        Err(_) => return write_result(b"{}"),
    };

    // Deployment name: explicit > upstream_model_name > model_id
    let deployment = ctx.deployment_name
        .as_deref()
        .or(ctx.upstream_model_name.as_deref())
        .unwrap_or(&ctx.model_id);

    // API version from extra config
    let api_version = ctx.provider_extra
        .get("api_version")
        .and_then(|v| v.as_str())
        .unwrap_or("2025-04-01-preview");

    // Map original path to Azure deployment path
    let azure_path = if ctx.original_path.contains("/chat/completions") {
        format!(
            "/openai/deployments/{}/chat/completions?api-version={}",
            deployment, api_version
        )
    } else if ctx.original_path.contains("/completions") {
        format!(
            "/openai/deployments/{}/completions?api-version={}",
            deployment, api_version
        )
    } else if ctx.original_path.contains("/embeddings") {
        format!(
            "/openai/deployments/{}/embeddings?api-version={}",
            deployment, api_version
        )
    } else if ctx.original_path.contains("/images/generations") {
        format!(
            "/openai/deployments/{}/images/generations?api-version={}",
            deployment, api_version
        )
    } else if ctx.original_path.contains("/images/edits") {
        format!(
            "/openai/deployments/{}/images/edits?api-version={}",
            deployment, api_version
        )
    } else {
        // Unknown path — pass through with api-version
        format!("{}?api-version={}", ctx.original_path, api_version)
    };

    let result = serde_json::json!({
        "path": azure_path
    });
    let json = serde_json::to_vec(&result).unwrap_or_default();
    write_result(&json)
}
