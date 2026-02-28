/// Supported LLM API protocols.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmProtocol {
    OpenAI,
    Anthropic,
    Unknown,
}

impl LlmProtocol {
    /// Detect the protocol from the request URI path.
    pub fn from_path(path: &str) -> Self {
        if path.starts_with("/v1/chat/completions") {
            Self::OpenAI
        } else if path.starts_with("/v1/completions") {
            Self::OpenAI
        } else if path.starts_with("/v1/embeddings") {
            Self::OpenAI
        } else if path.starts_with("/v1/messages") {
            Self::Anthropic
        } else {
            Self::Unknown
        }
    }

    /// Return the protocol identifier string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
            Self::Unknown => "unknown",
        }
    }
}

/// Extract the model name from the request body JSON (best-effort).
/// Returns None if parsing fails or no model field is present.
pub fn extract_model_from_body(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value.get("model")?.as_str().map(|s| s.to_string())
}

/// Rewrite upstream path based on protocol and provider.
///
/// For OpenAI-compatible providers, the path is typically forwarded as-is.
/// For Anthropic, the path may need adjustment.
pub fn rewrite_path(original_path: &str, _protocol: &LlmProtocol) -> String {
    // For now, pass through the path unchanged.
    // Provider-specific path rewriting can be added here.
    original_path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_detection() {
        assert_eq!(
            LlmProtocol::from_path("/v1/chat/completions"),
            LlmProtocol::OpenAI
        );
        assert_eq!(
            LlmProtocol::from_path("/v1/chat/completions?stream=true"),
            LlmProtocol::OpenAI
        );
        assert_eq!(
            LlmProtocol::from_path("/v1/messages"),
            LlmProtocol::Anthropic
        );
        assert_eq!(
            LlmProtocol::from_path("/v1/embeddings"),
            LlmProtocol::OpenAI
        );
        assert_eq!(
            LlmProtocol::from_path("/some/other/path"),
            LlmProtocol::Unknown
        );
    }

    #[test]
    fn test_extract_model_from_body() {
        let body = br#"{"model": "gpt-4o", "messages": []}"#;
        assert_eq!(
            extract_model_from_body(body),
            Some("gpt-4o".to_string())
        );

        let body = br#"{"messages": []}"#;
        assert_eq!(extract_model_from_body(body), None);

        let body = b"not json";
        assert_eq!(extract_model_from_body(body), None);
    }
}
