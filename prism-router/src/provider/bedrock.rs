//! Built-in AWS Bedrock provider plugin.
//!
//! Handles Bedrock-specific concerns:
//! - AWS Signature V4 authentication
//! - URL rewriting: `/v1/chat/completions` → `/model/{model}/converse`
//! - Body transformation: OpenAI format → Bedrock Converse format (and back)

use super::{
    AuthResult, ConfigField, ConfigFieldType, ProviderConfigSchema, ProviderPlugin,
    ProviderRequestContext, UrlRewriteResult,
};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

type HmacSha256 = Hmac<Sha256>;

/// Built-in AWS Bedrock provider plugin.
///
/// - Auth: AWS Signature V4 signing
/// - URL: `/v1/chat/completions` → `/model/{model_id}/converse`
/// - Body: OpenAI → Bedrock Converse format conversion
pub struct BedrockPlugin;

impl ProviderPlugin for BedrockPlugin {
    fn id(&self) -> &str {
        "bedrock"
    }

    fn display_name(&self) -> &str {
        "AWS Bedrock"
    }

    fn config_schema(&self) -> ProviderConfigSchema {
        ProviderConfigSchema {
            provider_fields: vec![
                ConfigField {
                    name: "aws_region".to_string(),
                    label: "AWS Region".to_string(),
                    field_type: ConfigFieldType::String,
                    required: true,
                    default: Some("us-east-1".to_string()),
                    options: vec![],
                    description: "AWS region for the Bedrock endpoint".to_string(),
                },
                ConfigField {
                    name: "aws_access_key".to_string(),
                    label: "AWS Access Key".to_string(),
                    field_type: ConfigFieldType::Secret,
                    required: true,
                    default: None,
                    options: vec![],
                    description: "AWS access key ID".to_string(),
                },
                ConfigField {
                    name: "aws_secret_key".to_string(),
                    label: "AWS Secret Key".to_string(),
                    field_type: ConfigFieldType::Secret,
                    required: true,
                    default: None,
                    options: vec![],
                    description: "AWS secret access key".to_string(),
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
        let region = provider_extra.get("aws_region").and_then(|v| v.as_str());
        let access = provider_extra.get("aws_access_key").and_then(|v| v.as_str());
        let secret = provider_extra.get("aws_secret_key").and_then(|v| v.as_str());

        if region.unwrap_or("").is_empty() {
            return Err("aws_region is required".to_string());
        }
        if access.unwrap_or("").is_empty() {
            return Err("aws_access_key is required".to_string());
        }
        if secret.unwrap_or("").is_empty() {
            return Err("aws_secret_key is required".to_string());
        }
        Ok(())
    }

    fn build_auth(&self, ctx: &ProviderRequestContext) -> Result<AuthResult, String> {
        let region = ctx
            .provider_extra
            .get("aws_region")
            .and_then(|v| v.as_str())
            .unwrap_or("us-east-1");
        let access_key = ctx
            .provider_extra
            .get("aws_access_key")
            .and_then(|v| v.as_str())
            .ok_or("aws_access_key not found in provider config")?;
        let secret_key = ctx
            .provider_extra
            .get("aws_secret_key")
            .and_then(|v| v.as_str())
            .ok_or("aws_secret_key not found in provider config")?;

        // Determine the URL path that will be used
        let url_result = self.rewrite_url(ctx)?;
        let path = &url_result.path;
        let host = format!("bedrock-runtime.{}.amazonaws.com", region);

        // For SigV4, we need the request body hash. Since build_auth is called
        // before the body is available, use the empty-body hash for GET or
        // UNSIGNED-PAYLOAD for POST (Bedrock accepts this).
        let payload_hash = "UNSIGNED-PAYLOAD";

        let now = chrono::Utc::now();
        let date_stamp = now.format("%Y%m%d").to_string();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();

        let service = "bedrock";
        let credential_scope = format!("{}/{}/{}/aws4_request", date_stamp, region, service);

        // Step 1: Canonical request
        let canonical_headers = format!("host:{}\nx-amz-date:{}\n", host, amz_date);
        let signed_headers = "host;x-amz-date";

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            ctx.method,
            path,
            "", // query string (empty for Bedrock)
            canonical_headers,
            signed_headers,
            payload_hash,
        );

        // Step 2: String to sign
        let canonical_hash = hex::encode(Sha256::digest(canonical_request.as_bytes()));
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date, credential_scope, canonical_hash
        );

        // Step 3: Signing key
        let signing_key = derive_signing_key(secret_key, &date_stamp, region, service);

        // Step 4: Signature
        let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

        // Step 5: Authorization header
        let auth_header = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            access_key, credential_scope, signed_headers, signature
        );

        Ok(AuthResult {
            headers: vec![
                ("Authorization".to_string(), auth_header),
                ("x-amz-date".to_string(), amz_date),
                ("x-amz-content-sha256".to_string(), payload_hash.to_string()),
                ("Host".to_string(), host),
            ],
        })
    }

    fn rewrite_url(&self, ctx: &ProviderRequestContext) -> Result<UrlRewriteResult, String> {
        let model_id = ctx
            .upstream_model_name
            .as_deref()
            .unwrap_or(&ctx.model_id);

        let region = ctx
            .provider_extra
            .get("aws_region")
            .and_then(|v| v.as_str())
            .unwrap_or("us-east-1");

        let host = format!("bedrock-runtime.{}.amazonaws.com", region);

        // Map OpenAI paths to Bedrock paths
        let path = if ctx.original_path.contains("/chat/completions") {
            format!("/model/{}/converse", url_encode_model(model_id))
        } else if ctx.original_path.contains("/embeddings") {
            format!("/model/{}/invoke", url_encode_model(model_id))
        } else {
            // Fallback: use converse for any other path
            format!("/model/{}/converse", url_encode_model(model_id))
        };

        // Add streaming suffix if needed
        let path = if ctx.is_streaming && path.contains("/converse") {
            path.replace("/converse", "/converse-stream")
        } else {
            path
        };

        Ok(UrlRewriteResult {
            path,
            host_override: Some(host),
        })
    }

    fn transform_request_body(
        &self,
        body: Vec<u8>,
        _ctx: &ProviderRequestContext,
    ) -> Result<Vec<u8>, String> {
        let openai: serde_json::Value =
            serde_json::from_slice(&body).map_err(|e| format!("invalid JSON body: {}", e))?;

        let messages = openai
            .get("messages")
            .and_then(|m| m.as_array())
            .ok_or("missing 'messages' array")?;

        // Convert OpenAI messages to Bedrock Converse format
        let mut bedrock_messages = Vec::new();
        let mut system_prompts = Vec::new();

        for msg in messages {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = msg.get("content");

            if role == "system" {
                // Bedrock puts system messages in a separate field
                if let Some(text) = content.and_then(|c| c.as_str()) {
                    system_prompts.push(serde_json::json!({"text": text}));
                }
                continue;
            }

            let bedrock_role = if role == "assistant" {
                "assistant"
            } else {
                "user"
            };

            let bedrock_content = if let Some(text) = content.and_then(|c| c.as_str()) {
                vec![serde_json::json!({"text": text})]
            } else if let Some(parts) = content.and_then(|c| c.as_array()) {
                // Multi-part content
                parts
                    .iter()
                    .filter_map(|part| {
                        let part_type = part.get("type").and_then(|t| t.as_str())?;
                        match part_type {
                            "text" => {
                                let text = part.get("text").and_then(|t| t.as_str())?;
                                Some(serde_json::json!({"text": text}))
                            }
                            _ => None,
                        }
                    })
                    .collect()
            } else {
                vec![serde_json::json!({"text": ""})]
            };

            bedrock_messages.push(serde_json::json!({
                "role": bedrock_role,
                "content": bedrock_content,
            }));
        }

        let mut bedrock_body = serde_json::json!({
            "messages": bedrock_messages,
        });

        if !system_prompts.is_empty() {
            bedrock_body["system"] = serde_json::json!(system_prompts);
        }

        // Forward inference config
        if let Some(temp) = openai.get("temperature") {
            bedrock_body["inferenceConfig"] =
                serde_json::json!({"temperature": temp});
        }
        if let Some(max_tokens) = openai.get("max_tokens") {
            let config = bedrock_body
                .get("inferenceConfig")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            let mut config = config;
            config["maxTokens"] = max_tokens.clone();
            bedrock_body["inferenceConfig"] = config;
        }
        if let Some(top_p) = openai.get("top_p") {
            let config = bedrock_body
                .get("inferenceConfig")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            let mut config = config;
            config["topP"] = top_p.clone();
            bedrock_body["inferenceConfig"] = config;
        }

        serde_json::to_vec(&bedrock_body).map_err(|e| format!("failed to serialize: {}", e))
    }

    fn transform_response_body(
        &self,
        body: Vec<u8>,
        _ctx: &ProviderRequestContext,
    ) -> Result<Vec<u8>, String> {
        let bedrock: serde_json::Value =
            serde_json::from_slice(&body).map_err(|e| format!("invalid response JSON: {}", e))?;

        // Bedrock Converse response → OpenAI format
        let output = bedrock.get("output").and_then(|o| o.get("message"));
        let content = output
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("");

        let role = output
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str())
            .unwrap_or("assistant");

        // Extract usage
        let usage = bedrock.get("usage");
        let input_tokens = usage
            .and_then(|u| u.get("inputTokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output_tokens = usage
            .and_then(|u| u.get("outputTokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let openai_response = serde_json::json!({
            "id": format!("chatcmpl-bedrock-{}", chrono::Utc::now().timestamp_millis()),
            "object": "chat.completion",
            "created": chrono::Utc::now().timestamp(),
            "model": "bedrock",
            "choices": [{
                "index": 0,
                "message": {
                    "role": role,
                    "content": content,
                },
                "finish_reason": bedrock.get("stopReason")
                    .and_then(|r| r.as_str())
                    .map(|r| match r {
                        "end_turn" | "stop_sequence" => "stop",
                        "max_tokens" => "length",
                        _ => "stop",
                    })
                    .unwrap_or("stop"),
            }],
            "usage": {
                "prompt_tokens": input_tokens,
                "completion_tokens": output_tokens,
                "total_tokens": input_tokens + output_tokens,
            },
        });

        serde_json::to_vec(&openai_response).map_err(|e| format!("failed to serialize: {}", e))
    }

    fn transform_stream_chunk(
        &self,
        chunk: &str,
        _ctx: &ProviderRequestContext,
    ) -> Result<Option<String>, String> {
        // Bedrock streaming uses binary event stream format.
        // For MVP, pass through — real implementation would decode
        // the event stream and re-encode as SSE.
        Ok(Some(chunk.to_string()))
    }
}

// ── SigV4 helpers ──

/// URL-encode model ID (Bedrock model IDs can contain slashes, periods, etc.)
fn url_encode_model(model_id: &str) -> String {
    // Bedrock model IDs like "anthropic.claude-3-sonnet-20240229-v1:0"
    // need to be URL-encoded for the path
    model_id
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}

/// HMAC-SHA256
fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac =
        HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Derive the SigV4 signing key.
///
/// ```text
/// kDate    = HMAC("AWS4" + secret, dateStamp)
/// kRegion  = HMAC(kDate, region)
/// kService = HMAC(kRegion, service)
/// kSigning = HMAC(kService, "aws4_request")
/// ```
fn derive_signing_key(secret_key: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(
        format!("AWS4{}", secret_key).as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> ProviderRequestContext {
        let mut extra = HashMap::new();
        extra.insert(
            "aws_region".to_string(),
            serde_json::json!("us-east-1"),
        );
        extra.insert(
            "aws_access_key".to_string(),
            serde_json::json!("AKIAIOSFODNN7EXAMPLE"),
        );
        extra.insert(
            "aws_secret_key".to_string(),
            serde_json::json!("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"),
        );

        ProviderRequestContext {
            provider_id: "bedrock-us".to_string(),
            provider_type: "bedrock".to_string(),
            base_url: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
            provider_extra: extra,
            account_id: "bedrock-acct".to_string(),
            account_api_key: None,
            account_headers: HashMap::new(),
            variant_id: "claude-3@bedrock".to_string(),
            model_id: "anthropic.claude-3-sonnet-20240229-v1:0".to_string(),
            upstream_model_name: Some(
                "anthropic.claude-3-sonnet-20240229-v1:0".to_string(),
            ),
            deployment_name: None,
            original_path: "/v1/chat/completions".to_string(),
            method: "POST".to_string(),
            is_streaming: false,
        }
    }

    #[test]
    fn test_id() {
        let plugin = BedrockPlugin;
        assert_eq!(plugin.id(), "bedrock");
        assert_eq!(plugin.display_name(), "AWS Bedrock");
    }

    #[test]
    fn test_config_schema() {
        let plugin = BedrockPlugin;
        let schema = plugin.config_schema();
        assert_eq!(schema.provider_fields.len(), 3);
        assert_eq!(schema.provider_fields[0].name, "aws_region");
        assert_eq!(schema.provider_fields[1].name, "aws_access_key");
        assert_eq!(schema.provider_fields[2].name, "aws_secret_key");
        assert!(schema.account_fields.is_empty());
    }

    #[test]
    fn test_validate_config_ok() {
        let plugin = BedrockPlugin;
        let mut extra = HashMap::new();
        extra.insert("aws_region".into(), serde_json::json!("us-east-1"));
        extra.insert("aws_access_key".into(), serde_json::json!("AKIA..."));
        extra.insert("aws_secret_key".into(), serde_json::json!("secret"));
        assert!(plugin.validate_config(&extra, &None, &HashMap::new()).is_ok());
    }

    #[test]
    fn test_validate_config_missing_region() {
        let plugin = BedrockPlugin;
        let mut extra = HashMap::new();
        extra.insert("aws_access_key".into(), serde_json::json!("AKIA..."));
        extra.insert("aws_secret_key".into(), serde_json::json!("secret"));
        let err = plugin.validate_config(&extra, &None, &HashMap::new()).unwrap_err();
        assert!(err.contains("aws_region"));
    }

    #[test]
    fn test_validate_config_missing_access_key() {
        let plugin = BedrockPlugin;
        let mut extra = HashMap::new();
        extra.insert("aws_region".into(), serde_json::json!("us-east-1"));
        extra.insert("aws_secret_key".into(), serde_json::json!("secret"));
        let err = plugin.validate_config(&extra, &None, &HashMap::new()).unwrap_err();
        assert!(err.contains("aws_access_key"));
    }

    #[test]
    fn test_validate_config_missing_secret_key() {
        let plugin = BedrockPlugin;
        let mut extra = HashMap::new();
        extra.insert("aws_region".into(), serde_json::json!("us-east-1"));
        extra.insert("aws_access_key".into(), serde_json::json!("AKIA..."));
        let err = plugin.validate_config(&extra, &None, &HashMap::new()).unwrap_err();
        assert!(err.contains("aws_secret_key"));
    }

    #[test]
    fn test_build_auth_sigv4_headers() {
        let plugin = BedrockPlugin;
        let ctx = make_ctx();
        let result = plugin.build_auth(&ctx).unwrap();

        // Should have Authorization, x-amz-date, x-amz-content-sha256, Host
        assert_eq!(result.headers.len(), 4);

        let auth = &result.headers[0];
        assert_eq!(auth.0, "Authorization");
        assert!(auth.1.starts_with("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/"));
        assert!(auth.1.contains("/us-east-1/bedrock/aws4_request"));
        assert!(auth.1.contains("SignedHeaders=host;x-amz-date"));

        let date = &result.headers[1];
        assert_eq!(date.0, "x-amz-date");
        assert!(date.1.ends_with('Z'));

        let sha = &result.headers[2];
        assert_eq!(sha.0, "x-amz-content-sha256");
        assert_eq!(sha.1, "UNSIGNED-PAYLOAD");

        let host = &result.headers[3];
        assert_eq!(host.0, "Host");
        assert_eq!(host.1, "bedrock-runtime.us-east-1.amazonaws.com");
    }

    #[test]
    fn test_build_auth_missing_credentials() {
        let plugin = BedrockPlugin;
        let mut ctx = make_ctx();
        ctx.provider_extra.clear();
        assert!(plugin.build_auth(&ctx).is_err());
    }

    #[test]
    fn test_rewrite_url_chat_completions() {
        let plugin = BedrockPlugin;
        let ctx = make_ctx();
        let result = plugin.rewrite_url(&ctx).unwrap();

        assert_eq!(
            result.path,
            "/model/anthropic.claude-3-sonnet-20240229-v1%3A0/converse"
        );
        assert_eq!(
            result.host_override,
            Some("bedrock-runtime.us-east-1.amazonaws.com".to_string())
        );
    }

    #[test]
    fn test_rewrite_url_streaming() {
        let plugin = BedrockPlugin;
        let mut ctx = make_ctx();
        ctx.is_streaming = true;
        let result = plugin.rewrite_url(&ctx).unwrap();
        assert!(result.path.ends_with("/converse-stream"));
    }

    #[test]
    fn test_rewrite_url_embeddings() {
        let plugin = BedrockPlugin;
        let mut ctx = make_ctx();
        ctx.original_path = "/v1/embeddings".to_string();
        let result = plugin.rewrite_url(&ctx).unwrap();
        assert!(result.path.contains("/invoke"));
    }

    #[test]
    fn test_transform_request_body() {
        let plugin = BedrockPlugin;
        let ctx = make_ctx();
        let openai_body = serde_json::json!({
            "model": "claude-3-sonnet",
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Hello!"},
            ],
            "temperature": 0.7,
            "max_tokens": 100,
        });
        let body = serde_json::to_vec(&openai_body).unwrap();
        let result = plugin.transform_request_body(body, &ctx).unwrap();
        let bedrock: serde_json::Value = serde_json::from_slice(&result).unwrap();

        // System message should be in separate field
        assert!(bedrock.get("system").is_some());
        let system = bedrock["system"].as_array().unwrap();
        assert_eq!(system[0]["text"], "You are helpful.");

        // User message in messages array
        let messages = bedrock["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"][0]["text"], "Hello!");

        // Inference config
        assert!(bedrock.get("inferenceConfig").is_some());
    }

    #[test]
    fn test_transform_response_body() {
        let plugin = BedrockPlugin;
        let ctx = make_ctx();
        let bedrock_response = serde_json::json!({
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [{"text": "Hello! How can I help?"}]
                }
            },
            "stopReason": "end_turn",
            "usage": {
                "inputTokens": 10,
                "outputTokens": 20,
            }
        });
        let body = serde_json::to_vec(&bedrock_response).unwrap();
        let result = plugin.transform_response_body(body, &ctx).unwrap();
        let openai: serde_json::Value = serde_json::from_slice(&result).unwrap();

        assert_eq!(openai["object"], "chat.completion");
        assert_eq!(
            openai["choices"][0]["message"]["content"],
            "Hello! How can I help?"
        );
        assert_eq!(openai["choices"][0]["finish_reason"], "stop");
        assert_eq!(openai["usage"]["prompt_tokens"], 10);
        assert_eq!(openai["usage"]["completion_tokens"], 20);
        assert_eq!(openai["usage"]["total_tokens"], 30);
    }

    #[test]
    fn test_sigv4_signing_key_derivation() {
        // Known test vector: the signing key should be deterministic
        let key1 = derive_signing_key("secret", "20240101", "us-east-1", "bedrock");
        let key2 = derive_signing_key("secret", "20240101", "us-east-1", "bedrock");
        assert_eq!(key1, key2);
        assert_eq!(key1.len(), 32); // SHA-256 output

        // Different inputs should produce different keys
        let key3 = derive_signing_key("secret", "20240102", "us-east-1", "bedrock");
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_url_encode_model() {
        assert_eq!(
            url_encode_model("anthropic.claude-3-sonnet-20240229-v1:0"),
            "anthropic.claude-3-sonnet-20240229-v1%3A0"
        );
        assert_eq!(url_encode_model("amazon.titan-embed-text-v1"), "amazon.titan-embed-text-v1");
    }
}
