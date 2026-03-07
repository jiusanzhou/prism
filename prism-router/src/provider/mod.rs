//! Provider plugin framework.
//!
//! Each LLM provider type (OpenAI, Azure, Bedrock, Anthropic, etc.) implements
//! the [`ProviderPlugin`] trait. The plugin handles provider-specific concerns:
//!
//! - **Authentication**: Bearer tokens, API keys, AWS SigV4 signing, etc.
//! - **URL rewriting**: OpenAI pass-through, Azure deployment URLs, Bedrock model paths.
//! - **Body transformation**: Request/response format conversion for non-OpenAI protocols.
//! - **Config schema**: Declares what fields the provider needs (for Admin UI dynamic forms).
//!
//! Unlike [`ProxyPlugin`](crate::codec::ProxyPlugin) which chains multiple plugins
//! for cross-cutting concerns (metering, rate limiting), a ProviderPlugin is
//! **selected per-request** based on `provider_type` — only one runs per request.
//!
//! # Execution order
//!
//! ```text
//! upstream_request_filter:
//!   1. ProviderPlugin::rewrite_url()        → set upstream path
//!   2. ProviderPlugin::build_auth()         → set auth headers
//!   3. ProxyPlugin chain: on_request_headers → cross-cutting header processing
//!
//! request_body_filter:
//!   4. ProviderPlugin::transform_request_body()  → provider-specific body conversion
//!   5. ProxyPlugin chain: on_request_body        → cross-cutting body processing
//!
//! response:
//!   6. ProxyPlugin chain: on_response_* (reverse)
//!   7. ProviderPlugin::transform_response_body() / transform_stream_chunk()
//! ```

pub mod bedrock;
pub mod openai;
pub mod registry;
pub mod vertex;
#[cfg(feature = "wasm")]
pub mod wasm_host;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

/// Provider-specific request context passed to plugin hooks.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderRequestContext {
    /// Provider ID (from routing decision).
    pub provider_id: String,
    /// Provider type (e.g., "openai", "azure", "bedrock").
    pub provider_type: String,
    /// Provider base URL.
    pub base_url: String,
    /// Provider extra config (provider-type-specific fields).
    pub provider_extra: HashMap<String, serde_json::Value>,
    /// Account ID (from routing decision).
    pub account_id: String,
    /// Account API key (raw value).
    pub account_api_key: Option<String>,
    /// Account extra headers.
    pub account_headers: HashMap<String, String>,
    /// Variant ID.
    pub variant_id: String,
    /// Model ID (Prism's model name).
    pub model_id: String,
    /// Upstream model name (vendor-specific, e.g. "deepseek-reasoner").
    pub upstream_model_name: Option<String>,
    /// Deployment name (for Azure etc.).
    pub deployment_name: Option<String>,
    /// Original request path (e.g. "/v1/chat/completions").
    pub original_path: String,
    /// HTTP method.
    pub method: String,
    /// Whether the request is streaming.
    pub is_streaming: bool,
}

/// Result of [`ProviderPlugin::build_auth`].
#[derive(Debug, Clone, Default)]
pub struct AuthResult {
    /// Headers to set on the upstream request.
    /// e.g., [("Authorization", "Bearer sk-..."), ("x-api-key", "...")]
    pub headers: Vec<(String, String)>,
}

/// Result of [`ProviderPlugin::rewrite_url`].
#[derive(Debug, Clone)]
pub struct UrlRewriteResult {
    /// The rewritten upstream path (including query string if needed).
    pub path: String,
    /// Optional host override (if the provider plugin wants to change the host).
    pub host_override: Option<String>,
}

/// Field type for dynamic config forms.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFieldType {
    /// Plain text input.
    String,
    /// Password / secret input (masked in UI).
    Secret,
    /// Dropdown select.
    Select,
    /// Numeric input.
    Number,
    /// Boolean toggle.
    Bool,
}

/// A config field descriptor for Admin UI dynamic forms.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfigField {
    /// Field key (e.g., "aws_region").
    pub name: String,
    /// Display label (e.g., "AWS Region").
    pub label: String,
    /// Field type.
    pub field_type: ConfigFieldType,
    /// Whether this field is required.
    pub required: bool,
    /// Default value (if any).
    #[serde(default)]
    pub default: Option<String>,
    /// Options for Select type.
    #[serde(default)]
    pub options: Vec<String>,
    /// Help text / description.
    #[serde(default)]
    pub description: String,
}

/// Config schema returned by [`ProviderPlugin::config_schema`].
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProviderConfigSchema {
    /// Extra fields needed on the Provider config.
    pub provider_fields: Vec<ConfigField>,
    /// Extra fields needed on the Account config.
    pub account_fields: Vec<ConfigField>,
}

/// The provider plugin trait.
///
/// Implement this for each provider type. Only one provider plugin runs per
/// request (selected by `provider_type`).
///
/// All methods have sensible defaults for OpenAI-compatible providers,
/// so a minimal implementation only needs `id()`.
pub trait ProviderPlugin: Send + Sync {
    /// Provider type identifier (e.g., "openai", "azure", "bedrock").
    /// Must match `ProviderConfig.provider_type`.
    fn id(&self) -> &str;

    /// Human-readable name (e.g., "OpenAI Compatible").
    fn display_name(&self) -> &str {
        self.id()
    }

    /// Declare what extra config fields this provider type needs.
    /// Admin UI uses this to render dynamic forms.
    fn config_schema(&self) -> ProviderConfigSchema {
        ProviderConfigSchema::default()
    }

    /// Validate provider + account configuration.
    /// Called when config is loaded or updated.
    fn validate_config(
        &self,
        _provider_extra: &HashMap<String, serde_json::Value>,
        _account_api_key: &Option<String>,
        _account_headers: &HashMap<String, String>,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Build authentication headers for the upstream request.
    ///
    /// Default: `Authorization: Bearer <api_key>` if api_key is present.
    fn build_auth(&self, ctx: &ProviderRequestContext) -> Result<AuthResult, String> {
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

    /// Rewrite the upstream URL path.
    ///
    /// Default: pass through the original path unchanged.
    fn rewrite_url(&self, ctx: &ProviderRequestContext) -> Result<UrlRewriteResult, String> {
        Ok(UrlRewriteResult {
            path: ctx.original_path.clone(),
            host_override: None,
        })
    }

    /// Transform the request body before sending to upstream.
    ///
    /// Default: pass through unchanged.
    fn transform_request_body(
        &self,
        body: Vec<u8>,
        _ctx: &ProviderRequestContext,
    ) -> Result<Vec<u8>, String> {
        Ok(body)
    }

    /// Transform the response body from upstream.
    ///
    /// Default: pass through unchanged.
    fn transform_response_body(
        &self,
        body: Vec<u8>,
        _ctx: &ProviderRequestContext,
    ) -> Result<Vec<u8>, String> {
        Ok(body)
    }

    /// Transform a streaming SSE chunk from upstream.
    ///
    /// Return `None` to filter out the chunk.
    /// Default: pass through unchanged.
    fn transform_stream_chunk(
        &self,
        chunk: &str,
        _ctx: &ProviderRequestContext,
    ) -> Result<Option<String>, String> {
        Ok(Some(chunk.to_string()))
    }
}
