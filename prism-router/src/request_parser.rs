//! Request parser abstraction — extracts model name from different content types.
//!
//! Supports JSON, multipart/form-data, URL-path, and no-body request formats.

pub use crate::codec::ContentKind;

/// Trait for extracting the model name from different request content types.
pub trait RequestParser: Send + Sync {
    /// What kind of content this parser handles.
    fn content_kind(&self) -> ContentKind;

    /// Extract the model name from the request.
    ///
    /// - `headers`: request headers as (name, value) pairs (lowercase names)
    /// - `body`: raw request body bytes (may be partial for multipart)
    /// - `path`: request URI path
    fn extract_model(
        &self,
        headers: &[(String, String)],
        body: &[u8],
        path: &str,
    ) -> Option<String>;
}

// ════════════════════════════════════════════════
// JSON parser — current behavior
// ════════════════════════════════════════════════

/// Extracts `"model"` field from a JSON request body.
pub struct JsonRequestParser;

impl RequestParser for JsonRequestParser {
    fn content_kind(&self) -> ContentKind {
        ContentKind::Json
    }

    fn extract_model(
        &self,
        _headers: &[(String, String)],
        body: &[u8],
        _path: &str,
    ) -> Option<String> {
        let value: serde_json::Value = serde_json::from_slice(body).ok()?;
        value.get("model")?.as_str().map(|s| s.to_string())
    }
}

// ════════════════════════════════════════════════
// Multipart parser — scan text fields only
// ════════════════════════════════════════════════

/// Extracts `"model"` from a multipart/form-data request by scanning only
/// the text fields (not file upload parts). Reads only the first `scan_limit`
/// bytes to avoid loading large file parts into memory.
pub struct MultipartRequestParser {
    /// Maximum bytes to scan for the model field (default: 8 KiB).
    pub scan_limit: usize,
}

impl Default for MultipartRequestParser {
    fn default() -> Self {
        Self {
            scan_limit: 8 * 1024,
        }
    }
}

impl MultipartRequestParser {
    /// Extract the multipart boundary from Content-Type header.
    fn extract_boundary(headers: &[(String, String)]) -> Option<String> {
        for (name, value) in headers {
            if name == "content-type" {
                // content-type: multipart/form-data; boundary=----WebKitFormBoundary...
                for part in value.split(';') {
                    let trimmed = part.trim();
                    if let Some(b) = trimmed.strip_prefix("boundary=") {
                        return Some(b.trim_matches('"').to_string());
                    }
                }
            }
        }
        None
    }
}

impl RequestParser for MultipartRequestParser {
    fn content_kind(&self) -> ContentKind {
        ContentKind::Multipart
    }

    fn extract_model(
        &self,
        headers: &[(String, String)],
        body: &[u8],
        _path: &str,
    ) -> Option<String> {
        let boundary = Self::extract_boundary(headers)?;

        // Only scan the first scan_limit bytes
        let scan_end = body.len().min(self.scan_limit);
        let scan_bytes = &body[..scan_end];
        let scan_str = std::str::from_utf8(scan_bytes).ok()?;

        let boundary_marker = format!("--{}", boundary);

        // Split on boundary markers and look for "model" text field
        for part in scan_str.split(&boundary_marker) {
            // Skip if this looks like a file upload (has filename=)
            if part.contains("filename=") {
                continue;
            }

            // Check if this part is the "model" field
            if part.contains("name=\"model\"") || part.contains("name=model") {
                // The value is after the double CRLF (or double LF) that separates
                // headers from body in a multipart part
                let body_start = if let Some(pos) = part.find("\r\n\r\n") {
                    pos + 4
                } else if let Some(pos) = part.find("\n\n") {
                    pos + 2
                } else {
                    continue;
                };
                let value = part[body_start..].trim();
                // Strip trailing boundary markers or dashes
                let value = value.trim_end_matches('-').trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }

        None
    }
}

// ════════════════════════════════════════════════
// URL path parser — extract model from path
// ════════════════════════════════════════════════

/// Extracts model from URL path segments.
/// Supports patterns like `/models/{model}:generateContent` (Gemini-style).
pub struct UrlRequestParser;

impl RequestParser for UrlRequestParser {
    fn content_kind(&self) -> ContentKind {
        ContentKind::None
    }

    fn extract_model(
        &self,
        _headers: &[(String, String)],
        _body: &[u8],
        path: &str,
    ) -> Option<String> {
        // Pattern: /v1/models/{model}:action or /models/{model}:action
        let segments: Vec<&str> = path.split('/').collect();
        for (i, seg) in segments.iter().enumerate() {
            if *seg == "models" {
                if let Some(next) = segments.get(i + 1) {
                    // Strip action suffix like ":generateContent"
                    let model = next.split(':').next().unwrap_or(next);
                    if !model.is_empty() {
                        return Some(model.to_string());
                    }
                }
            }
        }
        None
    }
}

// ════════════════════════════════════════════════
// No-body parser
// ════════════════════════════════════════════════

/// Parser for requests that have no body (WebSocket upgrades, GET requests).
pub struct NoBodyParser;

impl RequestParser for NoBodyParser {
    fn content_kind(&self) -> ContentKind {
        ContentKind::None
    }

    fn extract_model(
        &self,
        _headers: &[(String, String)],
        _body: &[u8],
        _path: &str,
    ) -> Option<String> {
        None
    }
}

// ════════════════════════════════════════════════
// Detect the right parser from headers
// ════════════════════════════════════════════════

/// Return the appropriate `ContentKind` based on the request's Content-Type header.
pub fn detect_content_kind(headers: &[(String, String)]) -> ContentKind {
    for (name, value) in headers {
        if name == "content-type" {
            let ct = value.to_lowercase();
            if ct.starts_with("application/json") {
                return ContentKind::Json;
            } else if ct.starts_with("multipart/form-data") {
                return ContentKind::Multipart;
            } else if ct.starts_with("application/octet-stream")
                || ct.starts_with("audio/")
                || ct.starts_with("image/")
                || ct.starts_with("video/")
            {
                return ContentKind::Binary;
            }
        }
    }
    ContentKind::None
}

/// Return a boxed `RequestParser` appropriate for the given content kind.
pub fn parser_for_content_kind(kind: ContentKind) -> Box<dyn RequestParser> {
    match kind {
        ContentKind::Json => Box::new(JsonRequestParser),
        ContentKind::Multipart => Box::new(MultipartRequestParser::default()),
        ContentKind::Binary => Box::new(NoBodyParser),
        ContentKind::None => Box::new(NoBodyParser),
    }
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── JSON parser tests ──

    #[test]
    fn test_json_parser_extracts_model() {
        let parser = JsonRequestParser;
        let body = br#"{"model": "gpt-4o", "messages": []}"#;
        assert_eq!(
            parser.extract_model(&[], body, "/v1/chat/completions"),
            Some("gpt-4o".to_string())
        );
    }

    #[test]
    fn test_json_parser_no_model() {
        let parser = JsonRequestParser;
        let body = br#"{"messages": []}"#;
        assert_eq!(
            parser.extract_model(&[], body, "/v1/chat/completions"),
            None
        );
    }

    #[test]
    fn test_json_parser_invalid_json() {
        let parser = JsonRequestParser;
        assert_eq!(
            parser.extract_model(&[], b"not json", "/v1/chat/completions"),
            None
        );
    }

    // ── Multipart parser tests ──

    #[test]
    fn test_multipart_parser_extracts_model() {
        let parser = MultipartRequestParser::default();
        let headers = vec![(
            "content-type".to_string(),
            "multipart/form-data; boundary=----boundary123".to_string(),
        )];

        let body = b"------boundary123\r\n\
            Content-Disposition: form-data; name=\"model\"\r\n\
            \r\n\
            whisper-1\r\n\
            ------boundary123\r\n\
            Content-Disposition: form-data; name=\"file\"; filename=\"audio.mp3\"\r\n\
            Content-Type: audio/mpeg\r\n\
            \r\n\
            <binary audio data>\r\n\
            ------boundary123--";

        assert_eq!(
            parser.extract_model(&headers, body, "/v1/audio/transcriptions"),
            Some("whisper-1".to_string())
        );
    }

    #[test]
    fn test_multipart_parser_no_boundary() {
        let parser = MultipartRequestParser::default();
        let headers = vec![(
            "content-type".to_string(),
            "multipart/form-data".to_string(),
        )];
        assert_eq!(
            parser.extract_model(&headers, b"", "/v1/audio/transcriptions"),
            None
        );
    }

    #[test]
    fn test_multipart_parser_no_model_field() {
        let parser = MultipartRequestParser::default();
        let headers = vec![(
            "content-type".to_string(),
            "multipart/form-data; boundary=----b".to_string(),
        )];
        let body = b"------b\r\n\
            Content-Disposition: form-data; name=\"file\"; filename=\"audio.mp3\"\r\n\
            \r\n\
            data\r\n\
            ------b--";
        assert_eq!(
            parser.extract_model(&headers, body, "/v1/audio/transcriptions"),
            None
        );
    }

    // ── URL parser tests ──

    #[test]
    fn test_url_parser_gemini_style() {
        let parser = UrlRequestParser;
        assert_eq!(
            parser.extract_model(&[], b"", "/v1/models/gemini-pro:generateContent"),
            Some("gemini-pro".to_string())
        );
    }

    #[test]
    fn test_url_parser_plain_model() {
        let parser = UrlRequestParser;
        assert_eq!(
            parser.extract_model(&[], b"", "/models/gpt-4"),
            Some("gpt-4".to_string())
        );
    }

    #[test]
    fn test_url_parser_no_models_segment() {
        let parser = UrlRequestParser;
        assert_eq!(parser.extract_model(&[], b"", "/v1/chat/completions"), None);
    }

    // ── NoBody parser tests ──

    #[test]
    fn test_no_body_parser() {
        let parser = NoBodyParser;
        assert_eq!(parser.extract_model(&[], b"", "/any"), None);
    }

    // ── Content kind detection ──

    #[test]
    fn test_detect_content_kind_json() {
        let headers = vec![(
            "content-type".to_string(),
            "application/json; charset=utf-8".to_string(),
        )];
        assert_eq!(detect_content_kind(&headers), ContentKind::Json);
    }

    #[test]
    fn test_detect_content_kind_multipart() {
        let headers = vec![(
            "content-type".to_string(),
            "multipart/form-data; boundary=abc".to_string(),
        )];
        assert_eq!(detect_content_kind(&headers), ContentKind::Multipart);
    }

    #[test]
    fn test_detect_content_kind_binary() {
        let headers = vec![("content-type".to_string(), "audio/mpeg".to_string())];
        assert_eq!(detect_content_kind(&headers), ContentKind::Binary);

        let headers = vec![(
            "content-type".to_string(),
            "application/octet-stream".to_string(),
        )];
        assert_eq!(detect_content_kind(&headers), ContentKind::Binary);
    }

    #[test]
    fn test_detect_content_kind_none() {
        assert_eq!(detect_content_kind(&[]), ContentKind::None);
    }

    // ── parser_for_content_kind ──

    #[test]
    fn test_parser_for_content_kind() {
        assert_eq!(
            parser_for_content_kind(ContentKind::Json).content_kind(),
            ContentKind::Json
        );
        assert_eq!(
            parser_for_content_kind(ContentKind::Multipart).content_kind(),
            ContentKind::Multipart
        );
        assert_eq!(
            parser_for_content_kind(ContentKind::Binary).content_kind(),
            ContentKind::None
        );
        assert_eq!(
            parser_for_content_kind(ContentKind::None).content_kind(),
            ContentKind::None
        );
    }
}
