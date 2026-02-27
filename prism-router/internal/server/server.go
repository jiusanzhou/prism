// Package server implements the Prism routing service HTTP handlers.
//
// This is Mode B: a standalone routing service that accepts HTTP/JSON requests,
// invokes the Rust FFI core for routing decisions, and returns structured responses.
// It can be deployed behind any API gateway (Higress, Envoy, APISIX).
package server

import (
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"sync"
	"time"

	"github.com/aspect-build/prism-router/internal/ffi"
)

// Server is the Mode B routing HTTP server.
type Server struct {
	router    *ffi.Router
	startTime time.Time
	mu        sync.RWMutex

	// Metrics counters (simple in-memory for now)
	routeCount   int64
	outcomeCount int64
	errorCount   int64
}

// New creates a new Server with the given FFI router.
func New(router *ffi.Router) *Server {
	return &Server{
		router:    router,
		startTime: time.Now(),
	}
}

// Handler returns the http.Handler for the server.
func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/route", s.handleRoute)
	mux.HandleFunc("POST /v1/route/batch", s.handleRouteBatch)
	mux.HandleFunc("POST /v1/outcome", s.handleOutcome)
	mux.HandleFunc("GET /health", s.handleHealth)
	mux.HandleFunc("GET /metrics", s.handleMetrics)
	return mux
}

// handleRoute performs a single routing decision.
func (s *Server) handleRoute(w http.ResponseWriter, r *http.Request) {
	var req RouteRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body: "+err.Error())
		return
	}

	resp, err := s.route(&req)
	if err != nil {
		s.mu.Lock()
		s.errorCount++
		s.mu.Unlock()
		writeError(w, http.StatusInternalServerError, "routing error: "+err.Error())
		return
	}

	s.mu.Lock()
	s.routeCount++
	s.mu.Unlock()

	writeJSON(w, http.StatusOK, resp)
}

// handleRouteBatch performs multiple routing decisions.
func (s *Server) handleRouteBatch(w http.ResponseWriter, r *http.Request) {
	var req RouteBatchRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body: "+err.Error())
		return
	}

	if len(req.Requests) == 0 {
		writeError(w, http.StatusBadRequest, "empty batch request")
		return
	}

	resp := RouteBatchResponse{
		Responses: make([]RouteResponse, 0, len(req.Requests)),
	}

	for i := range req.Requests {
		result, err := s.route(&req.Requests[i])
		if err != nil {
			s.mu.Lock()
			s.errorCount++
			s.mu.Unlock()
			// Include error as a zero response with metadata
			resp.Responses = append(resp.Responses, RouteResponse{
				ResponseMetadata: map[string]string{"error": err.Error()},
			})
			continue
		}
		resp.Responses = append(resp.Responses, *result)
	}

	s.mu.Lock()
	s.routeCount += int64(len(req.Requests))
	s.mu.Unlock()

	writeJSON(w, http.StatusOK, &resp)
}

// handleOutcome reports the result of a routed request for real-time tracking.
func (s *Server) handleOutcome(w http.ResponseWriter, r *http.Request) {
	var report OutcomeReport
	if err := json.NewDecoder(r.Body).Decode(&report); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body: "+err.Error())
		return
	}

	if err := s.reportOutcome(&report); err != nil {
		writeError(w, http.StatusInternalServerError, "outcome report error: "+err.Error())
		return
	}

	s.mu.Lock()
	s.outcomeCount++
	s.mu.Unlock()

	writeJSON(w, http.StatusOK, &ReportResponse{})
}

// handleHealth returns the router's health status.
func (s *Server) handleHealth(w http.ResponseWriter, r *http.Request) {
	uptime := int64(time.Since(s.startTime).Seconds())

	health := s.router.Health()

	resp := HealthCheckResponse{
		Status:           health.Status,
		UptimeSeconds:    uptime,
		RegisteredModels: health.RegisteredModels,
		ActiveVariants:   health.ActiveVariants,
	}
	writeJSON(w, http.StatusOK, &resp)
}

// handleMetrics returns Prometheus-format metrics.
func (s *Server) handleMetrics(w http.ResponseWriter, r *http.Request) {
	s.mu.RLock()
	routeCount := s.routeCount
	outcomeCount := s.outcomeCount
	errorCount := s.errorCount
	s.mu.RUnlock()

	uptime := time.Since(s.startTime).Seconds()

	w.Header().Set("Content-Type", "text/plain; version=0.0.4")
	fmt.Fprintf(w, "# HELP prism_router_requests_total Total routing requests processed.\n")
	fmt.Fprintf(w, "# TYPE prism_router_requests_total counter\n")
	fmt.Fprintf(w, "prism_router_requests_total %d\n\n", routeCount)

	fmt.Fprintf(w, "# HELP prism_router_outcomes_total Total outcome reports received.\n")
	fmt.Fprintf(w, "# TYPE prism_router_outcomes_total counter\n")
	fmt.Fprintf(w, "prism_router_outcomes_total %d\n\n", outcomeCount)

	fmt.Fprintf(w, "# HELP prism_router_errors_total Total routing errors.\n")
	fmt.Fprintf(w, "# TYPE prism_router_errors_total counter\n")
	fmt.Fprintf(w, "prism_router_errors_total %d\n\n", errorCount)

	fmt.Fprintf(w, "# HELP prism_router_uptime_seconds Router uptime in seconds.\n")
	fmt.Fprintf(w, "# TYPE prism_router_uptime_seconds gauge\n")
	fmt.Fprintf(w, "prism_router_uptime_seconds %.0f\n", uptime)
}

// route converts a RouteRequest into an FFI call and returns the result.
func (s *Server) route(req *RouteRequest) (*RouteResponse, error) {
	// Convert to FFI request
	ffiReq := s.convertToFFIRequest(req)

	// Call Rust FFI
	result, err := s.router.Route(ffiReq)
	if err != nil {
		return nil, err
	}

	// Convert FFI result to response
	resp := s.convertFromFFIResult(result, req)
	return resp, nil
}

// convertToFFIRequest translates a RouteRequest to an ffi.RouteRequest.
func (s *Server) convertToFFIRequest(req *RouteRequest) *ffi.RouteRequest {
	ffiReq := &ffi.RouteRequest{
		Messages: make([]ffi.Message, len(req.Messages)),
	}

	for i, m := range req.Messages {
		ffiReq.Messages[i] = ffi.Message{Role: m.Role, Content: m.Content}
	}

	if req.Strategy != nil {
		ffiReq.Strategy = *req.Strategy
	}
	if req.Model != nil {
		model := *req.Model
		// If a concrete model is specified (not "auto"), treat as pinned
		if model != "" && model != "auto" {
			ffiReq.Strategy = "pinned:" + model
		}
	}
	ffiReq.PoolIDs = req.PoolIDs

	if req.AffinityKey != nil {
		ffiReq.AffinityKey = *req.AffinityKey
	}
	if req.TenantID != nil {
		ffiReq.TenantID = *req.TenantID
	}

	// Strategy params
	if req.StrategyParams != nil {
		ffiReq.CostWeight = req.StrategyParams.CostWeight
		ffiReq.QualityWeight = req.StrategyParams.QualityWeight
		ffiReq.LatencyWeight = req.StrategyParams.LatencyWeight
		ffiReq.MaxLatencyMs = req.StrategyParams.MaxLatencyMs
		ffiReq.MaxTTFTMs = req.StrategyParams.MaxTTFTMs
	}

	return ffiReq
}

// convertFromFFIResult translates an ffi.RouteResult to a RouteResponse.
func (s *Server) convertFromFFIResult(result *ffi.RouteResult, req *RouteRequest) *RouteResponse {
	resp := &RouteResponse{
		Model:          result.Model,
		Provider:       result.Provider,
		Variant:        result.Variant,
		Account:        result.Account,
		Score:          result.Score,
		DecisionTimeUs: result.DecisionTimeUs,
	}

	// Build upstream from router result
	// The actual base_url/headers come from the config; the FFI returns the identifiers.
	// In Mode B the gateway itself resolves these from its config.
	resp.Upstream = &Upstream{
		ModelName: result.Model,
		VariantID: result.Variant,
		Path:      "/v1/chat/completions",
	}

	// Build fallback chain
	if len(result.FallbackChain) > 0 {
		resp.FallbackChain = make([]Upstream, len(result.FallbackChain))
		for i, fb := range result.FallbackChain {
			resp.FallbackChain[i] = Upstream{
				VariantID: fb,
				Path:      "/v1/chat/completions",
			}
		}
	}

	// Parse reason
	if result.Reason != "" {
		resp.Reason = &RoutingReason{
			L1Model:    result.Model,
			L2Provider: result.Provider,
			L3Account:  result.Account,
			L4Variant:  result.Variant,
		}
	}

	// Pass through metadata
	if len(req.Metadata) > 0 {
		resp.ResponseMetadata = req.Metadata
	}

	return resp
}

// reportOutcome sends outcome data to the FFI core for stats updates.
func (s *Server) reportOutcome(report *OutcomeReport) error {
	statsJSON, err := json.Marshal(map[string]interface{}{
		"variant_id":   report.VariantID,
		"status_code":  report.StatusCode,
		"latency_ms":   report.LatencyMs,
		"ttft_ms":      report.TTFTMs,
		"input_tokens":  report.InputTokens,
		"output_tokens": report.OutputTokens,
	})
	if err != nil {
		return fmt.Errorf("failed to marshal stats: %w", err)
	}

	return s.router.UpdateStats(string(statsJSON))
}

// ──────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────

func writeJSON(w http.ResponseWriter, status int, v interface{}) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	if err := json.NewEncoder(w).Encode(v); err != nil {
		log.Printf("Failed to write JSON response: %v", err)
	}
}

type errorResponse struct {
	Error string `json:"error"`
}

func writeError(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, &errorResponse{Error: msg})
}
