//! Access Log — Envoy-style configurable access logging with `%COMMAND_OPERATOR%` format.
//!
//! Supports JSON and text output formats with selectable fields.
//! Operators are parsed at startup for zero-allocation per-request formatting.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::config::types::{AccessLogConfig, AccessLogFormat, AccessLogOutput};

// ── Command Operators ──

/// A single command operator that extracts a value from request context.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandOperator {
    /// Request start time. Optional chrono strftime format string.
    StartTime(Option<String>),
    /// HTTP method (GET, POST, etc.)
    Method,
    /// Request URI path
    Path,
    /// Detected LLM protocol (openai_chat, anthropic, etc.)
    Protocol,
    /// HTTP response status code
    ResponseCode,
    /// Total request duration in milliseconds
    DurationMs,
    /// Total request duration in microseconds
    DurationUs,
    /// Request body bytes received
    BytesReceived,
    /// Response body bytes sent
    BytesSent,
    /// Selected variant ID from routing decision
    Variant,
    /// Selected provider ID from routing decision
    Provider,
    /// Selected model ID from routing decision
    Model,
    /// Upstream base URL
    UpstreamHost,
    /// Whether response was streamed
    Streaming,
    /// Whether protocol conversion was active
    Conversion,
    /// Routing decision time in microseconds
    DecisionTimeUs,
    /// Arbitrary request header value
    RequestHeader(String),
    /// Arbitrary response header value
    ResponseHeader(String),
    /// Input token count (if available from upstream response)
    InputTokens,
    /// Output token count (if available from upstream response)
    OutputTokens,
    /// Whether a rate limit rule matched
    RatelimitMatched,
    /// Rate limit rule name that matched (if any)
    RatelimitRule,
    /// Error message (if request failed)
    Error,
    /// Routing strategy used
    RouteStrategy,
    /// Route decision score/reason
    RouteScore,
    /// Number of candidate variants considered
    RouteCandidates,
    /// Upstream response time (time to first byte) in ms
    UpstreamResponseTimeMs,
    /// Whether the request used the reqwest fallback path
    ReqwestFallback,
    /// Response flags — short codes for error/termination reasons (e.g. RL, NR, UF, UC, UT)
    ResponseFlags,
    /// Downstream (client) remote address (ip:port)
    DownstreamRemoteAddress,
    /// Time to receive the complete request body (ms)
    RequestDurationMs,
    /// Time to first byte from upstream (ms)
    ResponseDurationMs,
    /// Number of upstream request attempts (retries)
    UpstreamRequestAttemptCount,
    /// Request start time in local timezone
    StartTimeLocal(Option<String>),
    /// Time when log entry is emitted
    EmitTime(Option<String>),
    /// Request headers total bytes (uncompressed)
    RequestHeadersBytes,
    /// Response headers total bytes (uncompressed)
    ResponseHeadersBytes,
    /// Duration from TTFB to last byte sent downstream (ms)
    ResponseTxDurationMs,
    /// Upstream protocol (outbound_protocol)
    UpstreamProtocol,
    /// Prism listening address (downstream local address)
    DownstreamLocalAddress,
    /// TLS/connection failure reason for upstream
    UpstreamTransportFailureReason,
    /// Connection termination details
    ConnectionTerminationDetails,
    /// Response code details — who set it and why
    ResponseCodeDetails,
    /// Pool name (virtual cluster equivalent)
    PoolName,
    /// Request unique ID (from x-request-id header or generated)
    RequestId,
    /// Downstream TLS version (TLSv1.2, TLSv1.3, etc.)
    DownstreamTlsVersion,
}

impl CommandOperator {
    /// JSON key name for this operator.
    pub fn json_key(&self) -> String {
        match self {
            Self::StartTime(_) => "start_time".into(),
            Self::Method => "method".into(),
            Self::Path => "path".into(),
            Self::Protocol => "protocol".into(),
            Self::ResponseCode => "response_code".into(),
            Self::DurationMs => "duration_ms".into(),
            Self::DurationUs => "duration_us".into(),
            Self::BytesReceived => "bytes_received".into(),
            Self::BytesSent => "bytes_sent".into(),
            Self::Variant => "variant".into(),
            Self::Provider => "provider".into(),
            Self::Model => "model".into(),
            Self::UpstreamHost => "upstream_host".into(),
            Self::Streaming => "streaming".into(),
            Self::Conversion => "conversion".into(),
            Self::DecisionTimeUs => "decision_time_us".into(),
            Self::RequestHeader(name) => format!("request_header_{}", name),
            Self::ResponseHeader(name) => format!("response_header_{}", name),
            Self::InputTokens => "input_tokens".into(),
            Self::OutputTokens => "output_tokens".into(),
            Self::RatelimitMatched => "ratelimit_matched".into(),
            Self::RatelimitRule => "ratelimit_rule".into(),
            Self::Error => "error".into(),
            Self::RouteStrategy => "route_strategy".into(),
            Self::RouteScore => "route_score".into(),
            Self::RouteCandidates => "route_candidates".into(),
            Self::UpstreamResponseTimeMs => "upstream_response_time_ms".into(),
            Self::ReqwestFallback => "reqwest_fallback".into(),
            Self::ResponseFlags => "response_flags".into(),
            Self::DownstreamRemoteAddress => "downstream_remote_address".into(),
            Self::RequestDurationMs => "request_duration_ms".into(),
            Self::ResponseDurationMs => "response_duration_ms".into(),
            Self::UpstreamRequestAttemptCount => "upstream_request_attempt_count".into(),
            Self::StartTimeLocal(_) => "start_time_local".into(),
            Self::EmitTime(_) => "emit_time".into(),
            Self::RequestHeadersBytes => "request_headers_bytes".into(),
            Self::ResponseHeadersBytes => "response_headers_bytes".into(),
            Self::ResponseTxDurationMs => "response_tx_duration_ms".into(),
            Self::UpstreamProtocol => "upstream_protocol".into(),
            Self::DownstreamLocalAddress => "downstream_local_address".into(),
            Self::UpstreamTransportFailureReason => "upstream_transport_failure_reason".into(),
            Self::ConnectionTerminationDetails => "connection_termination_details".into(),
            Self::ResponseCodeDetails => "response_code_details".into(),
            Self::PoolName => "pool_name".into(),
            Self::RequestId => "request_id".into(),
            Self::DownstreamTlsVersion => "downstream_tls_version".into(),
        }
    }

    /// All default operators for JSON output when no field selection is specified.
    pub fn defaults() -> Vec<Self> {
        vec![
            Self::StartTime(None),
            Self::Method,
            Self::Path,
            Self::Protocol,
            Self::ResponseCode,
            Self::DurationMs,
            Self::Variant,
            Self::Provider,
            Self::Model,
            Self::UpstreamHost,
            Self::Streaming,
            Self::Conversion,
            Self::DecisionTimeUs,
            Self::RouteStrategy,
            Self::RouteCandidates,
            Self::UpstreamResponseTimeMs,
            Self::ReqwestFallback,
            Self::ResponseFlags,
            Self::DownstreamRemoteAddress,
            Self::RequestDurationMs,
            Self::ResponseDurationMs,
            Self::UpstreamRequestAttemptCount,
            Self::RequestHeadersBytes,
            Self::ResponseHeadersBytes,
            Self::ResponseTxDurationMs,
            Self::UpstreamProtocol,
            Self::DownstreamLocalAddress,
            Self::DownstreamTlsVersion,
            Self::ResponseCodeDetails,
            Self::PoolName,
            Self::RequestId,
            Self::InputTokens,
            Self::OutputTokens,
            Self::RatelimitMatched,
            Self::RatelimitRule,
            Self::Error,
        ]
    }
}

// ── Format Parsing ──

/// A segment of the access log format string.
#[derive(Debug, Clone, PartialEq)]
pub enum FormatSegment {
    /// Literal text, copied verbatim.
    Literal(String),
    /// A command operator to be resolved at log time.
    Operator(CommandOperator),
}

/// Parse a `%OPERATOR%` format string into segments.
///
/// Supports:
/// - `%START_TIME%` or `%START_TIME(format)%` (format may contain `%` chars)
/// - `%REQUEST_HEADER(name)%`
/// - `%RESPONSE_HEADER(name)%`
/// - All other `%NAME%` operators
pub fn parse_format_string(s: &str) -> Vec<FormatSegment> {
    let mut segments = Vec::new();
    let mut rest = s;

    while !rest.is_empty() {
        if let Some(start) = rest.find('%') {
            // Push literal before the %
            if start > 0 {
                segments.push(FormatSegment::Literal(rest[..start].to_string()));
            }
            rest = &rest[start + 1..];

            // For parameterized operators like START_TIME(%Y-%m-%d)%,
            // we need to find the closing )% pattern, not just the next %.
            if let Some(paren_pos) = rest.find('(') {
                // Check if there's a closing )% somewhere
                let name_part = &rest[..paren_pos];
                if is_parameterized_operator(name_part) {
                    // Find ")%" after the opening paren
                    let after_paren = &rest[paren_pos + 1..];
                    if let Some(close_pos) = after_paren.find(")%") {
                        let param = &after_paren[..close_pos];
                        let full_end = paren_pos + 1 + close_pos + 2; // past )%
                        if let Some(op) = parse_operator_with_param(name_part, param) {
                            segments.push(FormatSegment::Operator(op));
                        } else {
                            segments.push(FormatSegment::Literal(format!(
                                "%{}%",
                                &rest[..full_end - 1]
                            )));
                        }
                        rest = &rest[full_end..];
                        continue;
                    }
                }
            }

            // Simple operator: find closing %
            if let Some(end) = rest.find('%') {
                let token = &rest[..end];
                if let Some(op) = parse_operator(token) {
                    segments.push(FormatSegment::Operator(op));
                } else {
                    // Unknown operator — emit as literal including the % delimiters
                    segments.push(FormatSegment::Literal(format!("%{}%", token)));
                }
                rest = &rest[end + 1..];
            } else {
                // No closing % — treat rest as literal
                segments.push(FormatSegment::Literal(format!("%{}", rest)));
                break;
            }
        } else {
            segments.push(FormatSegment::Literal(rest.to_string()));
            break;
        }
    }

    segments
}

/// Check if a name is a parameterized operator (has a `(param)` form).
fn is_parameterized_operator(name: &str) -> bool {
    matches!(
        name,
        "START_TIME" | "START_TIME_LOCAL" | "EMIT_TIME" | "REQUEST_HEADER" | "RESPONSE_HEADER"
    )
}

/// Parse a parameterized operator with its parameter value.
fn parse_operator_with_param(name: &str, param: &str) -> Option<CommandOperator> {
    match name {
        "START_TIME" => Some(CommandOperator::StartTime(Some(param.to_string()))),
        "START_TIME_LOCAL" => Some(CommandOperator::StartTimeLocal(Some(param.to_string()))),
        "EMIT_TIME" => Some(CommandOperator::EmitTime(Some(param.to_string()))),
        "REQUEST_HEADER" => Some(CommandOperator::RequestHeader(param.to_lowercase())),
        "RESPONSE_HEADER" => Some(CommandOperator::ResponseHeader(param.to_lowercase())),
        _ => None,
    }
}

/// Parse a single operator token (text between % delimiters).
fn parse_operator(token: &str) -> Option<CommandOperator> {
    // Handle parameterized operators: NAME(param)
    if let Some(paren_start) = token.find('(') {
        if token.ends_with(')') {
            let name = &token[..paren_start];
            let param = &token[paren_start + 1..token.len() - 1];
            return match name {
                "START_TIME" => Some(CommandOperator::StartTime(Some(param.to_string()))),
                "REQUEST_HEADER" => Some(CommandOperator::RequestHeader(param.to_lowercase())),
                "RESPONSE_HEADER" => Some(CommandOperator::ResponseHeader(param.to_lowercase())),
                _ => None,
            };
        }
    }

    match token {
        "START_TIME" => Some(CommandOperator::StartTime(None)),
        "METHOD" => Some(CommandOperator::Method),
        "PATH" => Some(CommandOperator::Path),
        "PROTOCOL" => Some(CommandOperator::Protocol),
        "RESPONSE_CODE" => Some(CommandOperator::ResponseCode),
        "DURATION_MS" => Some(CommandOperator::DurationMs),
        "DURATION_US" => Some(CommandOperator::DurationUs),
        "BYTES_RECEIVED" => Some(CommandOperator::BytesReceived),
        "BYTES_SENT" => Some(CommandOperator::BytesSent),
        "VARIANT" => Some(CommandOperator::Variant),
        "PROVIDER" => Some(CommandOperator::Provider),
        "MODEL" => Some(CommandOperator::Model),
        "UPSTREAM_HOST" => Some(CommandOperator::UpstreamHost),
        "STREAMING" => Some(CommandOperator::Streaming),
        "CONVERSION" => Some(CommandOperator::Conversion),
        "DECISION_TIME_US" => Some(CommandOperator::DecisionTimeUs),
        "INPUT_TOKENS" => Some(CommandOperator::InputTokens),
        "OUTPUT_TOKENS" => Some(CommandOperator::OutputTokens),
        "RATELIMIT_MATCHED" => Some(CommandOperator::RatelimitMatched),
        "RATELIMIT_RULE" => Some(CommandOperator::RatelimitRule),
        "ERROR" => Some(CommandOperator::Error),
        "ROUTE_STRATEGY" => Some(CommandOperator::RouteStrategy),
        "ROUTE_SCORE" => Some(CommandOperator::RouteScore),
        "ROUTE_CANDIDATES" => Some(CommandOperator::RouteCandidates),
        "UPSTREAM_RESPONSE_TIME_MS" => Some(CommandOperator::UpstreamResponseTimeMs),
        "REQWEST_FALLBACK" => Some(CommandOperator::ReqwestFallback),
        "RESPONSE_FLAGS" => Some(CommandOperator::ResponseFlags),
        "DOWNSTREAM_REMOTE_ADDRESS" => Some(CommandOperator::DownstreamRemoteAddress),
        "REQUEST_DURATION_MS" => Some(CommandOperator::RequestDurationMs),
        "RESPONSE_DURATION_MS" => Some(CommandOperator::ResponseDurationMs),
        "UPSTREAM_REQUEST_ATTEMPT_COUNT" => Some(CommandOperator::UpstreamRequestAttemptCount),
        "START_TIME_LOCAL" => Some(CommandOperator::StartTimeLocal(None)),
        "EMIT_TIME" => Some(CommandOperator::EmitTime(None)),
        "REQUEST_HEADERS_BYTES" => Some(CommandOperator::RequestHeadersBytes),
        "RESPONSE_HEADERS_BYTES" => Some(CommandOperator::ResponseHeadersBytes),
        "RESPONSE_TX_DURATION_MS" => Some(CommandOperator::ResponseTxDurationMs),
        "UPSTREAM_PROTOCOL" => Some(CommandOperator::UpstreamProtocol),
        "DOWNSTREAM_LOCAL_ADDRESS" => Some(CommandOperator::DownstreamLocalAddress),
        "UPSTREAM_TRANSPORT_FAILURE_REASON" => {
            Some(CommandOperator::UpstreamTransportFailureReason)
        }
        "CONNECTION_TERMINATION_DETAILS" => Some(CommandOperator::ConnectionTerminationDetails),
        "RESPONSE_CODE_DETAILS" => Some(CommandOperator::ResponseCodeDetails),
        "POOL_NAME" => Some(CommandOperator::PoolName),
        "REQUEST_ID" => Some(CommandOperator::RequestId),
        "DOWNSTREAM_TLS_VERSION" => Some(CommandOperator::DownstreamTlsVersion),
        _ => None,
    }
}

/// Parse a field spec like `"%VARIANT%"` into a CommandOperator.
fn parse_field_spec(spec: &str) -> Option<CommandOperator> {
    let trimmed = spec.trim();
    if trimmed.starts_with('%') && trimmed.ends_with('%') {
        parse_operator(&trimmed[1..trimmed.len() - 1])
    } else {
        None
    }
}

// ── Access Log Data ──

/// All data available for access log formatting.
/// Constructed per-request from the proxy context and session.
#[derive(Debug, Clone)]
pub struct AccessLogData {
    pub start_time: DateTime<Utc>,
    pub method: String,
    pub path: String,
    pub protocol: String,
    pub response_code: u16,
    pub duration_ms: u64,
    pub duration_us: u64,
    pub bytes_received: Option<u64>,
    pub bytes_sent: Option<u64>,
    pub variant: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub upstream_host: Option<String>,
    pub streaming: Option<bool>,
    pub conversion: bool,
    pub decision_time_us: Option<u64>,
    pub request_headers: Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub ratelimit_matched: bool,
    pub ratelimit_rule: Option<String>,
    pub error: Option<String>,
    pub route_strategy: Option<String>,
    pub route_score: Option<f64>,
    pub route_candidates: Option<u32>,
    pub upstream_response_time_ms: Option<u64>,
    pub reqwest_fallback: bool,
    /// Response flags — short codes like RL (rate limited), NR (no route),
    /// UF (upstream failure), UC (upstream connection termination),
    /// UT (upstream timeout), DC (downstream disconnect).
    pub response_flags: Option<String>,
    /// Downstream client address (ip:port).
    pub downstream_remote_address: Option<String>,
    /// Time to receive the complete request from downstream (ms).
    pub request_duration_ms: Option<u64>,
    /// Time to first byte from upstream — TTFB (ms).
    pub response_duration_ms: Option<u64>,
    /// Number of upstream request attempts (1 = no retry).
    pub upstream_request_attempt_count: Option<u32>,
    /// Emit time (when log entry is written, not request start).
    pub emit_time: DateTime<Utc>,
    /// Request headers total bytes.
    pub request_headers_bytes: Option<u64>,
    /// Response headers total bytes.
    pub response_headers_bytes: Option<u64>,
    /// Duration from TTFB to last byte sent downstream (ms).
    pub response_tx_duration_ms: Option<u64>,
    /// Upstream/outbound protocol.
    pub upstream_protocol: Option<String>,
    /// Prism listening address (downstream local).
    pub downstream_local_address: Option<String>,
    /// Upstream TLS/transport failure reason.
    pub upstream_transport_failure_reason: Option<String>,
    /// Connection termination details.
    pub connection_termination_details: Option<String>,
    /// Response code details — who set it (prism/upstream) and why.
    pub response_code_details: Option<String>,
    /// Pool name (virtual cluster).
    pub pool_name: Option<String>,
    /// Request unique ID.
    pub request_id: Option<String>,
    /// Downstream TLS version.
    pub downstream_tls_version: Option<String>,
}

impl Default for AccessLogData {
    fn default() -> Self {
        Self {
            start_time: Utc::now(),
            method: String::new(),
            path: String::new(),
            protocol: String::new(),
            response_code: 0,
            duration_ms: 0,
            duration_us: 0,
            bytes_received: None,
            bytes_sent: None,
            variant: None,
            provider: None,
            model: None,
            upstream_host: None,
            streaming: None,
            conversion: false,
            decision_time_us: None,
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            input_tokens: None,
            output_tokens: None,
            ratelimit_matched: false,
            ratelimit_rule: None,
            error: None,
            route_strategy: None,
            route_score: None,
            route_candidates: None,
            upstream_response_time_ms: None,
            reqwest_fallback: false,
            response_flags: None,
            downstream_remote_address: None,
            request_duration_ms: None,
            response_duration_ms: None,
            upstream_request_attempt_count: None,
            emit_time: Utc::now(),
            request_headers_bytes: None,
            response_headers_bytes: None,
            response_tx_duration_ms: None,
            upstream_protocol: None,
            downstream_local_address: None,
            upstream_transport_failure_reason: None,
            connection_termination_details: None,
            response_code_details: None,
            pool_name: None,
            request_id: None,
            downstream_tls_version: None,
        }
    }
}

// ── Value Resn ──

/// Resolve an operator to a string value for text output.
/// Returns `None` for missing/unavailable values (rendered as "-").
fn resolve_text(op: &CommandOperator, data: &AccessLogData) -> Option<String> {
    match op {
        CommandOperator::StartTime(fmt) => {
            let format_str = fmt.as_deref().unwrap_or("%Y-%m-%dT%H:%M:%S%.3fZ");
            Some(data.start_time.format(format_str).to_string())
        }
        CommandOperator::Method => Some(data.method.clone()),
        CommandOperator::Path => Some(data.path.clone()),
        CommandOperator::Protocol => Some(data.protocol.clone()),
        CommandOperator::ResponseCode => Some(data.response_code.to_string()),
        CommandOperator::DurationMs => Some(data.duration_ms.to_string()),
        CommandOperator::DurationUs => Some(data.duration_us.to_string()),
        CommandOperator::BytesReceived => data.bytes_received.map(|v| v.to_string()),
        CommandOperator::BytesSent => data.bytes_sent.map(|v| v.to_string()),
        CommandOperator::Variant => data.variant.clone(),
        CommandOperator::Provider => data.provider.clone(),
        CommandOperator::Model => data.model.clone(),
        CommandOperator::UpstreamHost => data.upstream_host.clone(),
        CommandOperator::Streaming => data.streaming.map(|v| v.to_string()),
        CommandOperator::Conversion => Some(data.conversion.to_string()),
        CommandOperator::DecisionTimeUs => data.decision_time_us.map(|v| v.to_string()),
        CommandOperator::RequestHeader(name) => data
            .request_headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone()),
        CommandOperator::ResponseHeader(name) => data
            .response_headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone()),
        CommandOperator::InputTokens => data.input_tokens.map(|v| v.to_string()),
        CommandOperator::OutputTokens => data.output_tokens.map(|v| v.to_string()),
        CommandOperator::RatelimitMatched => Some(data.ratelimit_matched.to_string()),
        CommandOperator::RatelimitRule => data.ratelimit_rule.clone(),
        CommandOperator::Error => data.error.clone(),
        CommandOperator::RouteStrategy => data.route_strategy.clone(),
        CommandOperator::RouteScore => data.route_score.map(|v| format!("{:.4}", v)),
        CommandOperator::RouteCandidates => data.route_candidates.map(|v| v.to_string()),
        CommandOperator::UpstreamResponseTimeMs => {
            data.upstream_response_time_ms.map(|v| v.to_string())
        }
        CommandOperator::ReqwestFallback => Some(data.reqwest_fallback.to_string()),
        CommandOperator::ResponseFlags => data.response_flags.clone(),
        CommandOperator::DownstreamRemoteAddress => data.downstream_remote_address.clone(),
        CommandOperator::RequestDurationMs => data.request_duration_ms.map(|v| v.to_string()),
        CommandOperator::ResponseDurationMs => data.response_duration_ms.map(|v| v.to_string()),
        CommandOperator::UpstreamRequestAttemptCount => {
            data.upstream_request_attempt_count.map(|v| v.to_string())
        }
        CommandOperator::StartTimeLocal(fmt) => {
            let local = data.start_time.with_timezone(&chrono::Local);
            let format_str = fmt.as_deref().unwrap_or("%Y-%m-%dT%H:%M:%S%.3f%z");
            Some(local.format(format_str).to_string())
        }
        CommandOperator::EmitTime(fmt) => {
            let format_str = fmt.as_deref().unwrap_or("%Y-%m-%dT%H:%M:%S%.3fZ");
            Some(data.emit_time.format(format_str).to_string())
        }
        CommandOperator::RequestHeadersBytes => data.request_headers_bytes.map(|v| v.to_string()),
        CommandOperator::ResponseHeadersBytes => data.response_headers_bytes.map(|v| v.to_string()),
        CommandOperator::ResponseTxDurationMs => {
            data.response_tx_duration_ms.map(|v| v.to_string())
        }
        CommandOperator::UpstreamProtocol => data.upstream_protocol.clone(),
        CommandOperator::DownstreamLocalAddress => data.downstream_local_address.clone(),
        CommandOperator::UpstreamTransportFailureReason => {
            data.upstream_transport_failure_reason.clone()
        }
        CommandOperator::ConnectionTerminationDetails => {
            data.connection_termination_details.clone()
        }
        CommandOperator::ResponseCodeDetails => data.response_code_details.clone(),
        CommandOperator::PoolName => data.pool_name.clone(),
        CommandOperator::RequestId => data.request_id.clone(),
        CommandOperator::DownstreamTlsVersion => data.downstream_tls_version.clone(),
    }
}

/// Resolve an operator to a typed JSON value.
fn resolve_json(op: &CommandOperator, data: &AccessLogData) -> Value {
    match op {
        CommandOperator::StartTime(fmt) => {
            let format_str = fmt.as_deref().unwrap_or("%Y-%m-%dT%H:%M:%S%.3fZ");
            Value::String(data.start_time.format(format_str).to_string())
        }
        CommandOperator::Method => Value::String(data.method.clone()),
        CommandOperator::Path => Value::String(data.path.clone()),
        CommandOperator::Protocol => Value::String(data.protocol.clone()),
        CommandOperator::ResponseCode => Value::Number(data.response_code.into()),
        CommandOperator::DurationMs => Value::Number(data.duration_ms.into()),
        CommandOperator::DurationUs => Value::Number(data.duration_us.into()),
        CommandOperator::BytesReceived => match data.bytes_received {
            Some(v) => Value::Number(v.into()),
            None => Value::Null,
        },
        CommandOperator::BytesSent => match data.bytes_sent {
            Some(v) => Value::Number(v.into()),
            None => Value::Null,
        },
        CommandOperator::Variant => match &data.variant {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::Provider => match &data.provider {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::Model => match &data.model {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::UpstreamHost => match &data.upstream_host {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::Streaming => match data.streaming {
            Some(v) => Value::Bool(v),
            None => Value::Null,
        },
        CommandOperator::Conversion => Value::Bool(data.conversion),
        CommandOperator::DecisionTimeUs => match data.decision_time_us {
            Some(v) => Value::Number(v.into()),
            None => Value::Null,
        },
        CommandOperator::RequestHeader(name) => {
            match data.request_headers.iter().find(|(k, _)| k == name) {
                Some((_, v)) => Value::String(v.clone()),
                None => Value::Null,
            }
        }
        CommandOperator::ResponseHeader(name) => {
            match data.response_headers.iter().find(|(k, _)| k == name) {
                Some((_, v)) => Value::String(v.clone()),
                None => Value::Null,
            }
        }
        CommandOperator::InputTokens => match data.input_tokens {
            Some(v) => Value::Number(v.into()),
            None => Value::Null,
        },
        CommandOperator::OutputTokens => match data.output_tokens {
            Some(v) => Value::Number(v.into()),
            None => Value::Null,
        },
        CommandOperator::RatelimitMatched => Value::Bool(data.ratelimit_matched),
        CommandOperator::RatelimitRule => match &data.ratelimit_rule {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::Error => match &data.error {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::RouteStrategy => match &data.route_strategy {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::RouteScore => match data.route_score {
            Some(v) => serde_json::Number::from_f64(v)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            None => Value::Null,
        },
        CommandOperator::RouteCandidates => match data.route_candidates {
            Some(v) => Value::Number(v.into()),
            None => Value::Null,
        },
        CommandOperator::UpstreamResponseTimeMs => match data.upstream_response_time_ms {
            Some(v) => Value::Number(v.into()),
            None => Value::Null,
        },
        CommandOperator::ReqwestFallback => Value::Bool(data.reqwest_fallback),
        CommandOperator::ResponseFlags => match &data.response_flags {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::DownstreamRemoteAddress => match &data.downstream_remote_address {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::RequestDurationMs => match data.request_duration_ms {
            Some(v) => Value::Number(v.into()),
            None => Value::Null,
        },
        CommandOperator::ResponseDurationMs => match data.response_duration_ms {
            Some(v) => Value::Number(v.into()),
            None => Value::Null,
        },
        CommandOperator::UpstreamRequestAttemptCount => match data.upstream_request_attempt_count {
            Some(v) => Value::Number(v.into()),
            None => Value::Null,
        },
        CommandOperator::StartTimeLocal(fmt) => {
            let local = data.start_time.with_timezone(&chrono::Local);
            let format_str = fmt.as_deref().unwrap_or("%Y-%m-%dT%H:%M:%S%.3f%z");
            Value::String(local.format(format_str).to_string())
        }
        CommandOperator::EmitTime(fmt) => {
            let format_str = fmt.as_deref().unwrap_or("%Y-%m-%dT%H:%M:%S%.3fZ");
            Value::String(data.emit_time.format(format_str).to_string())
        }
        CommandOperator::RequestHeadersBytes => match data.request_headers_bytes {
            Some(v) => Value::Number(v.into()),
            None => Value::Null,
        },
        CommandOperator::ResponseHeadersBytes => match data.response_headers_bytes {
            Some(v) => Value::Number(v.into()),
            None => Value::Null,
        },
        CommandOperator::ResponseTxDurationMs => match data.response_tx_duration_ms {
            Some(v) => Value::Number(v.into()),
            None => Value::Null,
        },
        CommandOperator::UpstreamProtocol => match &data.upstream_protocol {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::DownstreamLocalAddress => match &data.downstream_local_address {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::UpstreamTransportFailureReason => {
            match &data.upstream_transport_failure_reason {
                Some(v) => Value::String(v.clone()),
                None => Value::Null,
            }
        }
        CommandOperator::ConnectionTerminationDetails => match &data.connection_termination_details
        {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::ResponseCodeDetails => match &data.response_code_details {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::PoolName => match &data.pool_name {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::RequestId => match &data.request_id {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
        CommandOperator::DownstreamTlsVersion => match &data.downstream_tls_version {
            Some(v) => Value::String(v.clone()),
            None => Value::Null,
        },
    }
}

// ── Access Logger ──

/// Thread-safe access logger with pre-parsed format and optional file output.
pub struct AccessLogger {
    config: AccessLogConfig,
    /// Pre-parsed text format segments (used when format=text).
    segments: Vec<FormatSegment>,
    /// Pre-parsed JSON field operators (key, operator). Used when format=json.
    json_operators: Vec<(String, CommandOperator)>,
    /// File writer (if output includes file).
    file_writer: Option<Mutex<BufWriter<File>>>,
}

impl AccessLogger {
    /// Create a new AccessLogger from config.
    ///
    /// Parses the format string at startup and opens the log file if needed.
    pub fn new(config: AccessLogConfig) -> Result<Self, String> {
        if !config.enabled {
            return Ok(Self {
                config,
                segments: Vec::new(),
                json_operators: Vec::new(),
                file_writer: None,
            });
        }

        // Parse text format
        let segments = parse_format_string(&config.text_format);

        // Parse JSON field selection
        let json_operators = if config.fields.is_empty() {
            // Default: all operators
            CommandOperator::defaults()
                .into_iter()
                .map(|op| (op.json_key(), op))
                .collect()
        } else {
            config
                .fields
                .iter()
                .filter_map(|spec| parse_field_spec(spec).map(|op| (op.json_key(), op)))
                .collect()
        };

        // Open file if needed
        let file_writer = match config.output {
            AccessLogOutput::File | AccessLogOutput::Both => {
                let path = config
                    .file_path
                    .as_deref()
                    .ok_or("access_log.file_path is required when output is 'file' or 'both'")?;

                // Create parent directories
                if let Some(parent) = std::path::Path::new(path).parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create log directory: {}", e))?;
                }

                let file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|e| format!("Failed to open access log file '{}': {}", path, e))?;

                Some(Mutex::new(BufWriter::new(file)))
            }
            AccessLogOutput::Stdout => None,
        };

        Ok(Self {
            config,
            segments,
            json_operators,
            file_writer,
        })
    }

    /// Log an access record.
    pub fn log(&self, data: &AccessLogData) {
        if !self.config.enabled {
            return;
        }

        let line = match self.config.format {
            AccessLogFormat::Json => self.format_json(data),
            AccessLogFormat::Text => self.format_text(data),
        };

        self.write_line(&line);
    }

    /// Format as text using pre-parsed segments.
    pub fn format_text(&self, data: &AccessLogData) -> String {
        let mut out = String::with_capacity(256);
        for seg in &self.segments {
            match seg {
                FormatSegment::Literal(s) => out.push_str(s),
                FormatSegment::Operator(op) => {
                    let val = resolve_text(op, data).unwrap_or_else(|| "-".to_string());
                    out.push_str(&val);
                }
            }
        }
        out
    }

    /// Format as JSON using pre-parsed operators.
    pub fn format_json(&self, data: &AccessLogData) -> String {
        let mut map = BTreeMap::new();
        for (key, op) in &self.json_operators {
            map.insert(key.clone(), resolve_json(op, data));
        }
        serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
    }

    /// Write a formatted line to configured outputs.
    fn write_line(&self, line: &str) {
        match self.config.output {
            AccessLogOutput::Stdout => {
                println!("{}", line);
            }
            AccessLogOutput::File => {
                self.write_to_file(line);
            }
            AccessLogOutput::Both => {
                println!("{}", line);
                self.write_to_file(line);
            }
        }
    }

    fn write_to_file(&self, line: &str) {
        if let Some(ref writer) = self.file_writer {
            if let Ok(mut w) = writer.lock() {
                let _ = writeln!(w, "{}", line);
                let _ = w.flush();
            }
        }
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_data() -> AccessLogData {
        AccessLogData {
            start_time: DateTime::parse_from_rfc3339("2026-03-01T12:00:00.123Z")
                .unwrap()
                .with_timezone(&Utc),
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
            protocol: "openai_chat".into(),
            response_code: 200,
            duration_ms: 1250,
            duration_us: 1250000,
            bytes_received: Some(512),
            bytes_sent: Some(2048),
            variant: Some("gpt4-fast".into()),
            provider: Some("openai".into()),
            model: Some("gpt-4o".into()),
            upstream_host: Some("api.openai.com".into()),
            streaming: Some(true),
            conversion: false,
            decision_time_us: Some(92),
            request_headers: vec![
                ("x-api-key".into(), "sk-test-123".into()),
                ("content-type".into(), "application/json".into()),
            ],
            response_headers: vec![("x-request-id".into(), "req-abc-123".into())],
            input_tokens: Some(150),
            output_tokens: Some(300),
            ratelimit_matched: false,
            ratelimit_rule: None,
            error: None,
            route_strategy: Some("weighted_round_robin".into()),
            route_score: Some(0.8523),
            route_candidates: Some(5),
            upstream_response_time_ms: Some(1100),
            reqwest_fallback: false,
            response_flags: None,
            downstream_remote_address: Some("10.0.1.5:52431".into()),
            request_duration_ms: Some(12),
            response_duration_ms: Some(850),
            upstream_request_attempt_count: Some(1),
            emit_time: DateTime::parse_from_rfc3339("2026-03-01T12:00:01.373Z")
                .unwrap()
                .with_timezone(&Utc),
            request_headers_bytes: Some(256),
            response_headers_bytes: Some(128),
            response_tx_duration_ms: Some(400),
            upstream_protocol: Some("openai_chat".into()),
            downstream_local_address: Some("0.0.0.0:8000".into()),
            upstream_transport_failure_reason: None,
            connection_termination_details: None,
            response_code_details: Some("via_upstream".into()),
            pool_name: Some("default-pool".into()),
            request_id: Some("req-uuid-001".into()),
            downstream_tls_version: Some("TLSv1.3".into()),
        }
    }

    // ── Format Parsing ──

    #[test]
    fn test_parse_simple_operators() {
        let segments = parse_format_string("%METHOD% %PATH%");
        assert_eq!(
            segments,
            vec![
                FormatSegment::Operator(CommandOperator::Method),
                FormatSegment::Literal(" ".into()),
                FormatSegment::Operator(CommandOperator::Path),
            ]
        );
    }

    #[test]
    fn test_parse_with_literals() {
        let segments = parse_format_string("[%START_TIME%] \"%METHOD% %PATH%\" %RESPONSE_CODE%");
        assert_eq!(segments.len(), 8);
        assert_eq!(segments[0], FormatSegment::Literal("[".into()));
        assert_eq!(
            segments[1],
            FormatSegment::Operator(CommandOperator::StartTime(None))
        );
        assert_eq!(segments[2], FormatSegment::Literal("] \"".into()));
        assert_eq!(
            segments[3],
            FormatSegment::Operator(CommandOperator::Method)
        );
        assert_eq!(segments[4], FormatSegment::Literal(" ".into()));
        assert_eq!(segments[5], FormatSegment::Operator(CommandOperator::Path));
        assert_eq!(segments[6], FormatSegment::Literal("\" ".into()));
        assert_eq!(
            segments[7],
            FormatSegment::Operator(CommandOperator::ResponseCode)
        );
    }

    #[test]
    fn test_parse_parameterized_start_time() {
        let segments = parse_format_string("%START_TIME(%Y-%m-%d)%");
        assert_eq!(
            segments,
            vec![FormatSegment::Operator(CommandOperator::StartTime(Some(
                "%Y-%m-%d".into()
            ))),]
        );
    }

    #[test]
    fn test_parse_request_header() {
        let segments = parse_format_string("%REQUEST_HEADER(X-Api-Key)%");
        assert_eq!(
            segments,
            vec![FormatSegment::Operator(CommandOperator::RequestHeader(
                "x-api-key".into()
            )),]
        );
    }

    #[test]
    fn test_parse_response_header() {
        let segments = parse_format_string("%RESPONSE_HEADER(X-Request-Id)%");
        assert_eq!(
            segments,
            vec![FormatSegment::Operator(CommandOperator::ResponseHeader(
                "x-request-id".into()
            )),]
        );
    }

    #[test]
    fn test_parse_unknown_operator() {
        let segments = parse_format_string("%UNKNOWN_OP%");
        assert_eq!(
            segments,
            vec![FormatSegment::Literal("%UNKNOWN_OP%".into()),]
        );
    }

    #[test]
    fn test_parse_empty_string() {
        let segments = parse_format_string("");
        assert!(segments.is_empty());
    }

    #[test]
    fn test_parse_no_operators() {
        let segments = parse_format_string("just plain text");
        assert_eq!(
            segments,
            vec![FormatSegment::Literal("just plain text".into()),]
        );
    }

    #[test]
    fn test_parse_all_operators() {
        let ops = vec![
            "METHOD",
            "PATH",
            "PROTOCOL",
            "RESPONSE_CODE",
            "DURATION_MS",
            "DURATION_US",
            "BYTES_RECEIVED",
            "BYTES_SENT",
            "VARIANT",
            "PROVIDER",
            "MODEL",
            "UPSTREAM_HOST",
            "STREAMING",
            "CONVERSION",
            "DECISION_TIME_US",
            "INPUT_TOKENS",
            "OUTPUT_TOKENS",
            "RATELIMIT_MATCHED",
            "RATELIMIT_RULE",
            "ERROR",
            "ROUTE_STRATEGY",
            "ROUTE_SCORE",
            "ROUTE_CANDIDATES",
            "UPSTREAM_RESPONSE_TIME_MS",
            "REQWEST_FALLBACK",
            "RESPONSE_FLAGS",
            "DOWNSTREAM_REMOTE_ADDRESS",
            "REQUEST_DURATION_MS",
            "RESPONSE_DURATION_MS",
            "UPSTREAM_REQUEST_ATTEMPT_COUNT",
            "START_TIME_LOCAL",
            "EMIT_TIME",
            "REQUEST_HEADERS_BYTES",
            "RESPONSE_HEADERS_BYTES",
            "RESPONSE_TX_DURATION_MS",
            "UPSTREAM_PROTOCOL",
            "DOWNSTREAM_LOCAL_ADDRESS",
            "UPSTREAM_TRANSPORT_FAILURE_REASON",
            "CONNECTION_TERMINATION_DETAILS",
            "RESPONSE_CODE_DETAILS",
            "POOL_NAME",
            "REQUEST_ID",
            "DOWNSTREAM_TLS_VERSION",
        ];
        for op_name in ops {
            let s = format!("%{}%", op_name);
            let segments = parse_format_string(&s);
            assert_eq!(segments.len(), 1, "Failed to parse operator: {}", op_name);
            assert!(matches!(segments[0], FormatSegment::Operator(_)));
        }
    }

    // ── Text Formatting ──

    #[test]
    fn test_format_text_simple() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "[%START_TIME%] %METHOD% %PATH% %RESPONSE_CODE% %DURATION_MS%ms".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert!(text.contains("2026-03-01T12:00:00.123Z"));
        assert!(text.contains("POST"));
        assert!(text.contains("/v1/chat/completions"));
        assert!(text.contains("200"));
        assert!(text.contains("1250ms"));
    }

    #[test]
    fn test_format_text_custom_time() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "%START_TIME(%Y/%m/%d %H:%M)%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert_eq!(text, "2026/03/01 12:00");
    }

    #[test]
    fn test_format_text_missing_values() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "variant=%VARIANT% tokens=%INPUT_TOKENS% err=%ERROR%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let mut data = sample_data();
        data.variant = None;
        data.input_tokens = None;
        data.error = None;
        let text = logger.format_text(&data);
        assert_eq!(text, "variant=- tokens=- err=-");
    }

    #[test]
    fn test_format_text_request_header() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "key=%REQUEST_HEADER(x-api-key)% missing=%REQUEST_HEADER(x-nonexist)%"
                .into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert_eq!(text, "key=sk-test-123 missing=-");
    }

    #[test]
    fn test_format_text_routing_fields() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "strategy=%ROUTE_STRATEGY% score=%ROUTE_SCORE% candidates=%ROUTE_CANDIDATES% decision=%DECISION_TIME_US%us".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert!(text.contains("strategy=weighted_round_robin"));
        assert!(text.contains("score=0.8523"));
        assert!(text.contains("candidates=5"));
        assert!(text.contains("decision=92us"));
    }

    #[test]
    fn test_format_text_upstream_error() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "%RESPONSE_CODE% err=%ERROR% upstream_ms=%UPSTREAM_RESPONSE_TIME_MS% fallback=%REQWEST_FALLBACK%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let mut data = sample_data();
        data.response_code = 502;
        data.error = Some("upstream connection refused".into());
        data.upstream_response_time_ms = None;
        data.reqwest_fallback = true;
        let text = logger.format_text(&data);
        assert!(text.contains("502"));
        assert!(text.contains("err=upstream connection refused"));
        assert!(text.contains("upstream_ms=-"));
        assert!(text.contains("fallback=true"));
    }

    // ── JSON Formatting ──

    #[test]
    fn test_format_json_defaults() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Json,
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let json_str = logger.format_json(&data);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["method"], "POST");
        assert_eq!(parsed["path"], "/v1/chat/completions");
        assert_eq!(parsed["response_code"], 200);
        assert_eq!(parsed["duration_ms"], 1250);
        assert_eq!(parsed["variant"], "gpt4-fast");
        assert_eq!(parsed["provider"], "openai");
        assert_eq!(parsed["model"], "gpt-4o");
        assert_eq!(parsed["streaming"], true);
        assert_eq!(parsed["conversion"], false);
        assert_eq!(parsed["decision_time_us"], 92);
        assert_eq!(parsed["route_strategy"], "weighted_round_robin");
        assert_eq!(parsed["route_candidates"], 5);
        assert_eq!(parsed["upstream_response_time_ms"], 1100);
        assert_eq!(parsed["reqwest_fallback"], false);
        assert_eq!(parsed["ratelimit_matched"], false);
        assert_eq!(parsed["error"], Value::Null);
    }

    #[test]
    fn test_format_json_field_selection() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Json,
            output: AccessLogOutput::Stdout,
            fields: vec!["%METHOD%".into(), "%PATH%".into(), "%RESPONSE_CODE%".into()],
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let json_str = logger.format_json(&data);
        let parsed: BTreeMap<String, Value> = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed.len(), 3);
        assert!(parsed.contains_key("method"));
        assert!(parsed.contains_key("path"));
        assert!(parsed.contains_key("response_code"));
        assert!(!parsed.contains_key("variant"));
    }

    #[test]
    fn test_format_json_with_headers() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Json,
            output: AccessLogOutput::Stdout,
            fields: vec![
                "%REQUEST_HEADER(x-api-key)%".into(),
                "%RESPONSE_HEADER(x-request-id)%".into(),
            ],
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let json_str = logger.format_json(&data);
        let parsed: BTreeMap<String, Value> = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["request_header_x-api-key"], "sk-test-123");
        assert_eq!(parsed["response_header_x-request-id"], "req-abc-123");
    }

    #[test]
    fn test_format_json_null_values() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Json,
            output: AccessLogOutput::Stdout,
            fields: vec![
                "%VARIANT%".into(),
                "%INPUT_TOKENS%".into(),
                "%ERROR%".into(),
                "%RATELIMIT_RULE%".into(),
            ],
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let mut data = sample_data();
        data.variant = None;
        data.input_tokens = None;
        data.error = None;
        data.ratelimit_rule = None;
        let json_str = logger.format_json(&data);
        let parsed: BTreeMap<String, Value> = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["variant"], Value::Null);
        assert_eq!(parsed["input_tokens"], Value::Null);
        assert_eq!(parsed["error"], Value::Null);
        assert_eq!(parsed["ratelimit_rule"], Value::Null);
    }

    #[test]
    fn test_format_json_typed_values() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Json,
            output: AccessLogOutput::Stdout,
            fields: vec![
                "%RESPONSE_CODE%".into(),
                "%DURATION_MS%".into(),
                "%STREAMING%".into(),
                "%CONVERSION%".into(),
                "%RATELIMIT_MATCHED%".into(),
                "%REQWEST_FALLBACK%".into(),
            ],
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let json_str = logger.format_json(&data);
        let parsed: BTreeMap<String, Value> = serde_json::from_str(&json_str).unwrap();

        // Numbers should be numbers, not strings
        assert!(parsed["response_code"].is_number());
        assert!(parsed["duration_ms"].is_number());
        // Booleans should be booleans
        assert!(parsed["streaming"].is_boolean());
        assert!(parsed["conversion"].is_boolean());
        assert!(parsed["ratelimit_matched"].is_boolean());
        assert!(parsed["reqwest_fallback"].is_boolean());
    }

    #[test]
    fn test_format_json_routing_and_error_fields() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Json,
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let mut data = sample_data();
        data.response_code = 502;
        data.error = Some("connection reset by peer".into());
        data.ratelimit_matched = true;
        data.ratelimit_rule = Some("global-10rps".into());
        let json_str = logger.format_json(&data);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["response_code"], 502);
        assert_eq!(parsed["error"], "connection reset by peer");
        assert_eq!(parsed["ratelimit_matched"], true);
        assert_eq!(parsed["ratelimit_rule"], "global-10rps");
        assert_eq!(parsed["route_strategy"], "weighted_round_robin");
    }

    // ── Disabled Logger ──

    #[test]
    fn test_disabled_logger() {
        let config = AccessLogConfig {
            enabled: false,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        // Should not panic
        logger.log(&sample_data());
    }

    // ── File Output ──

    #[test]
    fn test_file_output() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("access.log");

        let config = AccessLogConfig {
            enabled: true,
            output: AccessLogOutput::File,
            file_path: Some(file_path.to_str().unwrap().into()),
            format: AccessLogFormat::Json,
            fields: vec!["%METHOD%".into(), "%RESPONSE_CODE%".into()],
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        logger.log(&sample_data());

        let content = std::fs::read_to_string(&file_path).unwrap();
        let parsed: BTreeMap<String, Value> = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["method"], "POST");
        assert_eq!(parsed["response_code"], 200);
    }

    #[test]
    fn test_file_requires_path() {
        let config = AccessLogConfig {
            enabled: true,
            output: AccessLogOutput::File,
            file_path: None,
            ..Default::default()
        };
        assert!(AccessLogger::new(config).is_err());
    }

    // ── Field Spec Parsing ──

    #[test]
    fn test_parse_field_spec_valid() {
        assert_eq!(parse_field_spec("%METHOD%"), Some(CommandOperator::Method));
        assert_eq!(
            parse_field_spec("%REQUEST_HEADER(host)%"),
            Some(CommandOperator::RequestHeader("host".into()))
        );
    }

    #[test]
    fn test_parse_field_spec_invalid() {
        assert_eq!(parse_field_spec("METHOD"), None);
        assert_eq!(parse_field_spec("%NOPE%"), None);
    }

    // ── Default Config ──

    #[test]
    fn test_default_config_is_disabled() {
        let config = AccessLogConfig::default();
        assert!(!config.enabled);
    }

    // ── Route Score Precision ──

    #[test]
    fn test_route_score_text_precision() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "%ROUTE_SCORE%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert_eq!(text, "0.8523");
    }

    #[test]
    fn test_route_score_json() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Json,
            fields: vec!["%ROUTE_SCORE%".into()],
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let json_str = logger.format_json(&data);
        let parsed: BTreeMap<String, Value> = serde_json::from_str(&json_str).unwrap();
        assert!(parsed["route_score"].is_number());
    }

    // ── Multiple Log Lines ──

    #[test]
    fn test_multiple_log_lines_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("multi.log");

        let config = AccessLogConfig {
            enabled: true,
            output: AccessLogOutput::File,
            file_path: Some(file_path.to_str().unwrap().into()),
            format: AccessLogFormat::Text,
            text_format: "%METHOD% %PATH% %RESPONSE_CODE%".into(),
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();

        let mut data1 = sample_data();
        data1.method = "GET".into();
        data1.response_code = 200;
        logger.log(&data1);

        let mut data2 = sample_data();
        data2.method = "POST".into();
        data2.response_code = 502;
        logger.log(&data2);

        let content = std::fs::read_to_string(&file_path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("GET"));
        assert!(lines[1].contains("502"));
    }

    // ── Envoy-like Default Format ──

    #[test]
    fn test_envoy_like_default_format() {
        let fmt = "[%START_TIME%] \"%METHOD% %PATH%\" %RESPONSE_CODE% %DURATION_MS%ms variant=%VARIANT% provider=%PROVIDER% model=%MODEL% strategy=%ROUTE_STRATEGY% candidates=%ROUTE_CANDIDATES% decision=%DECISION_TIME_US%us upstream=%UPSTREAM_HOST% upstream_ms=%UPSTREAM_RESPONSE_TIME_MS% stream=%STREAMING% err=%ERROR%";
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: fmt.into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);

        assert!(
            text.starts_with("[2026-03-01T12:00:00.123Z] \"POST /v1/chat/completions\" 200 1250ms")
        );
        assert!(text.contains("variant=gpt4-fast"));
        assert!(text.contains("provider=openai"));
        assert!(text.contains("model=gpt-4o"));
        assert!(text.contains("strategy=weighted_round_robin"));
        assert!(text.contains("candidates=5"));
        assert!(text.contains("decision=92us"));
        assert!(text.contains("upstream=api.openai.com"));
        assert!(text.contains("upstream_ms=1100"));
        assert!(text.contains("stream=true"));
        assert!(text.contains("err=-"));
    }

    // ── New fields: response_flags, downstream, request/response duration, attempt count ──

    #[test]
    fn test_format_text_response_flags() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "flags=%RESPONSE_FLAGS% client=%DOWNSTREAM_REMOTE_ADDRESS%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let mut data = sample_data();
        data.response_flags = Some("RL,UT".into());
        let text = logger.format_text(&data);
        assert_eq!(text, "flags=RL,UT client=10.0.1.5:52431");
    }

    #[test]
    fn test_format_text_response_flags_missing() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "flags=%RESPONSE_FLAGS%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data(); // response_flags is None
        let text = logger.format_text(&data);
        assert_eq!(text, "flags=-");
    }

    #[test]
    fn test_format_text_duration_fields() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "req=%REQUEST_DURATION_MS%ms ttfb=%RESPONSE_DURATION_MS%ms attempts=%UPSTREAM_REQUEST_ATTEMPT_COUNT%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert_eq!(text, "req=12ms ttfb=850ms attempts=1");
    }

    #[test]
    fn test_format_json_new_fields() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Json,
            fields: vec![
                "%RESPONSE_FLAGS%".into(),
                "%DOWNSTREAM_REMOTE_ADDRESS%".into(),
                "%REQUEST_DURATION_MS%".into(),
                "%RESPONSE_DURATION_MS%".into(),
                "%UPSTREAM_REQUEST_ATTEMPT_COUNT%".into(),
            ],
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let mut data = sample_data();
        data.response_flags = Some("UF".into());
        let json_str = logger.format_json(&data);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["response_flags"], "UF");
        assert_eq!(parsed["downstream_remote_address"], "10.0.1.5:52431");
        assert_eq!(parsed["request_duration_ms"], 12);
        assert_eq!(parsed["response_duration_ms"], 850);
        assert_eq!(parsed["upstream_request_attempt_count"], 1);
    }

    #[test]
    fn test_format_json_envoy_fields_null() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Json,
            fields: vec![
                "%RESPONSE_FLAGS%".into(),
                "%DOWNSTREAM_REMOTE_ADDRESS%".into(),
                "%REQUEST_DURATION_MS%".into(),
                "%RESPONSE_DURATION_MS%".into(),
                "%UPSTREAM_REQUEST_ATTEMPT_COUNT%".into(),
            ],
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let mut data = sample_data();
        data.response_flags = None;
        data.downstream_remote_address = None;
        data.request_duration_ms = None;
        data.response_duration_ms = None;
        data.upstream_request_attempt_count = None;
        let json_str = logger.format_json(&data);
        let parsed: BTreeMap<String, Value> = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["response_flags"], Value::Null);
        assert_eq!(parsed["downstream_remote_address"], Value::Null);
        assert_eq!(parsed["request_duration_ms"], Value::Null);
        assert_eq!(parsed["response_duration_ms"], Value::Null);
        assert_eq!(parsed["upstream_request_attempt_count"], Value::Null);
    }

    #[test]
    fn test_full_envoy_style_with_new_fields() {
        let fmt = "[%START_TIME%] \"%METHOD% %PATH%\" %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION_MS% %RESPONSE_DURATION_MS% %REQUEST_DURATION_MS% \"%DOWNSTREAM_REMOTE_ADDRESS%\" variant=%VARIANT% attempts=%UPSTREAM_REQUEST_ATTEMPT_COUNT%";
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: fmt.into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let mut data = sample_data();
        data.response_flags = Some("UF".into());
        let text = logger.format_text(&data);
        assert!(text.contains("200 UF 512 2048 1250 850 12"));
        assert!(text.contains("\"10.0.1.5:52431\""));
        assert!(text.contains("variant=gpt4-fast"));
        assert!(text.contains("attempts=1"));
    }

    // ── New Envoy-style operators (14 additional) ──

    #[test]
    fn test_format_text_start_time_local() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "%START_TIME_LOCAL%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        // Should produce a non-empty local time string
        assert!(!text.is_empty());
        assert_ne!(text, "-");
    }

    #[test]
    fn test_format_text_start_time_local_custom_format() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "%START_TIME_LOCAL(%H:%M:%S)%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        // Should match HH:MM:SS pattern
        assert_eq!(text.len(), 8);
        assert!(text.contains(':'));
    }

    #[test]
    fn test_format_text_emit_time() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "%EMIT_TIME%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert!(text.contains("2026-03-01T12:00:01.373Z"));
    }

    #[test]
    fn test_format_text_emit_time_custom_format() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "%EMIT_TIME(%s)%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        // Unix timestamp
        assert!(text.parse::<i64>().is_ok());
    }

    #[test]
    fn test_format_text_header_bytes() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "req_hdr=%REQUEST_HEADERS_BYTES% resp_hdr=%RESPONSE_HEADERS_BYTES%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert_eq!(text, "req_hdr=256 resp_hdr=128");
    }

    #[test]
    fn test_format_text_response_tx_duration() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "tx=%RESPONSE_TX_DURATION_MS%ms".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert_eq!(text, "tx=400ms");
    }

    #[test]
    fn test_format_text_upstream_protocol() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "up_proto=%UPSTREAM_PROTOCOL%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert_eq!(text, "up_proto=openai_chat");
    }

    #[test]
    fn test_format_text_downstream_local_address() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "listen=%DOWNSTREAM_LOCAL_ADDRESS%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert_eq!(text, "listen=0.0.0.0:8000");
    }

    #[test]
    fn test_format_text_response_code_details() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "%RESPONSE_CODE% %RESPONSE_CODE_DETAILS%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert_eq!(text, "200 via_upstream");
    }

    #[test]
    fn test_format_text_pool_and_request_id() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "pool=%POOL_NAME% rid=%REQUEST_ID%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert_eq!(text, "pool=default-pool rid=req-uuid-001");
    }

    #[test]
    fn test_format_text_tls_version() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: "tls=%DOWNSTREAM_TLS_VERSION%".into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);
        assert_eq!(text, "tls=TLSv1.3");
    }

    #[test]
    fn test_format_text_transport_failure() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format:
                "fail=%UPSTREAM_TRANSPORT_FAILURE_REASON% term=%CONNECTION_TERMINATION_DETAILS%"
                    .into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let mut data = sample_data();
        data.upstream_transport_failure_reason = Some("TLS_handshake_failed".into());
        data.connection_termination_details = Some("remote_reset".into());
        let text = logger.format_text(&data);
        assert_eq!(text, "fail=TLS_handshake_failed term=remote_reset");
    }

    #[test]
    fn test_format_json_new_envoy_fields() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Json,
            fields: vec![
                "%EMIT_TIME%".into(),
                "%REQUEST_HEADERS_BYTES%".into(),
                "%RESPONSE_HEADERS_BYTES%".into(),
                "%RESPONSE_TX_DURATION_MS%".into(),
                "%UPSTREAM_PROTOCOL%".into(),
                "%DOWNSTREAM_LOCAL_ADDRESS%".into(),
                "%RESPONSE_CODE_DETAILS%".into(),
                "%POOL_NAME%".into(),
                "%REQUEST_ID%".into(),
                "%DOWNSTREAM_TLS_VERSION%".into(),
            ],
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let json_str = logger.format_json(&data);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert!(parsed["emit_time"].is_string());
        assert_eq!(parsed["request_headers_bytes"], 256);
        assert_eq!(parsed["response_headers_bytes"], 128);
        assert_eq!(parsed["response_tx_duration_ms"], 400);
        assert_eq!(parsed["upstream_protocol"], "openai_chat");
        assert_eq!(parsed["downstream_local_address"], "0.0.0.0:8000");
        assert_eq!(parsed["response_code_details"], "via_upstream");
        assert_eq!(parsed["pool_name"], "default-pool");
        assert_eq!(parsed["request_id"], "req-uuid-001");
        assert_eq!(parsed["downstream_tls_version"], "TLSv1.3");
    }

    #[test]
    fn test_format_json_envoy_extended_fields_null() {
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Json,
            fields: vec![
                "%UPSTREAM_TRANSPORT_FAILURE_REASON%".into(),
                "%CONNECTION_TERMINATION_DETAILS%".into(),
                "%RESPONSE_CODE_DETAILS%".into(),
                "%POOL_NAME%".into(),
                "%REQUEST_ID%".into(),
                "%DOWNSTREAM_TLS_VERSION%".into(),
            ],
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let mut data = sample_data();
        data.upstream_transport_failure_reason = None;
        data.connection_termination_details = None;
        data.response_code_details = None;
        data.pool_name = None;
        data.request_id = None;
        data.downstream_tls_version = None;
        let json_str = logger.format_json(&data);
        let parsed: BTreeMap<String, Value> = serde_json::from_str(&json_str).unwrap();

        for v in parsed.values() {
            assert_eq!(*v, Value::Null);
        }
    }

    #[test]
    fn test_full_envoy_default_format() {
        // Mimics Envoy's default format adapted for Prism
        let fmt = "[%START_TIME%] \"%METHOD% %PATH%\" %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION_MS% %RESPONSE_DURATION_MS% %REQUEST_DURATION_MS% \"%DOWNSTREAM_REMOTE_ADDRESS%\" \"%REQUEST_HEADER(x-api-key)%\" \"%REQUEST_ID%\" variant=%VARIANT% provider=%PROVIDER% model=%MODEL% pool=%POOL_NAME% upstream=%UPSTREAM_HOST% up_proto=%UPSTREAM_PROTOCOL% tls=%DOWNSTREAM_TLS_VERSION% listen=%DOWNSTREAM_LOCAL_ADDRESS% attempts=%UPSTREAM_REQUEST_ATTEMPT_COUNT% req_hdr=%REQUEST_HEADERS_BYTES% resp_hdr=%RESPONSE_HEADERS_BYTES% tx=%RESPONSE_TX_DURATION_MS% details=%RESPONSE_CODE_DETAILS%";
        let config = AccessLogConfig {
            enabled: true,
            format: AccessLogFormat::Text,
            text_format: fmt.into(),
            output: AccessLogOutput::Stdout,
            ..Default::default()
        };
        let logger = AccessLogger::new(config).unwrap();
        let data = sample_data();
        let text = logger.format_text(&data);

        assert!(text.starts_with("[2026-03-01T12:00:00.123Z] \"POST /v1/chat/completions\" 200 -"));
        assert!(text.contains("\"10.0.1.5:52431\""));
        assert!(text.contains("\"sk-test-123\""));
        assert!(text.contains("\"req-uuid-001\""));
        assert!(text.contains("variant=gpt4-fast"));
        assert!(text.contains("pool=default-pool"));
        assert!(text.contains("up_proto=openai_chat"));
        assert!(text.contains("tls=TLSv1.3"));
        assert!(text.contains("listen=0.0.0.0:8000"));
        assert!(text.contains("attempts=1"));
        assert!(text.contains("req_hdr=256"));
        assert!(text.contains("resp_hdr=128"));
        assert!(text.contains("tx=400"));
        assert!(text.contains("details=via_upstream"));
    }
}
