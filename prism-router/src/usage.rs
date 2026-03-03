//! Token usage extraction from upstream LLM responses.
//!
//! Parses the OpenAI-compatible `usage` field from chat completion responses.
//! Supports both non-streaming (JSON body) and streaming (SSE `data:` chunks).
//!
//! # OpenAI usage format
//! ```json
//! {
//!   "usage": {
//!     "prompt_tokens": 42,
//!     "completion_tokens": 128,
//!     "total_tokens": 170
//!   }
//! }
//! ```

use serde::Deserialize;

/// Extracted token usage from an upstream LLM response.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

impl TokenUsage {
    /// True if any token count was extracted.
    pub fn has_data(&self) -> bool {
        self.prompt_tokens.is_some()
            || self.completion_tokens.is_some()
            || self.total_tokens.is_some()
    }

    /// Merge another usage into this one (later values win if present).
    pub fn merge(&mut self, other: &TokenUsage) {
        if other.prompt_tokens.is_some() {
            self.prompt_tokens = other.prompt_tokens;
        }
        if other.completion_tokens.is_some() {
            self.completion_tokens = other.completion_tokens;
        }
        if other.total_tokens.is_some() {
            self.total_tokens = other.total_tokens;
        }
    }
}

// ── Internal serde types ──

/// Partial OpenAI response — we only care about the `usage` field.
#[derive(Deserialize)]
struct PartialResponse {
    usage: Option<UsagePayload>,
}

/// The `usage` object inside an OpenAI chat completion response.
#[derive(Deserialize)]
struct UsagePayload {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

// ════════════════════════════════════════════════
// Public API
// ════════════════════════════════════════════════

/// Extract token usage from a complete (non-streaming) JSON response body.
pub fn extract_from_body(body: &[u8]) -> Option<TokenUsage> {
    let text = std::str::from_utf8(body).ok()?;
    extract_from_json(text)
}

/// Extract token usage from a JSON string.
pub fn extract_from_json(json: &str) -> Option<TokenUsage> {
    let parsed: PartialResponse = serde_json::from_str(json).ok()?;
    let usage = parsed.usage?;
    let result = TokenUsage {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
    };
    if result.has_data() {
        Some(result)
    } else {
        None
    }
}

/// Extract token usage from an SSE chunk string.
///
/// SSE chunks look like `data: {...}\n\n`. The usage field may appear in:
/// - The final non-`[DONE]` chunk (OpenAI API behavior with `stream_options.include_usage`)
/// - A dedicated chunk before `data: [DONE]`
///
/// This function scans all `data:` lines in the chunk and returns the last usage found.
pub fn extract_from_sse_chunk(chunk: &str) -> Option<TokenUsage> {
    let mut result: Option<TokenUsage> = None;
    for line in chunk.lines() {
        let data = if let Some(d) = line.strip_prefix("data: ") {
            d
        } else if let Some(d) = line.strip_prefix("data:") {
            d.trim_start()
        } else {
            continue;
        };
        if data == "[DONE]" || data.is_empty() {
            continue;
        }
        if let Some(usage) = extract_from_json(data) {
            result = Some(usage);
        }
    }
    result
}

/// SSE usage accumulator — call `feed()` for each chunk during streaming,
/// then `finish()` to get the final usage.
///
/// Usage typically appears in the last SSE chunk. This accumulator keeps
/// updating as chunks arrive, so the final value is always the most recent.
#[derive(Debug, Default)]
pub struct StreamingUsageAccumulator {
    usage: TokenUsage,
}

impl StreamingUsageAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed an SSE chunk (may contain multiple `data:` lines).
    /// Returns true if usage was found in this chunk.
    pub fn feed(&mut self, chunk: &str) -> bool {
        if let Some(usage) = extract_from_sse_chunk(chunk) {
            self.usage.merge(&usage);
            true
        } else {
            false
        }
    }

    /// Feed raw bytes (convenience for Bytes/Vec<u8>).
    pub fn feed_bytes(&mut self, bytes: &[u8]) -> bool {
        if let Ok(text) = std::str::from_utf8(bytes) {
            self.feed(text)
        } else {
            false
        }
    }

    /// Get the accumulated usage (if any was found).
    pub fn finish(&self) -> Option<TokenUsage> {
        if self.usage.has_data() {
            Some(self.usage.clone())
        } else {
            None
        }
    }
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_from_json_openai() {
        let json = r#"{
            "id": "chatcmpl-abc",
            "object": "chat.completion",
            "model": "gpt-4",
            "choices": [{"message": {"content": "hello"}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 42,
                "completion_tokens": 128,
                "total_tokens": 170
            }
        }"#;
        let usage = extract_from_json(json).unwrap();
        assert_eq!(usage.prompt_tokens, Some(42));
        assert_eq!(usage.completion_tokens, Some(128));
        assert_eq!(usage.total_tokens, Some(170));
    }

    #[test]
    fn test_extract_from_json_no_usage() {
        let json = r#"{"id": "chatcmpl-abc", "model": "gpt-4"}"#;
        assert!(extract_from_json(json).is_none());
    }

    #[test]
    fn test_extract_from_json_empty_usage() {
        let json = r#"{"usage": {}}"#;
        assert!(extract_from_json(json).is_none());
    }

    #[test]
    fn test_extract_from_json_partial_usage() {
        let json = r#"{"usage": {"prompt_tokens": 10}}"#;
        let usage = extract_from_json(json).unwrap();
        assert_eq!(usage.prompt_tokens, Some(10));
        assert_eq!(usage.completion_tokens, None);
        assert_eq!(usage.total_tokens, None);
    }

    #[test]
    fn test_extract_from_body() {
        let body = br#"{"usage": {"prompt_tokens": 5, "completion_tokens": 10, "total_tokens": 15}}"#;
        let usage = extract_from_body(body).unwrap();
        assert_eq!(usage.prompt_tokens, Some(5));
        assert_eq!(usage.completion_tokens, Some(10));
    }

    #[test]
    fn test_extract_from_body_invalid_utf8() {
        let body: &[u8] = &[0xFF, 0xFE, 0x00];
        assert!(extract_from_body(body).is_none());
    }

    #[test]
    fn test_extract_from_sse_chunk_with_usage() {
        let chunk = "data: {\"id\":\"chatcmpl-abc\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"id\":\"chatcmpl-abc\",\"choices\":[],\"usage\":{\"prompt_tokens\":42,\"completion_tokens\":128,\"total_tokens\":170}}\n\ndata: [DONE]\n\n";
        let usage = extract_from_sse_chunk(chunk).unwrap();
        assert_eq!(usage.prompt_tokens, Some(42));
        assert_eq!(usage.completion_tokens, Some(128));
        assert_eq!(usage.total_tokens, Some(170));
    }

    #[test]
    fn test_extract_from_sse_chunk_no_usage() {
        let chunk = "data: {\"id\":\"chatcmpl-abc\",\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n";
        assert!(extract_from_sse_chunk(chunk).is_none());
    }

    #[test]
    fn test_extract_from_sse_done_only() {
        let chunk = "data: [DONE]\n\n";
        assert!(extract_from_sse_chunk(chunk).is_none());
    }

    #[test]
    fn test_streaming_accumulator() {
        let mut acc = StreamingUsageAccumulator::new();
        assert!(!acc.feed("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n"));
        assert!(!acc.feed("data: {\"choices\":[{\"delta\":{\"content\":\" there\"}}]}\n\n"));
        assert!(acc.feed("data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":20,\"total_tokens\":30}}\n\n"));
        assert!(!acc.feed("data: [DONE]\n\n"));

        let usage = acc.finish().unwrap();
        assert_eq!(usage.prompt_tokens, Some(10));
        assert_eq!(usage.completion_tokens, Some(20));
        assert_eq!(usage.total_tokens, Some(30));
    }

    #[test]
    fn test_streaming_accumulator_no_usage() {
        let mut acc = StreamingUsageAccumulator::new();
        acc.feed("data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n");
        acc.feed("data: [DONE]\n\n");
        assert!(acc.finish().is_none());
    }

    #[test]
    fn test_streaming_accumulator_feed_bytes() {
        let mut acc = StreamingUsageAccumulator::new();
        let bytes = b"data: {\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":15,\"total_tokens\":20}}\n\n";
        assert!(acc.feed_bytes(bytes));
        let usage = acc.finish().unwrap();
        assert_eq!(usage.total_tokens, Some(20));
    }

    #[test]
    fn test_merge() {
        let mut a = TokenUsage {
            prompt_tokens: Some(10),
            completion_tokens: None,
            total_tokens: None,
        };
        let b = TokenUsage {
            prompt_tokens: None,
            completion_tokens: Some(20),
            total_tokens: Some(30),
        };
        a.merge(&b);
        assert_eq!(a.prompt_tokens, Some(10));
        assert_eq!(a.completion_tokens, Some(20));
        assert_eq!(a.total_tokens, Some(30));
    }

    #[test]
    fn test_merge_overwrites() {
        let mut a = TokenUsage {
            prompt_tokens: Some(10),
            completion_tokens: Some(5),
            total_tokens: Some(15),
        };
        let b = TokenUsage {
            prompt_tokens: Some(42),
            completion_tokens: None,
            total_tokens: Some(170),
        };
        a.merge(&b);
        assert_eq!(a.prompt_tokens, Some(42));
        assert_eq!(a.completion_tokens, Some(5));
        assert_eq!(a.total_tokens, Some(170));
    }

    #[test]
    fn test_extra_fields_ignored() {
        let json = r#"{
            "usage": {
                "prompt_tokens": 42,
                "completion_tokens": 128,
                "total_tokens": 170,
                "prompt_tokens_details": {"cached_tokens": 10},
                "completion_tokens_details": {"reasoning_tokens": 32}
            }
        }"#;
        let usage = extract_from_json(json).unwrap();
        assert_eq!(usage.prompt_tokens, Some(42));
        assert_eq!(usage.completion_tokens, Some(128));
    }

    #[test]
    fn test_integer_overflow_safe() {
        let json = r#"{"usage": {"prompt_tokens": 18446744073709551615, "completion_tokens": 0}}"#;
        let usage = extract_from_json(json).unwrap();
        assert_eq!(usage.prompt_tokens, Some(u64::MAX));
    }
}
