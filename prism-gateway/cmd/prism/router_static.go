//go:build !ffi

package main

import (
	"log"

	"go.zoe.im/prism-gateway/internal/config"
	"go.zoe.im/prism-gateway/internal/router"
)

// createRouter creates a StaticRouter as a fallback when the FFI build tag is not set.
func createRouter(cfg *config.Config) (router.Router, error) {
	// Pick the first variant from the first pool, or fall back to sensible defaults.
	modelID := "gpt-4o"
	providerID := "openai"
	variantID := "gpt-4o@openai"
	accountID := "default"

	if len(cfg.Pools) > 0 && len(cfg.Pools[0].Variants) > 0 {
		variantID = cfg.Pools[0].Variants[0]
	}

	// Try to resolve model/provider from variants config
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

	return &router.StaticRouter{
		ModelID:    modelID,
		ProviderID: providerID,
		VariantID:  variantID,
		AccountID:  accountID,
	}, nil
}
