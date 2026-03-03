use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use pingora::prelude::*;
use pingora::proxy::{ProxyHttp, Session};
use pingora::upstreams::peer::HttpPeer;
use tracing::{debug, error, info, warn};

use prism_core::analyzer::ChatMessage;
use prism_core::router::{PrismRouter, RouteMetadata, RouteRequest, Router};

use crate::access_log::{AccessLogData, AccessLogger};
use crate::codec;
use crate::codec::registry::CodecRegistry;
use crate::metrics::{MetricsCollector, RequestRecord};
use crate::protocol::LlmProtocol;
use crate::ratelimit::RateLimiter;
use crate::codec::usage_extractor::{ExtractedUsage, USAGE_KEY};
use crate::codec::quota_recorder::{HEADERS_KEY, LIMITER_KEY};

/// Per-request context stored across Pingora proxy phases.
pub struct ProxyContext {
    /// Detected LLM protocol
    pub protocol: LlmProtocol,
    /// Router's selected variant ID
    pub selected_variant: Option<String>,
    /// Selected provider ID
    pub selected_provider: Option<String>,
    /// Selected model ID
    pub selected_model: Option<String>,
    /// Upstream base URL
    pub upstream_base_url: Option<String>,
    /// Authorization header for upstream
    pub upstream_auth: Option<String>,
    /// Custom headers to send to upstream (e.g. x-api-key)
    pub upstream_custom_headers: Vec<(String, String)>,
    /// Request start time for latency tracking
    pub request_start: Instant,
    /// HTTP status from upstream response
    #[allow(dead_code)]
    pub response_status: Option<u16>,

    // ── Codec plugin chain integration ──
    /// Codec plugin chain context (if protocol conversion is needed).
    /// This is the `codec::ProxyContext`, distinct from this Pingora `ProxyContext`.
    pub codec_ctx: Option<codec::ProxyContext>,
    /// Whether protocol conversion is active for this request.
    pub needs_conversion: bool,
    /// Buffer for accumulating request body chunks when conversion is needed.
    pub request_body_buffer: Option<Vec<u8>>,
    /// Buffer for accumulating response body chunks when conversion is needed (non-streaming).
    pub response_body_buffer: Option<Vec<u8>>,
    /// Outbound protocol string determined from provider config.
    pub outbound_protocol: Option<String>,
    /// Model name to use in upstream request body.
    pub upstream_model_name: Option<String>,

    // ── Access log fields ──
    /// HTTP method captured at request_filter time.
    pub request_method: String,
    /// Request URI path captured at request_filter time.
    pub request_path: String,
    /// Routing decision time in microseconds.
    pub decision_time_us: Option<u64>,
    /// Whether response was streamed.
    pub is_streaming: Option<bool>,
    /// Request body bytes received.
    pub bytes_received: Option<u64>,
    /// Response body bytes sent.
    pub bytes_sent: Option<u64>,
    /// Whether a rate limit rule matched this request.
    pub ratelimit_matched: bool,
    /// Rate limit rule name that matched (if any).
    pub ratelimit_rule: Option<String>,
    /// Error message (if request failed).
    pub error_message: Option<String>,
    /// Request headers snapshot for access log.
    pub request_headers_snapshot: Vec<(String, String)>,
    /// Whether this request used the reqwest fallback path.
    pub reqwest_fallback: bool,
    /// Response flags — short error/termination codes (RL, NR, UF, UC, UT, DC).
    pub response_flags: Vec<String>,
    /// Downstream client remote address (ip:port).
    pub downstream_remote_address: Option<String>,
    /// Time when request body was fully received.
    pub request_body_received_at: Option<Instant>,
    /// Time to first byte from upstream (TTFB).
    pub upstream_first_byte_at: Option<Instant>,
    /// Request headers total bytes.
    pub request_headers_bytes: Option<u64>,
    /// Response headers total bytes.
    pub response_headers_bytes: Option<u64>,
    /// Response code details — who set it (prism vs upstream) and why.
    pub response_code_details: Option<String>,
    /// Upstream transport failure reason (TLS error, etc).
    pub upstream_transport_failure_reason: Option<String>,
    /// Connection termination details.
    pub connection_termination_details: Option<String>,
    /// Global plugin chain context (always initialized, not dependent on conversion).
    pub global_codec_ctx: Option<codec::ProxyContext>,
    /// Number of upstream request attempts.
    pub upstream_request_attempt_count: u32,
}

impl Default for ProxyContext {
    fn default() -> Self {
        Self {
            protocol: LlmProtocol::Unknown,
            selected_variant: None,
            selected_provider: None,
            selected_model: None,
            upstream_base_url: None,
            upstream_auth: None,
            upstream_custom_headers: Vec::new(),
            request_start: Instant::now(),
            response_status: None,
            codec_ctx: None,
            needs_conversion: false,
            request_body_buffer: None,
            response_body_buffer: None,
            outbound_protocol: None,
            upstream_model_name: None,
            request_method: String::new(),
            request_path: String::new(),
            decision_time_us: None,
            is_streaming: None,
            bytes_received: None,
            bytes_sent: None,
            ratelimit_matched: false,
            ratelimit_rule: None,
            error_message: None,
            request_headers_snapshot: Vec::new(),
            reqwest_fallback: false,
            response_flags: Vec::new(),
            downstream_remote_address: None,
            request_body_received_at: None,
            upstream_first_byte_at: None,
            request_headers_bytes: None,
            response_headers_bytes: None,
            response_code_details: None,
            upstream_transport_failure_reason: None,
            connection_termination_details: None,
            global_codec_ctx: None,
            upstream_request_attempt_count: 1,
        }
    }
}

/// The Prism proxy service — routes LLM API requests to appropriate providers.
pub struct PrismProxy {
    pub router: Arc<ArcSwap<PrismRouter>>,
    pub metrics: Arc<MetricsCollector>,
    pub codec_registry: Arc<CodecRegistry>,
    pub rate_limiter: Option<Arc<RateLimiter>>,
    pub access_logger: Option<Arc<AccessLogger>>,
}

impl PrismProxy {
    #[allow(dead_code)]
    pub fn new(router: Arc<ArcSwap<PrismRouter>>, metrics: Arc<MetricsCollector>) -> Self {
        Self {
            router,
            metrics,
            codec_registry: Arc::new(CodecRegistry::new()),
            rate_limiter: None,
            access_logger: None,
        }
    }

    #[allow(dead_code)]
    pub fn with_codec_registry(
        router: Arc<ArcSwap<PrismRouter>>,
        metrics: Arc<MetricsCollector>,
        codec_registry: Arc<CodecRegistry>,
    ) -> Self {
        Self {
            router,
            metrics,
            codec_registry,
            rate_limiter: None,
            access_logger: None,
        }
    }

    #[allow(dead_code)]
    pub fn with_rate_limiter(mut self, limiter: Arc<RateLimiter>) -> Self {
        self.rate_limiter = Some(limiter);
        self
    }

    #[allow(dead_code)]
    pub fn with_access_logger(mut self, logger: Arc<AccessLogger>) -> Self {
        self.access_logger = Some(logger);
        self
    }

    /// Parse a base URL into (host, port, use_tls).
    fn parse_upstream(base_url: &str) -> Result<(&str, u16, bool), Box<pingora::Error>> {
        let use_tls = base_url.starts_with("https://");
        let host_port = base_url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("localhost");

        let (host, port) = if let Some(colon) = host_port.rfind(':') {
            let port_str = &host_port[colon + 1..];
            match port_str.parse::<u16>() {
                Ok(p) => (&host_port[..colon], p),
                Err(_) => (host_port, if use_tls { 443 } else { 80 }),
            }
        } else {
            (host_port, if use_tls { 443 } else { 80 })
        };

        Ok((host, port, use_tls))
    }

    /// Initialize the codec context from the proxy context after routing is decided.
    /// Called internally when `needs_conversion` is detected.
    fn init_codec_ctx(proxy_ctx: &mut ProxyContext) {
        let mut codec_ctx = codec::ProxyContext::new();
        codec_ctx.inbound_protocol = proxy_ctx.protocol.as_str().to_string();
        codec_ctx.outbound_protocol = proxy_ctx.outbound_protocol.clone().unwrap_or_default();
        codec_ctx.selected_provider = proxy_ctx.selected_provider.clone().unwrap_or_default();
        codec_ctx.selected_model = proxy_ctx.selected_model.clone().unwrap_or_default();
        codec_ctx.selected_variant = proxy_ctx.selected_variant.clone().unwrap_or_default();
        if let Some(ref base_url) = proxy_ctx.upstream_base_url {
            let host = base_url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .split('/')
                .next()
                .unwrap_or("localhost");
            codec_ctx.upstream_host = host.to_string();
        }
        if let Some(ref auth) = proxy_ctx.upstream_auth {
            codec_ctx.upstream_auth = Some(auth.clone());
        }

        proxy_ctx.codec_ctx = Some(codec_ctx);
        proxy_ctx.request_body_buffer = Some(Vec::new());
        proxy_ctx.response_body_buffer = Some(Vec::new());
    }
    /// Use reqwest to forward the request directly, bypassing Pingora's upstream connector.
    /// Kept as fallback for environments where Pingora's openssl feature is unavailable.
    #[allow(dead_code)]
    async fn reqwest_forward(
        &self,
        session: &mut Session,
        ctx: &mut ProxyContext,
    ) -> Result<bool, Box<pingora::Error>> {
        let path = session.req_header().uri.path().to_string();
        let method = session.req_header().method.clone();

        // Build route request
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: format!("(proxy request via {})", path),
        }];
        let request = RouteRequest {
            messages,
            strategy: None,
            metadata: RouteMetadata::default(),
            analyze_context: None,
            required_protocol: if ctx.protocol.as_str() != "unknown" {
                Some(ctx.protocol.as_str().to_string())
            } else {
                None
            },
        };

        let router = self.router.load();
        let decision = match router.route(&request) {
            Ok(d) => d,
            Err(e) => {
                ctx.response_flags.push("NR".to_string());
                ctx.error_message = Some(format!("routing error: {}", e));
                error!(error = %e, "Routing failed");
                let header = pingora::http::ResponseHeader::build(503, None).unwrap();
                let body = serde_json::json!({"error": e.to_string()});
                session
                    .write_response_header(Box::new(header), true)
                    .await
                    .ok();
                session
                    .write_response_body(Some(Bytes::from(body.to_string())), true)
                    .await
                    .ok();
                return Ok(true);
            }
        };

        ctx.selected_variant = Some(decision.selected_variant.clone());
        ctx.selected_provider = Some(decision.selected_provider.clone());
        ctx.selected_model = Some(decision.selected_model.clone());

        // Look up provider
        let provider = router.registry.providers.get(&decision.selected_provider);
        let base_url = provider
            .map(|p| p.base_url.as_str())
            .unwrap_or("https://api.openai.com/v1");

        // Look up upstream_model_name
        if let Some(variant) = router.registry.variants.get(&decision.selected_variant) {
            ctx.upstream_model_name = variant.upstream_model_name.clone();
        }

        // Extract base_path from base_url
        let without_scheme = base_url
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let base_path = if let Some(slash_pos) = without_scheme.find('/') {
            let p = &without_scheme[slash_pos..];
            p.trim_end_matches('/')
        } else {
            ""
        };

        // Build upstream URL: rewrite /v1 prefix with base_path if needed
        let upstream_path = if !base_path.is_empty() && (path.starts_with("/v1/") || path == "/v1")
        {
            if base_path.ends_with("/v1") {
                format!("{}{}", base_path, &path[3..])
            } else {
                format!("{}{}", base_path, path)
            }
        } else if !base_path.is_empty() {
            format!("{}{}", base_path, path)
        } else {
            path.clone()
        };

        let scheme = if base_url.starts_with("https://") {
            "https"
        } else {
            "http"
        };
        let host = without_scheme.split('/').next().unwrap_or("localhost");
        let upstream_url = format!("{}://{}{}", scheme, host, upstream_path);

        info!(
            variant = %decision.selected_variant,
            provider = %decision.selected_provider,
            url = %upstream_url,
            "Forwarding via reqwest"
        );

        // Read request body
        let body_bytes = session
            .read_request_body()
            .await
            .map_err(|e| {
                pingora::Error::explain(
                    pingora::ErrorType::ReadError,
                    format!("read request body: {}", e),
                )
            })?
            .unwrap_or_default();

        // Optionally rewrite model name in body
        let final_body = if let Some(ref upstream_name) = ctx.upstream_model_name {
            if !body_bytes.is_empty() {
                if let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&body_bytes) {
                    if json.get("model").is_some() {
                        json["model"] = serde_json::Value::String(upstream_name.clone());
                        serde_json::to_vec(&json).unwrap_or_else(|_| body_bytes.to_vec())
                    } else {
                        body_bytes.to_vec()
                    }
                } else {
                    body_bytes.to_vec()
                }
            } else {
                body_bytes.to_vec()
            }
        } else {
            body_bytes.to_vec()
        };

        // Build reqwest request
        let req_method = match method.as_str() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            "PATCH" => reqwest::Method::PATCH,
            _ => reqwest::Method::POST,
        };
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| {
                pingora::Error::explain(
                    pingora::ErrorType::InternalError,
                    format!("create http client: {}", e),
                )
            })?;
        let mut req = client.request(req_method, &upstream_url);

        // Forward content-type
        if let Some(ct) = session.req_header().headers.get("content-type") {
            req = req.header("content-type", ct.to_str().unwrap_or("application/json"));
        } else {
            req = req.header("content-type", "application/json");
        }

        // Set auth headers
        if let Some(p) = provider {
            match &p.auth {
                prism_core::types::AuthConfig::ApiKey { key, header } => {
                    req = req.header(header.as_str(), key.as_str());
                }
                prism_core::types::AuthConfig::Custom { headers } => {
                    for (k, v) in headers {
                        req = req.header(k.as_str(), v.as_str());
                    }
                }
                _ => {}
            }
        }

        req = req.body(final_body);

        // Send request
        let upstream_resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "Upstream request failed");
                let header = pingora::http::ResponseHeader::build(502, None).unwrap();
                let body = serde_json::json!({"error": format!("upstream error: {}", e)});
                session
                    .write_response_header(Box::new(header), true)
                    .await
                    .ok();
                session
                    .write_response_body(Some(Bytes::from(body.to_string())), true)
                    .await
                    .ok();
                return Ok(true);
            }
        };

        let status = upstream_resp.status().as_u16();
        let resp_headers = upstream_resp.headers().clone();

        // Check if this is a streaming response
        let is_streaming = resp_headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("text/event-stream"))
            .unwrap_or(false);

        // Build response header
        let mut header = pingora::http::ResponseHeader::build(status, None).map_err(|e| {
            pingora::Error::explain(
                pingora::ErrorType::InternalError,
                format!("build response header: {:?}", e),
            )
        })?;

        for (name, value) in resp_headers.iter() {
            if name == "transfer-encoding" || name == "content-length" {
                continue; // Let Pingora handle these
            }
            if let Ok(v) = value.to_str() {
                let _ = header.insert_header(
                    http::header::HeaderName::from_bytes(name.as_ref())
                        .unwrap_or(http::header::CONTENT_TYPE),
                    v,
                );
            }
        }

        if is_streaming {
            // Stream response chunks
            let _ = header.insert_header("transfer-encoding", "chunked");
            session
                .write_response_header(Box::new(header), false)
                .await
                .map_err(|e| {
                    pingora::Error::explain(
                        pingora::ErrorType::WriteError,
                        format!("write response header: {}", e),
                    )
                })?;

            let mut stream = upstream_resp.bytes_stream();
            use futures_util::StreamExt;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(data) => {
                        session
                            .write_response_body(Some(data), false)
                            .await
                            .map_err(|e| {
                                pingora::Error::explain(
                                    pingora::ErrorType::WriteError,
                                    format!("write response chunk: {}", e),
                                )
                            })?;
                    }
                    Err(e) => {
                        warn!(error = %e, "Stream chunk error");
                        break;
                    }
                }
            }
            session.write_response_body(None, true).await.ok();
        } else {
            // Non-streaming: read full body and write at once
            let resp_body = upstream_resp.bytes().await.map_err(|e| {
                pingora::Error::explain(
                    pingora::ErrorType::ReadError,
                    format!("read upstream response: {}", e),
                )
            })?;

            session
                .write_response_header(Box::new(header), false)
                .await
                .map_err(|e| {
                    pingora::Error::explain(
                        pingora::ErrorType::WriteError,
                        format!("write response header: {}", e),
                    )
                })?;
            session
                .write_response_body(Some(resp_body), true)
                .await
                .map_err(|e| {
                    pingora::Error::explain(
                        pingora::ErrorType::WriteError,
                        format!("write response body: {}", e),
                    )
                })?;
        }

        let elapsed = ctx.request_start.elapsed();
        ctx.is_streaming = Some(is_streaming);
        ctx.reqwest_fallback = true;
        info!(
            variant = ?ctx.selected_variant,
            provider = ?ctx.selected_provider,
            status = %status,
            latency_ms = %elapsed.as_millis(),
            streaming = %is_streaming,
            "Request completed (reqwest)"
        );

        // Record metrics
        if let Some(ref variant_id) = ctx.selected_variant {
            self.metrics.record(RequestRecord {
                variant_id: variant_id.clone(),
                latency_ms: elapsed.as_millis() as u64,
                ttft_ms: None,
                success: (200..400).contains(&status),
                input_tokens: ctx.global_codec_ctx.as_ref().and_then(|c| c.get::<ExtractedUsage>(USAGE_KEY)).and_then(|u| u.input_tokens.map(|v| v as u32)),
                output_tokens: ctx.global_codec_ctx.as_ref().and_then(|c| c.get::<ExtractedUsage>(USAGE_KEY)).and_then(|u| u.output_tokens.map(|v| v as u32)),
                timestamp: Instant::now(),
            });
        }

        // Access log for reqwest path (logging() phase won't fire for early-return)
        if let Some(ref logger) = self.access_logger {
            let data = AccessLogData {
                start_time: Utc::now() - chrono::Duration::milliseconds(elapsed.as_millis() as i64),
                method: ctx.request_method.clone(),
                path: ctx.request_path.clone(),
                protocol: ctx.protocol.as_str().to_string(),
                response_code: status,
                duration_ms: elapsed.as_millis() as u64,
                duration_us: elapsed.as_micros() as u64,
                bytes_received: ctx.bytes_received,
                bytes_sent: ctx.bytes_sent,
                variant: ctx.selected_variant.clone(),
                provider: ctx.selected_provider.clone(),
                model: ctx.selected_model.clone(),
                upstream_host: ctx.upstream_base_url.clone(),
                streaming: Some(is_streaming),
                conversion: ctx.needs_conversion,
                decision_time_us: ctx.decision_time_us,
                request_headers: ctx.request_headers_snapshot.clone(),
                response_headers: Vec::new(), // reqwest path doesn't easily expose response headers
                input_tokens: ctx.global_codec_ctx.as_ref().and_then(|c| c.get::<ExtractedUsage>(USAGE_KEY)).and_then(|u| u.input_tokens),
                output_tokens: ctx.global_codec_ctx.as_ref().and_then(|c| c.get::<ExtractedUsage>(USAGE_KEY)).and_then(|u| u.output_tokens),
                ratelimit_matched: ctx.ratelimit_matched,
                ratelimit_rule: ctx.ratelimit_rule.clone(),
                error: ctx.error_message.clone(),
                route_strategy: None,
                route_score: None,
                route_candidates: None,
                upstream_response_time_ms: None,
                reqwest_fallback: true,
                response_flags: if ctx.response_flags.is_empty() {
                    None
                } else {
                    Some(ctx.response_flags.join(","))
                },
                downstream_remote_address: ctx.downstream_remote_address.clone(),
                request_duration_ms: ctx
                    .request_body_received_at
                    .map(|t| t.duration_since(ctx.request_start).as_millis() as u64),
                response_duration_ms: ctx
                    .upstream_first_byte_at
                    .map(|t| t.duration_since(ctx.request_start).as_millis() as u64),
                upstream_request_attempt_count: Some(ctx.upstream_request_attempt_count),
                emit_time: Utc::now(),
                request_headers_bytes: Some(ctx.request_headers_snapshot.iter().map(|(k, v)| (k.len() + v.len() + 4) as u64).sum()),
                response_headers_bytes: None,
                response_tx_duration_ms: None,
                upstream_protocol: ctx.outbound_protocol.clone(),
                downstream_local_address: None, // reqwest path doesn't have listener info
                upstream_transport_failure_reason: None,
                connection_termination_details: None,
                response_code_details: ctx.response_code_details.clone(),
                pool_name: None,
                request_id: ctx
                    .request_headers_snapshot
                    .iter()
                    .find(|(k, _)| k == "x-request-id")
                    .map(|(_, v)| v.clone()),
                downstream_tls_version: None,
            };
            logger.log(&data);
        }

        Ok(true) // handled — skip Pingora's upstream pipeline
    }
}

#[async_trait]
impl ProxyHttp for PrismProxy {
    type CTX = ProxyContext;

    fn new_ctx(&self) -> Self::CTX {
        ProxyContext::default()
    }

    /// Phase 1: Identify protocol and prepare routing context.
    async fn request_filter(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<bool, Box<pingora::Error>> {
        let path = session.req_header().uri.path().to_string();
        let method = session.req_header().method.as_str().to_string();
        ctx.protocol = LlmProtocol::from_path(&path);
        ctx.request_start = Instant::now();
        ctx.request_method = method;
        ctx.request_path = path.clone();

        // Capture downstream client address
        if let Some(addr) = session.client_addr() {
            ctx.downstream_remote_address = Some(addr.to_string());
        }

        // Snapshot request headers for access log
        let headers: Vec<(String, String)> = session
            .req_header()
            .headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_lowercase(),
                    v.to_str().unwrap_or("").to_string(),
                )
            })
            .collect();

        // Capture request headers bytes
        ctx.request_headers_bytes = Some(
            headers
                .iter()
                .map(|(k, v)| k.len() as u64 + v.len() as u64 + 4)
                .sum(),
        );
        ctx.request_headers_snapshot = headers;

        debug!(
            path = %path,
            protocol = %ctx.protocol.as_str(),
            "Request received"
        );

        // ── Global plugin chain: on_request_headers (pre-route) ──
        if let Some(global_chain) = self.codec_registry.global_chain() {
            let mut global_ctx = codec::ProxyContext::new();
            global_ctx.inbound_protocol = ctx.protocol.as_str().to_string();
            global_ctx.request_path = ctx.request_path.clone();
            global_ctx.request_method = ctx.request_method.clone();
            for (k, v) in &ctx.request_headers_snapshot {
                global_ctx.request_headers.insert(k.clone(), v.clone());
            }
            let action = global_chain.process_request_headers(&mut global_ctx);
            // Inject rate limiter and request headers for QuotaRecorderPlugin
            if let Some(ref limiter) = self.rate_limiter {
                global_ctx.set(LIMITER_KEY, Arc::clone(limiter));
            }
            global_ctx.set(HEADERS_KEY, ctx.request_headers_snapshot.clone());
            ctx.global_codec_ctx = Some(global_ctx);
            if action == codec::Action::Stop {
                debug!("Global plugin chain stopped request at headers");
                return Ok(true);
            }
        }

        // Rate limit check — early, before routing
        if let Some(ref limiter) = self.rate_limiter {
            let headers: Vec<(String, String)> = session
                .req_header()
                .headers
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_lowercase(),
                        v.to_str().unwrap_or("").to_string(),
                    )
                })
                .collect();

            if let Err(denied) = limiter.check(&headers) {
                warn!(rule = %denied.rule_name, "Rate limit exceeded");
                ctx.ratelimit_matched = true;
                ctx.ratelimit_rule = Some(denied.rule_name.clone());
                ctx.response_flags.push("RL".to_string());

                let mut header =
                    pingora::http::ResponseHeader::build(denied.status_code, None).unwrap();
                if let Some(retry_after) = denied.retry_after_secs {
                    header
                        .insert_header("Retry-After", retry_after.to_string())
                        .unwrap();
                }
                header
                    .insert_header("Content-Type", "application/json")
                    .unwrap();
                header
                    .insert_header("X-RateLimit-Rule", &denied.rule_name)
                    .unwrap();

                let body = serde_json::json!({
                    "error": {
                        "type": "rate_limit_exceeded",
                        "message": denied.message,
                        "rule": denied.rule_name,
                        "metric": denied.metric.to_string(),
                        "current": denied.current,
                        "limit": denied.limit,
                    }
                });

                session.set_keepalive(None);
                session
                    .write_response_header(Box::new(header), true)
                    .await
                    .map_err(|e| {
                        pingora::Error::explain(
                            pingora::ErrorType::WriteError,
                            format!("rate limit response header error: {}", e),
                        )
                    })?;
                session
                    .write_response_body(Some(Bytes::from(body.to_string())), true)
                    .await
                    .map_err(|e| {
                        pingora::Error::explain(
                            pingora::ErrorType::WriteError,
                            format!("rate limit response body error: {}", e),
                        )
                    })?;

                return Ok(true);
            }
        }

        // Try Pingora's native upstream connector first (requires "openssl" feature
        // for proper HTTPS support). Falls back to reqwest if Pingora connector
        // is not working (e.g., missing TLS feature).
        Ok(false) // continue to Pingora upstream pipeline
    }

    /// Phase 2: Determine the upstream peer based on routing decision.
    async fn upstream_peer(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>, Box<pingora::Error>> {
        // Build a minimal route request from path + headers
        let path = session.req_header().uri.path().to_string();
        let protocol_str = ctx.protocol.as_str();

        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: format!("(proxy request via {})", path),
        }];

        let request = RouteRequest {
            messages,
            strategy: None,
            metadata: RouteMetadata::default(),
            analyze_context: None,
            required_protocol: if protocol_str != "unknown" {
                Some(protocol_str.to_string())
            } else {
                None
            },
        };

        let router = self.router.load();
        let route_start = Instant::now();
        match router.route(&request) {
            Ok(decision) => {
                ctx.decision_time_us = Some(route_start.elapsed().as_micros() as u64);
                ctx.selected_variant = Some(decision.selected_variant.clone());
                ctx.selected_provider = Some(decision.selected_provider.clone());
                ctx.selected_model = Some(decision.selected_model.clone());

                // Look up upstream_model_name from the variant
                if let Some(variant) = router.registry.variants.get(&decision.selected_variant) {
                    ctx.upstream_model_name = variant.upstream_model_name.clone();
                }

                // Look up the provider's base_url and protocols
                let provider = router.registry.providers.get(&decision.selected_provider);

                let base_url = provider
                    .map(|p| p.base_url.as_str())
                    .unwrap_or("https://api.openai.com/v1");

                // Extract auth header from provider
                if let Some(p) = provider {
                    match &p.auth {
                        prism_core::types::AuthConfig::ApiKey { key, .. } => {
                            ctx.upstream_auth = Some(key.clone());
                        }
                        prism_core::types::AuthConfig::Custom { headers } => {
                            ctx.upstream_custom_headers = headers
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect();
                        }
                        _ => {}
                    }
                }

                ctx.upstream_base_url = Some(base_url.to_string());

                // ── Codec: determine outbound protocol and check if conversion is needed ──
                let outbound_protocol = provider
                    .and_then(|p| p.protocols.first())
                    .cloned()
                    .unwrap_or_else(|| protocol_str.to_string());

                let inbound = protocol_str;
                let provider_id = &decision.selected_provider;

                if inbound != outbound_protocol
                    && inbound != "unknown"
                    && self.codec_registry.needs_conversion(
                        provider_id,
                        inbound,
                        &outbound_protocol,
                    )
                {
                    ctx.needs_conversion = true;
                    ctx.outbound_protocol = Some(outbound_protocol.clone());
                    debug!(
                        inbound = %inbound,
                        outbound = %outbound_protocol,
                        provider = %provider_id,
                        "Protocol conversion enabled via codec plugin chain"
                    );
                }

                let (host, port, use_tls) = Self::parse_upstream(base_url).inspect_err(|_| {
                    error!(
                        provider = %decision.selected_provider,
                        base_url = %base_url,
                        "Failed to parse upstream URL"
                    );
                })?;

                info!(
                    variant = %decision.selected_variant,
                    provider = %decision.selected_provider,
                    host = %host,
                    port = %port,
                    tls = %use_tls,
                    conversion = %ctx.needs_conversion,
                    "Routing to upstream"
                );

                let mut peer = HttpPeer::new((host, port), use_tls, host.to_string());
                peer.options.connection_timeout = Some(std::time::Duration::from_secs(5));
                peer.options.read_timeout = Some(std::time::Duration::from_secs(60));
                peer.options.write_timeout = Some(std::time::Duration::from_secs(10));
                // Support HTTP/2 and HTTP/1.1 via ALPN negotiation
                // Note: some upstream gateways close connection on H2,
                // so default to H1 for maximum compatibility
                peer.options.set_http_version(1, 1);

                Ok(Box::new(peer))
            }
            Err(e) => {
                ctx.response_flags.push("NR".to_string());
                ctx.error_message = Some(format!("routing error: {}", e));
                error!(error = %e, "Routing failed");
                Err(pingora::Error::explain(
                    pingora::ErrorType::ConnectProxyFailure,
                    format!("routing error: {}", e),
                ))
            }
        }
    }

    /// Phase 3: Modify the upstream request headers.
    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        upstream_request: &mut pingora::http::RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<(), Box<pingora::Error>> {
        // Set the Host header to match the upstream and rewrite URI path
        if let Some(ref base_url) = ctx.upstream_base_url {
            let without_scheme = base_url
                .trim_start_matches("https://")
                .trim_start_matches("http://");
            let host = without_scheme.split('/').next().unwrap_or("localhost");

            // Extract path prefix from base_url (e.g. "/llm-serve/v1" from "https://host/llm-serve/v1")
            let base_path = if let Some(slash_pos) = without_scheme.find('/') {
                let p = &without_scheme[slash_pos..];
                p.trim_end_matches('/')
            } else {
                ""
            };

            // Rewrite the request URI to use the base_url path.
            // Client sends e.g. /v1/chat/completions; if base_url has /llm-serve/v1,
            // we replace the /v1 prefix with /llm-serve/v1.
            if !base_path.is_empty() {
                let original_path = upstream_request.uri.path().to_string();
                // Try to replace the standard API prefix (/v1) with the base path
                let new_path = if original_path.starts_with("/v1/") || original_path == "/v1" {
                    // Replace /v1 with base_path (which may end with /v1 or another version)
                    if base_path.ends_with("/v1") {
                        format!("{}{}", base_path, &original_path[3..]) // skip "/v1"
                    } else {
                        format!("{}{}", base_path, original_path)
                    }
                } else {
                    format!("{}{}", base_path, original_path)
                };
                if let Ok(uri) = new_path.parse::<http::Uri>() {
                    upstream_request.set_uri(uri);
                    debug!(
                        original = %original_path,
                        rewritten = %new_path,
                        "Rewrote upstream path with base_url prefix"
                    );
                }
            }

            upstream_request.insert_header("Host", host).map_err(|e| {
                pingora::Error::explain(
                    pingora::ErrorType::InternalError,
                    format!("header error: {:?}", e),
                )
            })?;
        }

        // Set the Authorization header
        if let Some(ref auth) = ctx.upstream_auth {
            upstream_request
                .insert_header("Authorization", auth.as_str())
                .map_err(|e| {
                    pingora::Error::explain(
                        pingora::ErrorType::InternalError,
                        format!("header error: {:?}", e),
                    )
                })?;
        }

        // Set custom headers (e.g. x-api-key for wanqing)
        {
            let headers_to_set: Vec<(String, String)> = ctx.upstream_custom_headers.clone();
            for (key, value) in headers_to_set {
                upstream_request
                    .insert_header(
                        http::header::HeaderName::from_bytes(key.as_bytes())
                            .unwrap_or(http::header::CONTENT_TYPE),
                        &value,
                    )
                    .map_err(|e| {
                        pingora::Error::explain(
                            pingora::ErrorType::InternalError,
                            format!("custom header error: {:?}", e),
                        )
                    })?;
            }
        }

        // ── Codec: process request headers through plugin chain ──
        if ctx.needs_conversion {
            // Initialize codec context now that we have full routing info
            Self::init_codec_ctx(ctx);

            if let Some(ref mut codec_ctx) = ctx.codec_ctx {
                // Copy original request headers into codec context
                let req_header = session.req_header();
                codec_ctx.request_path = req_header.uri.path().to_string();
                codec_ctx.request_method = req_header.method.as_str().to_string();
                for (name, value) in req_header.headers.iter() {
                    if let Ok(v) = value.to_str() {
                        codec_ctx
                            .request_headers
                            .insert(name.as_str().to_string(), v.to_string());
                    }
                }

                // Detect if streaming (best-effort from headers or content)
                if let Some(accept) = codec_ctx.request_headers.get("accept") {
                    if accept.contains("text/event-stream") {
                        codec_ctx.is_streaming = true;
                    }
                }

                let provider_id = ctx.selected_provider.as_deref().unwrap_or("");
                let inbound = ctx.protocol.as_str();
                let outbound = ctx.outbound_protocol.as_deref().unwrap_or("");

                if let Some(chain) = self
                    .codec_registry
                    .get_chain(provider_id, inbound, outbound)
                {
                    let action = chain.process_request_headers(codec_ctx);

                    if action == codec::Action::Continue {
                        // Apply any headers that the plugin chain modified.
                        // Clone to owned Strings since insert_header requires
                        // 'static str or String for IntoCaseHeaderName.
                        let modified_headers: Vec<(String, String)> = codec_ctx
                            .request_headers
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        let modified_auth = codec_ctx.upstream_auth.clone();
                        let modified_path = if !codec_ctx.upstream_path.is_empty() {
                            Some(codec_ctx.upstream_path.clone())
                        } else {
                            None
                        };

                        // Update upstream path if changed by plugin
                        if let Some(path) = modified_path {
                            if let Ok(uri) = path.parse::<http::Uri>() {
                                upstream_request.set_uri(uri);
                            }
                        }

                        for (key, value) in modified_headers {
                            let _ = upstream_request.insert_header(key, &value);
                        }

                        // Override auth if plugin changed it
                        if let Some(auth) = modified_auth {
                            let _ =
                                upstream_request.insert_header("Authorization".to_string(), &auth);
                        }

                        debug!("Codec plugin chain processed request headers");
                    } else {
                        warn!("Codec plugin chain returned Stop on request headers");
                    }
                }
            }
        }

        // ── Global plugin chain: update routing info after route decision ──
        if let Some(ref mut global_ctx) = ctx.global_codec_ctx {
            global_ctx.selected_variant = ctx.selected_variant.clone().unwrap_or_default();
            global_ctx.selected_provider = ctx.selected_provider.clone().unwrap_or_default();
            global_ctx.selected_model = ctx.selected_model.clone().unwrap_or_default();
            if let Some(ref base_url) = ctx.upstream_base_url {
                global_ctx.upstream_host = base_url.clone();
            }
            if let Some(ref outbound) = ctx.outbound_protocol {
                global_ctx.outbound_protocol = outbound.clone();
            }
        }

        debug!(
            variant = ?ctx.selected_variant,
            "Upstream request filter applied"
        );

        Ok(())
    }

    /// Phase 3b: Handle request body — buffer and transform if conversion is needed,
    /// and/or rewrite the model name for upstream.
    async fn request_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<(), Box<pingora::Error>> {
        let needs_model_rewrite = ctx.upstream_model_name.is_some();

        if !ctx.needs_conversion && !needs_model_rewrite {
            return Ok(());
        }

        // Buffer the body chunks
        if let Some(ref chunk) = body {
            if let Some(ref mut buf) = ctx.request_body_buffer {
                buf.extend_from_slice(chunk);
            } else {
                ctx.request_body_buffer = Some(chunk.to_vec());
            }
        }

        // When we have the complete body, optionally rewrite model name and/or
        // transform through the codec plugin chain.
        if end_of_stream {
            let mut full_body = ctx.request_body_buffer.take().unwrap_or_default();

            // Model name rewrite — before codec conversion
            if let Some(ref upstream_name) = ctx.upstream_model_name {
                if !full_body.is_empty() {
                    if let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&full_body) {
                        if json.get("model").is_some() {
                            json["model"] = serde_json::Value::String(upstream_name.clone());
                            if let Ok(rewritten) = serde_json::to_vec(&json) {
                                full_body = rewritten;
                            }
                        }
                    }
                }
            }

            if ctx.needs_conversion && !full_body.is_empty() {
                let provider_id = ctx.selected_provider.as_deref().unwrap_or("");
                let inbound = ctx.protocol.as_str();
                let outbound = ctx.outbound_protocol.as_deref().unwrap_or("");

                if let Some(chain) = self
                    .codec_registry
                    .get_chain(provider_id, inbound, outbound)
                {
                    if let Some(ref mut codec_ctx) = ctx.codec_ctx {
                        match chain.process_request_body(codec_ctx, full_body) {
                            Ok(transformed) => {
                                debug!(
                                    original_len = transformed.len(),
                                    "Codec plugin chain transformed request body"
                                );
                                *body = Some(Bytes::from(transformed));
                                return Ok(());
                            }
                            Err(e) => {
                                error!(error = %e, "Codec plugin chain failed to transform request body");
                                return Err(pingora::Error::explain(
                                    pingora::ErrorType::InternalError,
                                    format!("codec request body error: {}", e),
                                ));
                            }
                        }
                    }
                }
            }

            // Pass through (possibly rewritten) body
            *body = Some(Bytes::from(full_body));
        } else {
            // We're buffering — suppress sending partial chunks upstream.
            // Clear the chunk so Pingora doesn't send it yet.
            *body = None;
        }

        Ok(())
    }

    /// Phase 4: Modify response headers from upstream (after cache).
    async fn response_filter(
        &self,
        _session: &mut Session,
        upstream_response: &mut pingora::http::ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> Result<(), Box<pingora::Error>> {
        // ── Global plugin chain: on_response_headers (all requests) ──
        if let Some(global_chain) = self.codec_registry.global_chain() {
            if let Some(ref mut global_ctx) = ctx.global_codec_ctx {
                global_ctx.response_status = Some(upstream_response.status.as_u16());
                for (name, value) in upstream_response.headers.iter() {
                    if let Ok(v) = value.to_str() {
                        global_ctx
                            .response_headers
                            .insert(name.as_str().to_string(), v.to_string());
                    }
                }
                global_chain.process_response_headers(global_ctx);
            }
        }

        if !ctx.needs_conversion {
            return Ok(());
        }

        let provider_id = ctx.selected_provider.as_deref().unwrap_or("");
        let inbound = ctx.protocol.as_str();
        let outbound = ctx.outbound_protocol.as_deref().unwrap_or("");

        if let Some(chain) = self
            .codec_registry
            .get_chain(provider_id, inbound, outbound)
        {
            if let Some(ref mut codec_ctx) = ctx.codec_ctx {
                // Copy upstream response headers into codec context
                codec_ctx.response_status = Some(upstream_response.status.as_u16());
                for (name, value) in upstream_response.headers.iter() {
                    if let Ok(v) = value.to_str() {
                        codec_ctx
                            .response_headers
                            .insert(name.as_str().to_string(), v.to_string());
                    }
                }

                let action = chain.process_response_headers(codec_ctx);

                if action == codec::Action::Continue {
                    // Sync modified response headers back
                    for (key, value) in &codec_ctx.response_headers {
                        let _ = upstream_response.insert_header(
                            http::header::HeaderName::from_bytes(key.as_bytes())
                                .unwrap_or(http::header::CONTENT_TYPE),
                            value.as_str(),
                        );
                    }
                    debug!("Codec plugin chain processed response headers");
                } else {
                    warn!("Codec plugin chain returned Stop on response headers");
                }
            }
        }

        Ok(())
    }

    /// Phase 4b: Transform response body chunks.
    fn response_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<Option<std::time::Duration>, Box<pingora::Error>>
    where
        Self::CTX: Send + Sync,
    {
        if !ctx.needs_conversion {
            return Ok(None);
        }

        let is_streaming = ctx
            .codec_ctx
            .as_ref()
            .map(|c| c.is_streaming)
            .unwrap_or(false);

        if is_streaming {
            // Streaming mode: process each chunk individually
            if let Some(ref chunk) = body {
                if let Ok(chunk_str) = std::str::from_utf8(chunk) {
                    let provider_id = ctx.selected_provider.as_deref().unwrap_or("");
                    let inbound = ctx.protocol.as_str();
                    let outbound = ctx.outbound_protocol.as_deref().unwrap_or("");

                    if let Some(chain) =
                        self.codec_registry
                            .get_chain(provider_id, inbound, outbound)
                    {
                        if let Some(ref mut codec_ctx) = ctx.codec_ctx {
                            match chain.process_response_stream_chunk(codec_ctx, chunk_str) {
                                Ok(Some(transformed)) => {
                                    *body = Some(Bytes::from(transformed));
                                }
                                Ok(None) => {
                                    // Chunk filtered out
                                    *body = Some(Bytes::new());
                                }
                                Err(e) => {
                                    warn!(error = %e, "Codec stream chunk transform error");
                                    // Pass through on error
                                }
                            }
                        }
                    }
                }
            }
        } else {
            // Non-streaming: buffer body chunks, transform on end_of_stream
            if let Some(ref chunk) = body {
                if let Some(ref mut buf) = ctx.response_body_buffer {
                    buf.extend_from_slice(chunk);
                }
            }

            if end_of_stream {
                let full_body = ctx.response_body_buffer.take().unwrap_or_default();

                if !full_body.is_empty() {
                    let provider_id = ctx.selected_provider.as_deref().unwrap_or("");
                    let inbound = ctx.protocol.as_str();
                    let outbound = ctx.outbound_protocol.as_deref().unwrap_or("");

                    if let Some(chain) =
                        self.codec_registry
                            .get_chain(provider_id, inbound, outbound)
                    {
                        if let Some(ref mut codec_ctx) = ctx.codec_ctx {
                            match chain.process_response_body(codec_ctx, full_body) {
                                Ok(transformed) => {
                                    debug!(
                                        len = transformed.len(),
                                        "Codec plugin chain transformed response body"
                                    );
                                    *body = Some(Bytes::from(transformed));
                                }
                                Err(e) => {
                                    error!(
                                        error = %e,
                                        "Codec plugin chain failed to transform response body"
                                    );
                                    // Pass through the raw buffered body on error
                                }
                            }
                        }
                    } else {
                        *body = Some(Bytes::from(full_body));
                    }
                }
            } else {
                // Buffering — suppress partial chunks from reaching downstream
                *body = None;
            }
        }

        Ok(None)
    }

    /// Phase 5: Log the completed request and record metrics.
    async fn logging(
        &self,
        session: &mut Session,
        _e: Option<&pingora::Error>,
        ctx: &mut Self::CTX,
    ) {
        let elapsed = ctx.request_start.elapsed();
        let status = session
            .response_written()
            .map(|r| r.status.as_u16())
            .unwrap_or(0);

        // Log Pingora-level errors for debugging 502s
        if let Some(e) = _e {
            ctx.error_message = Some(format!("{}: {:?}", e, e.etype()));
            // Set response flags based on error type
            let flag = match e.etype() {
                pingora::ErrorType::ConnectTimedout => "UT",
                pingora::ErrorType::ConnectRefused => "UF",
                pingora::ErrorType::ConnectProxyFailure => "UF",
                pingora::ErrorType::ReadTimedout => "UT",
                pingora::ErrorType::WriteTimedout => "UT",
                pingora::ErrorType::ReadError => "UC",
                pingora::ErrorType::WriteError => "UC",
                _ => "UF",
            };
            if !ctx.response_flags.contains(&flag.to_string()) {
                ctx.response_flags.push(flag.to_string());
            }
            error!(
                error = %e,
                error_type = ?e.etype(),
                "Pingora proxy error"
            );
        }

        info!(
            variant = ?ctx.selected_variant,
            provider = ?ctx.selected_provider,
            model = ?ctx.selected_model,
            protocol = %ctx.protocol.as_str(),
            status = %status,
            latency_ms = %elapsed.as_millis(),
            conversion = %ctx.needs_conversion,
            outbound_protocol = ?ctx.outbound_protocol,
            "Request completed"
        );

        // Record outcome to metrics collector → DataLayer feedback loop
        if let Some(ref variant_id) = ctx.selected_variant {
            self.metrics.record(RequestRecord {
                variant_id: variant_id.clone(),
                latency_ms: elapsed.as_millis() as u64,
                ttft_ms: ctx.upstream_first_byte_at.map(|t| t.duration_since(ctx.request_start).as_millis() as u64),
                success: (200..400).contains(&status),
                input_tokens: ctx.global_codec_ctx.as_ref().and_then(|c| c.get::<ExtractedUsage>(USAGE_KEY)).and_then(|u| u.input_tokens.map(|v| v as u32)),
                output_tokens: ctx.global_codec_ctx.as_ref().and_then(|c| c.get::<ExtractedUsage>(USAGE_KEY)).and_then(|u| u.output_tokens.map(|v| v as u32)),
                timestamp: Instant::now(),
            });
        }

        // ── Global plugin chain: on_log (all requests) ──
        if let Some(global_chain) = self.codec_registry.global_chain() {
            if let Some(ref mut global_ctx) = ctx.global_codec_ctx {
                // Update final status info before on_log
                global_ctx.response_status = Some(status);
                global_chain.process_log(global_ctx);
            }
        }

        // Access log
        if let Some(ref logger) = self.access_logger {
            // Collect response headers
            let response_headers = session
                .response_written()
                .map(|r| {
                    r.headers
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.as_str().to_lowercase(),
                                v.to_str().unwrap_or("").to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();

            let data = AccessLogData {
                start_time: Utc::now() - chrono::Duration::milliseconds(elapsed.as_millis() as i64),
                method: ctx.request_method.clone(),
                path: ctx.request_path.clone(),
                protocol: ctx.protocol.as_str().to_string(),
                response_code: status,
                duration_ms: elapsed.as_millis() as u64,
                duration_us: elapsed.as_micros() as u64,
                bytes_received: ctx.bytes_received,
                bytes_sent: ctx.bytes_sent,
                variant: ctx.selected_variant.clone(),
                provider: ctx.selected_provider.clone(),
                model: ctx.selected_model.clone(),
                upstream_host: ctx.upstream_base_url.clone(),
                streaming: ctx.is_streaming,
                conversion: ctx.needs_conversion,
                decision_time_us: ctx.decision_time_us,
                request_headers: ctx.request_headers_snapshot.clone(),
                response_headers,
                input_tokens: ctx.global_codec_ctx.as_ref().and_then(|c| c.get::<ExtractedUsage>(USAGE_KEY)).and_then(|u| u.input_tokens),
                output_tokens: ctx.global_codec_ctx.as_ref().and_then(|c| c.get::<ExtractedUsage>(USAGE_KEY)).and_then(|u| u.output_tokens),
                ratelimit_matched: ctx.ratelimit_matched,
                ratelimit_rule: ctx.ratelimit_rule.clone(),
                error: ctx.error_message.clone(),
                route_strategy: None,
                route_score: None,
                route_candidates: None,
                upstream_response_time_ms: ctx.upstream_first_byte_at.map(|t| t.duration_since(ctx.request_start).as_millis() as u64),
                reqwest_fallback: ctx.reqwest_fallback,
                response_flags: if ctx.response_flags.is_empty() {
                    None
                } else {
                    Some(ctx.response_flags.join(","))
                },
                downstream_remote_address: ctx.downstream_remote_address.clone(),
                request_duration_ms: ctx
                    .request_body_received_at
                    .map(|t| t.duration_since(ctx.request_start).as_millis() as u64),
                response_duration_ms: ctx
                    .upstream_first_byte_at
                    .map(|t| t.duration_since(ctx.request_start).as_millis() as u64),
                upstream_request_attempt_count: Some(ctx.upstream_request_attempt_count),
                emit_time: Utc::now(),
                request_headers_bytes: ctx.request_headers_bytes,
                response_headers_bytes: ctx.response_headers_bytes,
                response_tx_duration_ms: ctx.upstream_first_byte_at.map(|ttfb| {
                    let total = elapsed.as_millis() as u64;
                    let ttfb_ms = ttfb.duration_since(ctx.request_start).as_millis() as u64;
                    total.saturating_sub(ttfb_ms)
                }),
                upstream_protocol: ctx.outbound_protocol.clone(),
                downstream_local_address: session.server_addr().map(|a| a.to_string()),
                upstream_transport_failure_reason: ctx.upstream_transport_failure_reason.clone(),
                connection_termination_details: ctx.connection_termination_details.clone(),
                response_code_details: ctx.response_code_details.clone(),
                pool_name: None,
                request_id: ctx
                    .request_headers_snapshot
                    .iter()
                    .find(|(k, _)| k == "x-request-id")
                    .map(|(_, v)| v.clone()),
                downstream_tls_version: None,
            };
            logger.log(&data);
        }
    }
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::chain::ProxyPluginChain;
    use crate::codec::registry::CodecRegistry;
    use crate::codec::{Action, ProxyPlugin as CodecProxyPlugin};
    use crate::ratelimit::{RateLimitRule, RateLimiter};

    // ── ProxyContext tests ──

    #[test]
    fn test_proxy_context_default() {
        let ctx = ProxyContext::default();
        assert_eq!(ctx.protocol, LlmProtocol::Unknown);
        assert!(ctx.selected_variant.is_none());
        assert!(ctx.selected_provider.is_none());
        assert!(ctx.selected_model.is_none());
        assert!(ctx.upstream_base_url.is_none());
        assert!(ctx.upstream_auth.is_none());
        assert!(ctx.response_status.is_none());
        // Codec fields
        assert!(ctx.codec_ctx.is_none());
        assert!(!ctx.needs_conversion);
        assert!(ctx.request_body_buffer.is_none());
        assert!(ctx.response_body_buffer.is_none());
        assert!(ctx.outbound_protocol.is_none());
    }

    #[test]
    fn test_proxy_context_codec_fields() {
        let mut ctx = ProxyContext {
            needs_conversion: true,
            outbound_protocol: Some("anthropic".to_string()),
            ..Default::default()
        };

        let codec_ctx = codec::ProxyContext::new();
        ctx.codec_ctx = Some(codec_ctx);

        assert!(ctx.needs_conversion);
        assert_eq!(ctx.outbound_protocol, Some("anthropic".to_string()));
        assert!(ctx.codec_ctx.is_some());
    }

    #[test]
    fn test_proxy_context_with_body_buffers() {
        let mut ctx = ProxyContext {
            request_body_buffer: Some(Vec::new()),
            response_body_buffer: Some(Vec::new()),
            ..Default::default()
        };

        ctx.request_body_buffer
            .as_mut()
            .unwrap()
            .extend_from_slice(b"request");
        ctx.response_body_buffer
            .as_mut()
            .unwrap()
            .extend_from_slice(b"response");

        assert_eq!(ctx.request_body_buffer.as_ref().unwrap(), b"request");
        assert_eq!(ctx.response_body_buffer.as_ref().unwrap(), b"response");
    }

    // ── PrismProxy construction tests ──

    #[test]
    fn test_prism_proxy_new_has_empty_codec_registry() {
        let registry = prism_core::registry::InMemoryRegistry::new();
        let router = Arc::new(ArcSwap::new(Arc::new(PrismRouter::new(registry))));
        let metrics = Arc::new(MetricsCollector::new(100));

        let proxy = PrismProxy::new(router, metrics);

        // Empty registry should not need conversion for anything
        assert!(!proxy
            .codec_registry
            .needs_conversion("any", "openai", "anthropic"));
    }

    #[test]
    fn test_prism_proxy_with_codec_registry() {
        let registry = prism_core::registry::InMemoryRegistry::new();
        let router = Arc::new(ArcSwap::new(Arc::new(PrismRouter::new(registry))));
        let metrics = Arc::new(MetricsCollector::new(100));

        let mut codec_reg = CodecRegistry::new();
        codec_reg.register_protocol_pair("openai", "anthropic", ProxyPluginChain::new());

        let proxy = PrismProxy::with_codec_registry(router, metrics, Arc::new(codec_reg));

        assert!(proxy
            .codec_registry
            .needs_conversion("any", "openai", "anthropic"));
        assert!(!proxy
            .codec_registry
            .needs_conversion("any", "anthropic", "openai"));
    }

    // ── init_codec_ctx tests ──

    #[test]
    fn test_init_codec_ctx() {
        let mut ctx = ProxyContext {
            protocol: LlmProtocol::OpenAI,
            selected_provider: Some("anthropic-main".to_string()),
            selected_model: Some("claude-3-opus".to_string()),
            selected_variant: Some("claude-variant".to_string()),
            upstream_base_url: Some("https://api.anthropic.com/v1".to_string()),
            upstream_auth: Some("Bearer sk-test".to_string()),
            outbound_protocol: Some("anthropic".to_string()),
            ..Default::default()
        };

        PrismProxy::init_codec_ctx(&mut ctx);

        let codec_ctx = ctx.codec_ctx.as_ref().unwrap();
        assert_eq!(codec_ctx.inbound_protocol, "openai");
        assert_eq!(codec_ctx.outbound_protocol, "anthropic");
        assert_eq!(codec_ctx.selected_provider, "anthropic-main");
        assert_eq!(codec_ctx.selected_model, "claude-3-opus");
        assert_eq!(codec_ctx.selected_variant, "claude-variant");
        assert_eq!(codec_ctx.upstream_host, "api.anthropic.com");
        assert_eq!(codec_ctx.upstream_auth, Some("Bearer sk-test".to_string()));

        // Buffers should be initialized
        assert!(ctx.request_body_buffer.is_some());
        assert!(ctx.response_body_buffer.is_some());
    }

    #[test]
    fn test_init_codec_ctx_without_upstream_url() {
        let mut ctx = ProxyContext {
            protocol: LlmProtocol::OpenAI,
            outbound_protocol: Some("anthropic".to_string()),
            ..Default::default()
        };

        PrismProxy::init_codec_ctx(&mut ctx);

        let codec_ctx = ctx.codec_ctx.as_ref().unwrap();
        assert_eq!(codec_ctx.upstream_host, "");
        assert!(codec_ctx.upstream_auth.is_none());
    }

    // ── Parse upstream tests ──

    #[test]
    fn test_parse_upstream_https() {
        let (host, port, tls) = PrismProxy::parse_upstream("https://api.openai.com/v1").unwrap();
        assert_eq!(host, "api.openai.com");
        assert_eq!(port, 443);
        assert!(tls);
    }

    #[test]
    fn test_parse_upstream_http() {
        let (host, port, tls) = PrismProxy::parse_upstream("http://localhost:8080/v1").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
        assert!(!tls);
    }

    #[test]
    fn test_parse_upstream_no_port() {
        let (host, port, tls) = PrismProxy::parse_upstream("http://example.com/path").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert!(!tls);
    }

    // ── Mock plugin chain integration tests ──

    /// A test plugin that transforms request/response bodies.
    struct MockCodecPlugin {
        name: String,
        /// Prefix to add to request body
        req_prefix: String,
        /// Prefix to add to response body
        resp_prefix: String,
        /// Header to inject
        inject_header: Option<(String, String)>,
        /// Path to rewrite to
        rewrite_path: Option<String>,
    }

    impl MockCodecPlugin {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                req_prefix: format!("[{}:req]", name),
                resp_prefix: format!("[{}:resp]", name),
                inject_header: None,
                rewrite_path: None,
            }
        }

        fn with_header(mut self, key: &str, value: &str) -> Self {
            self.inject_header = Some((key.to_string(), value.to_string()));
            self
        }

        fn with_path(mut self, path: &str) -> Self {
            self.rewrite_path = Some(path.to_string());
            self
        }
    }

    impl CodecProxyPlugin for MockCodecPlugin {
        fn name(&self) -> &str {
            &self.name
        }

        fn on_request_headers(&self, ctx: &mut codec::ProxyContext) -> Action {
            if let Some((ref k, ref v)) = self.inject_header {
                ctx.request_headers.insert(k.clone(), v.clone());
            }
            if let Some(ref path) = self.rewrite_path {
                ctx.upstream_path = path.clone();
            }
            Action::Continue
        }

        fn on_request_body(
            &self,
            _ctx: &mut codec::ProxyContext,
            body: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            let mut new_body = self.req_prefix.as_bytes().to_vec();
            new_body.extend_from_slice(&body);
            Ok(new_body)
        }

        fn on_response_headers(&self, _ctx: &mut codec::ProxyContext) -> Action {
            Action::Continue
        }

        fn on_response_body(
            &self,
            _ctx: &mut codec::ProxyContext,
            body: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            let mut new_body = self.resp_prefix.as_bytes().to_vec();
            new_body.extend_from_slice(&body);
            Ok(new_body)
        }

        fn on_response_stream_chunk(
            &self,
            _ctx: &mut codec::ProxyContext,
            chunk: &str,
        ) -> Result<Option<String>, String> {
            Ok(Some(format!("[{}]{}", self.name, chunk)))
        }
    }

    #[test]
    fn test_codec_registry_with_mock_plugin_request_body() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(MockCodecPlugin::new("openai2anthropic")));

        let mut registry = CodecRegistry::new();
        registry.register_protocol_pair("openai", "anthropic", chain);

        let mut codec_ctx = codec::ProxyContext::new();
        codec_ctx.inbound_protocol = "openai".to_string();
        codec_ctx.outbound_protocol = "anthropic".to_string();

        let chain = registry
            .get_chain("any-provider", "openai", "anthropic")
            .unwrap();

        // Test request body transformation
        let body = br#"{"model":"gpt-4","messages":[{"role":"user","content":"hi"}]}"#.to_vec();
        let result = chain
            .process_request_body(&mut codec_ctx, body.clone())
            .unwrap();
        assert!(result.starts_with(b"[openai2anthropic:req]"));
        assert!(result.ends_with(&body));
    }

    #[test]
    fn test_codec_registry_with_mock_plugin_response_body() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(MockCodecPlugin::new("converter")));

        let mut registry = CodecRegistry::new();
        registry.register_protocol_pair("openai", "anthropic", chain);

        let mut codec_ctx = codec::ProxyContext::new();
        let chain = registry.get_chain("any", "openai", "anthropic").unwrap();

        let body = b"upstream response".to_vec();
        let result = chain.process_response_body(&mut codec_ctx, body).unwrap();
        assert_eq!(result, b"[converter:resp]upstream response");
    }

    #[test]
    fn test_codec_registry_with_mock_plugin_headers() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(
            MockCodecPlugin::new("header-plugin")
                .with_header("x-anthropic-version", "2023-06-01")
                .with_path("/v1/messages"),
        ));

        let mut registry = CodecRegistry::new();
        registry.register_protocol_pair("openai", "anthropic", chain);

        let mut codec_ctx = codec::ProxyContext::new();
        let chain = registry.get_chain("any", "openai", "anthropic").unwrap();

        let action = chain.process_request_headers(&mut codec_ctx);
        assert_eq!(action, Action::Continue);
        assert_eq!(
            codec_ctx.request_headers.get("x-anthropic-version"),
            Some(&"2023-06-01".to_string())
        );
        assert_eq!(codec_ctx.upstream_path, "/v1/messages");
    }

    #[test]
    fn test_codec_registry_with_mock_plugin_stream_chunk() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(MockCodecPlugin::new("streamer")));

        let mut registry = CodecRegistry::new();
        registry.register_protocol_pair("openai", "anthropic", chain);

        let mut codec_ctx = codec::ProxyContext::new();
        let chain = registry.get_chain("any", "openai", "anthropic").unwrap();

        let result = chain
            .process_response_stream_chunk(&mut codec_ctx, "data: {\"id\":\"1\"}\n\n")
            .unwrap();
        assert_eq!(
            result,
            Some("[streamer]data: {\"id\":\"1\"}\n\n".to_string())
        );
    }

    #[test]
    fn test_no_conversion_when_same_protocol() {
        let registry = CodecRegistry::new();
        // Same inbound and outbound — never needs conversion
        assert!(!registry.needs_conversion("any", "openai", "openai"));
    }

    #[test]
    fn test_no_conversion_when_registry_empty() {
        let registry = CodecRegistry::new();
        assert!(!registry.needs_conversion("any", "openai", "anthropic"));
    }

    #[test]
    fn test_multi_plugin_chain_body_transform() {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(MockCodecPlugin::new("A")));
        chain.add(Box::new(MockCodecPlugin::new("B")));

        let mut registry = CodecRegistry::new();
        registry.register_protocol_pair("openai", "anthropic", chain);

        let mut codec_ctx = codec::ProxyContext::new();
        let chain = registry.get_chain("any", "openai", "anthropic").unwrap();

        // Request: A → B (in order)
        let result = chain
            .process_request_body(&mut codec_ctx, b"X".to_vec())
            .unwrap();
        assert_eq!(result, b"[B:req][A:req]X");

        // Response: B → A (reverse order)
        let result = chain
            .process_response_body(&mut codec_ctx, b"Y".to_vec())
            .unwrap();
        assert_eq!(result, b"[A:resp][B:resp]Y");
    }

    // ── PrismProxy with rate limiter tests ──

    #[test]
    fn test_prism_proxy_without_rate_limiter() {
        let registry = prism_core::registry::InMemoryRegistry::new();
        let router = Arc::new(ArcSwap::new(Arc::new(PrismRouter::new(registry))));
        let metrics = Arc::new(MetricsCollector::new(100));

        let proxy = PrismProxy::new(router, metrics);
        assert!(proxy.rate_limiter.is_none());
    }

    #[test]
    fn test_prism_proxy_with_rate_limiter() {
        let registry = prism_core::registry::InMemoryRegistry::new();
        let router = Arc::new(ArcSwap::new(Arc::new(PrismRouter::new(registry))));
        let metrics = Arc::new(MetricsCollector::new(100));

        let limiter = Arc::new(RateLimiter::new(vec![RateLimitRule {
            name: "global".to_string(),
            key_headers: vec![],
            window_secs: 60,
            max_requests: Some(100),
            max_tokens: None,
            quotas: vec![],
            overrides: vec![],
            enabled: true,
            status_code: 429,
            message: "Rate limit exceeded".to_string(),
        }]));

        let proxy = PrismProxy::new(router, metrics).with_rate_limiter(limiter.clone());
        assert!(proxy.rate_limiter.is_some());
        assert_eq!(proxy.rate_limiter.as_ref().unwrap().rules().len(), 1);
        assert_eq!(
            proxy.rate_limiter.as_ref().unwrap().rules()[0].name,
            "global"
        );
    }

    #[test]
    fn test_prism_proxy_with_rate_limiter_and_codec() {
        let registry = prism_core::registry::InMemoryRegistry::new();
        let router = Arc::new(ArcSwap::new(Arc::new(PrismRouter::new(registry))));
        let metrics = Arc::new(MetricsCollector::new(100));

        let mut codec_reg = CodecRegistry::new();
        codec_reg.register_protocol_pair("openai", "anthropic", ProxyPluginChain::new());

        let limiter = Arc::new(RateLimiter::new(vec![]));

        let proxy = PrismProxy::with_codec_registry(router, metrics, Arc::new(codec_reg))
            .with_rate_limiter(limiter);

        assert!(proxy.rate_limiter.is_some());
        assert!(proxy
            .codec_registry
            .needs_conversion("any", "openai", "anthropic"));
    }

    // ── upstream_model_name rewrite tests ──

    #[test]
    fn test_model_name_rewrite() {
        // Verify upstream_model_name rewrites the model field in the body
        let body = br#"{"model":"deepseek-r1","messages":[{"role":"user","content":"hi"}]}"#;
        let mut json: serde_json::Value = serde_json::from_slice(body).unwrap();
        let upstream_name = "deepseek-reasoner";
        json["model"] = serde_json::Value::String(upstream_name.to_string());
        let rewritten = serde_json::to_vec(&json).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&rewritten).unwrap();
        assert_eq!(parsed["model"], "deepseek-reasoner");
        assert_eq!(parsed["messages"][0]["content"], "hi"); // other fields unchanged
    }

    #[test]
    fn test_model_name_no_rewrite_when_none() {
        // When upstream_model_name is None, body should remain unchanged
        let body = br#"{"model":"deepseek-r1","messages":[{"role":"user","content":"hi"}]}"#;
        let upstream_model_name: Option<String> = None;

        let mut full_body = body.to_vec();
        if let Some(ref upstream_name) = upstream_model_name {
            if let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&full_body) {
                if json.get("model").is_some() {
                    json["model"] = serde_json::Value::String(upstream_name.clone());
                    if let Ok(rewritten) = serde_json::to_vec(&json) {
                        full_body = rewritten;
                    }
                }
            }
        }

        let parsed: serde_json::Value = serde_json::from_slice(&full_body).unwrap();
        assert_eq!(parsed["model"], "deepseek-r1"); // unchanged
        assert_eq!(parsed["messages"][0]["content"], "hi");
    }

    #[test]
    fn test_proxy_context_upstream_model_name_default() {
        let ctx = ProxyContext::default();
        assert!(ctx.upstream_model_name.is_none());
    }

    #[test]
    fn test_proxy_context_upstream_model_name_set() {
        let ctx = ProxyContext {
            upstream_model_name: Some("deepseek-reasoner".to_string()),
            ..Default::default()
        };
        assert_eq!(
            ctx.upstream_model_name,
            Some("deepseek-reasoner".to_string())
        );
    }
}
