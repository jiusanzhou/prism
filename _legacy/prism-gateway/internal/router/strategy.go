package router

import (
	"encoding/json"
	"strings"
)

// buildStrategyJSON converts a RouteRequest's strategy string + params into the
// Rust ModelSelectionStrategy enum JSON format.
func buildStrategyJSON(req *RouteRequest) ([]byte, error) {
	if req.Strategy == "" {
		return nil, nil
	}

	// Handle pinned model: "pinned:MODEL_ID"
	if strings.HasPrefix(req.Strategy, "pinned:") {
		modelID := strings.TrimPrefix(req.Strategy, "pinned:")
		return json.Marshal(map[string]interface{}{
			"Pinned": map[string]interface{}{
				"model_id": modelID,
			},
		})
	}

	// Build pool_ids (nullable)
	var poolIDs interface{}
	if len(req.PoolIDs) > 0 {
		poolIDs = req.PoolIDs
	}

	switch req.Strategy {
	case "balanced", "auto":
		costW := float32(0.3)
		qualW := float32(0.5)
		latW := float32(0.2)
		if req.CostWeight != nil {
			costW = *req.CostWeight
		}
		if req.QualityWeight != nil {
			qualW = *req.QualityWeight
		}
		if req.LatencyWeight != nil {
			latW = *req.LatencyWeight
		}
		return json.Marshal(map[string]interface{}{
			"Auto": map[string]interface{}{
				"objective": map[string]interface{}{
					"Balanced": map[string]interface{}{
						"cost_weight":    costW,
						"quality_weight": qualW,
						"latency_weight": latW,
					},
				},
				"pool_ids":              poolIDs,
				"required_capabilities": nil,
			},
		})

	case "cost_first":
		return json.Marshal(map[string]interface{}{
			"Auto": map[string]interface{}{
				"objective": map[string]interface{}{
					"CostFirst": map[string]interface{}{
						"min_quality_score":  0.0,
						"budget_per_request": nil,
					},
				},
				"pool_ids":              poolIDs,
				"required_capabilities": nil,
			},
		})

	case "quality_first":
		return json.Marshal(map[string]interface{}{
			"Auto": map[string]interface{}{
				"objective": map[string]interface{}{
					"QualityFirst": map[string]interface{}{
						"max_latency_ms": nil,
					},
				},
				"pool_ids":              poolIDs,
				"required_capabilities": nil,
			},
		})

	case "slo_first":
		var maxLatMs uint32 = 500
		var maxTtftMs uint32 = 200
		if req.MaxLatencyMs != nil {
			maxLatMs = *req.MaxLatencyMs
		}
		if req.MaxTTFTMs != nil {
			maxTtftMs = *req.MaxTTFTMs
		}
		return json.Marshal(map[string]interface{}{
			"Auto": map[string]interface{}{
				"objective": map[string]interface{}{
					"SloFirst": map[string]interface{}{
						"max_latency_ms":    maxLatMs,
						"max_ttft_ms":       maxTtftMs,
						"min_quality_score": 0.3,
					},
				},
				"pool_ids":              poolIDs,
				"required_capabilities": nil,
			},
		})

	default:
		// Unknown strategy — fall back to balanced default
		return json.Marshal(map[string]interface{}{
			"Auto": map[string]interface{}{
				"objective": map[string]interface{}{
					"Balanced": map[string]interface{}{
						"cost_weight":    0.3,
						"quality_weight": 0.5,
						"latency_weight": 0.2,
					},
				},
				"pool_ids":              poolIDs,
				"required_capabilities": nil,
			},
		})
	}
}

// buildAnalyzeContextJSON builds the analyze_context JSON from RouteRequest fields.
func buildAnalyzeContextJSON(req *RouteRequest) ([]byte, error) {
	if req.CallerType == "" && req.Source == "" && req.TenantID == "" && len(req.Tags) == 0 {
		return nil, nil
	}

	ctx := map[string]interface{}{}
	if req.CallerType != "" {
		ctx["caller_type"] = req.CallerType
	}
	if req.Source != "" {
		ctx["source"] = req.Source
	}
	if req.TenantID != "" {
		ctx["tenant_id"] = req.TenantID
	}
	if len(req.Tags) > 0 {
		ctx["tags"] = req.Tags
	}

	return json.Marshal(ctx)
}
