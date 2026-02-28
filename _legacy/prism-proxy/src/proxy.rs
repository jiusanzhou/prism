use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use pingora::prelude::*;
use pingora::proxy::{ProxyHttp, Session};
use pingora::upstreams::peer::HttpPeer;
use tracing::{debug, error, info, warn};

use prism_core::analyzer::ChatMessage;
use prism_core::router::{PrismRouter, RouteMetadata, RouteRequest, Router};

use crate::protocol::LlmProtocol;

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
    /// Request start time for latency tracking
    pub request_start: Instant,
    /// HTTP status from upstream response
    pub response_status: Option<u16>,
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
            request_start: Instant::now(),
            response_status: None,
        }
    }
}

/// The Prism proxy service — routes LLM API requests to appropriate providers.
pub struct PrismProxy {
    pub router: Arc<PrismRouter>,
}

impl PrismProxy {
    pub fn new(router: Arc<PrismRouter>) -> Self {
        Self { router }
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
        ctx.protocol = LlmProtocol::from_path(&path);
        ctx.request_start = Instant::now();

        debug!(
            path = %path,
            protocol = %ctx.protocol.as_str(),
            "Request received"
        );

        Ok(false) // continue to upstream
    }

    /// Phase 2: Determine the upstream peer based on routing decision.
    async fn upstream_peer(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>, Box<pingora::Error>> {
        // Build a minimal route request from path + headers
        // NOTE: Reading the request body in this phase is complex with Pingora,
        // so we route based on path (protocol) and use a generic message.
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

        match self.router.route(&request) {
            Ok(decision) => {
                ctx.selected_variant = Some(decision.selected_variant.clone());
                ctx.selected_provider = Some(decision.selected_provider.clone());
                ctx.selected_model = Some(decision.selected_model.clone());

                // Look up the provider's base_url
                let provider = self
                    .router
                    .registry
                    .providers
                    .get(&decision.selected_provider);

                let base_url = provider
                    .map(|p| p.base_url.as_str())
                    .unwrap_or("https://api.openai.com/v1");

                // Extract auth header from provider
                if let Some(p) = provider {
                    if let prism_core::types::AuthConfig::ApiKey { key, .. } = &p.auth {
                        ctx.upstream_auth = Some(key.clone());
                    }
                }

                ctx.upstream_base_url = Some(base_url.to_string());

                let (host, port, use_tls) =
                    Self::parse_upstream(base_url).map_err(|e| {
                        error!(
                            provider = %decision.selected_provider,
                            base_url = %base_url,
                            "Failed to parse upstream URL"
                        );
                        e
                    })?;

                info!(
                    variant = %decision.selected_variant,
                    provider = %decision.selected_provider,
                    host = %host,
                    port = %port,
                    tls = %use_tls,
                    "Routing to upstream"
                );

                let mut peer = HttpPeer::new((host, port), use_tls, host.to_string());
                peer.options.connection_timeout = Some(std::time::Duration::from_secs(5));
                peer.options.read_timeout = Some(std::time::Duration::from_secs(60));
                peer.options.write_timeout = Some(std::time::Duration::from_secs(10));

                Ok(Box::new(peer))
            }
            Err(e) => {
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
        _session: &mut Session,
        upstream_request: &mut pingora::http::RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<(), Box<pingora::Error>> {
        // Set the Host header to match the upstream
        if let Some(ref base_url) = ctx.upstream_base_url {
            let host = base_url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .split('/')
                .next()
                .unwrap_or("localhost");

            upstream_request
                .insert_header("Host", host)
                .map_err(|e| pingora::Error::explain(
                    pingora::ErrorType::InternalError,
                    format!("header error: {:?}", e),
                ))?;
        }

        // Set the Authorization header
        if let Some(ref auth) = ctx.upstream_auth {
            upstream_request
                .insert_header("Authorization", auth.as_str())
                .map_err(|e| pingora::Error::explain(
                    pingora::ErrorType::InternalError,
                    format!("header error: {:?}", e),
                ))?;
        }

        debug!(
            variant = ?ctx.selected_variant,
            "Upstream request filter applied"
        );

        Ok(())
    }

    /// Phase 5: Log the completed request.
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

        info!(
            variant = ?ctx.selected_variant,
            provider = ?ctx.selected_provider,
            model = ?ctx.selected_model,
            protocol = %ctx.protocol.as_str(),
            status = %status,
            latency_ms = %elapsed.as_millis(),
            "Request completed"
        );

        // TODO: Record outcome to feedback collector for quality-aware routing
        // if let Some(ref variant_id) = ctx.selected_variant {
        //     self.router.record_outcome(&OutcomeReport { ... });
        // }
    }
}
