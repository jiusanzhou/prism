//! Built-in OpenAI-compatible provider plugin.
//!
//! Handles the default case: Bearer token auth, path pass-through, body pass-through.
//! This is the only built-in provider plugin — all other provider types (Azure,
//! Bedrock, Anthropic, etc.) are implemented as WASM plugins.

use super::{
    AuthResult, ConfigField, ConfigFieldType, ProviderConfigSchema, ProviderPlugin,
    ProviderRequestContext, UrlRewriteResult,
};
use std::collections::HashMap;

/// The built-in OpenAI-compatible provider plugin.
///
/// Supports all providers that speak the OpenAI API format:
/// - OpenAI, DeepSeek, Qwen, Gemini (via OpenAI compat), novita.ai, etc.
/// - Auth: `Authorization: Bearer <api_key>`
/// - URL: pass-through (original request path)
/// - Body: pass-through (OpenAI JSON format)
pub struct OpenAICompatPlugin;

impl ProviderPlugin for OpenAICompatPlugin {
    fn id(&self) -> &str {
        "openai"
    }

    fn display_name(&self) -> &str {
        "OpenAI Compatible"
    }

    fn config_schema(&self) -> ProviderConfigSchema {
        ProviderConfigSchema {
            provider_fields: vec![],
            account_fields: vec![ConfigField {
                name: "api_key".to_string(),
                label: "API Key".to_string(),
                field_type: ConfigFieldType::Secret,
                required: true,
                default: None,
                options: vec![],
                description: "API key for authentication (sent as Bearer token)".to_string(),
            }],
        }
    }

    fn validate_config(
        &self,
        _provider_extra: &HashMap<String, serde_json::Value>,
        api_key: &Option<String>,
        _account_headers: &HashMap<String, String>,
    ) -> Result<(), String> {
        // API key is recommended but not strictly required
        // (some internal providers use no auth)
        if let Some(key) = api_key {
            if key.is_empty() {
                return Err("api_key is empty".to_string());
            }
        }
        Ok(())
    }

    // build_auth: uses default (Bearer token) — inherited from trait default
    // rewrite_url: uses default (pass-through) — inherited from trait default
    // transform_*: uses defaults (pass-through) — inherited from trait defaults
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> ProviderRequestContext {
        ProviderRequestContext {
            provider_id: "deepseek".to_string(),
            provider_type: "openai".to_string(),
            base_url: "https://api.deepseek.com/v1".to_string(),
            provider_extra: HashMap::new(),
            account_id: "deepseek-main".to_string(),
            account_api_key: Some("sk-test-key-123".to_string()),
            account_headers: HashMap::new(),
            variant_id: "deepseek-v3@deepseek".to_string(),
            model_id: "deepseek-v3".to_string(),
            upstream_model_name: Some("deepseek-chat".to_string()),
            deployment_name: None,
            original_path: "/v1/chat/completions".to_string(),
            method: "POST".to_string(),
            is_streaming: false,
        }
    }

    #[test]
    fn test_id() {
        let plugin = OpenAICompatPlugin;
        assert_eq!(plugin.id(), "openai");
        assert_eq!(plugin.display_name(), "OpenAI Compatible");
    }

    #[test]
    fn test_build_auth_bearer_token() {
        let plugin = OpenAICompatPlugin;
        let ctx = make_ctx();
        let result = plugin.build_auth(&ctx).unwrap();
        assert_eq!(result.headers.len(), 1);
        assert_eq!(result.headers[0].0, "Authorization");
        assert_eq!(result.headers[0].1, "Bearer sk-test-key-123");
    }

    #[test]
    fn test_build_auth_no_key() {
        let plugin = OpenAICompatPlugin;
        let mut ctx = make_ctx();
        ctx.account_api_key = None;
        let result = plugin.build_auth(&ctx).unwrap();
        assert!(result.headers.is_empty());
    }

    #[test]
    fn test_build_auth_empty_key() {
        let plugin = OpenAICompatPlugin;
        let mut ctx = make_ctx();
        ctx.account_api_key = Some(String::new());
        let result = plugin.build_auth(&ctx).unwrap();
        assert!(result.headers.is_empty());
    }

    #[test]
    fn test_rewrite_url_passthrough() {
        let plugin = OpenAICompatPlugin;
        let ctx = make_ctx();
        let result = plugin.rewrite_url(&ctx).unwrap();
        assert_eq!(result.path, "/v1/chat/completions");
        assert!(result.host_override.is_none());
    }

    #[test]
    fn test_transform_request_body_passthrough() {
        let plugin = OpenAICompatPlugin;
        let ctx = make_ctx();
        let body = br#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#.to_vec();
        let result = plugin.transform_request_body(body.clone(), &ctx).unwrap();
        assert_eq!(result, body);
    }

    #[test]
    fn test_transform_response_body_passthrough() {
        let plugin = OpenAICompatPlugin;
        let ctx = make_ctx();
        let body = br#"{"id":"chatcmpl-123","choices":[{"message":{"content":"hello"}}]}"#.to_vec();
        let result = plugin.transform_response_body(body.clone(), &ctx).unwrap();
        assert_eq!(result, body);
    }

    #[test]
    fn test_transform_stream_chunk_passthrough() {
        let plugin = OpenAICompatPlugin;
        let ctx = make_ctx();
        let chunk = r#"data: {"id":"chatcmpl-123","choices":[{"delta":{"content":"hi"}}]}"#;
        let result = plugin.transform_stream_chunk(chunk, &ctx).unwrap();
        assert_eq!(result, Some(chunk.to_string()));
    }

    #[test]
    fn test_config_schema() {
        let plugin = OpenAICompatPlugin;
        let schema = plugin.config_schema();
        assert!(schema.provider_fields.is_empty());
        assert_eq!(schema.account_fields.len(), 1);
        assert_eq!(schema.account_fields[0].name, "api_key");
    }

    #[test]
    fn test_validate_config_ok() {
        let plugin = OpenAICompatPlugin;
        assert!(plugin
            .validate_config(
                &HashMap::new(),
                &Some("sk-123".to_string()),
                &HashMap::new()
            )
            .is_ok());
    }

    #[test]
    fn test_validate_config_no_key_ok() {
        let plugin = OpenAICompatPlugin;
        // No key is allowed (some internal providers)
        assert!(plugin
            .validate_config(&HashMap::new(), &None, &HashMap::new())
            .is_ok());
    }

    #[test]
    fn test_validate_config_empty_key_err() {
        let plugin = OpenAICompatPlugin;
        assert!(plugin
            .validate_config(&HashMap::new(), &Some(String::new()), &HashMap::new())
            .is_err());
    }
}
