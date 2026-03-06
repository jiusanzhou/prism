//! Integration tests for the provider plugin framework.

use super::*;
use super::openai::OpenAICompatPlugin;
use super::registry::ProviderPluginRegistry;
use std::collections::HashMap;
use std::sync::Arc;

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

#[test]
fn test_provider_plugin_is_object_safe() {
    let _: Box<dyn ProviderPlugin> = Box::new(OpenAICompatPlugin);
    let _: Arc<dyn ProviderPlugin> = Arc::new(OpenAICompatPlugin);
}

struct MinimalPlugin;
impl ProviderPlugin for MinimalPlugin {
    fn id(&self) -> &str { "minimal" }
}

#[test]
fn test_default_display_name() { assert_eq!(MinimalPlugin.display_name(), "minimal"); }

#[test]
fn test_default_config_schema() {
    let s = MinimalPlugin.config_schema();
    assert!(s.provider_fields.is_empty());
    assert!(s.account_fields.is_empty());
}

#[test]
fn test_default_validate() {
    assert!(MinimalPlugin.validate_config(&HashMap::new(), &None, &HashMap::new()).is_ok());
}

#[test]
fn test_default_build_auth_bearer() {
    let auth = MinimalPlugin.build_auth(&base_ctx()).unwrap();
    assert_eq!(auth.headers, vec![("Authorization".into(), "Bearer sk-test-key".into())]);
}

#[test]
fn test_default_rewrite_url() {
    let url = MinimalPlugin.rewrite_url(&base_ctx()).unwrap();
    assert_eq!(url.path, "/v1/chat/completions");
    assert!(url.host_override.is_none());
}

#[test]
fn test_default_transform_body() {
    let b = b"test".to_vec();
    let ctx = base_ctx();
    assert_eq!(MinimalPlugin.transform_request_body(b.clone(), &ctx).unwrap(), b);
    assert_eq!(MinimalPlugin.transform_response_body(b.clone(), &ctx).unwrap(), b);
}

#[test]
fn test_default_transform_stream() {
    let r = MinimalPlugin.transform_stream_chunk("data: x", &base_ctx()).unwrap();
    assert_eq!(r, Some("data: x".into()));
}

#[test]
fn test_openai_full_pipeline() {
    let reg = ProviderPluginRegistry::new();
    let p = reg.get("openai");
    let ctx = base_ctx();
    assert_eq!(p.build_auth(&ctx).unwrap().headers[0].1, "Bearer sk-test-key");
    assert_eq!(p.rewrite_url(&ctx).unwrap().path, "/v1/chat/completions");
    let body = br#"{"model":"gpt-4o"}"#.to_vec();
    assert_eq!(p.transform_request_body(body.clone(), &ctx).unwrap(), body);
}

#[test]
fn test_dispatch_by_provider_type() {
    struct XApiKeyPlugin;
    impl ProviderPlugin for XApiKeyPlugin {
        fn id(&self) -> &str { "x-api" }
        fn build_auth(&self, ctx: &ProviderRequestContext) -> Result<AuthResult, String> {
            let mut r = AuthResult::default();
            if let Some(ref k) = ctx.account_api_key {
                r.headers.push(("x-api-key".into(), k.clone()));
            }
            Ok(r)
        }
    }
    let mut reg = ProviderPluginRegistry::new();
    reg.register(Arc::new(XApiKeyPlugin));
    assert_eq!(reg.get("x-api").build_auth(&base_ctx()).unwrap().headers[0].0, "x-api-key");
    assert_eq!(reg.get("openai").build_auth(&base_ctx()).unwrap().headers[0].0, "Authorization");
}

#[test]
fn test_auth_result_default() { assert!(AuthResult::default().headers.is_empty()); }

#[test]
fn test_url_rewrite_clone() {
    let r = UrlRewriteResult { path: "/x".into(), host_override: Some("h".into()) };
    assert_eq!(r.clone().path, "/x");
}

#[test]
fn test_config_field_serde() {
    let f = ConfigField {
        name: "r".into(), label: "R".into(), field_type: ConfigFieldType::Select,
        required: true, default: None, options: vec!["a".into()], description: "d".into(),
    };
    let j = serde_json::to_string(&f).unwrap();
    assert!(j.contains("\"select\""));
    let _: ConfigField = serde_json::from_str(&j).unwrap();
}

#[test]
fn test_provider_request_context_serde() {
    let ctx = base_ctx();
    let j = serde_json::to_string(&ctx).unwrap();
    let d: ProviderRequestContext = serde_json::from_str(&j).unwrap();
    assert_eq!(d.provider_id, "test-provider");
}

// ── WASM provider plugin tests ──

#[cfg(feature = "wasm")]
mod wasm_tests {
    use super::*;
    use crate::provider::wasm_host::WasmProviderPlugin;

    fn azure_wasm() -> Vec<u8> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../examples/wasm-plugins/azure-provider/target/wasm32-unknown-unknown/release/prism_azure_provider.wasm"
        );
        std::fs::read(path).expect("Azure WASM not built")
    }

    fn azure_ctx() -> ProviderRequestContext {
        let mut extra = HashMap::new();
        extra.insert("api_version".into(), serde_json::json!("2025-04-01-preview"));
        ProviderRequestContext {
            provider_id: "azure-eastus2".into(),
            provider_type: "azure".into(),
            base_url: "https://my-openai.openai.azure.com".into(),
            provider_extra: extra,
            account_id: "azure-main".into(),
            account_api_key: Some("7WWBiG5rgcNyxhds".into()),
            account_headers: HashMap::new(),
            variant_id: "gpt-5@azure".into(),
            model_id: "gpt-5".into(),
            upstream_model_name: None,
            deployment_name: Some("my-gpt-5-deploy".into()),
            original_path: "/v1/chat/completions".into(),
            method: "POST".into(),
            is_streaming: false,
        }
    }

    #[test]
    fn test_wasm_azure_load() {
        let p = WasmProviderPlugin::new("Azure OpenAI", &azure_wasm()).unwrap();
        assert_eq!(p.id(), "azure");
    }

    #[test]
    fn test_wasm_azure_schema() {
        let p = WasmProviderPlugin::new("Azure", &azure_wasm()).unwrap();
        let s = p.config_schema();
        assert_eq!(s.provider_fields.len(), 1);
        assert_eq!(s.provider_fields[0].name, "api_version");
        assert_eq!(s.account_fields.len(), 1);
        assert_eq!(s.account_fields[0].name, "api_key");
    }

    #[test]
    fn test_wasm_azure_auth() {
        let p = WasmProviderPlugin::new("Azure", &azure_wasm()).unwrap();
        let auth = p.build_auth(&azure_ctx()).unwrap();
        assert_eq!(auth.headers.len(), 1);
        assert_eq!(auth.headers[0].0, "api-key");
        assert_eq!(auth.headers[0].1, "7WWBiG5rgcNyxhds");
    }

    #[test]
    fn test_wasm_azure_url_chat() {
        let p = WasmProviderPlugin::new("Azure", &azure_wasm()).unwrap();
        let url = p.rewrite_url(&azure_ctx()).unwrap();
        assert_eq!(url.path, "/openai/deployments/my-gpt-5-deploy/chat/completions?api-version=2025-04-01-preview");
    }

    #[test]
    fn test_wasm_azure_url_embeddings() {
        let p = WasmProviderPlugin::new("Azure", &azure_wasm()).unwrap();
        let mut ctx = azure_ctx();
        ctx.original_path = "/v1/embeddings".into();
        ctx.deployment_name = Some("ada-002".into());
        let url = p.rewrite_url(&ctx).unwrap();
        assert_eq!(url.path, "/openai/deployments/ada-002/embeddings?api-version=2025-04-01-preview");
    }

    #[test]
    fn test_wasm_azure_url_fallback_model_id() {
        let p = WasmProviderPlugin::new("Azure", &azure_wasm()).unwrap();
        let mut ctx = azure_ctx();
        ctx.deployment_name = None;
        ctx.upstream_model_name = None;
        let url = p.rewrite_url(&ctx).unwrap();
        assert!(url.path.contains("/deployments/gpt-5/"));
    }

    #[test]
    fn test_wasm_azure_body_passthrough() {
        let p = WasmProviderPlugin::new("Azure", &azure_wasm()).unwrap();
        let body = br#"{"model":"gpt-5"}"#.to_vec();
        let result = p.transform_request_body(body.clone(), &azure_ctx()).unwrap();
        assert_eq!(result, body);
    }

    #[test]
    fn test_wasm_azure_registry_dispatch() {
        let p = WasmProviderPlugin::new("Azure OpenAI", &azure_wasm()).unwrap();
        let mut reg = ProviderPluginRegistry::new();
        reg.register(Arc::new(p));

        let az = reg.get("azure");
        assert_eq!(az.id(), "azure");
        let auth = az.build_auth(&azure_ctx()).unwrap();
        assert_eq!(auth.headers[0].0, "api-key");

        let oai = reg.get("openai");
        assert_eq!(oai.id(), "openai");
        let auth = oai.build_auth(&base_ctx()).unwrap();
        assert_eq!(auth.headers[0].0, "Authorization");
    }
}
