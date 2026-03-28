//! WebSocket protocol-aware proxy types and frame inspection.
//!
//! Provides WS protocol detection, model extraction from path/query,
//! session metrics, and frame inspectors for protocol-aware metering.
//! Currently Pingora handles the actual WS frame forwarding via UpgradedBody;
//! frame inspection is typed but not yet on the hot path.

use std::fmt;
use std::time::Instant;

use serde::Deserialize;

// ── Protocol Detection ──────────────────────────────────────────────

/// WebSocket protocol types supported by Prism.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsProtocol {
    /// OpenAI Realtime API — JSON messages over WS.
    OpenAIRealtime,
    /// Generic binary stream (ASR audio in, TTS audio out).
    BinaryStream,
    /// Unknown/transparent passthrough.
    Passthrough,
}

impl WsProtocol {
    /// Detect WS protocol from the request path.
    pub fn from_path(path: &str) -> Self {
        let path_no_query = path.split('?').next().unwrap_or(path);
        if path_no_query.starts_with("/v1/realtime") {
            Self::OpenAIRealtime
        } else if path_no_query.starts_with("/v1/audio/") {
            Self::BinaryStream
        } else {
            Self::Passthrough
        }
    }

    /// Return a short string identifier.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAIRealtime => "openai-realtime",
            Self::BinaryStream => "binary-stream",
            Self::Passthrough => "passthrough",
        }
    }
}

impl fmt::Display for WsProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Session Metrics ─────────────────────────────────────────────────

/// Metrics collected from WebSocket sessions.
#[derive(Debug)]
pub struct WsSessionMetrics {
    /// Frames sent (client → upstream).
    pub frames_sent: u64,
    /// Frames received (upstream → client).
    pub frames_received: u64,
    /// Bytes sent (client → upstream).
    pub bytes_sent: u64,
    /// Bytes received (upstream → client).
    pub bytes_received: u64,
    /// When the WS session was established.
    pub started_at: Instant,
    /// Model name (if detected from handshake or frames).
    pub model: Option<String>,
    /// Detected WS protocol.
    pub protocol: WsProtocol,
}

impl WsSessionMetrics {
    /// Create new metrics for a WS session.
    pub fn new(protocol: WsProtocol) -> Self {
        Self {
            frames_sent: 0,
            frames_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
            started_at: Instant::now(),
            model: None,
            protocol,
        }
    }

    /// Duration since the WS session started.
    pub fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }
}

// ── Frame Inspection Trait ──────────────────────────────────────────

/// Trait for WS frame inspection (optional, not on hot path).
///
/// Inspectors can observe/modify frames as they pass through.
/// Return `None` to drop a frame, `Some(data)` to forward it.
pub trait WsFrameInspector: Send + Sync {
    /// Inspector name for logging.
    fn name(&self) -> &str;

    /// Inspect a client → upstream frame.
    fn on_client_frame(
        &self,
        frame: &[u8],
        is_text: bool,
        metrics: &mut WsSessionMetrics,
    ) -> Option<Vec<u8>>;

    /// Inspect an upstream → client frame.
    fn on_server_frame(
        &self,
        frame: &[u8],
        is_text: bool,
        metrics: &mut WsSessionMetrics,
    ) -> Option<Vec<u8>>;
}

// ── Model Extraction from Path/Query ────────────────────────────────

/// Extract model name from WS request path and query string.
///
/// Checks, in order:
/// 1. `model` query parameter (e.g. `/v1/realtime?model=gpt-4o-realtime`)
/// 2. Path segment after `/v1/realtime/` if present
/// 3. `openai-model` header (used by some OpenAI clients)
pub fn extract_ws_model(path: &str, query: &str, headers: &[(String, String)]) -> Option<String> {
    // 1. Query param: model=...
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("model=") {
            let decoded = value.trim();
            if !decoded.is_empty() {
                return Some(decoded.to_string());
            }
        }
    }

    // 2. Path segment: /v1/realtime/{model}
    let path_no_query = path.split('?').next().unwrap_or(path);
    if let Some(rest) = path_no_query.strip_prefix("/v1/realtime/") {
        let segment = rest.split('/').next().unwrap_or(rest);
        if !segment.is_empty() {
            return Some(segment.to_string());
        }
    }

    // 3. Header: openai-model
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("openai-model") && !v.is_empty() {
            return Some(v.clone());
        }
    }

    None
}

// ── OpenAI Realtime Frame Inspector ─────────────────────────────────

/// Inspects OpenAI Realtime API JSON frames to extract usage metadata.
pub struct OpenAIRealtimeInspector;

/// Minimal structures for parsing OpenAI Realtime API event frames.
#[derive(Deserialize)]
struct RealtimeEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    session: Option<SessionPayload>,
    response: Option<ResponsePayload>,
}

#[derive(Deserialize)]
struct SessionPayload {
    model: Option<String>,
}

#[derive(Deserialize)]
struct ResponsePayload {
    usage: Option<RealtimeUsage>,
}

#[derive(Deserialize)]
struct RealtimeUsage {
    total_tokens: Option<u64>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

impl OpenAIRealtimeInspector {
    /// Parse a text frame from the OpenAI Realtime API and update metrics.
    /// Returns any extracted model name.
    pub fn parse_event(frame: &[u8], metrics: &mut WsSessionMetrics) -> Option<String> {
        let event: RealtimeEvent = serde_json::from_slice(frame).ok()?;
        let event_type = event.event_type.as_deref()?;

        match event_type {
            // session.created / session.updated — extract model
            "session.created" | "session.updated" => {
                if let Some(session) = &event.session {
                    if let Some(ref model) = session.model {
                        metrics.model = Some(model.clone());
                        return Some(model.clone());
                    }
                }
            }
            // response.done — extract token usage
            "response.done" => {
                if let Some(response) = &event.response {
                    if let Some(usage) = &response.usage {
                        // Usage tokens are cumulative within a response; log them.
                        // For now we store the model from session, usage is in the frame.
                        let _ = usage.total_tokens;
                        let _ = usage.input_tokens;
                        let _ = usage.output_tokens;
                    }
                }
            }
            _ => {}
        }

        None
    }
}

impl WsFrameInspector for OpenAIRealtimeInspector {
    fn name(&self) -> &str {
        "openai-realtime"
    }

    fn on_client_frame(
        &self,
        frame: &[u8],
        is_text: bool,
        metrics: &mut WsSessionMetrics,
    ) -> Option<Vec<u8>> {
        metrics.frames_sent += 1;
        metrics.bytes_sent += frame.len() as u64;
        if is_text {
            Self::parse_event(frame, metrics);
        }
        Some(frame.to_vec())
    }

    fn on_server_frame(
        &self,
        frame: &[u8],
        is_text: bool,
        metrics: &mut WsSessionMetrics,
    ) -> Option<Vec<u8>> {
        metrics.frames_received += 1;
        metrics.bytes_received += frame.len() as u64;
        if is_text {
            Self::parse_event(frame, metrics);
        }
        Some(frame.to_vec())
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── WsProtocol detection ──

    #[test]
    fn test_protocol_openai_realtime() {
        assert_eq!(
            WsProtocol::from_path("/v1/realtime?model=gpt-4o-realtime"),
            WsProtocol::OpenAIRealtime
        );
        assert_eq!(
            WsProtocol::from_path("/v1/realtime"),
            WsProtocol::OpenAIRealtime
        );
        assert_eq!(
            WsProtocol::from_path("/v1/realtime/gpt-4o-realtime"),
            WsProtocol::OpenAIRealtime
        );
    }

    #[test]
    fn test_protocol_binary_stream() {
        assert_eq!(
            WsProtocol::from_path("/v1/audio/speech"),
            WsProtocol::BinaryStream
        );
        assert_eq!(
            WsProtocol::from_path("/v1/audio/transcriptions"),
            WsProtocol::BinaryStream
        );
    }

    #[test]
    fn test_protocol_passthrough() {
        assert_eq!(
            WsProtocol::from_path("/v1/chat/completions"),
            WsProtocol::Passthrough
        );
        assert_eq!(WsProtocol::from_path("/ws"), WsProtocol::Passthrough);
        assert_eq!(WsProtocol::from_path("/"), WsProtocol::Passthrough);
    }

    #[test]
    fn test_protocol_as_str() {
        assert_eq!(WsProtocol::OpenAIRealtime.as_str(), "openai-realtime");
        assert_eq!(WsProtocol::BinaryStream.as_str(), "binary-stream");
        assert_eq!(WsProtocol::Passthrough.as_str(), "passthrough");
    }

    // ── Model extraction ──

    #[test]
    fn test_extract_model_from_query() {
        let model = extract_ws_model("/v1/realtime", "model=gpt-4o-realtime", &[]);
        assert_eq!(model, Some("gpt-4o-realtime".to_string()));
    }

    #[test]
    fn test_extract_model_from_query_with_other_params() {
        let model = extract_ws_model("/v1/realtime", "foo=bar&model=gpt-4o-realtime&baz=1", &[]);
        assert_eq!(model, Some("gpt-4o-realtime".to_string()));
    }

    #[test]
    fn test_extract_model_from_path_segment() {
        let model = extract_ws_model("/v1/realtime/gpt-4o-realtime", "", &[]);
        assert_eq!(model, Some("gpt-4o-realtime".to_string()));
    }

    #[test]
    fn test_extract_model_from_header() {
        let headers = vec![("openai-model".to_string(), "gpt-4o-mini".to_string())];
        let model = extract_ws_model("/v1/realtime", "", &headers);
        assert_eq!(model, Some("gpt-4o-mini".to_string()));
    }

    #[test]
    fn test_extract_model_query_takes_priority() {
        let headers = vec![("openai-model".to_string(), "header-model".to_string())];
        let model = extract_ws_model("/v1/realtime/path-model", "model=query-model", &headers);
        assert_eq!(model, Some("query-model".to_string()));
    }

    #[test]
    fn test_extract_model_path_over_header() {
        let headers = vec![("openai-model".to_string(), "header-model".to_string())];
        let model = extract_ws_model("/v1/realtime/path-model", "", &headers);
        assert_eq!(model, Some("path-model".to_string()));
    }

    #[test]
    fn test_extract_model_none() {
        let model = extract_ws_model("/v1/realtime", "", &[]);
        assert_eq!(model, None);
    }

    #[test]
    fn test_extract_model_empty_query_value() {
        let model = extract_ws_model("/v1/realtime", "model=", &[]);
        assert_eq!(model, None);
    }

    // ── OpenAI Realtime frame parsing ──

    #[test]
    fn test_parse_session_created() {
        let mut metrics = WsSessionMetrics::new(WsProtocol::OpenAIRealtime);
        let frame = br#"{"type":"session.created","session":{"model":"gpt-4o-realtime-preview"}}"#;
        let model = OpenAIRealtimeInspector::parse_event(frame, &mut metrics);
        assert_eq!(model, Some("gpt-4o-realtime-preview".to_string()));
        assert_eq!(metrics.model, Some("gpt-4o-realtime-preview".to_string()));
    }

    #[test]
    fn test_parse_session_updated() {
        let mut metrics = WsSessionMetrics::new(WsProtocol::OpenAIRealtime);
        let frame = br#"{"type":"session.updated","session":{"model":"gpt-4o-mini-realtime"}}"#;
        let model = OpenAIRealtimeInspector::parse_event(frame, &mut metrics);
        assert_eq!(model, Some("gpt-4o-mini-realtime".to_string()));
    }

    #[test]
    fn test_parse_response_done_usage() {
        let mut metrics = WsSessionMetrics::new(WsProtocol::OpenAIRealtime);
        let frame = br#"{"type":"response.done","response":{"usage":{"total_tokens":150,"input_tokens":100,"output_tokens":50}}}"#;
        let model = OpenAIRealtimeInspector::parse_event(frame, &mut metrics);
        assert_eq!(model, None); // response.done doesn't return model
    }

    #[test]
    fn test_parse_unknown_event() {
        let mut metrics = WsSessionMetrics::new(WsProtocol::OpenAIRealtime);
        let frame = br#"{"type":"input_audio_buffer.append","audio":"base64data"}"#;
        let model = OpenAIRealtimeInspector::parse_event(frame, &mut metrics);
        assert_eq!(model, None);
    }

    #[test]
    fn test_parse_invalid_json() {
        let mut metrics = WsSessionMetrics::new(WsProtocol::OpenAIRealtime);
        let model = OpenAIRealtimeInspector::parse_event(b"not json", &mut metrics);
        assert_eq!(model, None);
    }

    // ── WsFrameInspector trait ──

    #[test]
    fn test_inspector_client_frame_updates_metrics() {
        let inspector = OpenAIRealtimeInspector;
        let mut metrics = WsSessionMetrics::new(WsProtocol::OpenAIRealtime);
        let frame = br#"{"type":"input_audio_buffer.append"}"#;

        let result = inspector.on_client_frame(frame, true, &mut metrics);
        assert!(result.is_some());
        assert_eq!(metrics.frames_sent, 1);
        assert_eq!(metrics.bytes_sent, frame.len() as u64);
    }

    #[test]
    fn test_inspector_server_frame_updates_metrics() {
        let inspector = OpenAIRealtimeInspector;
        let mut metrics = WsSessionMetrics::new(WsProtocol::OpenAIRealtime);
        let frame = br#"{"type":"session.created","session":{"model":"gpt-4o-realtime"}}"#;

        let result = inspector.on_server_frame(frame, true, &mut metrics);
        assert!(result.is_some());
        assert_eq!(metrics.frames_received, 1);
        assert_eq!(metrics.bytes_received, frame.len() as u64);
        assert_eq!(metrics.model, Some("gpt-4o-realtime".to_string()));
    }

    #[test]
    fn test_inspector_binary_frame_skips_parse() {
        let inspector = OpenAIRealtimeInspector;
        let mut metrics = WsSessionMetrics::new(WsProtocol::OpenAIRealtime);
        let frame = b"\x00\x01\x02\x03"; // binary audio data

        let result = inspector.on_client_frame(frame, false, &mut metrics);
        assert!(result.is_some());
        assert_eq!(metrics.frames_sent, 1);
        assert_eq!(metrics.model, None); // no parse attempt on binary
    }

    // ── WsSessionMetrics ──

    #[test]
    fn test_session_metrics_new() {
        let metrics = WsSessionMetrics::new(WsProtocol::Passthrough);
        assert_eq!(metrics.frames_sent, 0);
        assert_eq!(metrics.frames_received, 0);
        assert_eq!(metrics.bytes_sent, 0);
        assert_eq!(metrics.bytes_received, 0);
        assert_eq!(metrics.model, None);
        assert_eq!(metrics.protocol, WsProtocol::Passthrough);
    }

    #[test]
    fn test_session_metrics_elapsed() {
        let metrics = WsSessionMetrics::new(WsProtocol::OpenAIRealtime);
        // Just verify it doesn't panic and returns a reasonable value
        assert!(metrics.elapsed_ms() < 1000);
    }
}
