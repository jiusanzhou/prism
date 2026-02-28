package gateway

import (
	"encoding/json"
	"fmt"
	"net/http"
	"time"

	"github.com/aspect-build/prism-gateway/internal/metrics"
	"github.com/aspect-build/prism-gateway/internal/provider"
	"github.com/aspect-build/prism-gateway/internal/router"
	"github.com/aspect-build/prism-gateway/pkg/api"
)

// AnthropicMessages handles POST /v1/messages (Anthropic Messages API).
//
// Flow:
//  1. Parse Anthropic format request
//  2. Convert to internal ChatCompletionRequest
//  3. Route via buildRouteRequest
//  4. Dispatch to provider
//  5. Convert response back to Anthropic format
func (h *Handler) AnthropicMessages(w http.ResponseWriter, r *http.Request) {
	// Parse Anthropic request
	var req api.AnthropicMessagesRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeAnthropicError(w, http.StatusBadRequest, "invalid_request_error",
			"invalid request body: "+err.Error())
		return
	}

	// Validate required fields
	if req.Model == "" {
		writeAnthropicError(w, http.StatusBadRequest, "invalid_request_error",
			"model is required")
		return
	}
	if req.MaxTokens <= 0 {
		writeAnthropicError(w, http.StatusBadRequest, "invalid_request_error",
			"max_tokens is required and must be > 0")
		return
	}

	// Convert to internal format
	internalReq, err := api.AnthropicToInternal(&req)
	if err != nil {
		writeAnthropicError(w, http.StatusBadRequest, "invalid_request_error", err.Error())
		return
	}

	// Build route request (reusing the same logic as OpenAI handler)
	routeReq := h.buildRouteRequest(internalReq, r)
	routeReq.RequiredProtocol = "anthropic"

	// Perform routing decision
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
		writeAnthropicError(w, http.StatusServiceUnavailable, "api_error",
			"routing failed: "+err.Error())
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

	// A/B experiment assignment (same as OpenAI handler)
	if h.abManager != nil {
		requestID := r.Header.Get(api.HeaderPrismTraceID)
		if requestID == "" {
			requestID = routeResult.Variant
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
		writeAnthropicError(w, http.StatusBadGateway, "api_error",
			"provider not found: "+routeResult.Provider)
		return
	}

	// Dispatch
	providerStart := time.Now()

	if req.Stream {
		h.handleAnthropicStream(w, r, p, routeResult, internalReq, providerStart)
	} else {
		h.handleAnthropicNonStream(w, r, p, routeResult, internalReq, providerStart)
	}
}

func (h *Handler) handleAnthropicNonStream(w http.ResponseWriter, r *http.Request, p provider.Provider, result *router.RouteResult, req *api.ChatCompletionRequest, providerStart time.Time) {
	resp, err := p.ChatCompletion(r.Context(), result.Model, req)
	providerDuration := time.Since(providerStart)

	if err != nil {
		metrics.ProviderRequestDuration.WithLabelValues(result.Provider, result.Model, "false").Observe(providerDuration.Seconds())
		metrics.ProviderRequestTotal.WithLabelValues(result.Provider, result.Model, "502").Inc()
		metrics.ProviderErrorTotal.WithLabelValues(result.Provider, result.Model, "request_error").Inc()
		if h.state != nil {
			_ = h.state.RecordError(r.Context(), result.Variant)
		}
		writeAnthropicError(w, http.StatusBadGateway, "api_error",
			"provider error: "+err.Error())
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

	// Convert to Anthropic format
	anthropicResp := api.InternalToAnthropic(resp)

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(anthropicResp)
}

func (h *Handler) handleAnthropicStream(w http.ResponseWriter, r *http.Request, p provider.Provider, result *router.RouteResult, req *api.ChatCompletionRequest, providerStart time.Time) {
	// Set stream to true on internal request
	req.Stream = true

	stream, err := p.ChatCompletionStream(r.Context(), result.Model, req)
	if err != nil {
		streamSetupDuration := time.Since(providerStart)
		metrics.ProviderRequestDuration.WithLabelValues(result.Provider, result.Model, "true").Observe(streamSetupDuration.Seconds())
		metrics.ProviderRequestTotal.WithLabelValues(result.Provider, result.Model, "502").Inc()
		metrics.ProviderErrorTotal.WithLabelValues(result.Provider, result.Model, "stream_error").Inc()
		if h.state != nil {
			_ = h.state.RecordError(r.Context(), result.Variant)
		}
		writeAnthropicError(w, http.StatusBadGateway, "api_error",
			"provider stream error: "+err.Error())
		return
	}
	defer stream.Close()

	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("Connection", "keep-alive")
	w.WriteHeader(http.StatusOK)

	flusher, ok := w.(http.Flusher)
	if !ok {
		writeAnthropicError(w, http.StatusInternalServerError, "api_error",
			"streaming not supported")
		return
	}

	isFirst := true
	gotFinish := false
	for {
		chunk, err := stream.Recv()
		if err != nil {
			// EOF or error — send closing events if we haven't seen a finish_reason
			if isFirst {
				// Empty stream: send message_start + content_block_start
				writeAnthropicSSEEvents(w, flusher, api.InternalChunkToAnthropicEvents(
					&api.ChatCompletionChunk{
						ID:    "msg_empty",
						Model: result.Model,
					}, true, result.Model))
			}

			if !gotFinish {
				// Stream ended without a finish_reason chunk — send closing events
				endTurn := "end_turn"
				closeEvents := []api.AnthropicSSEEvent{
					{Event: "content_block_stop", Data: api.AnthropicContentBlockStopEvent{Type: "content_block_stop", Index: 0}},
					{Event: "message_delta", Data: api.AnthropicMessageDeltaEvent{Type: "message_delta", Delta: api.AnthropicMessageDeltaDelta{StopReason: &endTurn}, Usage: api.AnthropicMessageDeltaUsage{OutputTokens: 0}}},
					{Event: "message_stop", Data: api.AnthropicMessageStopEvent{Type: "message_stop"}},
				}
				writeAnthropicSSEEvents(w, flusher, closeEvents)
			}

			// Record total streaming duration
			totalDuration := time.Since(providerStart)
			metrics.ProviderRequestDuration.WithLabelValues(result.Provider, result.Model, "true").Observe(totalDuration.Seconds())
			metrics.ProviderRequestTotal.WithLabelValues(result.Provider, result.Model, "200").Inc()

			if h.state != nil {
				_ = h.state.RecordLatency(r.Context(), result.Variant, totalDuration.Milliseconds())
			}
			return
		}

		// Record TTFT on first chunk
		if isFirst {
			ttft := time.Since(providerStart)
			metrics.TTFTDuration.WithLabelValues(result.Provider, result.Model).Observe(ttft.Seconds())
		}

		// Convert chunk to Anthropic SSE events
		events := api.InternalChunkToAnthropicEvents(chunk, isFirst, result.Model)
		writeAnthropicSSEEvents(w, flusher, events)

		// Check if this chunk had a finish_reason
		if len(chunk.Choices) > 0 && chunk.Choices[0].FinishReason != nil {
			gotFinish = true
		}

		isFirst = false
	}
}

// writeAnthropicSSEEvents writes Anthropic SSE events in the correct format.
func writeAnthropicSSEEvents(w http.ResponseWriter, flusher http.Flusher, events []api.AnthropicSSEEvent) {
	for _, event := range events {
		data, err := json.Marshal(event.Data)
		if err != nil {
			continue
		}
		fmt.Fprintf(w, "event: %s\ndata: %s\n\n", event.Event, data)
	}
	flusher.Flush()
}

// writeAnthropicError writes an error response in Anthropic format.
func writeAnthropicError(w http.ResponseWriter, statusCode int, errType, message string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(statusCode)
	json.NewEncoder(w).Encode(api.NewAnthropicErrorResponse(errType, message))
}
