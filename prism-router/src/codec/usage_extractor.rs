//! Usage extractor plugin — extracts token/resource usage from upstream responses.
//!
//! Runs as a global plugin in the codec chain. After codec conversion, the response
//! is in the inbound (normalized) format, so extraction is protocol-specific based
//! on the inbound protocol.
//!
//! Built-in strategies:
//! - `openai`: `usage.prompt_tokens` / `completion_tokens` / `total_tokens`
//! - `anthropic`: `usage.input_tokens` / `output_tokens`
//! - Fallback: tries OpenAI format first, then Anthropic
//!
//! Extracted metrics are stored in `ProxyContext.data` under key [`USAGE_KEY`].

use serde::Deserialize;

use super::{ProxyContext, ProxyPlugin};

/// Key for storing extracted usage in ProxyContext data map.
pub const USAGE_KEY: &str = "prism.usage";

/// Extracted usage metrics — protocol-agnostic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractedUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    /// Bytes processed (for binary responses like TTS audio, images).
    pub bytes_processed: Option<u64>,
    /// Media type of the response (e.g., "audio/mpeg", "image/png").
    pub media_type: Option<String>,
    /// Raw metric bag for future extensibility (e.g., AIGC: steps, pixels, duration_ms).
    pub metrics: std::collections::HashMap<String, u64>,
}

impl ExtractedUsage {
    pub fn has_data(&self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.total_tokens.is_some()
            || self.bytes_processed.is_some()
            || !self.metrics.is_empty()
    }

    /// Merge another usage (later values win).
    pub fn merge(&mut self, other: &ExtractedUsage) {
        if other.input_tokens.is_some() {
            self.input_tokens = other.input_tokens;
        }
        if other.output_tokens.is_some() {
            self.output_tokens = other.output_tokens;
        }
        if other.total_tokens.is_some() {
            self.total_tokens = other.total_tokens;
        }
        if other.bytes_processed.is_some() {
            self.bytes_processed = other.bytes_processed;
        }
        if other.media_type.is_some() {
            self.media_type = other.media_type.clone();
        }
        for (k, v) in &other.metrics {
            self.metrics.insert(k.clone(), *v);
        }
    }
}

// ════════════════════════════════════════════════
// Extraction strategies
// ════════════════════════════════════════════════

/// Trait for extracting usage from response JSON.
pub trait UsageExtractStrategy: Send + Sync {
    /// Strategy name (for logging).
    fn name(&self) -> &str;
    /// Try to extract usage from a JSON string. Returns None if no usage found.
    fn extract(&self, json: &str) -> Option<ExtractedUsage>;
}

/// OpenAI format: `usage.prompt_tokens`, `completion_tokens`, `total_tokens`.
pub struct OpenAIStrategy;

#[derive(Deserialize)]
struct OpenAIUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct OpenAIResponse {
    usage: Option<OpenAIUsage>,
}

impl UsageExtractStrategy for OpenAIStrategy {
    fn name(&self) -> &str {
        "openai"
    }

    fn extract(&self, json: &str) -> Option<ExtractedUsage> {
        let parsed: OpenAIResponse = serde_json::from_str(json).ok()?;
        let u = parsed.usage?;
        let result = ExtractedUsage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
            ..Default::default()
        };
        if result.has_data() {
            Some(result)
        } else {
            None
        }
    }
}

/// Anthropic format: `usage.input_tokens`, `output_tokens`.
pub struct AnthropicStrategy;

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    usage: Option<AnthropicUsage>,
}

impl UsageExtractStrategy for AnthropicStrategy {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn extract(&self, json: &str) -> Option<ExtractedUsage> {
        let parsed: AnthropicResponse = serde_json::from_str(json).ok()?;
        let u = parsed.usage?;
        let result = ExtractedUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            total_tokens: u.input_tokens.zip(u.output_tokens).map(|(i, o)| i + o),
            ..Default::default()
        };
        if result.has_data() {
            Some(result)
        } else {
            None
        }
    }
}

/// Fallback: try OpenAI first, then Anthropic.
pub struct AutoStrategy;

impl UsageExtractStrategy for AutoStrategy {
    fn name(&self) -> &str {
        "auto"
    }

    fn extract(&self, json: &str) -> Option<ExtractedUsage> {
        OpenAIStrategy
            .extract(json)
            .or_else(|| AnthropicStrategy.extract(json))
    }
}

/// Strategy for binary (non-JSON) responses: TTS audio, raw images, etc.
///
/// Does not extract from JSON — instead, callers record `bytes_processed` and
/// `media_type` directly into `ExtractedUsage` and store it in the context.
/// This strategy exists so it can be registered as a named strategy for
/// binary-response protocols.
pub struct BinaryUsageStrategy;

impl UsageExtractStrategy for BinaryUsageStrategy {
    fn name(&self) -> &str {
        "binary"
    }

    fn extract(&self, _json: &str) -> Option<ExtractedUsage> {
        // Binary responses are not JSON — usage is tracked externally
        // via bytes_processed / media_type fields.
        None
    }
}

impl BinaryUsageStrategy {
    /// Create an `ExtractedUsage` from binary response metadata.
    pub fn usage_from_binary(bytes: u64, media_type: &str) -> ExtractedUsage {
        let mut metrics = std::collections::HashMap::new();
        metrics.insert("bytes_sent".to_string(), bytes);

        // Estimate audio duration from bytes + format (rough heuristic)
        if let Some(duration_ms) = Self::estimate_audio_duration_ms(bytes, media_type) {
            metrics.insert("duration_ms".to_string(), duration_ms);
        }

        ExtractedUsage {
            bytes_processed: Some(bytes),
            media_type: Some(media_type.to_string()),
            metrics,
            ..Default::default()
        }
    }

    /// Rough estimate of audio duration from byte count and content type.
    /// Returns None for non-audio or unknown formats.
    fn estimate_audio_duration_ms(bytes: u64, media_type: &str) -> Option<u64> {
        // Approximate bitrates (bytes/sec)
        let bytes_per_sec = match media_type {
            "audio/mpeg" | "audio/mp3" => 16_000, // ~128 kbps MP3
            "audio/opus" | "audio/ogg" => 8_000,  // ~64 kbps Opus
            "audio/aac" => 16_000,                // ~128 kbps AAC
            "audio/flac" => 88_200,               // ~705 kbps FLAC (CD quality)
            "audio/wav" | "audio/pcm" => 96_000,  // ~768 kbps (16-bit 48kHz stereo)
            _ if media_type.starts_with("audio/") => 16_000, // fallback
            _ => return None,
        };
        Some(bytes * 1000 / bytes_per_sec)
    }
}

// ════════════════════════════════════════════════
// Plugin
// ════════════════════════════════════════════════

/// Usage extractor plugin — implements `ProxyPlugin`.
///
/// Default: auto-detect strategy based on inbound protocol.
/// Custom strategies can be registered for protocol names.
pub struct UsageExtractorPlugin {
    /// Protocol → strategy mapping.
    strategies: Vec<(String, Box<dyn UsageExtractStrategy>)>,
    /// Fallback strategy when protocol doesn't match.
    fallback: Box<dyn UsageExtractStrategy>,
}

impl Default for UsageExtractorPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl UsageExtractorPlugin {
    pub fn new() -> Self {
        Self {
            strategies: vec![
                ("openai".to_string(), Box::new(OpenAIStrategy)),
                ("anthropic".to_string(), Box::new(AnthropicStrategy)),
            ],
            fallback: Box::new(AutoStrategy),
        }
    }

    /// Register a custom strategy for a protocol.
    pub fn with_strategy(
        mut self,
        protocol: &str,
        strategy: Box<dyn UsageExtractStrategy>,
    ) -> Self {
        self.strategies.push((protocol.to_string(), strategy));
        self
    }

    /// Get the strategy for the given protocol.
    fn strategy_for(&self, protocol: &str) -> &dyn UsageExtractStrategy {
        for (proto, strategy) in &self.strategies {
            if proto == protocol {
                return strategy.as_ref();
            }
        }
        self.fallback.as_ref()
    }

    /// Try to extract usage from a JSON string using the appropriate strategy.
    fn try_extract(&self, protocol: &str, json: &str) -> Option<ExtractedUsage> {
        self.strategy_for(protocol).extract(json)
    }

    /// Extract from an SSE data line.
    fn extract_from_sse_line(&self, protocol: &str, line: &str) -> Option<ExtractedUsage> {
        let data = if let Some(d) = line.strip_prefix("data: ") {
            d
        } else if let Some(d) = line.strip_prefix("data:") {
            d.trim_start()
        } else {
            return None;
        };
        if data == "[DONE]" || data.is_empty() {
            return None;
        }
        self.try_extract(protocol, data)
    }
}

impl ProxyPlugin for UsageExtractorPlugin {
    fn name(&self) -> &str {
        "usage_extractor"
    }

    fn on_response_body(&self, ctx: &mut ProxyContext, body: Vec<u8>) -> Result<Vec<u8>, String> {
        // Check if this is a binary response (stored by proxy in context)
        let content_kind = ctx
            .get::<String>("prism.response_content_kind")
            .cloned()
            .unwrap_or_default();
        if content_kind == "binary" {
            // For binary responses, record bytes + content-type as usage
            let media_type = ctx
                .response_headers
                .get("content-type")
                .cloned()
                .unwrap_or_default();
            let usage = BinaryUsageStrategy::usage_from_binary(body.len() as u64, &media_type);
            ctx.set(USAGE_KEY, usage);
            return Ok(body);
        }

        // Non-streaming JSON: extract from the full response body (after codec conversion)
        if let Ok(json) = std::str::from_utf8(&body) {
            // Use inbound protocol — after codec, body is in inbound format
            let protocol = &ctx.inbound_protocol;
            if let Some(usage) = self.try_extract(protocol, json) {
                ctx.set(USAGE_KEY, usage);
            }
        }
        Ok(body) // pass through unchanged
    }

    fn on_response_stream_chunk(
        &self,
        ctx: &mut ProxyContext,
        chunk: &str,
    ) -> Result<Option<String>, String> {
        let protocol = &ctx.inbound_protocol.clone();

        // Scan each line in the chunk for usage data
        for line in chunk.lines() {
            if let Some(new_usage) = self.extract_from_sse_line(protocol, line) {
                // Merge with existing usage (streaming can have partial updates)
                let mut current = ctx
                    .get::<ExtractedUsage>(USAGE_KEY)
                    .cloned()
                    .unwrap_or_default();
                current.merge(&new_usage);
                ctx.set(USAGE_KEY, current);
            }
        }

        Ok(Some(chunk.to_string())) // pass through unchanged
    }

    fn on_log(&self, _ctx: &mut ProxyContext) {
        // Usage is already stored in ctx.data — nothing extra to do at log time.
        // QuotaRecorderPlugin (or other downstream plugins) will read it.
    }
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Strategy tests ──

    #[test]
    fn test_openai_strategy() {
        let s = OpenAIStrategy;
        let json = r#"{"usage":{"prompt_tokens":42,"completion_tokens":128,"total_tokens":170}}"#;
        let u = s.extract(json).unwrap();
        assert_eq!(u.input_tokens, Some(42));
        assert_eq!(u.output_tokens, Some(128));
        assert_eq!(u.total_tokens, Some(170));
    }

    #[test]
    fn test_anthropic_strategy() {
        let s = AnthropicStrategy;
        let json = r#"{"usage":{"input_tokens":100,"output_tokens":50}}"#;
        let u = s.extract(json).unwrap();
        assert_eq!(u.input_tokens, Some(100));
        assert_eq!(u.output_tokens, Some(50));
        assert_eq!(u.total_tokens, Some(150)); // auto-summed
    }

    #[test]
    fn test_auto_strategy_openai() {
        let s = AutoStrategy;
        let json = r#"{"usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}}"#;
        let u = s.extract(json).unwrap();
        assert_eq!(u.input_tokens, Some(10));
    }

    #[test]
    fn test_auto_strategy_anthropic() {
        let s = AutoStrategy;
        // Anthropic doesn't have prompt_tokens, only input_tokens
        let json = r#"{"usage":{"input_tokens":10,"output_tokens":20}}"#;
        let u = s.extract(json).unwrap();
        assert_eq!(u.input_tokens, Some(10));
        assert_eq!(u.output_tokens, Some(20));
    }

    #[test]
    fn test_no_usage() {
        let s = OpenAIStrategy;
        assert!(s.extract(r#"{"id":"chatcmpl-abc"}"#).is_none());
    }

    #[test]
    fn test_empty_usage() {
        let s = OpenAIStrategy;
        assert!(s.extract(r#"{"usage":{}}"#).is_none());
    }

    #[test]
    fn test_extra_fields_ignored() {
        let s = OpenAIStrategy;
        let json = r#"{"usage":{"prompt_tokens":42,"completion_tokens":128,"total_tokens":170,"prompt_tokens_details":{"cached_tokens":10}}}"#;
        let u = s.extract(json).unwrap();
        assert_eq!(u.input_tokens, Some(42));
    }

    // ── Plugin tests ──

    #[test]
    fn test_plugin_non_streaming() {
        let plugin = UsageExtractorPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.inbound_protocol = "openai".to_string();

        let body =
            br#"{"usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}}"#.to_vec();
        let result = plugin.on_response_body(&mut ctx, body).unwrap();
        assert!(!result.is_empty()); // pass through

        let usage = ctx.get::<ExtractedUsage>(USAGE_KEY).unwrap();
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.total_tokens, Some(30));
    }

    #[test]
    fn test_plugin_streaming() {
        let plugin = UsageExtractorPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.inbound_protocol = "openai".to_string();

        // Regular content chunk — no usage
        let r = plugin
            .on_response_stream_chunk(
                &mut ctx,
                "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n",
            )
            .unwrap();
        assert!(r.is_some());
        assert!(ctx.get::<ExtractedUsage>(USAGE_KEY).is_none());

        // Usage chunk
        let r = plugin.on_response_stream_chunk(&mut ctx, "data: {\"usage\":{\"prompt_tokens\":42,\"completion_tokens\":128,\"total_tokens\":170}}\n").unwrap();
        assert!(r.is_some());
        let usage = ctx.get::<ExtractedUsage>(USAGE_KEY).unwrap();
        assert_eq!(usage.input_tokens, Some(42));
        assert_eq!(usage.output_tokens, Some(128));
    }

    #[test]
    fn test_plugin_streaming_done() {
        let plugin = UsageExtractorPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.inbound_protocol = "openai".to_string();

        let r = plugin
            .on_response_stream_chunk(&mut ctx, "data: [DONE]\n")
            .unwrap();
        assert!(r.is_some());
        assert!(ctx.get::<ExtractedUsage>(USAGE_KEY).is_none());
    }

    #[test]
    fn test_plugin_anthropic_protocol() {
        let plugin = UsageExtractorPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.inbound_protocol = "anthropic".to_string();

        let body = br#"{"usage":{"input_tokens":100,"output_tokens":50}}"#.to_vec();
        plugin.on_response_body(&mut ctx, body).unwrap();

        let usage = ctx.get::<ExtractedUsage>(USAGE_KEY).unwrap();
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(50));
        assert_eq!(usage.total_tokens, Some(150));
    }

    #[test]
    fn test_plugin_unknown_protocol_auto_fallback() {
        let plugin = UsageExtractorPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.inbound_protocol = "custom-llm".to_string(); // unknown protocol

        let body =
            br#"{"usage":{"prompt_tokens":5,"completion_tokens":10,"total_tokens":15}}"#.to_vec();
        plugin.on_response_body(&mut ctx, body).unwrap();

        let usage = ctx.get::<ExtractedUsage>(USAGE_KEY).unwrap();
        assert_eq!(usage.input_tokens, Some(5)); // auto-detected as OpenAI
    }

    // ── ExtractedUsage merge tests ──

    #[test]
    fn test_merge() {
        let mut a = ExtractedUsage {
            input_tokens: Some(10),
            output_tokens: None,
            total_tokens: None,
            ..Default::default()
        };
        let b = ExtractedUsage {
            input_tokens: None,
            output_tokens: Some(20),
            total_tokens: Some(30),
            metrics: [("steps".to_string(), 50)].into(),
            ..Default::default()
        };
        a.merge(&b);
        assert_eq!(a.input_tokens, Some(10));
        assert_eq!(a.output_tokens, Some(20));
        assert_eq!(a.total_tokens, Some(30));
        assert_eq!(a.metrics.get("steps"), Some(&50));
    }

    // ── Custom strategy test ──

    struct ImageGenStrategy;
    impl UsageExtractStrategy for ImageGenStrategy {
        fn name(&self) -> &str {
            "image_gen"
        }
        fn extract(&self, json: &str) -> Option<ExtractedUsage> {
            #[derive(Deserialize)]
            struct ImgResp {
                steps: Option<u64>,
                width: Option<u64>,
                height: Option<u64>,
            }
            let r: ImgResp = serde_json::from_str(json).ok()?;
            let mut metrics = std::collections::HashMap::new();
            if let Some(s) = r.steps {
                metrics.insert("steps".to_string(), s);
            }
            if let (Some(w), Some(h)) = (r.width, r.height) {
                metrics.insert("pixels".to_string(), w * h);
            }
            if metrics.is_empty() {
                None
            } else {
                Some(ExtractedUsage {
                    input_tokens: None,
                    output_tokens: None,
                    total_tokens: None,
                    metrics,
                    ..Default::default()
                })
            }
        }
    }

    #[test]
    fn test_custom_strategy_aigc() {
        let plugin =
            UsageExtractorPlugin::new().with_strategy("image_gen", Box::new(ImageGenStrategy));
        let mut ctx = ProxyContext::new();
        ctx.inbound_protocol = "image_gen".to_string();

        let body = br#"{"steps":50,"width":1024,"height":1024}"#.to_vec();
        plugin.on_response_body(&mut ctx, body).unwrap();

        let usage = ctx.get::<ExtractedUsage>(USAGE_KEY).unwrap();
        assert_eq!(usage.metrics.get("steps"), Some(&50));
        assert_eq!(usage.metrics.get("pixels"), Some(&1048576));
        assert!(usage.input_tokens.is_none());
    }

    // ── Binary usage strategy tests ──

    #[test]
    fn test_binary_strategy_returns_none_for_json() {
        let s = BinaryUsageStrategy;
        assert!(s.extract(r#"{"usage":{"prompt_tokens":10}}"#).is_none());
    }

    #[test]
    fn test_binary_usage_from_binary_audio() {
        let usage = BinaryUsageStrategy::usage_from_binary(160_000, "audio/mpeg");
        assert_eq!(usage.bytes_processed, Some(160_000));
        assert_eq!(usage.media_type.as_deref(), Some("audio/mpeg"));
        assert_eq!(usage.metrics.get("bytes_sent"), Some(&160_000));
        // 160_000 bytes at ~16KB/s ≈ 10_000ms
        assert_eq!(usage.metrics.get("duration_ms"), Some(&10_000));
        assert!(usage.has_data());
    }

    #[test]
    fn test_binary_usage_from_binary_non_audio() {
        let usage = BinaryUsageStrategy::usage_from_binary(50_000, "image/png");
        assert_eq!(usage.bytes_processed, Some(50_000));
        assert_eq!(usage.media_type.as_deref(), Some("image/png"));
        assert!(usage.metrics.get("duration_ms").is_none()); // No duration for images
    }

    #[test]
    fn test_binary_usage_opus_format() {
        let usage = BinaryUsageStrategy::usage_from_binary(80_000, "audio/opus");
        // 80_000 bytes at ~8KB/s ≈ 10_000ms
        assert_eq!(usage.metrics.get("duration_ms"), Some(&10_000));
    }

    #[test]
    fn test_plugin_binary_response() {
        let plugin = UsageExtractorPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.inbound_protocol = "openai".to_string();
        ctx.set("prism.response_content_kind", "binary".to_string());
        ctx.response_headers
            .insert("content-type".to_string(), "audio/mpeg".to_string());

        // Simulate a binary body (e.g., 1KB of TTS audio)
        let body = vec![0u8; 1024];
        plugin.on_response_body(&mut ctx, body).unwrap();

        let usage = ctx.get::<ExtractedUsage>(USAGE_KEY).unwrap();
        assert_eq!(usage.bytes_processed, Some(1024));
        assert_eq!(usage.media_type.as_deref(), Some("audio/mpeg"));
    }

    // ── ExtractedUsage merge with new fields ──

    #[test]
    fn test_merge_binary_fields() {
        let mut a = ExtractedUsage::default();
        let b = ExtractedUsage {
            bytes_processed: Some(5000),
            media_type: Some("audio/mpeg".to_string()),
            ..Default::default()
        };
        a.merge(&b);
        assert_eq!(a.bytes_processed, Some(5000));
        assert_eq!(a.media_type.as_deref(), Some("audio/mpeg"));
    }
}
