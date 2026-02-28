//go:build ffi

package main

import (
	"log"

	"go.zoe.im/prism-gateway/internal/config"
	"go.zoe.im/prism-gateway/internal/router"
)

// createRouter creates an FFIRouter backed by the Rust prism-core library.
func createRouter(cfg *config.Config) (router.Router, error) {
	configJSON, err := cfg.ToRouterConfigJSON()
	if err != nil {
		return nil, err
	}

	log.Printf("[router] Initializing FFI router with Rust prism-core...")
	r, err := router.NewFFIRouter(configJSON)
	if err != nil {
		return nil, err
	}

	log.Printf("[router] FFI router initialized (version: %s)", r.Version())
	return r, nil
}
