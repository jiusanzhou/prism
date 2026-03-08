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
use crate::provider;
use crate::provider::registry::ProviderPluginRegistry;
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
    /// Auth value for upstream (key/token)
    pub upstream_auth: Option<String>,
    /// Auth header name (default: "Authorization")
    pub upstream_auth_header: String,
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
    /// Provider plugin request context (set in upstream_peer).
    pub provider_ctx: Option<provider::ProviderRequestContext>,
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
    /// Request ID (generated or from X-Request-ID header).
    pub request_id: String,
    /// Fallback chain from routing decision for retry/failover.
    pub retry_fallback_chain: Vec<String>,
    /// P/D routing decision from RoutingDecision.
    pub pd_decision: Option<prism_core::plugins::pd_routing::PDDecision>,
    /// Captured request body bytes for P/D prefill fire-and-forget.
    pub original_body_bytes: Option<Vec<u8>>,
    /// Client-requested model name extracted from request body.
    pub requested_model: Option<String>,
    /// Original model name before alias resolution (set only when alias was applied).
    pub original_model: Option<String>,
    /// Active concurrency guards — dropped when request completes.
    pub concurrency_guards: Vec<crate::ratelimit::concurrency::ConcurrencyGuard>,
    /// Whether X-Prism-Debug header was set on the request.
    pub debug_mode: bool,

    // ── Routing decision metadata ──
    /// Routing strategy description (from RoutingReason).
    pub route_strategy: Option<String>,
    /// Routing score (from RoutingDecision.score).
    pub route_score: Option<f64>,
    /// Number of candidates considered/filtered during routing.
    pub route_candidates: Option<u32>,
    /// Pool name associated with the selected variant.
    pub pool_name: Option<String>,
    /// Resolved upstream timeout for this request (per-provider or global).
    pub resolved_timeout_secs: Option<u64>,
    /// Resolved idle timeout for this request (per-provider or global).
    pub resolved_idle_timeout_secs: Option<u64>,
    /// Last SSE data chunk timestamp (for idle timeout enforcement).
    pub last_data_chunk_at: Option<Instant>,
    /// Whether upstream 5xx triggered retry.
    pub retry_on_5xx: bool,
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
            upstream_auth_header: "Authorization".to_string(),
            upstream_custom_headers: Vec::new(),
            request_start: Instant::now(),
            response_status: None,
            codec_ctx: None,
            needs_conversion: false,
            provider_ctx: None,
            request_body_buffer: None,
            response_body_buffer: None,
            pd_decision: None,
            original_body_bytes: None,
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
            request_id: String::new(),
            retry_fallback_chain: Vec::new(),
            requested_model: None,
            original_model: None,
            concurrency_guards: Vec::new(),
            debug_mode: false,
            route_strategy: None,
            route_score: None,
            route_candidates: None,
            pool_name: None,
            resolved_timeout_secs: None,
            resolved_idle_timeout_secs: None,
            last_data_chunk_at: None,
            retry_on_5xx: false,
        }
    }
}

/// The Prism proxy service — routes LLM API requests to appropriate providers.
pub struct PrismProxy {
    pub router: Arc<ArcSwap<PrismRouter>>,
    pub metrics: Arc<MetricsCollector>,
    pub codec_registry: Arc<CodecRegistry>,
    pub provider_plugins: Arc<ProviderPluginRegistry>,
    pub rate_limiter: Option<Arc<RateLimiter>>,
    pub access_logger: Option<Arc<AccessLogger>>,
    /// Concurrency limiter.
    pub concurrency_limiter: Option<Arc<crate::ratelimit::concurrency::ConcurrencyLimiter>>,
    /// Active concurrency guards for current requests (stored per-request in ctx).
    /// Upstream request timeout in seconds.
    pub request_timeout_secs: u64,
    pub idle_timeout_secs: u64,
    /// TCP keepalive interval in seconds (0 = disabled).
    pub keepalive_secs: u64,
    /// Maximum request body size in bytes (0 = unlimited).
    pub max_request_body_bytes: u64,
    /// Provider-level circuit breaker.
    pub circuit_breaker: Option<Arc<crate::circuit_breaker::CircuitBreaker>>,
    /// Client-side API key authentication.
    pub auth: Option<crate::config::types::AuthConfig>,
    /// In-flight request tracker for load-aware scheduling.
    pub inflight: Arc<prism_core::inflight::InFlightTracker>,
    /// Prefix cache index.
    pub prefix_cache: Arc<dyn prism_core::prefix_cache::PrefixCacheIndex>,
    /// Active health checker for upstream endpoints.
    pub health_checker: Option<Arc<crate::health::HealthChecker>>,
    /// Model alias mapping for resolving client model names.
    pub model_aliases: Arc<ArcSwap<std::collections::HashMap<String, String>>>,
}

impl PrismProxy {
    #[allow(dead_code)]
    pub fn new(router: Arc<ArcSwap<PrismRouter>>, metrics: Arc<MetricsCollector>) -> Self {
        Self {
            router,
            metrics,
            codec_registry: Arc::new(CodecRegistry::new()),
            provider_plugins: Arc::new(ProviderPluginRegistry::new()),
            rate_limiter: None,
            access_logger: None,
            concurrency_limiter: None,
            request_timeout_secs: 300,
            idle_timeout_secs: 90,
            keepalive_secs: 60,
            max_request_body_bytes: 0,
            circuit_breaker: None,
            auth: None,
            health_checker: None,
            inflight: Arc::new(prism_core::inflight::InFlightTracker::new()),
            prefix_cache: prism_core::prefix_cache::default_prefix_cache(),
            model_aliases: Arc::new(ArcSwap::new(Arc::new(std::collections::HashMap::new()))),
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
            provider_plugins: Arc::new(ProviderPluginRegistry::new()),
            rate_limiter: None,
            access_logger: None,
            concurrency_limiter: None,
            request_timeout_secs: 300,
            idle_timeout_secs: 90,
            keepalive_secs: 60,
            max_request_body_bytes: 0,
            circuit_breaker: None,
            auth: None,
            health_checker: None,
            inflight: Arc::new(prism_core::inflight::InFlightTracker::new()),
            prefix_cache: prism_core::prefix_cache::default_prefix_cache(),
            model_aliases: Arc::new(ArcSwap::new(Arc::new(std::collections::HashMap::new()))),
        }
    }

    /// Set model alias mapping (shared with AppState for hot-reload).
    #[allow(dead_code)]
    pub fn with_model_aliases(mut self, aliases: Arc<ArcSwap<std::collections::HashMap<String, String>>>) -> Self {
        self.model_aliases = aliases;
        self
    }

    /// Resolve a model name through alias mapping.
    fn resolve_model_alias(&self, model: &str) -> String {
        let aliases = self.model_aliases.load();
        if let Some(resolved) = aliases.get(model) {
            resolved.clone()
        } else if let Some(wildcard) = aliases.get("*") {
            wildcard.clone()
        } else {
            model.to_string()
        }
    }

    /// Enable client-side API key authentication.
    #[allow(dead_code)]
    pub fn with_auth(mut self, auth: crate::config::types::AuthConfig) -> Self {
        if auth.enabled {
            info!("  Client auth → enabled ({} keys, header: {})", auth.keys.len(), auth.header);
            self.auth = Some(auth);
        }
        self
    }

    /// Enable active health checking.
    #[allow(dead_code)]
    pub fn with_health_checker(mut self, hc: Arc<crate::health::HealthChecker>) -> Self {
        self.health_checker = Some(hc);
        self
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

    /// Set concurrency limiter.
    pub fn with_concurrency_limiter(mut self, limiter: Arc<crate::ratelimit::concurrency::ConcurrencyLimiter>) -> Self {
        self.concurrency_limiter = Some(limiter);
        self
    }

    /// Set timeout configuration.
    pub fn with_timeouts(mut self, request_timeout_secs: u64, keepalive_secs: u64, idle_timeout_secs: u64) -> Self {
        self.request_timeout_secs = request_timeout_secs;
        self.idle_timeout_secs = idle_timeout_secs;
        self.keepalive_secs = keepalive_secs;
        self
    }

    /// Set maximum request body size.
    pub fn with_max_request_body(mut self, max_bytes: u64) -> Self {
        self.max_request_body_bytes = max_bytes;
        self
    }

    /// Set circuit breaker.
    pub fn with_inflight(mut self, inflight: Arc<prism_core::inflight::InFlightTracker>) -> Self {
        self.inflight = inflight;
        self
    }

    pub fn with_circuit_breaker(mut self, cb: Arc<crate::circuit_breaker::CircuitBreaker>) -> Self {
        self.circuit_breaker = Some(cb);
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

        // Generate or extract request ID
        ctx.request_id = ctx
            .request_headers_snapshot
            .iter()
            .find(|(k, _)| k == "x-request-id")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| {
                use std::sync::atomic::{AtomicU64, Ordering};
                use std::time::{SystemTime, UNIX_EPOCH};
                static COUNTER: AtomicU64 = AtomicU64::new(0);
                let ts_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
                // prism-<epoch_ms_hex>-<seq_hex>
                // e.g. prism-18e5a2b3c00-1a → ts可直接解析为wall clock
                format!("prism-{:x}-{:x}", ts_ms, seq)
            });

        // Check if client wants debug trace headers
        ctx.debug_mode = ctx.request_headers_snapshot
            .iter()
            .any(|(k, v)| k == "x-prism-debug" && (v == "true" || v == "1"));

        debug!(
            path = %path,
            protocol = %ctx.protocol.as_str(),
            debug_mode = %ctx.debug_mode,
            "Request received"
        );

        // ── Client API key authentication ──
        if let Some(ref auth) = self.auth {
            // Skip auth for health/readyz/version paths
            let skip = auth.skip_paths.iter().any(|p| path.starts_with(p));
            if !skip {
                let header_val = ctx.request_headers_snapshot
                    .iter()
                    .find(|(k, _)| k == &auth.header)
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("");

                let key = if let Some(ref prefix) = auth.strip_prefix {
                    header_val.strip_prefix(prefix.as_str()).unwrap_or(header_val)
                } else {
                    header_val
                };

                if key.is_empty() || !auth.keys.iter().any(|k| k == key) {
                    warn!(path = %path, "Client auth failed — invalid or missing API key");
                    ctx.response_flags.push("UA".to_string());

                    let header = pingora::http::ResponseHeader::build(401, None).unwrap();
                    let body = serde_json::json!({
                        "error": {
                            "type": "authentication_error",
                            "message": "Invalid or missing API key",
                            "code": "invalid_api_key"
                        }
                    });

                    session.set_keepalive(None);
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
                debug!("Client auth passed");
            }
        }

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

        // Request body size limit check
        if self.max_request_body_bytes > 0 {
            if let Some(content_length) = session
                .req_header()
                .headers
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
            {
                if content_length > self.max_request_body_bytes {
                    warn!(
                        content_length = %content_length,
                        limit = %self.max_request_body_bytes,
                        "Request body too large"
                    );
                    ctx.response_flags.push("BL".to_string());
                    let header =
                        pingora::http::ResponseHeader::build(413, None).unwrap();
                    let body = serde_json::json!({
                        "error": {
                            "type": "request_body_too_large",
                            "message": format!("Request body size {} exceeds limit {}", content_length, self.max_request_body_bytes),
                            "limit": self.max_request_body_bytes,
                        }
                    });
                    session.set_keepalive(None);
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
            }
        }

        // Concurrency limit check — after rate limit, before routing
        if let Some(ref concurrency_limiter) = self.concurrency_limiter {
            let headers: Vec<(String, String)> = ctx
                .request_headers_snapshot
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();

            match concurrency_limiter.try_acquire(&headers) {
                Ok(guards) => {
                    ctx.concurrency_guards = guards;
                }
                Err(denied) => {
                    warn!(rule = %denied.rule_name, current = %denied.current, limit = %denied.limit, "Concurrency limit exceeded");
                    ctx.response_flags.push("CL".to_string());

                    let header =
                        pingora::http::ResponseHeader::build(denied.status_code, None).unwrap();
                    let body = serde_json::json!({
                        "error": {
                            "type": "concurrency_limit_exceeded",
                            "message": denied.message,
                            "rule": denied.rule_name,
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
                                format!("concurrency limit response error: {}", e),
                            )
                        })?;
                    session
                        .write_response_body(Some(Bytes::from(body.to_string())), true)
                        .await
                        .ok();

                    return Ok(true);
                }
            }
        }


        // Read body to extract model name for routing.
        // We use enable_retry_buffering() so Pingora saves the body internally.
        // After reading, proxy_handle_downstream will replay the retry buffer
        // through request_body_filter, where we do model rewrite / codec.
        if ctx.protocol != LlmProtocol::Unknown {
            session.downstream_session.enable_retry_buffering();
            let mut body_bytes = Vec::new();
            loop {
                match session.downstream_session.read_request_body().await {
                    Ok(Some(bytes)) => body_bytes.extend_from_slice(&bytes),
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            if !body_bytes.is_empty() {
                ctx.bytes_received = Some(body_bytes.len() as u64);
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body_bytes) {
                    if let Some(model) = json.get("model").and_then(|m| m.as_str()) {
                        // Resolve model alias before storing
                        let resolved = self.resolve_model_alias(model);
                        if resolved != model {
                            ctx.original_model = Some(model.to_string());
                            debug!(original = %model, resolved = %resolved, "Resolved model alias");
                        }
                        ctx.requested_model = Some(resolved);
                    }
                }
            }
            // Store body for P/D prefill fire-and-forget
            if !body_bytes.is_empty() {
                ctx.original_body_bytes = Some(body_bytes);
            }
            ctx.request_body_received_at = Some(Instant::now());
        }

        Ok(false) // continue to Pingora upstream pipeline
    }

    /// Phase 2: Determine the upstream peer based on routing decision.
    async fn upstream_peer(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>, Box<pingora::Error>> {

        // ── Retry fast-path: if fail_to_connect set a new variant, use it directly ──
        if ctx.upstream_request_attempt_count > 1 {
            if let Some(ref variant_id) = ctx.selected_variant {
                let router = self.router.load();
                if let Some(variant) = router.registry.variants.get(variant_id) {
                    // Resolve provider from variant's endpoint
                    let endpoint = router.registry.endpoints.get(&variant.endpoint_id);
                    let provider_id = endpoint.map(|ep| ep.provider_id.clone()).unwrap_or_default();
                    let provider = router.registry.providers.get(&provider_id);

                    ctx.selected_provider = Some(provider_id.clone());
                    ctx.upstream_model_name = variant.upstream_model_name.clone();

                    let base_url = variant.base_url.as_deref()
                        .or_else(|| endpoint.and_then(|ep| ep.base_url.as_deref()))
                        .or_else(|| provider.map(|p| p.base_url.as_str()))
                        .unwrap_or("https://api.openai.com/v1");
                    ctx.upstream_base_url = Some(base_url.to_string());

                    // Re-resolve auth from account
                    ctx.upstream_auth = None;
                    ctx.upstream_custom_headers.clear();
                    let account_id = variant.account_id.as_ref();
                    if let Some(acct) = account_id.and_then(|id| router.registry.accounts.get(id)) {
                        match &acct.auth {
                            prism_core::types::AuthConfig::ApiKey { key, header } if !key.is_empty() => {
                                ctx.upstream_auth = Some(key.clone());
                                ctx.upstream_auth_header = header.clone();
                            }
                            prism_core::types::AuthConfig::Custom { headers } if !headers.is_empty() => {
                                ctx.upstream_custom_headers = headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                            }
                            _ => {}
                        }
                    }
                    // Fallback to provider auth
                    if ctx.upstream_auth.is_none() && ctx.upstream_custom_headers.is_empty() {
                        if let Some(p) = provider {
                            match &p.auth {
                                prism_core::types::AuthConfig::ApiKey { key, header } => {
                                    ctx.upstream_auth = Some(key.clone());
                                    ctx.upstream_auth_header = header.clone();
                                }
                                _ => {}
                            }
                        }
                    }

                    let (host, port, use_tls) = Self::parse_upstream(&base_url)?;
                    info!(
                        variant = %variant_id,
                        provider = %provider_id,
                        host = %host,
                        attempt = ctx.upstream_request_attempt_count,
                        "Retry → upstream"
                    );

                    let mut peer = HttpPeer::new((host, port), use_tls, host.to_string());
                    peer.options.connection_timeout = Some(std::time::Duration::from_secs(5));
                    peer.options.read_timeout = Some(std::time::Duration::from_secs(self.request_timeout_secs));
                    peer.options.write_timeout = Some(std::time::Duration::from_secs(10));
                    peer.options.set_http_version(1, 1);
                    return Ok(Box::new(peer));
                }
            }
        }

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
            requested_model: ctx.requested_model.clone(),
            inflight_snapshot: Some(self.inflight.snapshot()),
            prefix_cache_index: Some(self.prefix_cache.clone()),
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

                // Capture routing decision metadata for access log
                ctx.route_strategy = Some(format!(
                    "L1:{} L2:{} L3:{} L4:{}",
                    decision.reason.l1_model,
                    decision.reason.l2_provider,
                    decision.reason.l3_account,
                    decision.reason.l4_variant,
                ));
                ctx.route_score = Some(decision.score as f64);
                ctx.route_candidates = Some(decision.reason.candidates_filtered);

                // Build candidate list: primary + fallback chain
                let mut candidates = vec![decision.selected_variant.clone()];
                candidates.extend(decision.fallback_chain.iter().cloned());

                // Find the first healthy candidate
                let mut chosen_variant = None;
                for candidate in &candidates {
                    let variant = router.registry.variants.get(candidate);
                    let provider_id = variant
                        .and_then(|v| {
                            router.registry.endpoints.get(&v.endpoint_id)
                                .map(|ep| ep.provider_id.clone())
                        })
                        .unwrap_or_default();

                    // Circuit breaker check
                    let cb_ok = if let Some(ref cb) = self.circuit_breaker {
                        use crate::circuit_breaker::CircuitState;
                        match cb.check(&provider_id) {
                            CircuitState::Open => {
                                warn!(provider = %provider_id, variant = %candidate, "Circuit breaker OPEN — skipping");
                                false
                            }
                            CircuitState::HalfOpen => {
                                info!(provider = %provider_id, variant = %candidate, "Circuit breaker HALF-OPEN — probe");
                                true
                            }
                            CircuitState::Closed => true,
                        }
                    } else {
                        true
                    };

                    // Health checker check
                    let health_ok = if let Some(ref hc) = self.health_checker {
                        hc.is_healthy(candidate)
                    } else {
                        true
                    };

                    if cb_ok && health_ok {
                        chosen_variant = Some(candidate.clone());
                        if candidate != &decision.selected_variant {
                            info!(
                                original = %decision.selected_variant,
                                failover = %candidate,
                                "Failover to healthy variant"
                            );
                            ctx.response_flags.push("FO".to_string()); // failover flag
                        }
                        break;
                    }
                }

                let chosen_variant = match chosen_variant {
                    Some(v) => v,
                    None => {
                        // All candidates unhealthy — use primary anyway (best effort)
                        warn!("All candidates unhealthy, using primary as best effort");
                        ctx.response_flags.push("AUH".to_string()); // all unhealthy
                        decision.selected_variant.clone()
                    }
                };

                // Resolve chosen variant's provider
                let chosen_provider = router.registry.variants.get(&chosen_variant)
                    .and_then(|v| router.registry.endpoints.get(&v.endpoint_id))
                    .map(|ep| ep.provider_id.clone())
                    .unwrap_or_else(|| decision.selected_provider.clone());

                ctx.selected_variant = Some(chosen_variant.clone());
                ctx.selected_provider = Some(chosen_provider.clone());
                ctx.selected_model = Some(decision.selected_model.clone());

                // Track in-flight request for load-aware scheduling
                self.inflight.increment(&chosen_variant);

                // Store remaining fallback chain (after chosen)
                let chosen_idx = candidates.iter().position(|c| c == &chosen_variant).unwrap_or(0);
                ctx.retry_fallback_chain = candidates[chosen_idx + 1..].to_vec();

                // Look up pool name for the chosen variant
                ctx.pool_name = router.registry.pools.values()
                    .find(|p| p.variants.contains(&chosen_variant))
                    .map(|p| p.name.clone());

                // Look up upstream_model_name from the chosen variant
                if let Some(variant) = router.registry.variants.get(&chosen_variant) {
                    ctx.upstream_model_name = variant.upstream_model_name.clone();
                }

                // Look up the provider's base_url and protocols
                let provider = router.registry.providers.get(&chosen_provider);

                let mut base_url = provider
                    .map(|p| p.base_url.clone())
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

                // Variant base_url overrides provider base_url (for self-hosted/k8s instances)
                if let Some(variant) = router.registry.variants.get(&chosen_variant) {
                    if let Some(ref vurl) = variant.base_url {
                        base_url = vurl.clone();
                    }
                }

                // Extract auth header from provider
                // First try account auth (preferred — accounts carry the real API keys)
                // Resolve account from chosen variant
                let chosen_account_id = router.registry.variants.get(&chosen_variant)
                    .and_then(|v| v.account_id.clone())
                    .unwrap_or_else(|| decision.selected_account.clone());
                let account = router.registry.accounts.get(&chosen_account_id);
                if let Some(acct) = account {
                    match &acct.auth {
                        prism_core::types::AuthConfig::ApiKey { key, header } if !key.is_empty() => {
                            ctx.upstream_auth = Some(key.clone());
                            ctx.upstream_auth_header = header.clone();
                        }
                        prism_core::types::AuthConfig::Custom { headers } if !headers.is_empty() => {
                            ctx.upstream_custom_headers = headers
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect();
                        }
                        _ => {}
                    }
                }

                // Fall back to provider auth if account didn't provide any
                if ctx.upstream_auth.is_none() && ctx.upstream_custom_headers.is_empty() {
                if let Some(p) = provider {
                    match &p.auth {
                        prism_core::types::AuthConfig::ApiKey { key, header } => {
                            ctx.upstream_auth = Some(key.clone());
                            ctx.upstream_auth_header = header.clone();
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
                }

                ctx.upstream_base_url = Some(base_url.to_string());

                // Resolve per-provider timeouts (provider metadata override > global)
                if let Some(p) = provider {
                    ctx.resolved_timeout_secs = p.metadata.get("request_timeout_secs")
                        .and_then(|v| v.parse::<u64>().ok())
                        .or(Some(self.request_timeout_secs));
                    ctx.resolved_idle_timeout_secs = p.metadata.get("idle_timeout_secs")
                        .and_then(|v| v.parse::<u64>().ok());
                }

                // ── Build ProviderRequestContext for provider plugin ──
                {
                    let provider_type = provider
                        .and_then(|p| p.protocols.first().cloned())
                        .unwrap_or_default();
                    let provider_extra = provider
                        .map(|p| p.metadata.clone().into_iter().map(|(k, v)| {
                            (k, serde_json::Value::String(v))
                        }).collect())
               .unwrap_or_default();

                    // Extract api_key from account's AuthConfig.
                    // Note: registry bakes "Bearer " prefix into ApiKey.key for Bearer auth.
                    // We strip it here to give the provider plugin the raw key.
                    let (acct_key, acct_headers) = match account.map(|a| &a.auth) {
                        Some(prism_core::types::AuthConfig::ApiKey { key, .. }) if !key.is_empty() => {
                            let raw_key = key.strip_prefix("Bearer ").unwrap_or(key).to_string();
                            (Some(raw_key), std::collections::HashMap::new())
                        }
                        Some(prism_core::types::AuthConfig::Custom { headers }) => {
                            (None, headers.clone())
                        }
                        _ => (None, std::collections::HashMap::new()),
                    };

                    let variant_obj = router.registry.variants.get(&decision.selected_variant);
                    let deployment = variant_obj.and_then(|v| v.deployment_name.clone());
                    let upstream_model = ctx.upstream_model_name.clone();

                    ctx.provider_ctx = Some(provider::ProviderRequestContext {
                        provider_id: decision.selected_provider.clone(),
                        provider_type,
                        base_url: base_url.to_string(),
                        provider_extra,
                        account_id: decision.selected_account.clone(),
                        account_api_key: acct_key,
                        account_headers: acct_headers,
                        variant_id: decision.selected_variant.clone(),
                        model_id: decision.selected_model.clone(),
                        upstream_model_name: upstream_model,
                        deployment_name: deployment,
                        original_path: path.clone(),
                        method: ctx.request_method.clone(),
                        is_streaming: false,
                    });
                }

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

                // ── P/D: store decision and fire prefill if Split ──
                ctx.pd_decision = decision.pd_decision.clone();
                if let Some(prism_core::plugins::pd_routing::PDDecision::Split {
                    ref prefill_variant_ids,
                    ..
                }) = decision.pd_decision
                {
                    // Fire-and-forget prefill: pick least-loaded P variant
                    let prefill_variant_id = select_least_loaded(
                        prefill_variant_ids, &self.inflight,
                    );
                    let prefill_variant = router.registry.variants.get(prefill_variant_id);
                    if let Some(pv) = prefill_variant {
                        if let Some(ref purl) = pv.base_url {
                            let prefill_url = format!("{}{}", purl,
                                ctx.provider_ctx.as_ref().map(|p| p.original_path.as_str()).unwrap_or("/v1/chat/completions"));
                            let body = ctx.original_body_bytes.clone();
                            let headers = ctx.provider_ctx.as_ref().and_then(|p| {
                                let mut h = reqwest::header::HeaderMap::new();
                                if let Some(ref key) = p.account_api_key {
                                    if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", key)) {
                                        h.insert(reqwest::header::AUTHORIZATION, v);
                                    }
                                }
                                for (k, v) in &p.account_headers {
                                    if let (Ok(name), Ok(val)) = (
                                        reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                                        reqwest::header::HeaderValue::from_str(v),
                                    ) {
                                        h.insert(name, val);
                                    }
                                }
                                Some(h)
                            });

                            info!(
                                prefill_variant = %prefill_variant_id,
                                prefill_url = %prefill_url,
                                "P/D Split: firing prefill request (fire-and-forget)"
                            );

                            tokio::spawn(async move {
                                let client = reqwest::Client::builder()
                                    .timeout(std::time::Duration::from_secs(120))
                                    .build()
                                    .unwrap_or_default();
                                let mut req = client.post(&prefill_url);
                                if let Some(h) = headers {
                                    req = req.headers(h);
                                }
                                if let Some(b) = body {
                                    req = req.body(b);
                                    req = req.header("content-type", "application/json");
                                }
                                match req.send().await {
                                    Ok(resp) => {
                                        // Drain the response body to prevent connection issues
                                        let status = resp.status();
                                        let _ = resp.bytes().await;
                                        if !status.is_success() {
                                            tracing::warn!(
                                                url = %prefill_url,
                                                status = %status,
                                                "Prefill fire-and-forget got error status"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            url = %prefill_url,
                                            error = %e,
                                            "Prefill fire-and-forget failed"
                                        );
                                    }
                                }
                            });
                        }
                    }
                }

                // ── P/D: override upstream to decode variant ──
                if let Some(prism_core::plugins::pd_routing::PDDecision::Split {
                    ref decode_variant_ids,
                    ..
                }) = ctx.pd_decision
                {
                    if !decode_variant_ids.is_empty() {
                        // Pick decode variant: for now round-robin via attempt count
                        // TODO: load-based selection
                        let d_idx = (ctx.upstream_request_attempt_count as usize) % decode_variant_ids.len();
                        let d_variant_id = &decode_variant_ids[d_idx];

                        if let Some(d_variant) = router.registry.variants.get(d_variant_id) {
                            if let Some(ref d_url) = d_variant.base_url {
                                info!(
                                    decode_variant = %d_variant_id,
                                    decode_url = %d_url,
                                    candidates = decode_variant_ids.len(),
                                    "P/D Split: routing upstream to decode variant"
                                );
                                base_url = d_url.clone();
                                // Update provider context with decode variant info
                                if let Some(ref mut pctx) = ctx.provider_ctx {
                                    pctx.variant_id = d_variant_id.clone();
                                    pctx.base_url = d_url.clone();
                                }
                            }
                        }
                    }
                }

                let (host, port, use_tls) = Self::parse_upstream(&base_url).inspect_err(|_| {
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
                let timeout = ctx.resolved_timeout_secs.unwrap_or(self.request_timeout_secs);
                peer.options.read_timeout = Some(std::time::Duration::from_secs(timeout));
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
                // Try to replace the standard API prefix (/v1) with the base path.
                // Handles base_urls ending with any version prefix: /v1, /v2, /v3, etc.
                let new_path = if original_path.starts_with("/v1/") || original_path == "/v1" {
                    // Check if base_path ends with a versioned prefix (/v1, /v2, /v3, ...)
                    let base_ends_with_version = {
                        let bytes = base_path.as_bytes();
                        bytes.len() >= 3
                            && bytes[bytes.len() - 3] == b'/'
                            && bytes[bytes.len() - 2] == b'v'
                            && bytes[bytes.len() - 1].is_ascii_digit()
                    };
                    if base_ends_with_version {
                        // base_path = "/api/v3", original = "/v1/chat/completions"
                        // → "/api/v3/chat/completions" (replace /v1 with base's version)
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

        // ── Default Content-Type to application/json ──
        // LLM APIs are JSON-only. Default to application/json if missing or wrong.
        {
            let needs_fix = upstream_request.headers.get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|ct| !ct.contains("application/json"))
                .unwrap_or(true);
            if needs_fix {
                let _ = upstream_request.insert_header("content-type", "application/json");
            }
        }

        // ── Provider plugin: build auth headers + rewrite URL ──
        if let Some(ref pctx) = ctx.provider_ctx {
            let provider_type = if pctx.provider_type.is_empty() {
                "openai"
            } else {
                &pctx.provider_type
            };
            let plugin = self.provider_plugins.get(provider_type);

            // Build auth headers via provider plugin
            match plugin.build_auth(pctx) {
                Ok(auth_result) => {
                    let n = auth_result.headers.len();
                    for (key, value) in auth_result.headers {
                        upstream_request
                            .insert_header(key, &value)
                            .map_err(|e| {
                                pingora::Error::explain(
                                    pingora::ErrorType::InternalError,
                                    format!("provider auth header error: {:?}", e),
                                )
                            })?;
                    }
                    debug!(
                        provider_type = %provider_type,
                        headers = %n,
                        "Provider plugin set auth headers"
                    );
                }
                Err(e) => {
                    warn!(
                        provider_type = %provider_type,
                        error = %e,
                        "Provider plugin build_auth failed, falling back"
                    );
                }
            }

            // Rewrite URL via provider plugin
            match plugin.rewrite_url(pctx) {
                Ok(url_result) => {
                    if url_result.path != pctx.original_path {
                        if let Ok(uri) = url_result.path.parse::<http::Uri>() {
                            upstream_request.set_uri(uri);
                            debug!(
                                original = %pctx.original_path,
                                rewritten = %url_result.path,
                                "Provider plugin rewrote URL"
                            );
                        }
                    }
                    if let Some(ref host) = url_result.host_override {
                        let host_str: &str = host.as_str();
                        let _ = upstream_request.insert_header("Host", host_str);
                    }
                }
                Err(e) => {
                    warn!(
                        provider_type = %provider_type,
                        error = %e,
                        "Provider plugin rewrite_url failed"
                    );
                }
            }
        } else {
            // Fallback: legacy auth path (no provider_ctx — shouldn't happen)
            if let Some(ref auth) = ctx.upstream_auth {
                let auth_header = ctx.upstream_auth_header.clone();
                let auth_value = auth.clone();
                upstream_request
                    .insert_header(auth_header, &auth_value)
                    .map_err(|e| {
                        pingora::Error::explain(
                            pingora::ErrorType::InternalError,
                            format!("header error: {:?}", e),
                        )
                    })?;
            }

            // Set custom headers (legacy path)
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
        }

        // Forward X-Request-ID to upstream
        let _ = upstream_request.insert_header("X-Request-ID", &ctx.request_id);

        // If body will be rewritten (model name rewrite or codec conversion),
        // use chunked transfer encoding since the body size may change.
        // Must explicitly set Transfer-Encoding: chunked — just removing
        // Content-Length makes Pingora's BodyWriter fall back to HTTP/1.0
        // close-delimited mode which silently drops the body.
        // Note: some upstreams (e.g. Spring-based APIs) don't handle chunked
        // encoding properly. Consider using reqwest-based forwarding for those.
        if ctx.upstream_model_name.is_some() || ctx.needs_conversion {
            let _ = upstream_request.remove_header("content-length");
            let _ = upstream_request.insert_header("transfer-encoding", "chunked");
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

    /// Phase 3b: Handle request body — rewrite model name and/or transform via codec.
    ///
    /// Body arrives here via Pingora's retry buffer (pre-read in request_filter
    /// with enable_retry_buffering). For codec conversion or model rewrite,
    /// we buffer all chunks and emit the transformed body on end_of_stream.
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

            // Emit the (possibly rewritten) body
            *body = Some(Bytes::from(full_body));
        } else {
            // Buffering — suppress partial chunks from going upstream
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

        // ── 5xx detection: record health feedback for circuit breaker + health checker ──
        let status_code = upstream_response.status.as_u16();
        ctx.response_status = Some(status_code);
        if status_code >= 500 {
            ctx.response_flags.push("5xx".to_string());

            if let (Some(ref variant_id), Some(ref provider_id)) =
                (&ctx.selected_variant, &ctx.selected_provider)
            {
                // Record passive health failure → circuit breaker will trip after threshold
                if let Some(ref cb) = self.circuit_breaker {
                    cb.record_failure(provider_id);
                    warn!(
                        variant = %variant_id,
                        provider = %provider_id,
                        status = status_code,
                        "Upstream 5xx — recorded failure for circuit breaker"
                    );
                }

                // Mark variant unhealthy for health checker (passive feedback)
                if let Some(ref hc) = self.health_checker {
                    hc.record_failure(variant_id, &format!("upstream {}",  status_code));
                }
            }

            // If we have retry candidates left AND response body hasn't started streaming,
            // attempt failover on the next request to a different variant.
            // Note: Pingora doesn't support mid-response retry, so we rely on
            // circuit breaker + health checker to avoid the bad variant on subsequent requests.
        } else if let Some(ref provider_id) = ctx.selected_provider {
            // Record success for circuit breaker recovery
            if let Some(ref cb) = self.circuit_breaker {
                cb.record_success(provider_id);
            }
        }

        // ── X-Prism-Debug response headers ──
        if ctx.debug_mode {
            let _ = upstream_response.insert_header("X-Prism-Request-Id", &ctx.request_id);
            let _ = upstream_response.insert_header("X-Prism-Model", ctx.requested_model.as_deref().unwrap_or("-"));
            if let Some(ref original) = ctx.original_model {
                let _ = upstream_response.insert_header("X-Prism-Original-Model", original);
            }
            let _ = upstream_response.insert_header("X-Prism-Variant", ctx.selected_variant.as_deref().unwrap_or("-"));
            let _ = upstream_response.insert_header("X-Prism-Provider", ctx.selected_provider.as_deref().unwrap_or("-"));
            let _ = upstream_response.insert_header("X-Prism-Upstream-Model", ctx.upstream_model_name.as_deref().unwrap_or("-"));
            let _ = upstream_response.insert_header("X-Prism-Upstream-Base-Url", ctx.upstream_base_url.as_deref().unwrap_or("-"));
            let _ = upstream_response.insert_header("X-Prism-Protocol", ctx.protocol.as_str());
            let _ = upstream_response.insert_header("X-Prism-Outbound-Protocol", ctx.outbound_protocol.as_deref().unwrap_or("-"));
            if let Some(dt) = ctx.decision_time_us {
                let _ = upstream_response.insert_header("X-Prism-Decision-Time-Us", &dt.to_string());
            }
            if let Some(ref ttfb) = ctx.upstream_first_byte_at {
                let ms = ttfb.duration_since(ctx.request_start).as_millis();
                let _ = upstream_response.insert_header("X-Prism-Upstream-Ttfb-Ms", &ms.to_string());
            }
            let total_ms = ctx.request_start.elapsed().as_millis();
            let _ = upstream_response.insert_header("X-Prism-Total-Latency-Ms", &total_ms.to_string());
            if !ctx.response_flags.is_empty() {
                let _ = upstream_response.insert_header("X-Prism-Flags", &ctx.response_flags.join(","));
            }
            if !ctx.retry_fallback_chain.is_empty() {
                let _ = upstream_response.insert_header("X-Prism-Fallback-Chain", &ctx.retry_fallback_chain.join(","));
            }
            let _ = upstream_response.insert_header("X-Prism-Conversion", if ctx.needs_conversion { "true" } else { "false" });
            if let Some(bytes) = ctx.bytes_received {
                let _ = upstream_response.insert_header("X-Prism-Bytes-Received", &bytes.to_string());
            }
            let _ = upstream_response.insert_header("X-Prism-Attempt-Count", &ctx.upstream_request_attempt_count.to_string());
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
        // Track response body chunks for byte counting + idle timeout
        if let Some(ref chunk) = body {
            if !chunk.is_empty() {
                *ctx.bytes_sent.get_or_insert(0) += chunk.len() as u64;
                ctx.last_data_chunk_at = Some(Instant::now());
            }
        }

        // Determine idle timeout for streaming responses.
        // Return value tells Pingora: "wait at most this long for the next chunk".
        let idle_timeout = if !end_of_stream {
            let idle_secs = ctx.resolved_idle_timeout_secs
                .unwrap_or(self.idle_timeout_secs);
            if idle_secs > 0 {
                Some(std::time::Duration::from_secs(idle_secs))
            } else {
                None
            }
        } else {
            None
        };

        if !ctx.needs_conversion {
            return Ok(idle_timeout);
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

        Ok(idle_timeout)
    }

    /// Connection failure handler — mark retryable if fallback chain has candidates.
    fn fail_to_connect(
        &self,
        _session: &mut Session,
        _peer: &HttpPeer,
        ctx: &mut Self::CTX,
        mut e: Box<pingora::Error>,
    ) -> Box<pingora::Error> {
        if !ctx.retry_fallback_chain.is_empty() {
            let next = ctx.retry_fallback_chain.remove(0);
            info!(
                failed_variant = ?ctx.selected_variant,
                next_variant = %next,
                remaining = ctx.retry_fallback_chain.len(),
                "Connection failed, retrying with fallback variant"
            );
            // Update context for the next upstream_peer call
            ctx.selected_variant = Some(next);
            ctx.upstream_request_attempt_count += 1;
            ctx.response_flags.push("RT".to_string()); // retry flag
            // Mark as retryable — Pingora will call upstream_peer again
            e.set_retry(true);
        }
        e
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

        // Decrement in-flight counter (request done — success or failure)
        if let Some(ref variant_id) = ctx.selected_variant {
            self.inflight.decrement(variant_id);
        }

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

        // ── Circuit breaker feedback ──
        if let Some(ref cb) = self.circuit_breaker {
            if let Some(ref provider_id) = ctx.selected_provider {
                let success = (200..500).contains(&status) && _e.is_none();
                if success {
                    cb.record_success(provider_id);
                } else {
                    cb.record_failure(provider_id);
                    debug!(provider = %provider_id, status = %status, "Circuit breaker recorded failure");
                }
            }
        }

        // ── Health checker passive feedback ──
        if let Some(ref hc) = self.health_checker {
            // Use variant id as endpoint key (provider_id@endpoint_id in variant)
            if let Some(ref variant_id) = ctx.selected_variant {
                let success = (200..500).contains(&status) && _e.is_none();
                if success {
                    hc.record_success(variant_id);
                } else {
                    let reason = ctx.error_message.clone().unwrap_or_else(|| format!("HTTP {}", status));
                    hc.record_failure(variant_id, &reason);
                }
            }
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
                route_strategy: ctx.route_strategy.clone(),
                route_score: ctx.route_score,
                route_candidates: ctx.route_candidates,
                upstream_response_time_ms: ctx.upstream_first_byte_at.map(|t| t.duration_since(ctx.request_start).as_millis() as u64),
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
                pool_name: ctx.pool_name.clone(),
                request_id: Some(ctx.request_id.clone()),
                downstream_tls_version: None,
                original_model: ctx.original_model.clone(),
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
            additional_windows: vec![],
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

/// Select the variant with the fewest in-flight requests from a list.
/// Falls back to first element if tracker has no data.
fn select_least_loaded<'a>(
    variant_ids: &'a [String],
    tracker: &prism_core::inflight::InFlightTracker,
) -> &'a String {
    if variant_ids.len() <= 1 {
        return &variant_ids[0];
    }
    variant_ids
        .iter()
        .min_by_key(|id| tracker.get(id))
        .unwrap_or(&variant_ids[0])
}
