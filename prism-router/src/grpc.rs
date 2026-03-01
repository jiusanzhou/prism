use tonic::{Request, Response, Status};

use std::time::Instant;

use prism_core::analyzer::ChatMessage;
use prism_core::feedback::OutcomeReport as CoreOutcomeReport;
use prism_core::registry::ModelRegistryTrait;
use prism_core::router::{RouteMetadata, RouteRequest as CoreRouteRequest, Router, RouterError};
use prism_core::strategy::ModelSelectionStrategy;
use prism_core::types::AuthConfig;

use crate::gen::prism_router_v1 as pb;
use crate::gen::prism_router_v1::router_service_server::RouterService;
use crate::metrics::RequestRecord;
use crate::state::AppState;

/// gRPC service implementation backed by the shared AppState.
pub struct GrpcRouterService {
    state: AppState,
}

impl GrpcRouterService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl RouterService for GrpcRouterService {
    async fn route(
        &self,
        request: Request<pb::RouteRequest>,
    ) -> Result<Response<pb::RouteResponse>, Status> {
        let req = request.into_inner();

        if req.messages.is_empty() {
            return Err(Status::invalid_argument("messages must not be empty"));
        }

        let messages: Vec<ChatMessage> = req
            .messages
            .iter()
            .map(|m| ChatMessage {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();

        // Determine strategy: model field takes priority over strategy field
        let strategy = if let Some(ref model) = req.model {
            if model == "auto" || model.is_empty() {
                parse_strategy(req.strategy.as_deref())
            } else {
                Some(ModelSelectionStrategy::Pinned {
                    model_id: model.clone(),
                })
            }
        } else {
            parse_strategy(req.strategy.as_deref())
        };

        let metadata = RouteMetadata {
            affinity_key: req.affinity_key,
            tenant_id: req.tenant_id,
            region: None,
            analyze_context: None,
            ..Default::default()
        };

        let core_request = CoreRouteRequest {
            messages,
            strategy,
            metadata,
            analyze_context: None,
            required_protocol: None,
        };

        match self.state.router().route(&core_request) {
            Ok(decision) => {
                // Build primary upstream from registry data
                let upstream = build_upstream(
                    &self.state,
                    &decision.selected_variant,
                    &decision.selected_provider,
                    &decision.selected_model,
                );

                // Build fallback upstreams
                let router = self.state.router();
                let fallback_chain: Vec<pb::Upstream> = decision
                    .fallback_chain
                    .iter()
                    .filter_map(|variant_id| {
                        // Look up variant to find provider/model
                        let variant = router.registry.variants.get(variant_id)?;
                        let endpoint = router.registry.endpoints.get(&variant.endpoint_id)?;
                        Some(build_upstream(
                            &self.state,
                            variant_id,
                            &endpoint.provider_id,
                            &variant.model_id,
                        ))
                    })
                    .collect();

                let reason = pb::RoutingReason {
                    l1_model: decision.reason.l1_model.clone(),
                    l2_provider: decision.reason.l2_provider.clone(),
                    l3_account: decision.reason.l3_account.clone(),
                    l4_variant: decision.reason.l4_variant.clone(),
                    constraints_applied: decision.reason.constraints_applied.clone(),
                    candidates_filtered: decision.reason.candidates_filtered,
                };

                let response = pb::RouteResponse {
                    model: decision.selected_model,
                    provider: decision.selected_provider,
                    variant: decision.selected_variant,
                    account: decision.selected_account,
                    upstream: Some(upstream),
                    fallback_chain,
                    reason: Some(reason),
                    score: decision.score,
                    decision_time_us: decision.decision_time_us as i64,
                    response_metadata: req.metadata,
                };

                Ok(Response::new(response))
            }
            Err(e) => Err(router_error_to_status(e)),
        }
    }

    async fn route_batch(
        &self,
        request: Request<pb::RouteBatchRequest>,
    ) -> Result<Response<pb::RouteBatchResponse>, Status> {
        let batch = request.into_inner();
        let mut responses = Vec::with_capacity(batch.requests.len());

        for req in batch.requests {
            let result = self
                .route(Request::new(req))
                .await;
            match result {
                Ok(resp) => responses.push(resp.into_inner()),
                Err(status) => return Err(status),
            }
        }

        Ok(Response::new(pb::RouteBatchResponse { responses }))
    }

    async fn report_outcome(
        &self,
        request: Request<pb::OutcomeReport>,
    ) -> Result<Response<pb::ReportResponse>, Status> {
        let report = request.into_inner();

        let success = report.status_code >= 200 && report.status_code < 400 && report.error.is_none();

        // Record into metrics collector for DataLayer feedback loop
        self.state.metrics.record(RequestRecord {
            variant_id: report.variant_id.clone(),
            latency_ms: report.latency_ms as u64,
            ttft_ms: if report.ttft_ms > 0 {
                Some(report.ttft_ms as u64)
            } else {
                None
            },
            success,
            input_tokens: if report.input_tokens > 0 {
                Some(report.input_tokens as u32)
            } else {
                None
            },
            output_tokens: if report.output_tokens > 0 {
                Some(report.output_tokens as u32)
            } else {
                None
            },
            timestamp: Instant::now(),
        });

        let core_report = CoreOutcomeReport {
            variant_id: report.variant_id,
            success,
            latency_ms: report.latency_ms as u64,
            prompt_tokens: if report.input_tokens > 0 {
                Some(report.input_tokens as u32)
            } else {
                None
            },
            completion_tokens: if report.output_tokens > 0 {
                Some(report.output_tokens as u32)
            } else {
                None
            },
            cost: None,
            user_rating: None,
        };

        self.state.router().record_outcome(&core_report);
        Ok(Response::new(pb::ReportResponse {}))
    }

    async fn health_check(
        &self,
        _request: Request<pb::HealthCheckRequest>,
    ) -> Result<Response<pb::HealthCheckResponse>, Status> {
        let router = self.state.router();
        let candidates = router.registry.get_all_candidates();
        let model_count = router.registry.models.len();
        let variant_count = candidates.len();
        let uptime = self.state.start_time.elapsed().as_secs();

        Ok(Response::new(pb::HealthCheckResponse {
            status: "healthy".to_string(),
            uptime_seconds: uptime as i64,
            registered_models: model_count as u32,
            active_variants: variant_count as u32,
        }))
    }
}

// ──────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────

/// Build an Upstream proto from registry data.
fn build_upstream(
    state: &AppState,
    variant_id: &str,
    provider_id: &str,
    model_id: &str,
) -> pb::Upstream {
    let mut base_url = String::new();
    let mut headers = std::collections::HashMap::new();
    let timeout_ms: i32 = 30_000;

    let router = state.router();

    // Look up provider for base_url and auth
    if let Some(provider) = router.registry.providers.get(provider_id) {
        base_url = provider.base_url.clone();
        apply_auth_headers(&provider.auth, &mut headers);
    }

    // Look up variant → endpoint for possible overrides
    if let Some(variant) = router.registry.variants.get(variant_id) {
        if let Some(endpoint) = router.registry.endpoints.get(&variant.endpoint_id) {
            if let Some(ref url) = endpoint.base_url {
                base_url = url.clone();
            }
            if let Some(ref auth) = endpoint.auth_override {
                // Clear and re-apply
                headers.clear();
                apply_auth_headers(auth, &mut headers);
            }
        }
        // Account-level auth override
        if let Some(ref acct_id) = variant.account_id {
            if let Some(account) = router.registry.accounts.get(acct_id) {
                headers.clear();
                apply_auth_headers(&account.auth, &mut headers);
            }
        }
    }

    pb::Upstream {
        base_url,
        model_name: model_id.to_string(),
        headers,
        path: "/v1/chat/completions".to_string(),
        timeout_ms,
        variant_id: variant_id.to_string(),
    }
}

/// Extract auth headers from AuthConfig.
fn apply_auth_headers(auth: &AuthConfig, headers: &mut std::collections::HashMap<String, String>) {
    match auth {
        AuthConfig::ApiKey { key, header } => {
            if !key.is_empty() {
                headers.insert(header.clone(), key.clone());
            }
        }
        AuthConfig::Custom { headers: custom } => {
            headers.extend(custom.clone());
        }
        AuthConfig::OAuth { .. } => {
            // OAuth tokens are dynamic; nothing to set statically
        }
    }
}

/// Parse strategy string to ModelSelectionStrategy (reused from api.rs logic).
fn parse_strategy(name: Option<&str>) -> Option<ModelSelectionStrategy> {
    match name {
        Some("balanced") | None => None,
        Some("cost_first") => Some(ModelSelectionStrategy::Auto {
            objective: prism_core::strategy::RoutingObjective::CostFirst {
                min_quality_score: 0.0,
                budget_per_request: None,
            },
            pool_ids: None,
            required_capabilities: None,
        }),
        Some("quality_first") => Some(ModelSelectionStrategy::Auto {
            objective: prism_core::strategy::RoutingObjective::QualityFirst {
                max_latency_ms: None,
            },
            pool_ids: None,
            required_capabilities: None,
        }),
        Some("slo_first") | Some("latency_first") => Some(ModelSelectionStrategy::Auto {
            objective: prism_core::strategy::RoutingObjective::SloFirst {
                max_latency_ms: 5000,
                max_ttft_ms: 2000,
                min_quality_score: 0.0,
            },
            pool_ids: None,
            required_capabilities: None,
        }),
        Some(model_id) => Some(ModelSelectionStrategy::Pinned {
            model_id: model_id.to_string(),
        }),
    }
}

/// Convert RouterError to tonic::Status.
fn router_error_to_status(err: RouterError) -> Status {
    match err {
        RouterError::NoCandidates => Status::not_found("no candidates available"),
        RouterError::AllFiltered { reason } => {
            Status::unavailable(format!("all candidates filtered: {}", reason))
        }
        RouterError::InternalError { message } => {
            Status::internal(format!("internal error: {}", message))
        }
    }
}

// ──────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::registry::InMemoryRegistry;
    use prism_core::router::PrismRouter;
    use prism_core::types::*;
    use std::collections::HashMap;

    /// Build a minimal registry with one provider/model/variant for testing.
    fn test_registry() -> InMemoryRegistry {
        let mut registry = InMemoryRegistry::new();

        let provider = Provider {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-test".to_string(),
                header: "Authorization".to_string(),
            },
            base_url: "https://api.openai.com/v1".to_string(),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
            protocols: vec!["openai".to_string()],
        };
        registry.add_provider(provider);

        let endpoint = Endpoint {
            id: "openai-ep".to_string(),
            provider_id: "openai".to_string(),
            base_url: Some("https://api.openai.com/v1".to_string()),
            region: None,
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        };
        registry.add_endpoint(endpoint);

        let account = Account {
            id: "openai-acct".to_string(),
            endpoint_id: "openai-ep".to_string(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-test".to_string(),
                header: "Authorization".to_string(),
            },
            project_id: None,
            rate_limit: None,
            billing_tag: None,
            weight: 1.0,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
        };
        registry.add_account(account);

        let model = Model {
            id: "gpt-4o".to_string(),
            display_name: "GPT-4o".to_string(),
            quality_tier: QualityTier::S,
            capabilities: ModelCapabilities {
                max_context_window: 128000,
                max_output_tokens: 16384,
                supports_streaming: true,
                supports_function_calling: true,
                supports_vision: true,
                supports_json_mode: true,
                supports_reasoning: false,
            },
            pricing: ModelPricing {
                input_per_1m_tokens: 2.5,
                output_per_1m_tokens: 10.0,
                ..Default::default()
            },
            specialties: vec![],
            limits: None,
            status: ModelStatus::Available,
        };
        registry.add_model(model);

        let variant = ModelVariant {
            id: "gpt-4o@openai".to_string(),
            model_id: "gpt-4o".to_string(),
            endpoint_id: "openai-ep".to_string(),
            account_id: Some("openai-acct".to_string()),
            deployment_name: None,
            weight: 1.0,
            performance: ModelPerformance::default(),
            pricing_override: None,
            status: VariantStatus::Active,
            base_url: None,
            upstream_model_name: None,
        };
        registry.add_variant(variant);

        let pool = ModelPool {
            id: "default".to_string(),
            name: "Default".to_string(),
            variants: vec!["gpt-4o@openai".to_string()],
            selection_mode: SelectionMode::default(),
            description: None,
        };
        registry.add_pool(pool);

        registry
    }

    fn test_state() -> AppState {
        let registry = test_registry();
        let router = PrismRouter::new(registry);
        AppState::new(router, std::path::PathBuf::from("test.yaml"))
    }

    fn test_service() -> GrpcRouterService {
        GrpcRouterService::new(test_state())
    }

    fn simple_route_request() -> pb::RouteRequest {
        pb::RouteRequest {
            messages: vec![pb::ChatMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            }],
            strategy: None,
            model: None,
            pool_ids: vec![],
            exclude_models: vec![],
            exclude_providers: vec![],
            affinity_key: None,
            tenant_id: None,
            capabilities: None,
            strategy_params: None,
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_grpc_route_basic() {
        let svc = test_service();
        let req = Request::new(simple_route_request());
        let resp = svc.route(req).await.unwrap();
        let body = resp.into_inner();

        assert_eq!(body.model, "gpt-4o");
        assert_eq!(body.provider, "openai");
        assert_eq!(body.variant, "gpt-4o@openai");
        assert_eq!(body.account, "openai-acct");
        assert!(body.upstream.is_some());

        let upstream = body.upstream.unwrap();
        assert_eq!(upstream.base_url, "https://api.openai.com/v1");
        assert_eq!(upstream.model_name, "gpt-4o");
        assert_eq!(upstream.variant_id, "gpt-4o@openai");
        assert_eq!(upstream.path, "/v1/chat/completions");
        assert!(upstream.headers.contains_key("Authorization"));
    }

    #[tokio::test]
    async fn test_grpc_route_pinned_model() {
        let svc = test_service();
        let mut req = simple_route_request();
        req.model = Some("gpt-4o".to_string());
        let resp = svc.route(Request::new(req)).await.unwrap();
        let body = resp.into_inner();

        assert_eq!(body.model, "gpt-4o");
        assert_eq!(body.variant, "gpt-4o@openai");
    }

    #[tokio::test]
    async fn test_grpc_route_auto_model() {
        let svc = test_service();
        let mut req = simple_route_request();
        req.model = Some("auto".to_string());
        let resp = svc.route(Request::new(req)).await.unwrap();
        let body = resp.into_inner();

        // Should route normally (not pinned)
        assert_eq!(body.model, "gpt-4o");
    }

    #[tokio::test]
    async fn test_grpc_route_empty_messages() {
        let svc = test_service();
        let req = pb::RouteRequest {
            messages: vec![],
            ..simple_route_request()
        };
        let result = svc.route(Request::new(req)).await;
        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_grpc_route_batch() {
        let svc = test_service();
        let batch = pb::RouteBatchRequest {
            requests: vec![simple_route_request(), simple_route_request()],
        };
        let resp = svc.route_batch(Request::new(batch)).await.unwrap();
        let body = resp.into_inner();

        assert_eq!(body.responses.len(), 2);
        for r in &body.responses {
            assert_eq!(r.model, "gpt-4o");
            assert_eq!(r.provider, "openai");
        }
    }

    #[tokio::test]
    async fn test_grpc_report_outcome() {
        let svc = test_service();
        let report = pb::OutcomeReport {
            request_id: "req-1".to_string(),
            variant_id: "gpt-4o@openai".to_string(),
            status_code: 200,
            latency_ms: 150,
            ttft_ms: 50,
            input_tokens: 100,
            output_tokens: 200,
            error: None,
            used_fallback: false,
            fallback_variant_id: None,
        };
        let resp = svc.report_outcome(Request::new(report)).await;
        assert!(resp.is_ok());
    }

    #[tokio::test]
    async fn test_grpc_health_check() {
        let svc = test_service();
        let resp = svc
            .health_check(Request::new(pb::HealthCheckRequest {}))
            .await
            .unwrap();
        let body = resp.into_inner();

        assert_eq!(body.status, "healthy");
        assert_eq!(body.registered_models, 1);
        assert_eq!(body.active_variants, 1);
    }

    #[tokio::test]
    async fn test_grpc_route_no_candidates() {
        // Empty registry → NoCandidates
        let registry = InMemoryRegistry::new();
        let router = PrismRouter::new(registry);
        let state = AppState::new(router, std::path::PathBuf::from("test.yaml"));
        let svc = GrpcRouterService::new(state);

        let result = svc.route(Request::new(simple_route_request())).await;
        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn test_grpc_route_metadata_passthrough() {
        let svc = test_service();
        let mut req = simple_route_request();
        req.metadata.insert("x-request-id".to_string(), "abc-123".to_string());
        req.metadata.insert("x-tenant".to_string(), "acme".to_string());

        let resp = svc.route(Request::new(req)).await.unwrap();
        let body = resp.into_inner();

        assert_eq!(body.response_metadata.get("x-request-id").unwrap(), "abc-123");
        assert_eq!(body.response_metadata.get("x-tenant").unwrap(), "acme");
    }

    #[tokio::test]
    async fn test_grpc_route_strategy_cost_first() {
        let svc = test_service();
        let mut req = simple_route_request();
        req.strategy = Some("cost_first".to_string());

        let resp = svc.route(Request::new(req)).await.unwrap();
        let body = resp.into_inner();
        // With only one candidate, it should still route successfully
        assert_eq!(body.model, "gpt-4o");
    }

    #[tokio::test]
    async fn test_grpc_upstream_auth_headers() {
        let svc = test_service();
        let resp = svc.route(Request::new(simple_route_request())).await.unwrap();
        let upstream = resp.into_inner().upstream.unwrap();

        assert_eq!(
            upstream.headers.get("Authorization").unwrap(),
            "Bearer sk-test"
        );
    }
}
