package api

import (
	"encoding/json"
	"net/http"

	"github.com/aspect-build/prism-admin/internal/config"
)

// ---------- JSON response helpers ----------

// APIResponse is the consistent response envelope.
type APIResponse struct {
	Data interface{} `json:"data"`
	Meta *Meta       `json:"meta,omitempty"`
}

// Meta holds pagination/count info.
type Meta struct {
	Total int `json:"total"`
}

// ErrorResponse is returned on errors.
type ErrorResponse struct {
	Error   string `json:"error"`
	Details string `json:"details,omitempty"`
}

func writeJSON(w http.ResponseWriter, status int, v interface{}) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func writeData(w http.ResponseWriter, data interface{}) {
	writeJSON(w, http.StatusOK, APIResponse{Data: data})
}

func writeDataList(w http.ResponseWriter, data interface{}, total int) {
	writeJSON(w, http.StatusOK, APIResponse{Data: data, Meta: &Meta{Total: total}})
}

func writeError(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, ErrorResponse{Error: msg})
}

// ---------- System ----------

// StatusResponse represents system health.
type StatusResponse struct {
	Status          string            `json:"status"`
	ActiveVariants  int               `json:"active_variants"`
	TotalProviders  int               `json:"total_providers"`
	TotalModels     int               `json:"total_models"`
	TotalPools      int               `json:"total_pools"`
	RoutingStrategy string            `json:"routing_strategy"`
	ProviderStatus  map[string]string `json:"provider_status"`
}

func (s *Server) handleStatus(w http.ResponseWriter, r *http.Request) {
	providerStatus := make(map[string]string)
	for id := range s.prismCfg.Providers {
		providerStatus[id] = "healthy"
	}

	writeData(w, StatusResponse{
		Status:          "ok",
		ActiveVariants:  len(s.prismCfg.Variants),
		TotalProviders:  len(s.prismCfg.Providers),
		TotalModels:     len(s.prismCfg.Models),
		TotalPools:      len(s.prismCfg.Pools),
		RoutingStrategy: s.prismCfg.Routing.DefaultStrategy,
		ProviderStatus:  providerStatus,
	})
}

func (s *Server) handleConfig(w http.ResponseWriter, r *http.Request) {
	sanitized := config.SanitizeConfig(s.prismCfg)
	writeData(w, sanitized)
}

// ---------- Providers ----------

// ProviderResponse is a single provider in API responses.
type ProviderResponse struct {
	ID            string  `json:"id"`
	Type          string  `json:"type"`
	BaseURL       string  `json:"base_url"`
	TimeoutMs     int     `json:"timeout_ms"`
	MaxConcurrent int     `json:"max_concurrent"`
	Status        string  `json:"status"`
	QPS           float64 `json:"qps"`
	AvgLatencyMs  float64 `json:"avg_latency_ms"`
	ErrorRate     float64 `json:"error_rate"`
}

func (s *Server) handleProviders(w http.ResponseWriter, r *http.Request) {
	providers := make([]ProviderResponse, 0, len(s.prismCfg.Providers))
	for id, p := range s.prismCfg.Providers {
		providers = append(providers, ProviderResponse{
			ID:            id,
			Type:          p.Type,
			BaseURL:       p.BaseURL,
			TimeoutMs:     p.TimeoutMs,
			MaxConcurrent: p.MaxConcurrent,
			Status:        "healthy",
		})
	}
	writeDataList(w, providers, len(providers))
}

func (s *Server) handleProviderByID(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	p, ok := s.prismCfg.Providers[id]
	if !ok {
		writeError(w, http.StatusNotFound, "provider not found")
		return
	}

	// Find variants for this provider.
	variants := make([]string, 0)
	for _, v := range s.prismCfg.Variants {
		if v.ProviderID == id {
			variants = append(variants, v.ID)
		}
	}

	resp := struct {
		ProviderResponse
		Variants []string `json:"variants"`
	}{
		ProviderResponse: ProviderResponse{
			ID:            id,
			Type:          p.Type,
			BaseURL:       p.BaseURL,
			TimeoutMs:     p.TimeoutMs,
			MaxConcurrent: p.MaxConcurrent,
			Status:        "healthy",
		},
		Variants: variants,
	}
	writeData(w, resp)
}

// ---------- Models ----------

// ModelResponse is a model in API responses.
type ModelResponse struct {
	ID           string                    `json:"id"`
	DisplayName  string                    `json:"display_name"`
	QualityTier  string                    `json:"quality_tier"`
	Capabilities config.CapabilitiesConfig `json:"capabilities"`
	Pricing      config.PricingConfig      `json:"pricing"`
	Specialties  []string                  `json:"specialties"`
}

func (s *Server) handleModels(w http.ResponseWriter, r *http.Request) {
	models := make([]ModelResponse, 0, len(s.prismCfg.Models))
	for _, m := range s.prismCfg.Models {
		models = append(models, ModelResponse{
			ID:           m.ID,
			DisplayName:  m.DisplayName,
			QualityTier:  m.QualityTier,
			Capabilities: m.Capabilities,
			Pricing:      m.Pricing,
			Specialties:  m.Specialties,
		})
	}
	writeDataList(w, models, len(models))
}

func (s *Server) handleModelByID(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")

	for _, m := range s.prismCfg.Models {
		if m.ID == id {
			// Find variants using this model.
			variants := make([]string, 0)
			for _, v := range s.prismCfg.Variants {
				if v.ModelID == id {
					variants = append(variants, v.ID)
				}
			}

			resp := struct {
				ModelResponse
				Variants []string `json:"variants"`
			}{
				ModelResponse: ModelResponse{
					ID:           m.ID,
					DisplayName:  m.DisplayName,
					QualityTier:  m.QualityTier,
					Capabilities: m.Capabilities,
					Pricing:      m.Pricing,
					Specialties:  m.Specialties,
				},
				Variants: variants,
			}
			writeData(w, resp)
			return
		}
	}

	writeError(w, http.StatusNotFound, "model not found")
}

// ---------- Variants ----------

// VariantResponse is a variant in API responses.
type VariantResponse struct {
	ID         string  `json:"id"`
	ModelID    string  `json:"model_id"`
	ProviderID string  `json:"provider_id"`
	Status     string  `json:"status"`
	Requests   int64   `json:"requests"`
	AvgLatency float64 `json:"avg_latency_ms"`
	P99Latency float64 `json:"p99_latency_ms"`
	ErrorRate  float64 `json:"error_rate"`
	TotalCost  float64 `json:"total_cost_usd"`
}

func (s *Server) handleVariants(w http.ResponseWriter, r *http.Request) {
	metrics := s.dashboard.GetVariantMetrics()
	variants := make([]VariantResponse, 0, len(s.prismCfg.Variants))

	for i, v := range s.prismCfg.Variants {
		status := "active"
		if override, ok := s.variantStatus[v.ID]; ok {
			status = override
		}

		var m = metrics[i]
		variants = append(variants, VariantResponse{
			ID:         v.ID,
			ModelID:    v.ModelID,
			ProviderID: v.ProviderID,
			Status:     status,
			Requests:   m.Requests,
			AvgLatency: m.AvgLatency,
			P99Latency: m.P99Latency,
			ErrorRate:  m.ErrorRate,
			TotalCost:  m.TotalCost,
		})
	}
	writeDataList(w, variants, len(variants))
}

func (s *Server) handleVariantByID(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")

	for _, v := range s.prismCfg.Variants {
		if v.ID == id {
			status := "active"
			if override, ok := s.variantStatus[v.ID]; ok {
				status = override
			}
			m := s.dashboard.GetVariantMetricsByID(id)
			resp := VariantResponse{
				ID:         v.ID,
				ModelID:    v.ModelID,
				ProviderID: v.ProviderID,
				Status:     status,
			}
			if m != nil {
				resp.Requests = m.Requests
				resp.AvgLatency = m.AvgLatency
				resp.P99Latency = m.P99Latency
				resp.ErrorRate = m.ErrorRate
				resp.TotalCost = m.TotalCost
			}
			writeData(w, resp)
			return
		}
	}

	writeError(w, http.StatusNotFound, "variant not found")
}

// VariantStatusUpdate is the payload for PUT /api/v1/variants/{id}/status.
type VariantStatusUpdate struct {
	Status string `json:"status"` // "active", "disabled", "draining"
}

func (s *Server) handleVariantStatusUpdate(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")

	// Check variant exists.
	found := false
	for _, v := range s.prismCfg.Variants {
		if v.ID == id {
			found = true
			break
		}
	}
	if !found {
		writeError(w, http.StatusNotFound, "variant not found")
		return
	}

	var update VariantStatusUpdate
	if err := json.NewDecoder(r.Body).Decode(&update); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}

	switch update.Status {
	case "active", "disabled", "draining":
		// valid
	default:
		writeError(w, http.StatusBadRequest, "status must be one of: active, disabled, draining")
		return
	}

	s.variantStatus[id] = update.Status
	writeData(w, map[string]string{
		"variant_id": id,
		"status":     update.Status,
	})
}

// ---------- Pools ----------

// PoolResponse is a pool in API responses.
type PoolResponse struct {
	ID       string   `json:"id"`
	Name     string   `json:"name"`
	Variants []string `json:"variants"`
}

func (s *Server) handlePools(w http.ResponseWriter, r *http.Request) {
	pools := make([]PoolResponse, 0, len(s.prismCfg.Pools))
	for _, p := range s.prismCfg.Pools {
		pools = append(pools, PoolResponse{
			ID:       p.ID,
			Name:     p.Name,
			Variants: p.Variants,
		})
	}
	writeDataList(w, pools, len(pools))
}

func (s *Server) handlePoolByID(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")

	for _, p := range s.prismCfg.Pools {
		if p.ID == id {
			writeData(w, PoolResponse{
				ID:       p.ID,
				Name:     p.Name,
				Variants: p.Variants,
			})
			return
		}
	}

	writeError(w, http.StatusNotFound, "pool not found")
}

// ---------- Dashboard ----------

func (s *Server) handleDashboardOverview(w http.ResponseWriter, r *http.Request) {
	writeData(w, s.dashboard.GetOverview())
}

func (s *Server) handleDashboardRouting(w http.ResponseWriter, r *http.Request) {
	writeData(w, s.dashboard.GetRoutingBreakdown())
}

func (s *Server) handleDashboardProviders(w http.ResponseWriter, r *http.Request) {
	stats := s.dashboard.GetProviderStats()
	writeDataList(w, stats, len(stats))
}

// ---------- Experiments ----------

func (s *Server) handleExperiments(w http.ResponseWriter, r *http.Request) {
	experiments := s.dashboard.GetExperiments()
	writeDataList(w, experiments, len(experiments))
}

func (s *Server) handleExperimentResults(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	results := s.dashboard.GetExperimentResults(id)
	if results == nil {
		writeError(w, http.StatusNotFound, "experiment not found")
		return
	}
	writeData(w, results)
}
