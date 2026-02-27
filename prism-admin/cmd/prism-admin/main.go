// Command prism-admin starts the admin/dashboard backend for the Prism routing system.
package main

import (
	"context"
	"flag"
	"log"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/aspect-build/prism-admin/internal/api"
	"github.com/aspect-build/prism-admin/internal/config"
)

func main() {
	var (
		listenAddr = flag.String("listen", ":8081", "admin API listen address")
		configPath = flag.String("config", "", "path to prism.yaml (or set PRISM_CONFIG)")
		gatewayURL = flag.String("gateway", "http://localhost:8080", "prism-gateway URL")
	)
	flag.Parse()

	// Resolve config path.
	cfgPath := *configPath
	if cfgPath == "" {
		cfgPath = os.Getenv("PRISM_CONFIG")
	}
	if cfgPath == "" {
		// Search common locations.
		for _, p := range []string{"prism.yaml", "configs/prism.yaml", "../configs/prism.yaml"} {
			if _, err := os.Stat(p); err == nil {
				cfgPath = p
				break
			}
		}
	}

	if cfgPath == "" {
		log.Fatal("no prism.yaml config found; use -config flag or PRISM_CONFIG env var")
	}

	log.Printf("loading config from %s", cfgPath)
	prismCfg, err := config.LoadPrismConfig(cfgPath)
	if err != nil {
		log.Fatalf("failed to load config: %v", err)
	}

	adminCfg := &config.AdminConfig{
		ListenAddr: *listenAddr,
		GatewayURL: *gatewayURL,
		ConfigPath: cfgPath,
	}

	srv := api.NewServer(adminCfg, prismCfg)

	// Graceful shutdown.
	go func() {
		sigCh := make(chan os.Signal, 1)
		signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
		<-sigCh
		log.Println("shutting down...")
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		if err := srv.Shutdown(ctx); err != nil {
			log.Printf("shutdown error: %v", err)
		}
	}()

	if err := srv.Start(); err != nil && err.Error() != "http: Server closed" {
		log.Fatalf("server error: %v", err)
	}
	log.Println("prism-admin stopped")
}
