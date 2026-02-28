package gateway

import (
	"context"
	"fmt"
	"log"
	"net/http"
	"time"

	"github.com/prometheus/client_golang/prometheus/promhttp"

	"go.zoe.im/prism-gateway/internal/config"
	"go.zoe.im/prism-gateway/internal/abtesting"
	"go.zoe.im/prism-gateway/internal/metrics"
	"go.zoe.im/prism-gateway/internal/provider"
	"go.zoe.im/prism-gateway/internal/router"
	"go.zoe.im/prism-gateway/internal/state"
)

// Server is the main Prism gateway HTTP server.
type Server struct {
	cfg       *config.Config
	handler   *Handler
	httpSrv   *http.Server
	state     state.StateStore
	cancelSync context.CancelFunc
}

// NewServer creates a new gateway server.
func NewServer(cfg *config.Config, r router.Router, providers map[string]provider.Provider) *Server {
	// Register Prometheus metrics
	metrics.Register()

	h := NewHandler(r, providers)

	// Create state store (in-memory by default, Redis if configured)
	var stateStore state.StateStore
	if cfg.Redis != nil && cfg.Redis.Addr != "" {
		rs, err := state.NewRedisStore(cfg.Redis.Addr, cfg.Redis.Password, cfg.Redis.DB)
		if err != nil {
			log.Printf("Warning: Redis state store unavailable (%v), falling back to in-memory", err)
			stateStore = state.NewMemoryStore()
		} else {
			log.Printf("Using Redis state store at %s", cfg.Redis.Addr)
			stateStore = rs
		}
	} else {
		stateStore = state.NewMemoryStore()
	}
	h.SetStateStore(stateStore)

	// Set up A/B testing manager if experiments are configured
	if len(cfg.Experiments) > 0 {
		abMgr := abtesting.NewManager(cfg.Experiments)
		h.SetABManager(abMgr)
		log.Printf("A/B testing enabled with %d experiment(s)", len(cfg.Experiments))
	}

	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/chat/completions", h.ChatCompletion)
	mux.HandleFunc("POST /v1/messages", h.AnthropicMessages)
	mux.HandleFunc("POST /v1/outcomes", h.ReportOutcome)
	mux.HandleFunc("GET /v1/experiments", h.ListExperiments)
	mux.HandleFunc("GET /v1/experiments/{id}", h.GetExperiment)
	mux.HandleFunc("GET /health", h.Health)
	mux.Handle("GET /metrics", promhttp.Handler())

	// Start sync loop if router supports UpdateStats
	var cancelSync context.CancelFunc
	if statsUpdater, ok := r.(state.StatsUpdater); ok {
		ctx, cancel := context.WithCancel(context.Background())
		cancelSync = cancel
		go state.SyncLoop(ctx, stateStore, statsUpdater, 5*time.Second)
	}

	return &Server{
		cfg:        cfg,
		handler:    h,
		state:      stateStore,
		cancelSync: cancelSync,
		httpSrv: &http.Server{
			Addr:         fmt.Sprintf(":%d", cfg.Server.HTTPPort),
			Handler:      mux,
			ReadTimeout:  30 * time.Second,
			WriteTimeout: 120 * time.Second,
			IdleTimeout:  120 * time.Second,
		},
	}
}

// Start begins listening. Blocks until the server stops.
func (s *Server) Start() error {
	log.Printf("Prism gateway listening on %s", s.httpSrv.Addr)
	return s.httpSrv.ListenAndServe()
}

// Shutdown gracefully stops the server.
func (s *Server) Shutdown(ctx context.Context) error {
	if s.cancelSync != nil {
		s.cancelSync()
	}
	if s.state != nil {
		s.state.Close()
	}
	return s.httpSrv.Shutdown(ctx)
}
