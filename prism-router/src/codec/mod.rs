//! Proxy plugin framework for protocol translation.
//!
//! Provides a plugin chain that intercepts HTTP request/response at the proxy layer,
//! enabling protocol conversion (e.g., OpenAI ↔ Anthropic) via WASM plugins.
//!
//! Inspired by Higress ai-proxy / Envoy proxy-wasm filter chain.

pub mod chain;
pub mod quota_recorder;
pub mod auth;
pub mod retry;
pub mod usage_extractor;
pub mod registry;
#[cfg(feature = "wasm")]
pub mod wasm_host;

use std::collections::HashMap;

// ════════════════════════════════════════════════
// Action
// ════════════════════════════════════════════════

/// Action returned by plugin hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Continue to the next plugin / phase.
    Continue,
    /// Stop processing the plugin chain and return immediately.
    /// (e.g., plugin generated a direct response)
    Stop,
}

// ════════════════════════════════════════════════
// ProxyContext — per-request shared state
// ════════════════════════════════════════════════

/// Per-request context shared across proxy plugin phases.
/// Plugins can read/modify headers, path, auth, and store arbitrary data.
pub struct ProxyContext {
    // ── Inbound (from user) ──
    /// Detected inbound protocol (e.g., "openai", "anthropic").
    pub inbound_protocol: String,
    /// Original request path.
    pub request_path: String,
    /// Original request method.
    pub request_method: String,
    /// Request headers (mutable).
    pub request_headers: HashMap<String, String>,
    /// Whether the request is streaming.
    pub is_streaming: bool,

    // ── Outbound (to upstream) ──
    /// Target outbound protocol (determined by provider config).
    pub outbound_protocol: String,
    /// Rewritten upstream path (plugins can modify).
    pub upstream_path: String,
    /// Upstream host.
    pub upstream_host: String,
    /// Upstream auth header value.
    pub upstream_auth: Option<String>,

    // ── Response ──
    /// Response status code from upstream.
    pub response_status: Option<u16>,
    /// Response headers (mutable).
    pub response_headers: HashMap<String, String>,

    // ── Routing info (set by proxy before plugin chain) ──
    /// Provider ID selected by routing.
    pub selected_provider: String,
    /// Model name selected by routing.
    pub selected_model: String,
    /// Variant ID selected by routing.
    pub selected_variant: String,

    // ── Plugin private data (like PluginState in routing) ──
    data: HashMap<String, Box<dyn std::any::Any + Send + Sync>>,
}

impl ProxyContext {
    /// Create a new empty context with all fields set to defaults.
    pub fn new() -> Self {
        Self {
            inbound_protocol: String::new(),
            request_path: String::new(),
            request_method: String::new(),
            request_headers: HashMap::new(),
            is_streaming: false,
            outbound_protocol: String::new(),
            upstream_path: String::new(),
            upstream_host: String::new(),
            upstream_auth: None,
            response_status: None,
            response_headers: HashMap::new(),
            selected_provider: String::new(),
            selected_model: String::new(),
            selected_variant: String::new(),
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

impl Default for ProxyContext {
    fn default() -> Self {
        Self::new()
    }
}

// ════════════════════════════════════════════════
// ProxyPlugin trait
// ════════════════════════════════════════════════

/// The proxy plugin trait — each plugin implements the hooks it needs.
/// Default implementations are no-ops (pass through).
///
/// Hook execution order:
///   Request:  `on_request_headers` → `on_request_body`  (in chain order)
///   Response: `on_response_headers` → `on_response_body` (in REVERSE chain order)
pub trait ProxyPlugin: Send + Sync {
    /// Plugin name, used for logging and config references.
    fn name(&self) -> &str;

    /// Called when the plugin is initialized with its config.
    /// Return Err to indicate invalid config.
    fn on_config(&mut self, _config: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }

    /// Phase 1: Request headers arrived.
    /// Can modify `ctx.upstream_path`, `ctx.request_headers`, `ctx.upstream_auth`, etc.
    fn on_request_headers(&self, _ctx: &mut ProxyContext) -> Action {
        Action::Continue
    }

    /// Phase 2: Request body arrived (full body for non-streaming).
    /// Can transform the body (e.g., OpenAI JSON → Anthropic JSON).
    /// Return the (potentially modified) body.
    fn on_request_body(&self, _ctx: &mut ProxyContext, body: Vec<u8>) -> Result<Vec<u8>, String> {
        Ok(body)
    }

    /// Phase 3: Response headers arrived from upstream.
    fn on_response_headers(&self, _ctx: &mut ProxyContext) -> Action {
        Action::Continue
    }

    /// Phase 4: Response body arrived (full body for non-streaming).
    /// Can transform the response body back to inbound protocol format.
    fn on_response_body(&self, _ctx: &mut ProxyContext, body: Vec<u8>) -> Result<Vec<u8>, String> {
        Ok(body)
    }

    /// Phase 4b: Streaming response — called for each SSE chunk/line.
    /// Return `None` to filter out the chunk, `Some(new_chunk)` to transform.
    fn on_response_stream_chunk(
        &self,
        _ctx: &mut ProxyContext,
        chunk: &str,
    ) -> Result<Option<String>, String> {
        Ok(Some(chunk.to_string()))
    }

    /// Phase 5: Called at request completion (logging phase).
    /// Used for metering, billing, auditing — has access to the full ctx
    /// including response status, duration, selected variant/provider/model, etc.
    fn on_log(&self, _ctx: &mut ProxyContext) {}
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Test helpers ──

    /// A plugin that does nothing — all defaults.
    struct PassthroughPlugin;

    impl ProxyPlugin for PassthroughPlugin {
        fn name(&self) -> &str {
            "passthrough"
        }
    }

    // ── ProxyContext tests ──

    #[test]
    fn test_context_new_defaults() {
        let ctx = ProxyContext::new();
        assert_eq!(ctx.inbound_protocol, "");
        assert_eq!(ctx.request_path, "");
        assert_eq!(ctx.request_method, "");
        assert!(ctx.request_headers.is_empty());
        assert!(!ctx.is_streaming);
        assert_eq!(ctx.outbound_protocol, "");
        assert_eq!(ctx.upstream_path, "");
        assert_eq!(ctx.upstream_host, "");
        assert!(ctx.upstream_auth.is_none());
        assert!(ctx.response_status.is_none());
        assert!(ctx.response_headers.is_empty());
        assert_eq!(ctx.selected_provider, "");
        assert_eq!(ctx.selected_model, "");
        assert_eq!(ctx.selected_variant, "");
    }

    #[test]
    fn test_context_set_get_typed_data() {
        let mut ctx = ProxyContext::new();

        ctx.set("counter", 42u64);
        ctx.set("label", "hello".to_string());
        ctx.set("flag", true);

        assert_eq!(ctx.get::<u64>("counter"), Some(&42u64));
        assert_eq!(ctx.get::<String>("label"), Some(&"hello".to_string()));
        assert_eq!(ctx.get::<bool>("flag"), Some(&true));

        // Wrong type returns None.
        assert_eq!(ctx.get::<String>("counter"), None);
        // Missing key returns None.
        assert_eq!(ctx.get::<u64>("missing"), None);
    }

    #[test]
    fn test_context_set_overwrites() {
        let mut ctx = ProxyContext::new();
        ctx.set("val", 1u32);
        assert_eq!(ctx.get::<u32>("val"), Some(&1));
        ctx.set("val", 2u32);
        assert_eq!(ctx.get::<u32>("val"), Some(&2));
    }

    #[test]
    fn test_context_fields_read_write() {
        let mut ctx = ProxyContext::new();
        ctx.inbound_protocol = "openai".to_string();
        ctx.request_path = "/v1/chat/completions".to_string();
        ctx.request_method = "POST".to_string();
        ctx.request_headers
            .insert("content-type".to_string(), "application/json".to_string());
        ctx.is_streaming = true;
        ctx.outbound_protocol = "anthropic".to_string();
        ctx.upstream_path = "/v1/messages".to_string();
        ctx.upstream_host = "api.anthropic.com".to_string();
        ctx.upstream_auth = Some("Bearer sk-ant-xxx".to_string());
        ctx.response_status = Some(200);
        ctx.response_headers
            .insert("x-request-id".to_string(), "abc123".to_string());
        ctx.selected_provider = "anthropic-main".to_string();
        ctx.selected_model = "claude-3-opus".to_string();
        ctx.selected_variant = "claude-3-opus-anthropic".to_string();

        assert_eq!(ctx.inbound_protocol, "openai");
        assert_eq!(ctx.request_path, "/v1/chat/completions");
        assert_eq!(ctx.request_method, "POST");
        assert_eq!(
            ctx.request_headers.get("content-type"),
            Some(&"application/json".to_string())
        );
        assert!(ctx.is_streaming);
        assert_eq!(ctx.outbound_protocol, "anthropic");
        assert_eq!(ctx.upstream_path, "/v1/messages");
        assert_eq!(ctx.upstream_host, "api.anthropic.com");
        assert_eq!(ctx.upstream_auth, Some("Bearer sk-ant-xxx".to_string()));
        assert_eq!(ctx.response_status, Some(200));
        assert_eq!(
            ctx.response_headers.get("x-request-id"),
            Some(&"abc123".to_string())
        );
        assert_eq!(ctx.selected_provider, "anthropic-main");
        assert_eq!(ctx.selected_model, "claude-3-opus");
        assert_eq!(ctx.selected_variant, "claude-3-opus-anthropic");
    }

    // ── ProxyPlugin trait default tests ──

    #[test]
    fn test_plugin_default_on_config() {
        let mut plugin = PassthroughPlugin;
        let config = serde_json::json!({});
        assert!(plugin.on_config(&config).is_ok());
    }

    #[test]
    fn test_plugin_default_on_request_headers() {
        let plugin = PassthroughPlugin;
        let mut ctx = ProxyContext::new();
        assert_eq!(plugin.on_request_headers(&mut ctx), Action::Continue);
    }

    #[test]
    fn test_plugin_default_on_request_body() {
        let plugin = PassthroughPlugin;
        let mut ctx = ProxyContext::new();
        let body = b"hello world".to_vec();
        let result = plugin.on_request_body(&mut ctx, body.clone());
        assert_eq!(result.unwrap(), body);
    }

    #[test]
    fn test_plugin_default_on_response_headers() {
        let plugin = PassthroughPlugin;
        let mut ctx = ProxyContext::new();
        assert_eq!(plugin.on_response_headers(&mut ctx), Action::Continue);
    }

    #[test]
    fn test_plugin_default_on_response_body() {
        let plugin = PassthroughPlugin;
        let mut ctx = ProxyContext::new();
        let body = b"response data".to_vec();
        let result = plugin.on_response_body(&mut ctx, body.clone());
        assert_eq!(result.unwrap(), body);
    }

    #[test]
    fn test_plugin_default_on_response_stream_chunk() {
        let plugin = PassthroughPlugin;
        let mut ctx = ProxyContext::new();
        let chunk = "data: {\"id\":\"1\"}\n\n";
        let result = plugin.on_response_stream_chunk(&mut ctx, chunk);
        assert_eq!(result.unwrap(), Some(chunk.to_string()));
    }
}
