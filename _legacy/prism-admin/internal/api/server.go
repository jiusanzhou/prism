// Package api provides the HTTP server and handlers for the prism-admin service.
package api

import (
	"context"
	"log"
	"net/http"
	"time"

	"go.zoe.im/prism-admin/internal/config"
	"go.zoe.im/prism-admin/internal/dashboard"
)

// Server is the admin HTTP server.
type Server struct {
	cfg       *config.AdminConfig
	prismCfg  *config.PrismConfig
	dashboard *dashboard.Service
	httpSrv   *http.Server
	// variantStatus tracks overridden statuses for variants.
	variantStatus map[string]string
}

// NewServer creates a new admin server.
func NewServer(adminCfg *config.AdminConfig, prismCfg *config.PrismConfig) *Server {
	s := &Server{
		cfg:           adminCfg,
		prismCfg:      prismCfg,
		dashboard:     dashboard.NewService(prismCfg),
		variantStatus: make(map[string]string),
	}

	mux := http.NewServeMux()
	s.registerRoutes(mux)

	handler := s.applyMiddleware(mux)

	s.httpSrv = &http.Server{
		Addr:         adminCfg.ListenAddr,
		Handler:      handler,
		ReadTimeout:  10 * time.Second,
		WriteTimeout: 30 * time.Second,
		IdleTimeout:  60 * time.Second,
	}

	return s
}

// registerRoutes sets up all API routes.
func (s *Server) registerRoutes(mux *http.ServeMux) {
	// System
	mux.HandleFunc("GET /api/v1/status", s.handleStatus)
	mux.HandleFunc("GET /api/v1/config", s.handleConfig)

	// Providers
	mux.HandleFunc("GET /api/v1/providers", s.handleProviders)
	mux.HandleFunc("GET /api/v1/providers/{id}", s.handleProviderByID)

	// Models
	mux.HandleFunc("GET /api/v1/models", s.handleModels)
	mux.HandleFunc("GET /api/v1/models/{id}", s.handleModelByID)

	// Variants
	mux.HandleFunc("GET /api/v1/variants", s.handleVariants)
	mux.HandleFunc("GET /api/v1/variants/{id}", s.handleVariantByID)
	mux.HandleFunc("PUT /api/v1/variants/{id}/status", s.handleVariantStatusUpdate)

	// Pools
	mux.HandleFunc("GET /api/v1/pools", s.handlePools)
	mux.HandleFunc("GET /api/v1/pools/{id}", s.handlePoolByID)

	// Dashboard
	mux.HandleFunc("GET /api/v1/dashboard/overview", s.handleDashboardOverview)
	mux.HandleFunc("GET /api/v1/dashboard/routing", s.handleDashboardRouting)
	mux.HandleFunc("GET /api/v1/dashboard/providers", s.handleDashboardProviders)

	// Experiments
	mux.HandleFunc("GET /api/v1/experiments", s.handleExperiments)
	mux.HandleFunc("GET /api/v1/experiments/{id}/results", s.handleExperimentResults)
}

// applyMiddleware wraps the handler with middleware.
func (s *Server) applyMiddleware(handler http.Handler) http.Handler {
	h := handler
	h = LoggingMiddleware(h)
	h = CORSMiddleware(h)
	if s.cfg.BasicAuth != nil {
		h = BasicAuthMiddleware(s.cfg.BasicAuth.Username, s.cfg.BasicAuth.Password)(h)
	}
	return h
}

// Start begins listening for HTTP requests.
func (s *Server) Start() error {
	log.Printf("prism-admin listening on %s", s.cfg.ListenAddr)
	return s.httpSrv.ListenAndServe()
}

// Shutdown gracefully shuts down the HTTP server.
func (s *Server) Shutdown(ctx context.Context) error {
	return s.httpSrv.Shutdown(ctx)
}

// Handler returns the HTTP handler (for testing).
func (s *Server) Handler() http.Handler {
	return s.httpSrv.Handler
}
