//go:build ffi

package router

/*
#cgo LDFLAGS: -L${SRCDIR}/../../../target/release -lprism_core
#include <stdint.h>
#include <stdlib.h>

// FFI type declarations
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

// FFI function declarations
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
	"sync"
	"unsafe"
)

// FFIRouter routes requests through the Rust prism-core library via CGO.
type FFIRouter struct {
	mu sync.Mutex // prism_init is not guaranteed reentrant
}

// NewFFIRouter initializes the Rust routing engine with the given config JSON
// and returns a ready-to-use FFIRouter.
func NewFFIRouter(configJSON string) (*FFIRouter, error) {
	cConfig := C.CString(configJSON)
	defer C.free(unsafe.Pointer(cConfig))

	rc := C.prism_init(cConfig)
	if rc != 0 {
		return nil, fmt.Errorf("prism_init failed with code %d", int(rc))
	}

	return &FFIRouter{}, nil
}

// Route performs a routing decision by calling into the Rust core.
func (r *FFIRouter) Route(req *RouteRequest) (*RouteResult, error) {
	// Serialize messages to JSON
	messagesJSON, err := json.Marshal(req.Messages)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal messages: %w", err)
	}

	// Build strategy JSON (proper Rust enum format)
	strategyJSON, err := buildStrategyJSON(req)
	if err != nil {
		return nil, fmt.Errorf("failed to build strategy JSON: %w", err)
	}

	// Build metadata JSON (nullable)
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

	// Build analyze context JSON (nullable)
	analyzeContextJSON, err := buildAnalyzeContextJSON(req)
	if err != nil {
		return nil, fmt.Errorf("failed to build analyze context JSON: %w", err)
	}

	// Allocate C strings
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

	var cAnalyzeContext *C.char
	if analyzeContextJSON != nil {
		cAnalyzeContext = C.CString(string(analyzeContextJSON))
		defer C.free(unsafe.Pointer(cAnalyzeContext))
	}

	// Build the C request struct
	cReq := C.PrismRouteRequest{
		messages_json:        cMessages,
		strategy_json:        cStrategy,
		metadata_json:        cMetadata,
		analyze_context_json: cAnalyzeContext,
	}

	// Call Rust
	cResult := C.prism_route(&cReq)
	if cResult == nil {
		return nil, fmt.Errorf("prism_route returned nil")
	}
	defer C.prism_free_result(cResult)

	// Check for error
	if cResult.error != nil {
		errMsg := C.GoString(cResult.error)
		return nil, fmt.Errorf("prism_route error: %s", errMsg)
	}

	// Convert result to Go
	result := &RouteResult{
		Model:          C.GoString(cResult.model_id),
		Provider:       C.GoString(cResult.provider),
		Variant:        C.GoString(cResult.variant_id),
		Account:        C.GoString(cResult.account_id),
		DecisionTimeUs: int64(cResult.decision_time_us),
	}

	// Parse fallbacks JSON
	if cResult.fallbacks_json != nil {
		fallbacksStr := C.GoString(cResult.fallbacks_json)
		if fallbacksStr != "" {
			var fallbacks []string
			if err := json.Unmarshal([]byte(fallbacksStr), &fallbacks); err == nil {
				result.FallbackChain = fallbacks
			}
		}
	}

	// Parse reason JSON
	if cResult.reason_json != nil {
		result.Reason = C.GoString(cResult.reason_json)
	}

	return result, nil
}

// UpdateStats sends model performance statistics to the Rust core.
func (r *FFIRouter) UpdateStats(statsJSON string) error {
	cStats := C.CString(statsJSON)
	defer C.free(unsafe.Pointer(cStats))

	rc := C.prism_update_model_stats(cStats)
	if rc != 0 {
		return fmt.Errorf("prism_update_model_stats failed with code %d", int(rc))
	}
	return nil
}

// Version returns the version string from the Rust core.
func (r *FFIRouter) Version() string {
	return C.GoString(C.prism_version())
}

// Health returns the health status JSON from the Rust core.
func (r *FFIRouter) Health() string {
	return C.GoString(C.prism_health())
}
