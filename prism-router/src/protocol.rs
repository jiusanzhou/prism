/// Supported API protocols — covers LLM, audio, image, video, and utility endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ApiProtocol {
    /// OpenAI-compatible chat completions (/v1/chat/completions)
    ChatCompletion,
    /// Legacy completions (/v1/completions)
    Completion,
    /// Anthropic messages (/v1/messages)
    Messages,
    /// Embeddings (/v1/embeddings)
    Embedding,
    /// Rerank (/v1/rerank)
    Rerank,
    /// Audio transcription / ASR (/v1/audio/transcriptions)
    AudioTranscription,
    /// Audio translation (/v1/audio/translations)
    AudioTranslation,
    /// Text-to-speech (/v1/audio/speech)
    AudioSpeech,
    /// Image generation (/v1/images/generations)
    ImageGeneration,
    /// Image edits (/v1/images/edits)
    ImageEdit,
    /// Image variations (/v1/images/variations)
    ImageVariation,
    /// Video generation (/v1/videos/generations)
    VideoGeneration,
    /// Content moderation (/v1/moderations)
    Moderation,
    /// Unknown / not recognized
    Unknown,
}

impl ApiProtocol {
    /// Detect the protocol from the request URI path.
    pub fn from_path(path: &str) -> Self {
        // Strip query string for matching
        let p = path.split('?').next().unwrap_or(path);
        // Match most specific paths first
        if p.starts_with("/v1/chat/completions") {
            Self::ChatCompletion
        } else if p.starts_with("/v1/completions") {
            Self::Completion
        } else if p.starts_with("/v1/messages") {
            Self::Messages
        } else if p.starts_with("/v1/embeddings") {
            Self::Embedding
        } else if p.starts_with("/v1/rerank") {
            Self::Rerank
        } else if p.starts_with("/v1/audio/transcriptions") {
            Self::AudioTranscription
        } else if p.starts_with("/v1/audio/translations") {
            Self::AudioTranslation
        } else if p.starts_with("/v1/audio/speech") {
            Self::AudioSpeech
        } else if p.starts_with("/v1/images/generations") {
            Self::ImageGeneration
        } else if p.starts_with("/v1/images/edits") {
            Self::ImageEdit
        } else if p.starts_with("/v1/images/variations") {
            Self::ImageVariation
        } else if p.starts_with("/v1/videos/generations") {
            Self::VideoGeneration
        } else if p.starts_with("/v1/moderations") {
            Self::Moderation
        } else {
            Self::Unknown
        }
    }

    /// Return the protocol identifier string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ChatCompletion => "openai",
            Self::Completion => "openai",
            Self::Messages => "anthropic",
            Self::Embedding => "openai",
            Self::Rerank => "openai",
            Self::AudioTranscription => "openai",
            Self::AudioTranslation => "openai",
            Self::AudioSpeech => "openai",
            Self::ImageGeneration => "openai",
            Self::ImageEdit => "openai",
            Self::ImageVariation => "openai",
            Self::VideoGeneration => "openai",
            Self::Moderation => "openai",
            Self::Unknown => "unknown",
        }
    }

    /// Whether this is an LLM chat/completion protocol.
    pub fn is_llm_chat(&self) -> bool {
        matches!(
            self,
            Self::ChatCompletion | Self::Completion | Self::Messages
        )
    }

    /// Whether this is an audio endpoint.
    pub fn is_audio(&self) -> bool {
        matches!(
            self,
            Self::AudioTranscription | Self::AudioTranslation | Self::AudioSpeech
        )
    }

    /// Whether this is an image endpoint.
    pub fn is_image(&self) -> bool {
        matches!(
            self,
            Self::ImageGeneration | Self::ImageEdit | Self::ImageVariation
        )
    }

    /// Whether the upstream response is expected to be binary (not JSON).
    pub fn is_binary_response(&self) -> bool {
        matches!(self, Self::AudioSpeech)
    }

    /// Whether the request body is expected to be multipart/form-data.
    pub fn expects_multipart(&self) -> bool {
        matches!(
            self,
            Self::AudioTranscription
                | Self::AudioTranslation
                | Self::ImageEdit
                | Self::ImageVariation
        )
    }

    /// Whether this is a known API protocol (not Unknown).
    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

// ── Backward compatibility ──

/// Type alias for backward compatibility. `LlmProtocol` is now `ApiProtocol`.
pub type LlmProtocol = ApiProtocol;

/// Extract the model name from the request body JSON (best-effort).
/// Returns None if parsing fails or no model field is present.
#[allow(dead_code)]
pub fn extract_model_from_body(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value.get("model")?.as_str().map(|s| s.to_string())
}

/// Rewrite upstream path based on protocol and provider.
///
/// For OpenAI-compatible providers, the path is typically forwarded as-is.
/// For Anthropic, the path may need adjustment.
#[allow(dead_code)]
pub fn rewrite_path(original_path: &str, _protocol: &ApiProtocol) -> String {
    // For now, pass through the path unchanged.
    // Provider-specific path rewriting can be added here.
    original_path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Backward compatibility: LlmProtocol is ApiProtocol ──

    #[test]
    fn test_llm_protocol_alias() {
        // The old LlmProtocol type alias still works
        let p: LlmProtocol = LlmProtocol::from_path("/v1/chat/completions");
        assert_eq!(p.as_str(), "openai");
    }

    // ── Legacy tests (updated to use new variants but same semantics) ──

    #[test]
    fn test_protocol_detection() {
        assert_eq!(
            LlmProtocol::from_path("/v1/chat/completions"),
            LlmProtocol::ChatCompletion
        );
        assert_eq!(
            LlmProtocol::from_path("/v1/chat/completions?stream=true"),
            LlmProtocol::ChatCompletion
        );
        assert_eq!(
            LlmProtocol::from_path("/v1/messages"),
            LlmProtocol::Messages
        );
        assert_eq!(
            LlmProtocol::from_path("/v1/embeddings"),
            LlmProtocol::Embedding
        );
        assert_eq!(
            LlmProtocol::from_path("/some/other/path"),
            LlmProtocol::Unknown
        );
    }

    #[test]
    fn test_protocol_completions() {
        assert_eq!(
            LlmProtocol::from_path("/v1/completions"),
            LlmProtocol::Completion
        );
    }

    #[test]
    fn test_protocol_messages_with_query() {
        assert_eq!(
            LlmProtocol::from_path("/v1/messages?foo=bar"),
            LlmProtocol::Messages
        );
    }

    #[test]
    fn test_protocol_empty_path() {
        assert_eq!(LlmProtocol::from_path(""), LlmProtocol::Unknown);
    }

    #[test]
    fn test_protocol_root_path() {
        assert_eq!(LlmProtocol::from_path("/"), LlmProtocol::Unknown);
    }

    #[test]
    fn test_protocol_as_str() {
        assert_eq!(LlmProtocol::ChatCompletion.as_str(), "openai");
        assert_eq!(LlmProtocol::Completion.as_str(), "openai");
        assert_eq!(LlmProtocol::Messages.as_str(), "anthropic");
        assert_eq!(LlmProtocol::Embedding.as_str(), "openai");
        assert_eq!(LlmProtocol::Unknown.as_str(), "unknown");
    }

    #[test]
    fn test_extract_model_from_body() {
        let body = br#"{"model": "gpt-4o", "messages": []}"#;
        assert_eq!(extract_model_from_body(body), Some("gpt-4o".to_string()));

        let body = br#"{"messages": []}"#;
        assert_eq!(extract_model_from_body(body), None);

        let body = b"not json";
        assert_eq!(extract_model_from_body(body), None);
    }

    #[test]
    fn test_extract_model_empty_body() {
        assert_eq!(extract_model_from_body(b""), None);
    }

    #[test]
    fn test_extract_model_number_value() {
        let body = br#"{"model": 42}"#;
        assert_eq!(extract_model_from_body(body), None);
    }

    #[test]
    fn test_extract_model_anthropic_format() {
        let body = br#"{"model": "claude-3-opus-20240229", "max_tokens": 1024}"#;
        assert_eq!(
            extract_model_from_body(body),
            Some("claude-3-opus-20240229".to_string()),
        );
    }

    #[test]
    fn test_rewrite_path_passthrough() {
        assert_eq!(
            rewrite_path("/v1/chat/completions", &LlmProtocol::ChatCompletion),
            "/v1/chat/completions"
        );
        assert_eq!(
            rewrite_path("/v1/messages", &LlmProtocol::Messages),
            "/v1/messages"
        );
    }

    // ── New protocol detection tests ──

    #[test]
    fn test_audio_protocols() {
        assert_eq!(
            ApiProtocol::from_path("/v1/audio/transcriptions"),
            ApiProtocol::AudioTranscription
        );
        assert_eq!(
            ApiProtocol::from_path("/v1/audio/translations"),
            ApiProtocol::AudioTranslation
        );
        assert_eq!(
            ApiProtocol::from_path("/v1/audio/speech"),
            ApiProtocol::AudioSpeech
        );
    }

    #[test]
    fn test_image_protocols() {
        assert_eq!(
            ApiProtocol::from_path("/v1/images/generations"),
            ApiProtocol::ImageGeneration
        );
        assert_eq!(
            ApiProtocol::from_path("/v1/images/edits"),
            ApiProtocol::ImageEdit
        );
        assert_eq!(
            ApiProtocol::from_path("/v1/images/variations"),
            ApiProtocol::ImageVariation
        );
    }

    #[test]
    fn test_other_protocols() {
        assert_eq!(ApiProtocol::from_path("/v1/rerank"), ApiProtocol::Rerank);
        assert_eq!(
            ApiProtocol::from_path("/v1/videos/generations"),
            ApiProtocol::VideoGeneration
        );
        assert_eq!(
            ApiProtocol::from_path("/v1/moderations"),
            ApiProtocol::Moderation
        );
    }

    #[test]
    fn test_category_helpers() {
        assert!(ApiProtocol::ChatCompletion.is_llm_chat());
        assert!(ApiProtocol::Messages.is_llm_chat());
        assert!(!ApiProtocol::Embedding.is_llm_chat());
        assert!(!ApiProtocol::AudioSpeech.is_llm_chat());

        assert!(ApiProtocol::AudioTranscription.is_audio());
        assert!(ApiProtocol::AudioSpeech.is_audio());
        assert!(!ApiProtocol::ChatCompletion.is_audio());

        assert!(ApiProtocol::ImageGeneration.is_image());
        assert!(ApiProtocol::ImageEdit.is_image());
        assert!(!ApiProtocol::ChatCompletion.is_image());

        assert!(ApiProtocol::AudioSpeech.is_binary_response());
        assert!(!ApiProtocol::ChatCompletion.is_binary_response());

        assert!(ApiProtocol::AudioTranscription.expects_multipart());
        assert!(ApiProtocol::ImageEdit.expects_multipart());
        assert!(!ApiProtocol::ChatCompletion.expects_multipart());
    }

    #[test]
    fn test_is_known() {
        assert!(ApiProtocol::ChatCompletion.is_known());
        assert!(ApiProtocol::AudioSpeech.is_known());
        assert!(!ApiProtocol::Unknown.is_known());
    }
}
