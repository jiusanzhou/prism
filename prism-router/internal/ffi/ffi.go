//go:build ffi

package ffi

/*
#cgo LDFLAGS: -L${SRCDIR}/../../../../target/release -lprism_core
#include <stdint.h>
#include <stdlib.h>

typedef struct {
    const char* messages_json;
    const char* strategy_json;
    const char* metadata_json;
    const char* analyze_context_json;
} PrismRouteRequest;

typedef struct {
    const char* model_id;
    const char* provider;
    const char* variant_id;
    const char* account_id;
    const char* fallbacks_json;
    const char* reason_json;
    uint64_t    decision_time_us;
    const char* error;
} PrismRouteResult;

extern int32_t prism_init(const char* config_json);
extern PrismRouteResult* prism_route(const PrismRouteRequest* request);
extern int32_t prism_update_model_stats(const char* stats_json);
extern void prism_free_result(PrismRouteResult* result);
extern const char* prism_version();
extern const char* prism_health();
*/
import "C"

import (
	"encoding/json"
	"fmt"
	"strings"
	"unsafe"
)

// FFIImpl implements routerImpl via the Rust prism-core library.
type FFIImpl struct{}

// NewFFIRouter initializes the Rust routing engine and returns a Router.
func NewFFIRouter(configJSON string) (*Router, error) {
	cConfig := C.CString(configJSON)
	defer C.free(unsafe.Pointer(cConfig))

	rc := C.prism_init(cConfig)
	if rc != 0 {
		return nil, fmt.Errorf("prism_init failed with code %d", int(rc))
	}

	return NewRouter(&FFIImpl{}), nil
}

func (f *FFIImpl) Route(req *RouteRequest) (*RouteResult, error) {
	messagesJSON, err := json.Marshal(req.Messages)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal messages: %w", err)
	}

	strategyJSON, err := buildStrategyJSON(req)
	if err != nil {
		return nil, fmt.Errorf("failed to build strategy JSON: %w", err)
	}

	var metadataJSON []byte
	if req.AffinityKey != "" || req.TenantID != "" || req.Region != "" {
		meta := map[string]string{}
		if req.AffinityKey != "" {
			meta["affinity_key"] = req.AffinityKey
		}
		if req.TenantID != "" {
			meta["tenant_id"] = req.TenantID
		}
		if req.Region != "" {
			meta["region"] = req.Region
		}
		metadataJSON, err = json.Marshal(meta)
		if err != nil {
			return nil, fmt.Errorf("failed to marshal metadata: %w", err)
		}
	}

	cMessages := C.CString(string(messagesJSON))
	defer C.free(unsafe.Pointer(cMessages))

	var cStrategy *C.char
	if strategyJSON != nil {
		cStrategy = C.CString(string(strategyJSON))
		defer C.free(unsafe.Pointer(cStrategy))
	}

	var cMetadata *C.char
	if metadataJSON != nil {
		cMetadata = C.CString(string(metadataJSON))
		defer C.free(unsafe.Pointer(cMetadata))
	}

	cReq := C.PrismRouteRequest{
		messages_json:        cMessages,
		strategy_json:        cStrategy,
		metadata_json:        cMetadata,
		analyze_context_json: nil,
	}

	cResult := C.prism_route(&cReq)
	if cResult == nil {
		return nil, fmt.Errorf("prism_route returned nil")
	}
	defer C.prism_free_result(cResult)

	if cResult.error != nil {
		return nil, fmt.Errorf("prism_route error: %s", C.GoString(cResult.error))
	}

	result := &RouteResult{
		Model:          C.GoString(cResult.model_id),
		Provider:       C.GoString(cResult.provider),
		Variant:        C.GoString(cResult.variant_id),
		Account:        C.GoString(cResult.account_id),
		DecisionTimeUs: int64(cResult.decision_time_us),
	}

	if cResult.fallbacks_json != nil {
		fallbacksStr := C.GoString(cResult.fallbacks_json)
		if fallbacksStr != "" {
			var fallbacks []string
			if err := json.Unmarshal([]byte(fallbacksStr), &fallbacks); err == nil {
				result.FallbackChain = fallbacks
			}
		}
	}

	if cResult.reason_json != nil {
		result.Reason = C.GoString(cResult.reason_json)
	}

	return result, nil
}

func (f *FFIImpl) UpdateStats(statsJSON string) error {
	cStats := C.CString(statsJSON)
	defer C.free(unsafe.Pointer(cStats))

	rc := C.prism_update_model_stats(cStats)
	if rc != 0 {
		return fmt.Errorf("prism_update_model_stats failed with code %d", int(rc))
	}
	return nil
}

func (f *FFIImpl) Version() string {
	return C.GoString(C.prism_version())
}

func (f *FFIImpl) Health() *HealthStatus {
	healthJSON := C.GoString(C.prism_health())
	// Parse minimal health info from JSON
	var raw map[string]interface{}
	if err := json.Unmarshal([]byte(healthJSON), &raw); err != nil {
		return &HealthStatus{Status: "unknown"}
	}

	status := "healthy"
	if s, ok := raw["status"].(string); ok {
		status = s
	}

	var models, variants uint32
	if m, ok := raw["registered_models"].(float64); ok {
		models = uint32(m)
	}
	if v, ok := raw["active_variants"].(float64); ok {
		variants = uint32(v)
	}

	return &HealthStatus{
		Status:           status,
		RegisteredModels: models,
		ActiveVariants:   variants,
	}
}

// buildStrategyJSON converts strategy string + params into the Rust enum JSON format.
// Mirrors prism-gateway/internal/router/strategy.go.
func buildStrategyJSON(req *RouteRequest) ([]byte, error) {
	if req.Strategy == "" {
		return nil, nil
	}

	if strings.HasPrefix(req.Strategy, "pinned:") {
		modelID := strings.TrimPrefix(req.Strategy, "pinned:")
		return json.Marshal(map[string]interface{}{
			"Pinned": map[string]interface{}{
				"model_id": modelID,
			},
		})
	}

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
