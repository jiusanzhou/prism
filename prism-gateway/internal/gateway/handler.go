// Package gateway implements the HTTP server and request handling.
package gateway

import (
	"encoding/json"
	"fmt"
	"net/http"
	"strconv"
	"time"

	"github.com/aspect-build/prism-gateway/internal/abtesting"
	"github.com/aspect-build/prism-gateway/internal/metrics"
	"github.com/aspect-build/prism-gateway/internal/provider"
	"github.com/aspect-build/prism-gateway/internal/router"
	"github.com/aspect-build/prism-gateway/internal/state"
	"github.com/aspect-build/prism-gateway/pkg/api"
)

// Handler handles incoming OpenAI-compatible API requests.
type Handler struct {
	router       router.Router
	providers    map[string]provider.Provider
	state        state.StateStore
	abManager    *abtesting.Manager
}

// NewHandler creates a new handler with the given router and providers.
func NewHandler(r router.Router, providers map[string]provider.Provider) *Handler {
	return &Handler{
		router:    r,
		providers: providers,
	}
}

// SetStateStore sets the state store for tracking variant metrics.
func (h *Handler) SetStateStore(s state.StateStore) {
	h.state = s
}

// SetABManager sets the A/B testing manager.
func (h *Handler) SetABManager(m *abtesting.Manager) {
	h.abManager = m
}

// ChatCompletion handles POST /v1/chat/completions.
func (h *Handler) ChatCompletion(w http.ResponseWriter, r *http.Request) {
	// Parse request
	var req api.ChatCompletionRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body: "+err.Error())
		return
	}

	// Build route request from headers + body
	routeReq := h.buildRouteRequest(&req, r)

	// Perform routing decision (timed)
	routeStart := time.Now()
	routeResult, err := h.router.Route(routeReq)
	routeDuration := time.Since(routeStart)

	strategy := routeReq.Strategy
	if strategy == "" {
		strategy = "auto"
	}

	if err != nil {
		metrics.RoutingDecisionDuration.WithLabelValues(strategy).Observe(routeDuration.Seconds())
		metrics.RoutingDecisionTotal.WithLabelValues(strategy, "", "", "error").Inc()
		writeError(w, http.StatusServiceUnavailable, "routing failed: "+err.Error())
		return
	}

	metrics.RoutingDecisionDuration.WithLabelValues(strategy).Observe(routeDuration.Seconds())
	metrics.RoutingDecisionTotal.WithLabelValues(strategy, routeResult.Model, routeResult.Provider, "ok").Inc()

	// Track active requests
	metrics.ActiveRequests.WithLabelValues(routeResult.Provider).Inc()
	defer metrics.ActiveRequests.WithLabelValues(routeResult.Provider).Dec()

	// Increment QPS in state store
	if h.state != nil {
		_ = h.state.IncrementQPS(r.Context(), routeResult.Variant)
	}

	// Set routing info headers
	h.setRoutingHeaders(w, routeResult)

	// A/B experiment assignment
	if h.abManager != nil {
		// Use a request ID from the trace header or generate one from the route
		requestID := r.Header.Get(api.HeaderPrismTraceID)
		if requestID == "" {
			requestID = routeResult.Variant // fallback for consistent assignment
		}
		poolID := ""
		if len(routeReq.PoolIDs) > 0 {
			poolID = routeReq.PoolIDs[0]
		}
		expID, expVariant := h.abManager.Assign(requestID, poolID, routeReq.TenantID, routeReq.Source)
		if expID != "" {
			w.Header().Set(api.HeaderPrismExperimentID, expID)
			w.Header().Set(api.HeaderPrismExperimentVariant, expVariant)
		}
	}

	// Find the provider
	p, ok := h.providers[routeResult.Provider]
	if !ok {
		metrics.ProviderErrorTotal.WithLabelValues(routeResult.Provider, routeResult.Model, "not_found").Inc()
		writeError(w, http.StatusBadGateway, "provider not found: "+routeResult.Provider)
		return
	}

	// Dispatch to provider
	providerStart := time.Now()

	if req.Stream {
		h.handleStream(w, r, p, routeResult, &req, providerStart)
	} else {
		h.handleNonStream(w, r, p, routeResult, &req, providerStart)
	}
}

func (h *Handler) handleNonStream(w http.ResponseWriter, r *http.Request, p provider.Provider, result *router.RouteResult, req *api.ChatCompletionRequest, providerStart time.Time) {
	resp, err := p.ChatCompletion(r.Context(), result.Model, req)
	providerDuration := time.Since(providerStart)

	if err != nil {
		metrics.ProviderRequestDuration.WithLabelValues(result.Provider, result.Model, "false").Observe(providerDuration.Seconds())
		metrics.ProviderRequestTotal.WithLabelValues(result.Provider, result.Model, "502").Inc()
		metrics.ProviderErrorTotal.WithLabelValues(result.Provider, result.Model, "request_error").Inc()
		if h.state != nil {
			_ = h.state.RecordError(r.Context(), result.Variant)
		}
		writeError(w, http.StatusBadGateway, "provider error: "+err.Error())
		return
	}

	metrics.ProviderRequestDuration.WithLabelValues(result.Provider, result.Model, "false").Observe(providerDuration.Seconds())
	metrics.ProviderRequestTotal.WithLabelValues(result.Provider, result.Model, "200").Inc()

	// Record latency in state store
	if h.state != nil {
		_ = h.state.RecordLatency(r.Context(), result.Variant, providerDuration.Milliseconds())
	}

	// Record token usage
	if resp.Usage != nil {
		metrics.TokensUsed.WithLabelValues(result.Provider, result.Model, "prompt").Add(float64(resp.Usage.PromptTokens))
		metrics.TokensUsed.WithLabelValues(result.Provider, result.Model, "completion").Add(float64(resp.Usage.CompletionTokens))
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(resp)
}

func (h *Handler) handleStream(w http.ResponseWriter, r *http.Request, p provider.Provider, result *router.RouteResult, req *api.ChatCompletionRequest, providerStart time.Time) {
	stream, err := p.ChatCompletionStream(r.Context(), result.Model, req)
	if err != nil {
		streamSetupDuration := time.Since(providerStart)
		metrics.ProviderRequestDuration.WithLabelValues(result.Provider, result.Model, "true").Observe(streamSetupDuration.Seconds())
		metrics.ProviderRequestTotal.WithLabelValues(result.Provider, result.Model, "502").Inc()
		metrics.ProviderErrorTotal.WithLabelValues(result.Provider, result.Model, "stream_error").Inc()
		if h.state != nil {
			_ = h.state.RecordError(r.Context(), result.Variant)
		}
		writeError(w, http.StatusBadGateway, "provider stream error: "+err.Error())
		return
	}
	defer stream.Close()

	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("Connection", "keep-alive")
	w.WriteHeader(http.StatusOK)

	flusher, ok := w.(http.Flusher)
	if !ok {
		writeError(w, http.StatusInternalServerError, "streaming not supported")
		return
	}

	firstChunk := true
	for {
		chunk, err := stream.Recv()
		if err != nil {
			// EOF or error — send [DONE]
			fmt.Fprintf(w, "data: [DONE]\n\n")
			flusher.Flush()

			// Record total streaming duration
			totalDuration := time.Since(providerStart)
			metrics.ProviderRequestDuration.WithLabelValues(result.Provider, result.Model, "true").Observe(totalDuration.Seconds())
			metrics.ProviderRequestTotal.WithLabelValues(result.Provider, result.Model, "200").Inc()

			// Record latency in state store
			if h.state != nil {
				_ = h.state.RecordLatency(r.Context(), result.Variant, totalDuration.Milliseconds())
			}

			return
		}

		// Record TTFT on first chunk
		if firstChunk {
			ttft := time.Since(providerStart)
			metrics.TTFTDuration.WithLabelValues(result.Provider, result.Model).Observe(ttft.Seconds())
			firstChunk = false
		}

		data, _ := json.Marshal(chunk)
		fmt.Fprintf(w, "data: %s\n\n", data)
		flusher.Flush()
	}
}

// ReportOutcome handles POST /v1/outcomes.
func (h *Handler) ReportOutcome(w http.ResponseWriter, r *http.Request) {
	var report api.OutcomeReport
	if err := json.NewDecoder(r.Body).Decode(&report); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body: "+err.Error())
		return
	}

	// Update Prometheus metrics from outcome report
	providerID := report.ProviderID
	modelID := report.ModelID

	if report.Success {
		statusCode := strconv.Itoa(report.StatusCode)
		if statusCode == "0" {
			statusCode = "200"
		}
		metrics.ProviderRequestTotal.WithLabelValues(providerID, modelID, statusCode).Inc()
	} else {
		errorType := report.ErrorType
		if errorType == "" {
			errorType = "unknown"
		}
		metrics.ProviderErrorTotal.WithLabelValues(providerID, modelID, errorType).Inc()
		metrics.ProviderRequestTotal.WithLabelValues(providerID, modelID, strconv.Itoa(report.StatusCode)).Inc()
	}

	// Record latency
	if report.TotalLatencyMs > 0 {
		metrics.ProviderRequestDuration.WithLabelValues(providerID, modelID, "false").Observe(float64(report.TotalLatencyMs) / 1000.0)
	}

	// Record TTFT
	if report.TTFTMs != nil {
		metrics.TTFTDuration.WithLabelValues(providerID, modelID).Observe(float64(*report.TTFTMs) / 1000.0)
	}

	// Record token usage
	if report.PromptTokens > 0 {
		metrics.TokensUsed.WithLabelValues(providerID, modelID, "prompt").Add(float64(report.PromptTokens))
	}
	if report.CompletionTokens > 0 {
		metrics.TokensUsed.WithLabelValues(providerID, modelID, "completion").Add(float64(report.CompletionTokens))
	}

	// Update state store if configured
	if h.state != nil {
		ctx := r.Context()
		variantID := report.VariantID
		if variantID != "" {
			if report.TotalLatencyMs > 0 {
				_ = h.state.RecordLatency(ctx, variantID, report.TotalLatencyMs)
			}
			if !report.Success {
				_ = h.state.RecordError(ctx, variantID)
			}
			_ = h.state.IncrementQPS(ctx, variantID)
		}
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(api.OutcomeResponse{Status: "ok"})
}

func (h *Handler) buildRouteRequest(req *api.ChatCompletionRequest, r *http.Request) *router.RouteRequest {
	routeReq := &router.RouteRequest{
		Messages: req.Messages,
		Headers:  make(map[string]string),
	}

	// Strategy from header or body
	if strategy := r.Header.Get(api.HeaderPrismStrategy); strategy != "" {
		routeReq.Strategy = strategy
	} else if req.Prism != nil && req.Prism.Strategy != "" {
		routeReq.Strategy = req.Prism.Strategy
	} else if req.Model != "" && req.Model != "auto" {
		routeReq.Strategy = "pinned:" + req.Model
	}

	// Pool IDs
	if pool := r.Header.Get(api.HeaderPrismModelPool); pool != "" {
		routeReq.PoolIDs = []string{pool}
	} else if req.Prism != nil {
		routeReq.PoolIDs = req.Prism.PoolIDs
	}

	// Affinity
	if key := r.Header.Get(api.HeaderPrismAffinityKey); key != "" {
		routeReq.AffinityKey = key
	} else if key := r.Header.Get(api.HeaderPrismSessionID); key != "" {
		routeReq.AffinityKey = key
	}

	// Tenant ID
	if tid := r.Header.Get(api.HeaderPrismTenantID); tid != "" {
		routeReq.TenantID = tid
	}

	// Strategy weights from headers or body
	if v := r.Header.Get(api.HeaderPrismCostWeight); v != "" {
		if f, err := strconv.ParseFloat(v, 32); err == nil {
			f32 := float32(f)
			routeReq.CostWeight = &f32
		}
	} else if req.Prism != nil && req.Prism.CostWeight != nil {
		routeReq.CostWeight = req.Prism.CostWeight
	}

	if v := r.Header.Get(api.HeaderPrismQualWeight); v != "" {
		if f, err := strconv.ParseFloat(v, 32); err == nil {
			f32 := float32(f)
			routeReq.QualityWeight = &f32
		}
	} else if req.Prism != nil && req.Prism.QualityWeight != nil {
		routeReq.QualityWeight = req.Prism.QualityWeight
	}

	if v := r.Header.Get(api.HeaderPrismLatWeight); v != "" {
		if f, err := strconv.ParseFloat(v, 32); err == nil {
			f32 := float32(f)
			routeReq.LatencyWeight = &f32
		}
	} else if req.Prism != nil && req.Prism.LatencyWeight != nil {
		routeReq.LatencyWeight = req.Prism.LatencyWeight
	}

	// SLO constraints from headers or body
	if v := r.Header.Get(api.HeaderPrismMaxLatencyMs); v != "" {
		if n, err := strconv.ParseUint(v, 10, 32); err == nil {
			u32 := uint32(n)
			routeReq.MaxLatencyMs = &u32
		}
	} else if req.Prism != nil && req.Prism.SLO != nil && req.Prism.SLO.MaxLatencyMs > 0 {
		u32 := uint32(req.Prism.SLO.MaxLatencyMs)
		routeReq.MaxLatencyMs = &u32
	}

	if v := r.Header.Get(api.HeaderPrismMaxTTFTMs); v != "" {
		if n, err := strconv.ParseUint(v, 10, 32); err == nil {
			u32 := uint32(n)
			routeReq.MaxTTFTMs = &u32
		}
	} else if req.Prism != nil && req.Prism.SLO != nil && req.Prism.SLO.MaxTTFTMs > 0 {
		u32 := uint32(req.Prism.SLO.MaxTTFTMs)
		routeReq.MaxTTFTMs = &u32
	}

	// AnalyzeContext: CallerType
	if v := r.Header.Get(api.HeaderPrismCallerType); v != "" {
		routeReq.CallerType = v
	} else if req.Prism != nil && req.Prism.CallerType != "" {
		routeReq.CallerType = req.Prism.CallerType
	}

	// AnalyzeContext: Source
	if v := r.Header.Get(api.HeaderPrismSource); v != "" {
		routeReq.Source = v
	} else if req.Prism != nil && req.Prism.Source != "" {
		routeReq.Source = req.Prism.Source
	}

	// Copy all Prism headers
	for _, h := range []string{
		api.HeaderPrismStrategy,
		api.HeaderPrismCostWeight,
		api.HeaderPrismQualWeight,
		api.HeaderPrismLatWeight,
		api.HeaderPrismMaxLatencyMs,
		api.HeaderPrismMaxTTFTMs,
		api.HeaderPrismModelPool,
		api.HeaderPrismTraceID,
		api.HeaderPrismAffinityKey,
		api.HeaderPrismSessionID,
		api.HeaderPrismCallerType,
		api.HeaderPrismSource,
		api.HeaderPrismTenantID,
	} {
		if v := r.Header.Get(h); v != "" {
			routeReq.Headers[h] = v
		}
	}

	return routeReq
}

func (h *Handler) setRoutingHeaders(w http.ResponseWriter, result *router.RouteResult) {
	w.Header().Set(api.HeaderPrismModel, result.Model)
	w.Header().Set(api.HeaderPrismProvider, result.Provider)
	w.Header().Set(api.HeaderPrismVariant, result.Variant)
	w.Header().Set(api.HeaderPrismAccount, result.Account)
	w.Header().Set(api.HeaderPrismDecisionTime, strconv.FormatInt(result.DecisionTimeUs, 10))
	w.Header().Set(api.HeaderPrismReason, result.Reason)
	if len(result.FallbackChain) > 0 {
		data, _ := json.Marshal(result.FallbackChain)
		w.Header().Set(api.HeaderPrismFallback, string(data))
	}
}

func writeError(w http.ResponseWriter, code int, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	json.NewEncoder(w).Encode(map[string]interface{}{
		"error": map[string]interface{}{
			"message": msg,
			"type":    "prism_error",
			"code":    code,
		},
	})
}

// Health handles GET /health.
func (h *Handler) Health(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
}

// ListExperiments handles GET /v1/experiments.
func (h *Handler) ListExperiments(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	if h.abManager == nil {
		w.WriteHeader(http.StatusOK)
		json.NewEncoder(w).Encode(map[string]interface{}{
			"experiments": []interface{}{},
		})
		return
	}

	experiments := h.abManager.ListExperiments()
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(map[string]interface{}{
		"experiments": experiments,
	})
}

// GetExperiment handles GET /v1/experiments/{id}.
func (h *Handler) GetExperiment(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if id == "" {
		writeError(w, http.StatusBadRequest, "experiment id required")
		return
	}

	if h.abManager == nil {
		writeError(w, http.StatusNotFound, "experiment not found: "+id)
		return
	}

	exp := h.abManager.GetExperiment(id)
	if exp == nil {
		writeError(w, http.StatusNotFound, "experiment not found: "+id)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(exp)
}
