//! Built-in Google Vertex AI provider plugin.
//!
//! Handles Vertex-specific concerns:
//! - OAuth2 authentication with GCP Service Account (JWT → access token exchange)
//! - URL rewriting: `/v1/chat/completions` → Vertex OpenAI-compat endpoint
//! - Body: pass-through (Vertex AI supports OpenAI-compatible format)

use super::{
    AuthResult, ConfigField, ConfigFieldType, ProviderConfigSchema, ProviderPlugin,
    ProviderRequestContext, UrlRewriteResult,
};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// Cached OAuth2 access token with expiry.
struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

/// Built-in Google Vertex AI provider plugin.
///
/// - Auth: OAuth2 via GCP Service Account JWT exchange
/// - URL: `/v1/chat/completions` → Vertex OpenAI-compat endpoint
/// - Body: pass-through (Vertex supports OpenAI format)
pub struct VertexPlugin {
    token_cache: Mutex<Option<CachedToken>>,
}

impl VertexPlugin {
    pub fn new() -> Self {
        Self {
            token_cache: Mutex::new(None),
        }
    }
}

impl Default for VertexPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderPlugin for VertexPlugin {
    fn id(&self) -> &str {
        "vertex"
    }

    fn display_name(&self) -> &str {
        "Google Vertex AI"
    }

    fn config_schema(&self) -> ProviderConfigSchema {
        ProviderConfigSchema {
            provider_fields: vec![
                ConfigField {
                    name: "project_id".to_string(),
                    label: "GCP Project ID".to_string(),
                    field_type: ConfigFieldType::String,
                    required: true,
                    default: None,
                    options: vec![],
                    description: "Google Cloud project ID".to_string(),
                },
                ConfigField {
                    name: "region".to_string(),
                    label: "Region".to_string(),
                    field_type: ConfigFieldType::String,
                    required: true,
                    default: Some("us-central1".to_string()),
                    options: vec![],
                    description: "GCP region for the Vertex AI endpoint".to_string(),
                },
                ConfigField {
                    name: "service_account_json".to_string(),
                    label: "Service Account JSON".to_string(),
                    field_type: ConfigFieldType::Secret,
                    required: true,
                    default: None,
                    options: vec![],
                    description: "GCP service account key JSON (full contents)".to_string(),
                },
            ],
            account_fields: vec![],
        }
    }

    fn validate_config(
        &self,
        provider_extra: &HashMap<String, serde_json::Value>,
        _api_key: &Option<String>,
        _account_headers: &HashMap<String, String>,
    ) -> Result<(), String> {
        let project_id = provider_extra
            .get("project_id")
            .and_then(|v| v.as_str());
        let region = provider_extra.get("region").and_then(|v| v.as_str());
        let sa_json = provider_extra
            .get("service_account_json")
            .and_then(|v| v.as_str());

        if project_id.unwrap_or("").is_empty() {
            return Err("project_id is required".to_string());
        }
        if region.unwrap_or("").is_empty() {
            return Err("region is required".to_string());
        }
        if sa_json.unwrap_or("").is_empty() {
            return Err("service_account_json is required".to_string());
        }

        // Validate that service_account_json is valid JSON with required fields
        if let Some(json_str) = sa_json {
            let parsed: serde_json::Value = serde_json::from_str(json_str)
                .map_err(|e| format!("service_account_json is not valid JSON: {}", e))?;
            if parsed.get("client_email").and_then(|v| v.as_str()).is_none() {
                return Err("service_account_json missing 'client_email' field".to_string());
            }
            if parsed.get("private_key").and_then(|v| v.as_str()).is_none() {
                return Err("service_account_json missing 'private_key' field".to_string());
            }
        }

        Ok(())
    }

    fn build_auth(&self, ctx: &ProviderRequestContext) -> Result<AuthResult, String> {
        let sa_json_str = ctx
            .provider_extra
            .get("service_account_json")
            .and_then(|v| v.as_str())
            .ok_or("service_account_json not found in provider config")?;

        // Check cache first
        {
            let cache = self.token_cache.lock().map_err(|e| format!("lock error: {}", e))?;
            if let Some(ref cached) = *cache {
                // Refresh 60s before expiry
                if cached.expires_at > Instant::now() + std::time::Duration::from_secs(60) {
                    return Ok(AuthResult {
                        headers: vec![(
                            "Authorization".to_string(),
                            format!("Bearer {}", cached.access_token),
                        )],
                    });
                }
            }
        }

        // Parse service account JSON
        let sa: serde_json::Value = serde_json::from_str(sa_json_str)
            .map_err(|e| format!("invalid service_account_json: {}", e))?;

        let client_email = sa
            .get("client_email")
            .and_then(|v| v.as_str())
            .ok_or("missing client_email in service account")?;
        let private_key = sa
            .get("private_key")
            .and_then(|v| v.as_str())
            .ok_or("missing private_key in service account")?;
        let token_uri = sa
            .get("token_uri")
            .and_then(|v| v.as_str())
            .unwrap_or("https://oauth2.googleapis.com/token");

        // Build JWT
        let now = chrono::Utc::now().timestamp();
        let claims = serde_json::json!({
            "iss": client_email,
            "scope": "https://www.googleapis.com/auth/cloud-platform",
            "aud": token_uri,
            "iat": now,
            "exp": now + 3600,
        });

        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key.as_bytes())
            .map_err(|e| format!("invalid RSA private key: {}", e))?;

        let jwt = jsonwebtoken::encode(&header, &claims, &encoding_key)
            .map_err(|e| format!("JWT encoding failed: {}", e))?;

        // Exchange JWT for access token (synchronous HTTP using ureq)
        let form_body = format!(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={}",
            jwt
        );

        let response = ureq::post(token_uri)
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_string(&form_body)
            .map_err(|e| format!("token exchange request failed: {}", e))?;

        let token_response: serde_json::Value = response
            .into_json()
            .map_err(|e| format!("token exchange response parse failed: {}", e))?;

        let access_token = token_response
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or("no access_token in token response")?
            .to_string();

        let expires_in = token_response
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(3600);

        // Cache the token
        {
            let mut cache = self.token_cache.lock().map_err(|e| format!("lock error: {}", e))?;
            *cache = Some(CachedToken {
                access_token: access_token.clone(),
                expires_at: Instant::now() + std::time::Duration::from_secs(expires_in),
            });
        }

        Ok(AuthResult {
            headers: vec![(
                "Authorization".to_string(),
                format!("Bearer {}", access_token),
            )],
        })
    }

    fn rewrite_url(&self, ctx: &ProviderRequestContext) -> Result<UrlRewriteResult, String> {
        let region = ctx
            .provider_extra
            .get("region")
            .and_then(|v| v.as_str())
            .unwrap_or("us-central1");

        let project_id = ctx
            .provider_extra
            .get("project_id")
            .and_then(|v| v.as_str())
            .ok_or("project_id not found in provider config")?;

        let host = format!("{}-aiplatform.googleapis.com", region);

        // Use the OpenAI-compatible endpoint for pass-through
        let path = format!(
            "/v1beta1/projects/{}/locations/{}/endpoints/openapi{}",
            project_id, region, ctx.original_path
        );

        Ok(UrlRewriteResult {
            path,
            host_override: Some(host),
        })
    }

    // transform_request_body: pass-through (Vertex OpenAI-compat) — inherited
    // transform_response_body: pass-through — inherited
    // transform_stream_chunk: pass-through — inherited
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> ProviderRequestContext {
        let mut extra = HashMap::new();
        extra.insert("project_id".to_string(), serde_json::json!("my-gcp-project"));
        extra.insert("region".to_string(), serde_json::json!("us-central1"));
        extra.insert(
            "service_account_json".to_string(),
            serde_json::json!(r#"{"client_email":"sa@proj.iam.gserviceaccount.com","private_key":"-----BEGIN RSA PRIVATE KEY-----\nMIIE...fake\n-----END RSA PRIVATE KEY-----\n","token_uri":"https://oauth2.googleapis.com/token"}"#),
        );

        ProviderRequestContext {
            provider_id: "vertex-us".to_string(),
            provider_type: "vertex".to_string(),
            base_url: "https://us-central1-aiplatform.googleapis.com".to_string(),
            provider_extra: extra,
            account_id: "vertex-acct".to_string(),
            account_api_key: None,
            account_headers: HashMap::new(),
            variant_id: "gemini-pro@vertex".to_string(),
            model_id: "gemini-pro".to_string(),
            upstream_model_name: Some("gemini-pro".to_string()),
            deployment_name: None,
            original_path: "/v1/chat/completions".to_string(),
            method: "POST".to_string(),
            is_streaming: false,
        }
    }

    #[test]
    fn test_id() {
        let plugin = VertexPlugin::new();
        assert_eq!(plugin.id(), "vertex");
        assert_eq!(plugin.display_name(), "Google Vertex AI");
    }

    #[test]
    fn test_config_schema() {
        let plugin = VertexPlugin::new();
        let schema = plugin.config_schema();
        assert_eq!(schema.provider_fields.len(), 3);
        assert_eq!(schema.provider_fields[0].name, "project_id");
        assert_eq!(schema.provider_fields[1].name, "region");
        assert_eq!(schema.provider_fields[2].name, "service_account_json");
        assert!(schema.account_fields.is_empty());
    }

    #[test]
    fn test_validate_config_ok() {
        let plugin = VertexPlugin::new();
        let mut extra = HashMap::new();
        extra.insert("project_id".into(), serde_json::json!("my-project"));
        extra.insert("region".into(), serde_json::json!("us-central1"));
        extra.insert(
            "service_account_json".into(),
            serde_json::json!(r#"{"client_email":"sa@test.iam.gserviceaccount.com","private_key":"-----BEGIN RSA PRIVATE KEY-----\nfake\n-----END RSA PRIVATE KEY-----\n"}"#),
        );
        assert!(plugin
            .validate_config(&extra, &None, &HashMap::new())
            .is_ok());
    }

    #[test]
    fn test_validate_config_missing_project_id() {
        let plugin = VertexPlugin::new();
        let mut extra = HashMap::new();
        extra.insert("region".into(), serde_json::json!("us-central1"));
        extra.insert(
            "service_account_json".into(),
            serde_json::json!(r#"{"client_email":"a","private_key":"b"}"#),
        );
        let err = plugin
            .validate_config(&extra, &None, &HashMap::new())
            .unwrap_err();
        assert!(err.contains("project_id"));
    }

    #[test]
    fn test_validate_config_missing_region() {
        let plugin = VertexPlugin::new();
        let mut extra = HashMap::new();
        extra.insert("project_id".into(), serde_json::json!("my-project"));
        extra.insert(
            "service_account_json".into(),
            serde_json::json!(r#"{"client_email":"a","private_key":"b"}"#),
        );
        let err = plugin
            .validate_config(&extra, &None, &HashMap::new())
            .unwrap_err();
        assert!(err.contains("region"));
    }

    #[test]
    fn test_validate_config_missing_sa_json() {
        let plugin = VertexPlugin::new();
        let mut extra = HashMap::new();
        extra.insert("project_id".into(), serde_json::json!("my-project"));
        extra.insert("region".into(), serde_json::json!("us-central1"));
        let err = plugin
            .validate_config(&extra, &None, &HashMap::new())
            .unwrap_err();
        assert!(err.contains("service_account_json"));
    }

    #[test]
    fn test_validate_config_invalid_sa_json() {
        let plugin = VertexPlugin::new();
        let mut extra = HashMap::new();
        extra.insert("project_id".into(), serde_json::json!("my-project"));
        extra.insert("region".into(), serde_json::json!("us-central1"));
        extra.insert(
            "service_account_json".into(),
            serde_json::json!("not valid json"),
        );
        let err = plugin
            .validate_config(&extra, &None, &HashMap::new())
            .unwrap_err();
        assert!(err.contains("not valid JSON"));
    }

    #[test]
    fn test_validate_config_sa_json_missing_client_email() {
        let plugin = VertexPlugin::new();
        let mut extra = HashMap::new();
        extra.insert("project_id".into(), serde_json::json!("my-project"));
        extra.insert("region".into(), serde_json::json!("us-central1"));
        extra.insert(
            "service_account_json".into(),
            serde_json::json!(r#"{"private_key":"key"}"#),
        );
        let err = plugin
            .validate_config(&extra, &None, &HashMap::new())
            .unwrap_err();
        assert!(err.contains("client_email"));
    }

    #[test]
    fn test_validate_config_sa_json_missing_private_key() {
        let plugin = VertexPlugin::new();
        let mut extra = HashMap::new();
        extra.insert("project_id".into(), serde_json::json!("my-project"));
        extra.insert("region".into(), serde_json::json!("us-central1"));
        extra.insert(
            "service_account_json".into(),
            serde_json::json!(r#"{"client_email":"sa@test.iam.gserviceaccount.com"}"#),
        );
        let err = plugin
            .validate_config(&extra, &None, &HashMap::new())
            .unwrap_err();
        assert!(err.contains("private_key"));
    }

    #[test]
    fn test_rewrite_url_chat_completions() {
        let plugin = VertexPlugin::new();
        let ctx = make_ctx();
        let result = plugin.rewrite_url(&ctx).unwrap();

        assert_eq!(
            result.path,
            "/v1beta1/projects/my-gcp-project/locations/us-central1/endpoints/openapi/v1/chat/completions"
        );
        assert_eq!(
            result.host_override,
            Some("us-central1-aiplatform.googleapis.com".to_string())
        );
    }

    #[test]
    fn test_rewrite_url_embeddings() {
        let plugin = VertexPlugin::new();
        let mut ctx = make_ctx();
        ctx.original_path = "/v1/embeddings".to_string();
        let result = plugin.rewrite_url(&ctx).unwrap();

        assert_eq!(
            result.path,
            "/v1beta1/projects/my-gcp-project/locations/us-central1/endpoints/openapi/v1/embeddings"
        );
    }

    #[test]
    fn test_rewrite_url_custom_region() {
        let plugin = VertexPlugin::new();
        let mut ctx = make_ctx();
        ctx.provider_extra
            .insert("region".to_string(), serde_json::json!("europe-west1"));
        let result = plugin.rewrite_url(&ctx).unwrap();

        assert_eq!(
            result.host_override,
            Some("europe-west1-aiplatform.googleapis.com".to_string())
        );
        assert!(result.path.contains("europe-west1"));
    }

    #[test]
    fn test_rewrite_url_missing_project_id() {
        let plugin = VertexPlugin::new();
        let mut ctx = make_ctx();
        ctx.provider_extra.remove("project_id");
        assert!(plugin.rewrite_url(&ctx).is_err());
    }

    #[test]
    fn test_transform_request_body_passthrough() {
        let plugin = VertexPlugin::new();
        let ctx = make_ctx();
        let body = br#"{"model":"gemini-pro","messages":[{"role":"user","content":"hi"}]}"#.to_vec();
        let result = plugin.transform_request_body(body.clone(), &ctx).unwrap();
        assert_eq!(result, body);
    }

    #[test]
    fn test_transform_response_body_passthrough() {
        let plugin = VertexPlugin::new();
        let ctx = make_ctx();
        let body = br#"{"id":"chatcmpl-123","choices":[{"message":{"content":"hello"}}]}"#.to_vec();
        let result = plugin.transform_response_body(body.clone(), &ctx).unwrap();
        assert_eq!(result, body);
    }

    #[test]
    fn test_transform_stream_chunk_passthrough() {
        let plugin = VertexPlugin::new();
        let ctx = make_ctx();
        let chunk = r#"data: {"id":"chatcmpl-123","choices":[{"delta":{"content":"hi"}}]}"#;
        let result = plugin.transform_stream_chunk(chunk, &ctx).unwrap();
        assert_eq!(result, Some(chunk.to_string()));
    }

    // Note: build_auth() requires a real GCP service account key and network access
    // to exchange JWT for token. It cannot be unit tested without mocking.
    // Integration tests with real credentials would go in a separate test suite.

    #[test]
    fn test_build_auth_missing_sa_json() {
        let plugin = VertexPlugin::new();
        let mut ctx = make_ctx();
        ctx.provider_extra.remove("service_account_json");
        assert!(plugin.build_auth(&ctx).is_err());
    }
}
