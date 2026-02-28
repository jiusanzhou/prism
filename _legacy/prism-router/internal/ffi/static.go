//go:build !ffi

package ffi

import (
	"fmt"
)

// StaticImpl is a fallback router when the Rust FFI library is not available.
// It picks the first variant from the config, similar to prism-gateway's StaticRouter.
type StaticImpl struct {
	modelID    string
	providerID string
	variantID  string
	accountID  string
	models     uint32
	variants   uint32
}

// NewStaticRouter creates a Router that always returns a fixed result.
func NewStaticRouter(modelID, providerID, variantID, accountID string, numModels, numVariants uint32) *Router {
	return NewRouter(&StaticImpl{
		modelID:    modelID,
		providerID: providerID,
		variantID:  variantID,
		accountID:  accountID,
		models:     numModels,
		variants:   numVariants,
	})
}

func (s *StaticImpl) Route(req *RouteRequest) (*RouteResult, error) {
	return &RouteResult{
		Model:          s.modelID,
		Provider:       s.providerID,
		Variant:        s.variantID,
		Account:        s.accountID,
		FallbackChain:  nil,
		Reason:         "static",
		Score:          1.0,
		DecisionTimeUs: 0,
	}, nil
}

func (s *StaticImpl) UpdateStats(statsJSON string) error {
	// No-op for static router
	return nil
}

func (s *StaticImpl) Version() string {
	return "static-0.1.0"
}

func (s *StaticImpl) Health() *HealthStatus {
	return &HealthStatus{
		Status:           "healthy",
		RegisteredModels: s.models,
		ActiveVariants:   s.variants,
	}
}

// NewFFIRouter is a build-tag placeholder — returns an error when FFI is not available.
func NewFFIRouter(configJSON string) (*Router, error) {
	return nil, fmt.Errorf("FFI router not available: build without -tags ffi")
}
