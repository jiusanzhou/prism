package main

import (
	"context"
	"flag"
	"fmt"
	"log"
	"os"
	"os/signal"
	"syscall"

	"go.zoe.im/prism-gateway/internal/config"
	"go.zoe.im/prism-gateway/internal/gateway"
	"go.zoe.im/prism-gateway/internal/provider"
)

func main() {
	configPath := flag.String("config", "", "Path to prism.yaml config file (default: $PRISM_CONFIG or ./prism.yaml)")
	flag.Parse()

	// 1. Load configuration
	var cfg *config.Config
	var err error

	if *configPath != "" {
		cfg, err = config.LoadConfig(*configPath)
	} else {
		cfg, err = config.LoadConfigFromEnv()
	}
	if err != nil {
		log.Fatalf("Failed to load config: %v", err)
	}

	log.Printf("Loaded config: %d providers, %d models, %d variants, %d pools",
		len(cfg.Providers), len(cfg.Models), len(cfg.Variants), len(cfg.Pools))

	// 2. Create router (StaticRouter or FFIRouter depending on build tag)
	r, err := createRouter(cfg)
	if err != nil {
		log.Fatalf("Failed to create router: %v", err)
	}

	// 3. Create providers from config
	providers := make(map[string]provider.Provider)
	for id, pcfg := range cfg.Providers {
		providerType := pcfg.Type
		if providerType == "" {
			providerType = "openai-compatible"
		}

		p, err := provider.NewProvider(providerType, provider.ProviderConfig{
			ID:            id,
			Name:          id,
			BaseURL:       pcfg.BaseURL,
			APIKey:        pcfg.APIKey,
			TimeoutMs:     pcfg.TimeoutMs,
			MaxConcurrent: pcfg.MaxConcurrent,
		})
		if err != nil {
			log.Fatalf("Failed to create provider %q: %v", id, err)
		}
		providers[id] = p
		log.Printf("Provider %q (%s) → %s", id, providerType, pcfg.BaseURL)
	}

	// If no providers configured, add a mock for dev convenience
	if len(providers) == 0 {
		log.Println("No providers configured, adding mock provider")
		providers["mock"] = provider.NewMockProvider("mock")
	}

	// 4. Start server
	srv := gateway.NewServer(cfg, r, providers)

	// 5. Graceful shutdown
	go func() {
		sigCh := make(chan os.Signal, 1)
		signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
		<-sigCh
		fmt.Println() // newline after ^C
		log.Println("Shutting down...")
		srv.Shutdown(context.Background())
	}()

	log.Printf("Starting Prism gateway on :%d", cfg.Server.HTTPPort)
	if err := srv.Start(); err != nil {
		log.Fatalf("Server error: %v", err)
	}
}
