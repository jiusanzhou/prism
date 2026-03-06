//! Provider plugin registry.
//!
//! Maps `provider_type` strings to [`ProviderPlugin`] implementations.
//! The proxy layer looks up the appropriate plugin per-request based on
//! the routed provider's type.

use std::collections::HashMap;
use std::sync::Arc;

use super::{ProviderConfigSchema, ProviderPlugin};
use super::openai::OpenAICompatPlugin;

/// Registry of provider plugins, keyed by provider type.
pub struct ProviderPluginRegistry {
    plugins: HashMap<String, Arc<dyn ProviderPlugin>>,
    /// Fallback plugin used when provider_type is unknown or empty.
    fallback: Arc<dyn ProviderPlugin>,
}

impl ProviderPluginRegistry {
    /// Create a new registry with the built-in OpenAI-compatible plugin.
    pub fn new() -> Self {
        let openai: Arc<dyn ProviderPlugin> = Arc::new(OpenAICompatPlugin);
        let mut plugins = HashMap::new();
        plugins.insert("openai".to_string(), openai.clone());
        Self {
            plugins,
            fallback: openai,
        }
    }

    /// Register a provider plugin.
    /// Replaces any existing plugin for the same provider type.
    pub fn register(&mut self, plugin: Arc<dyn ProviderPlugin>) {
        let id = plugin.id().to_string();
        self.plugins.insert(id, plugin);
    }

    /// Look up a provider plugin by type.
    /// Returns the fallback (OpenAI compat) if not found.
    pub fn get(&self, provider_type: &str) -> &dyn ProviderPlugin {
        self.plugins
            .get(provider_type)
            .map(|p| p.as_ref())
            .unwrap_or(self.fallback.as_ref())
    }

    /// Check if a specific provider type has a registered plugin.
    pub fn has(&self, provider_type: &str) -> bool {
        self.plugins.contains_key(provider_type)
    }

    /// List all registered provider type IDs.
    pub fn provider_types(&self) -> Vec<&str> {
        self.plugins.keys().map(|k| k.as_str()).collect()
    }

    /// Get config schemas for all registered provider types.
    /// Used by Admin API to expose dynamic form definitions.
    pub fn all_schemas(&self) -> HashMap<String, ProviderConfigSchema> {
        self.plugins
            .iter()
            .map(|(id, plugin)| (id.clone(), plugin.config_schema()))
            .collect()
    }
}

impl Default for ProviderPluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{AuthResult, ProviderRequestContext, UrlRewriteResult};

    /// A mock provider plugin for testing registration.
    struct MockProvider {
        id: String,
        auth_header: String,
    }

    impl MockProvider {
        fn new(id: &str, auth_header: &str) -> Self {
            Self {
                id: id.to_string(),
                auth_header: auth_header.to_string(),
            }
        }
    }

    impl ProviderPlugin for MockProvider {
        fn id(&self) -> &str {
            &self.id
        }

        fn display_name(&self) -> &str {
            "Mock Provider"
        }

        fn build_auth(&self, ctx: &ProviderRequestContext) -> Result<AuthResult, String> {
            let mut result = AuthResult::default();
            if let Some(ref key) = ctx.account_api_key {
                result
                    .headers
                    .push((self.auth_header.clone(), key.clone()));
            }
            Ok(result)
        }
    }

    fn make_ctx(provider_type: &str) -> ProviderRequestContext {
        ProviderRequestContext {
            provider_id: "test".to_string(),
            provider_type: provider_type.to_string(),
            base_url: "https://api.test.com".to_string(),
            provider_extra: std::collections::HashMap::new(),
            account_id: "acct1".to_string(),
            account_api_key: Some("test-key".to_string()),
            account_headers: std::collections::HashMap::new(),
            variant_id: "v1".to_string(),
            model_id: "model1".to_string(),
            upstream_model_name: None,
            deployment_name: None,
            original_path: "/v1/chat/completions".to_string(),
            method: "POST".to_string(),
            is_streaming: false,
        }
    }

    #[test]
    fn test_new_has_openai() {
        let registry = ProviderPluginRegistry::new();
        assert!(registry.has("openai"));
        assert_eq!(registry.get("openai").id(), "openai");
    }

    #[test]
    fn test_fallback_to_openai() {
        let registry = ProviderPluginRegistry::new();
        // Unknown type falls back to openai
        let plugin = registry.get("unknown-type");
        assert_eq!(plugin.id(), "openai");

        // Empty string also falls back
        let plugin = registry.get("");
        assert_eq!(plugin.id(), "openai");
    }

    #[test]
    fn test_register_custom_plugin() {
        let mut registry = ProviderPluginRegistry::new();
        registry.register(Arc::new(MockProvider::new("azure", "api-key")));

        assert!(registry.has("azure"));
        let plugin = registry.get("azure");
        assert_eq!(plugin.id(), "azure");

        // Build auth uses custom header
        let ctx = make_ctx("azure");
        let auth = plugin.build_auth(&ctx).unwrap();
        assert_eq!(auth.headers.len(), 1);
        assert_eq!(auth.headers[0].0, "api-key");
        assert_eq!(auth.headers[0].1, "test-key");
    }

    #[test]
    fn test_register_replaces_existing() {
        let mut registry = ProviderPluginRegistry::new();
        registry.register(Arc::new(MockProvider::new("openai", "X-Custom-Auth")));

        // Should replace the built-in openai plugin
        let plugin = registry.get("openai");
        let ctx = make_ctx("openai");
        let auth = plugin.build_auth(&ctx).unwrap();
        assert_eq!(auth.headers[0].0, "X-Custom-Auth");
    }

    #[test]
    fn test_provider_types() {
        let mut registry = ProviderPluginRegistry::new();
        registry.register(Arc::new(MockProvider::new("azure", "api-key")));
        registry.register(Arc::new(MockProvider::new("bedrock", "x-amz")));

        let mut types = registry.provider_types();
        types.sort();
        assert_eq!(types, vec!["azure", "bedrock", "openai"]);
    }

    #[test]
    fn test_all_schemas() {
        let registry = ProviderPluginRegistry::new();
        let schemas = registry.all_schemas();
        assert!(schemas.contains_key("openai"));
        // OpenAI schema has no provider fields, one account field
        let schema = &schemas["openai"];
        assert!(schema.provider_fields.is_empty());
        assert_eq!(schema.account_fields.len(), 1);
    }
}
