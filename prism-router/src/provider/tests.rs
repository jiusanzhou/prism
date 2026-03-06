//! Integration tests for the provider plugin framework.

use super::*;
use super::openai::OpenAICompatPlugin;
use super::registry::ProviderPluginRegistry;
use std::collections::HashMap;
use std::sync::Arc;

// ── ProviderRequestContext builder ──

fn base_ctx() -> ProviderRequestContext {
    ProviderRequestContext {
        provider_id: "test-provider".to_string(),
        provider_type: "openai".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        provider_extra: HashMap::new(),
        account_id: "test-account".to_string(),
        account_api_key: Some("sk-test-key".to_string()),
        account_headers: HashMap::new(),
        variant_id: "gpt-4o@openai".to_string(),
        model_id: "gpt-4o".to_string(),
        upstream_model_name: None,
        deployment_name: None,
        original_path: "/v1/chat/completions".to_string(),
        method: "POST".to_string(),
        is_streaming: false,
    }
}

// ── Trait object safety ──

#[test]
fn test_provider_plugin_is_object_safe() {
    // Verify ProviderPlugin can be used as trait object
    let plugin: Box<dyn ProviderPlugin> = Box::new(OpenAICompatPlugin);
    assert_eq!(plugin.id(), "openai");

    let plugin_arc: Arc<dyn ProviderPlugin> = Arc::new(OpenAICompatPlugin);
    assert_eq!(plugin_arc.id(), "openai");
}

// ── Default trait implementations ──

/// A minimal plugin that only implements id() — all other methods use defaults.
struct MinimalPlugin;

impl ProviderPlugin for MinimalPlugin {
    fn id(&self) -> &str {
        "minimal"
    }
}

#[test]
fn test_default_display_name_is_id() {
    let plugin = MinimalPlugin;
    assert_eq!(plugin.display_name(), "minimal");
}

#[test]
fn test_default_config_schema_empty() {
    let plugin = MinimalPlugin;
    let schema = plugin.config_schema();
    assert!(schema.provider_fields.is_empty());
    assert!(schema.account_fields.is_empty());
}

#[test]
fn test_default_validate_config_ok() {
    let plugin = MinimalPlugin;
    assert!(plugin
        .validate_config(&HashMap::new(), &None, &HashMap::new())
        .is_ok());
}

#[test]
fn test_default_build_auth_bearer() {
    let plugin = MinimalPlugin;
    let ctx = base_ctx();
    let auth = plugin.build_auth(&ctx).unwrap();
    assert_eq!(auth.headers.len(), 1);
    assert_eq!(auth.headers[0].0, "Authorization");
    assert_eq!(auth.headers[0].1, "Bearer sk-test-key");
}

#[test]
fn test_default_rewrite_url_passthrough() {
    let plugin = MinimalPlugin;
    let ctx = base_ctx();
    let url = plugin.rewrite_url(&ctx).unwrap();
    assert_eq!(url.path, "/v1/chat/completions");
    assert!(url.host_override.is_none());
}

#[test]
fn test_default_transform_body_passthrough() {
    let plugin = MinimalPlugin;
    let ctx = base_ctx();
    let body = b"test body".to_vec();
    assert_eq!(
        plugin.transform_request_body(body.clone(), &ctx).unwrap(),
        body
    );
    assert_eq!(
        plugin.transform_response_body(body.clone(), &ctx).unwrap(),
        body
    );
}

#[test]
fn test_default_transform_stream_passthrough() {
    let plugin = MinimalPlugin;
    let ctx = base_ctx();
    let chunk = "data: {\"test\": true}\n\n";
    let result = plugin.transform_stream_chunk(chunk, &ctx).unwrap();
    assert_eq!(result, Some(chunk.to_string()));
}

// ── Full pipeline simulation ──

#[test]
fn test_openai_full_pipeline() {
    let registry = ProviderPluginRegistry::new();
    let plugin = registry.get("openai");

    let ctx = base_ctx();

    // 1. Build auth
    let auth = plugin.build_auth(&ctx).unwrap();
    assert_eq!(auth.headers[0].1, "Bearer sk-test-key");

    // 2. Rewrite URL
    let url = plugin.rewrite_url(&ctx).unwrap();
    assert_eq!(url.path, "/v1/chat/completions");

    // 3. Transform request body
    let req_body = br#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#.to_vec();
    let transformed = plugin.transform_request_body(req_body.clone(), &ctx).unwrap();
    assert_eq!(transformed, req_body); // pass-through

    // 4. Transform response body
    let resp_body = br#"{"id":"1","choices":[{"message":{"content":"hello"}}]}"#.to_vec();
    let transformed = plugin.transform_response_body(resp_body.clone(), &ctx).unwrap();
    assert_eq!(transformed, resp_body); // pass-through

    // 5. Transform stream chunk
    let chunk = r#"data: {"id":"1","choices":[{"delta":{"content":"h"}}]}"#;
    let result = plugin.transform_stream_chunk(chunk, &ctx).unwrap();
    assert_eq!(result, Some(chunk.to_string()));
}

// ── Registry + plugin dispatch simulation ──

/// Simulates how proxy.rs will dispatch to the right provider plugin.
#[test]
fn test_dispatch_by_provider_type() {
    struct CustomAuthPlugin;

    impl ProviderPlugin for CustomAuthPlugin {
        fn id(&self) -> &str {
            "custom-auth"
        }

        fn build_auth(&self, ctx: &ProviderRequestContext) -> Result<AuthResult, String> {
            let mut result = AuthResult::default();
            if let Some(ref key) = ctx.account_api_key {
                result
                    .headers
                    .push(("x-api-key".to_string(), key.clone()));
                result
                    .headers
                    .push(("x-provider".to_string(), ctx.provider_id.clone()));
            }
            Ok(result)
        }
    }

    let mut registry = ProviderPluginRegistry::new();
    registry.register(Arc::new(CustomAuthPlugin));

    // Simulate routing decision → provider_type dispatch
    let scenarios = vec![
        ("openai", "Authorization", "Bearer sk-key"),
        ("custom-auth", "x-api-key", "sk-key"),
    ];

    for (ptype, expected_header, expected_value) in scenarios {
        let plugin = registry.get(ptype);
        let mut ctx = base_ctx();
        ctx.provider_type = ptype.to_string();
        ctx.account_api_key = Some("sk-key".to_string());

        let auth = plugin.build_auth(&ctx).unwrap();
        assert!(
            !auth.headers.is_empty(),
            "no auth headers for {}",
            ptype
        );
        assert_eq!(auth.headers[0].0, expected_header, "wrong header for {}", ptype);
        assert_eq!(auth.headers[0].1, expected_value, "wrong value for {}", ptype);
    }
}

// ── AuthResult / UrlRewriteResult ──

#[test]
fn test_auth_result_default_empty() {
    let result = AuthResult::default();
    assert!(result.headers.is_empty());
}

#[test]
fn test_url_rewrite_result_clone() {
    let result = UrlRewriteResult {
        path: "/v1/test".to_string(),
        host_override: Some("new-host.com".to_string()),
    };
    let cloned = result.clone();
    assert_eq!(cloned.path, "/v1/test");
    assert_eq!(cloned.host_override, Some("new-host.com".to_string()));
}

// ── ConfigField serialization ──

#[test]
fn test_config_field_type_serde() {
    let field = ConfigField {
        name: "aws_region".to_string(),
        label: "AWS Region".to_string(),
        field_type: ConfigFieldType::Select,
        required: true,
        default: Some("us-east-1".to_string()),
        options: vec![
            "us-east-1".to_string(),
            "us-west-2".to_string(),
            "eu-west-1".to_string(),
        ],
        description: "AWS region for Bedrock API".to_string(),
    };

    let json = serde_json::to_string(&field).unwrap();
    assert!(json.contains("\"field_type\":\"select\""));
    assert!(json.contains("\"us-east-1\""));

    let deserialized: ConfigField = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.name, "aws_region");
    assert_eq!(deserialized.options.len(), 3);
}

#[test]
fn test_provider_config_schema_serde() {
    let schema = ProviderConfigSchema {
        provider_fields: vec![ConfigField {
            name: "base_url".to_string(),
            label: "Base URL".to_string(),
            field_type: ConfigFieldType::String,
            required: true,
            default: None,
            options: vec![],
            description: "Provider API base URL".to_string(),
        }],
        account_fields: vec![ConfigField {
            name: "api_key".to_string(),
            label: "API Key".to_string(),
            field_type: ConfigFieldType::Secret,
            required: true,
            default: None,
            options: vec![],
            description: "API key".to_string(),
        }],
    };

    let json = serde_json::to_string(&schema).unwrap();
    let deserialized: ProviderConfigSchema = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.provider_fields.len(), 1);
    assert_eq!(deserialized.account_fields.len(), 1);
}
