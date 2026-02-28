// prism-router: Standalone routing service (Mode B)
//
// Accepts HTTP/JSON requests and uses the Rust FFI core (or static fallback)
// for routing decisions. Can be deployed behind any API gateway (Higress, Envoy, APISIX).
//
// Endpoints:
//   POST /v1/route        — single routing decision
//   POST /v1/route/batch  — batch routing
//   POST /v1/outcome      — report outcome for stats
//   GET  /health          — health check
//   GET  /metrics         — prometheus metrics
package main

import (
	"context"
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"go.zoe.im/prism-router/internal/server"
)

func main() {
	configPath := flag.String("config", "", "Path to prism.yaml config file (default: $PRISM_CONFIG or ./prism.yaml)")
	listenAddr := flag.String("addr", ":9080", "HTTP listen address")
	flag.Parse()

	// 1. Load configuration (reuse the same YAML format as prism-gateway)
	cfg, err := loadConfig(*configPath)
	if err != nil {
		log.Fatalf("Failed to load config: %v", err)
	}

	log.Printf("Loaded config: %d models, %d variants, %d pools",
		len(cfg.Models), len(cfg.Variants), len(cfg.Pools))

	// 2. Create router (FFI or static depending on build tag)
	router, err := createRouter(cfg)
	if err != nil {
		log.Fatalf("Failed to create router: %v", err)
	}
	log.Printf("[router] Engine version: %s", router.Version())

	// 3. Create and start HTTP server
	srv := server.New(router)

	httpSrv := &http.Server{
		Addr:         *listenAddr,
		Handler:      srv.Handler(),
		ReadTimeout:  30 * time.Second,
		WriteTimeout: 30 * time.Second,
		IdleTimeout:  120 * time.Second,
	}

	// 4. Graceful shutdown
	go func() {
		sigCh := make(chan os.Signal, 1)
		signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
		<-sigCh
		fmt.Println()
		log.Println("Shutting down...")
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		httpSrv.Shutdown(ctx)
	}()

	log.Printf("Prism router (Mode B) listening on %s", *listenAddr)
	if err := httpSrv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		log.Fatalf("Server error: %v", err)
	}
}
