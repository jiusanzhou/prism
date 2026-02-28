//go:build !ffi

package main

import (
	"log"

	"go.zoe.im/prism-router/internal/ffi"
)

// createRouter creates a static fallback Router when the FFI build tag is not set.
func createRouter(cfg *Config) (*ffi.Router, error) {
	modelID := "gpt-4o"
	providerID := "openai"
	variantID := "gpt-4o@openai"
	accountID := "default"

	if len(cfg.Pools) > 0 && len(cfg.Pools[0].Variants) > 0 {
		variantID = cfg.Pools[0].Variants[0]
	}

	for _, v := range cfg.Variants {
		if v.ID == variantID {
			modelID = v.ModelID
			providerID = v.ProviderID
			accountID = providerID + "-acct"
			break
		}
	}

	log.Printf("[router] Using StaticRouter (build without -tags ffi): model=%s provider=%s variant=%s",
		modelID, providerID, variantID)

	return ffi.NewStaticRouter(
		modelID, providerID, variantID, accountID,
		uint32(len(cfg.Models)), uint32(len(cfg.Variants)),
	), nil
}
